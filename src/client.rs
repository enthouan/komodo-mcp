use std::{fmt, sync::Arc, time::Duration};

use komodo_client::{
    KomodoClient,
    api::{
        execute::KomodoExecuteRequest,
        read::{GetVersion, KomodoReadRequest},
        write::KomodoWriteRequest,
    },
};
use reqwest::{Client, redirect::Policy};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    capabilities::{Capability, CapabilityPolicy},
    config::Config,
    diagnostics::Redactor,
    error::{AppError, ErrorKind},
};

/// JSON body budget for evidence reads and execution replies, before typed decoding.
pub const MAX_EVIDENCE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Compatibility name for the unchanged workload-log body budget.
pub const MAX_LOG_RESPONSE_BYTES: usize = MAX_EVIDENCE_RESPONSE_BYTES;

/// Production deadlines; injectable without changing the process environment.
#[derive(Debug, Clone, Copy)]
pub struct RequestPolicy {
    pub connect_timeout: Duration,
    pub operation_timeout: Duration,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            operation_timeout: Duration::from_secs(15),
        }
    }
}

struct Upstream {
    sdk: KomodoClient,
    bounded_request: reqwest::RequestBuilder,
    execute_request: reqwest::RequestBuilder,
    write_request: reqwest::RequestBuilder,
    current_caller_request: reqwest::RequestBuilder,
    terminal_request: reqwest::RequestBuilder,
    policy: RequestPolicy,
    redactor: Redactor,
    capabilities: CapabilityPolicy,
}

/// Clones keep the same session. A distinct MCP session shares the upstream
/// Core identity and connection pool, with its own preview and terminal owner.
#[derive(Clone)]
pub struct CoreClient {
    upstream: Arc<Upstream>,
    previews: Arc<crate::configuration::PreviewStore>,
    terminals: Arc<crate::terminal::Runtime>,
    bound_reads: bool,
}

impl fmt::Debug for CoreClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreClient")
            .field("policy", &self.upstream.policy)
            .finish_non_exhaustive()
    }
}

/// The observed response, validated as SemVer and redacted before any reporting.
/// The SDK release and fixture release are documented independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoreVersion(String);

impl fmt::Display for CoreVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl CoreClient {
    pub fn new(config: Config, policy: RequestPolicy) -> Result<Self, AppError> {
        if policy.connect_timeout.is_zero() || policy.operation_timeout.is_zero() {
            return Err(AppError::configuration(
                "RequestPolicy",
                "timeouts must be greater than zero",
            ));
        }
        let http = Client::builder()
            .connect_timeout(policy.connect_timeout)
            // The outer operation deadline is authoritative, including SDK body
            // reads which erase reqwest errors. Do not race it with an HTTP timeout.
            .redirect(Policy::none())
            // Execution cannot be retried even on a connection/protocol failure.
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| AppError::new(ErrorKind::Unreachable, "configure_transport", None))?;
        let redactor = config.redactor();
        let capabilities = config.capabilities();
        // The SDK's private transport buffers complete bodies. Evidence reads use
        // its typed request/response contract over the same authenticated pool,
        // with a streaming body cap before deserialization.
        let bounded_request = http
            .post(format!("{}/read", config.address))
            .header("x-api-key", &config.key)
            .header("x-api-secret", &config.secret);
        let execute_request = http
            .post(format!("{}/execute", config.address))
            .header("x-api-key", &config.key)
            .header("x-api-secret", &config.secret);
        let write_request = http
            .post(format!("{}/write", config.address))
            .header("x-api-key", &config.key)
            .header("x-api-secret", &config.secret);
        // Pinned Core exposes the authenticated current caller through GET
        // /user, outside the typed Rust read API. No caller-supplied path.
        let current_caller_request = http
            .get(format!("{}/user", config.address))
            .header("x-api-key", &config.key)
            .header("x-api-secret", &config.secret);
        let terminal_request = http
            .post(format!("{}/terminal/execute", config.address))
            .header("x-api-key", &config.key)
            .header("x-api-secret", &config.secret);
        let sdk = KomodoClient::new(config.address, config.key, config.secret).set_reqwest(http);
        Ok(Self {
            upstream: Arc::new(Upstream {
                sdk,
                bounded_request,
                execute_request,
                write_request,
                current_caller_request,
                terminal_request,
                policy,
                redactor,
                capabilities,
            }),
            terminals: Arc::default(),
            previews: Arc::default(),
            bound_reads: false,
        })
    }

    /// Create a fresh private owner within the same Core pool and process limits.
    pub fn isolated_session(&self) -> Self {
        Self {
            upstream: Arc::clone(&self.upstream),
            previews: Arc::new(self.previews.isolated()),
            terminals: Arc::new(self.terminals.isolated()),
            bound_reads: self.bound_reads,
        }
    }

    /// Invalidate private state immediately, including registrations attempted
    /// by in-flight requests. Owned terminal workers perform their usual cleanup.
    pub fn invalidate_session(&self) {
        self.previews.close();
        self.terminals.begin_close();
    }

    /// Wait up to the terminal cleanup deadline for this owner's workers.
    pub async fn close_session(&self) {
        self.invalidate_session();
        self.terminals.wait_for_cleanup().await;
    }

    pub fn capabilities(&self) -> CapabilityPolicy {
        self.upstream.capabilities
    }

    pub(crate) fn terminal_runtime(&self) -> &Arc<crate::terminal::Runtime> {
        &self.terminals
    }

    /// Cancel observations and allow bounded owned cleanup before runtime exit.
    pub async fn shutdown_terminals(&self) {
        self.terminals.begin_close();
        self.terminals.wait_for_cleanup().await;
    }

    /// Closed terminal lifecycle surface. Generic configuration writes still
    /// reject these operations, and configuration-edit never substitutes for
    /// the independent terminal + operate grants or Core Read + Terminal.
    pub(crate) async fn terminal_step(
        &self,
        owned: &crate::terminal::Owned,
        phase: crate::terminal::Phase,
    ) -> Result<serde_json::Value, AppError> {
        use crate::terminal::Phase;
        let operation = match phase {
            Phase::Create => "CreateTerminal",
            Phase::Delete => "DeleteTerminal",
            Phase::Command => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "terminal_write",
                    None,
                ));
            }
        };
        tokio::time::timeout(Duration::from_secs(10), owned.guard(self, phase))
            .await
            .map_err(|_| AppError::new(ErrorKind::Timeout, operation, None))??;
        let body = serde_json::json!({"type":operation,"params":owned.body(phase)?});
        let request = self
            .clone_request(&self.upstream.write_request, operation)?
            .json(&body);
        tokio::time::timeout(Duration::from_secs(10), async {
            owned.mark_attempt(phase);
            let mut response = request
                .send()
                .await
                .map_err(|e| AppError::from_upstream(operation, e.into()))?;
            self.decode_bounded_body(&mut response, operation, None)
                .await
        })
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// Authenticated one-shot streaming endpoint, fixed path, no initializer,
    /// redirects or retries. Only the sealed private session proof can reach it.
    pub(crate) async fn terminal_stream(
        &self,
        owned: &crate::terminal::Owned,
    ) -> Result<Vec<u8>, AppError> {
        const OP: &str = "ExecuteTerminal";
        use crate::terminal::Phase;
        tokio::time::timeout(Duration::from_secs(10), owned.guard(self, Phase::Command))
            .await
            .map_err(|_| AppError::new(ErrorKind::Timeout, OP, None))??;
        let request = self
            .clone_request(&self.upstream.terminal_request, OP)?
            .json(&owned.body(Phase::Command)?);
        owned.mark_attempt(Phase::Command);
        let mut response = request
            .send()
            .await
            .map_err(|e| AppError::from_upstream(OP, e.into()))?;
        if !response.status().is_success() {
            return Err(AppError::from_http_failure(OP, response.status(), &[]));
        }
        const LIMIT: usize = 65_536;
        if response.content_length().is_some_and(|v| v > LIMIT as u64) {
            return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AppError::new(ErrorKind::InvalidResponse, OP, None))?
        {
            if chunk.len() > LIMIT - bytes.len() {
                return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub(crate) fn previews(&self) -> &crate::configuration::PreviewStore {
        &self.previews
    }

    /// Drop authentication configuration immediately, regardless of upstream
    /// sanitization. GET /user itself allows disabled callers in pinned Core;
    /// mutation handlers explicitly require enabled administrator authority.
    pub(crate) async fn current_caller(&self) -> Result<serde_json::Value, AppError> {
        const OP: &str = "CurrentCaller";
        tokio::time::timeout(self.upstream.policy.operation_timeout, async {
            let mut response = self.clone_request(&self.upstream.current_caller_request, OP)?
                .send().await.map_err(|e| AppError::from_upstream(OP, e.into()))?;
            let raw: serde_json::Value = self.decode_bounded_body(&mut response, OP, None).await?;
            if !raw["_id"]["$oid"].as_str().is_some_and(crate::deletion::object_id)
                || !raw["username"].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 500 && !s.chars().any(char::is_control))
                || ["enabled", "admin", "super_admin"].iter().any(|k| !raw[*k].is_boolean())
            {
                return Err(AppError::new(ErrorKind::InvalidResponse, OP, None));
            }
            Ok(serde_json::json!({"id":raw["_id"]["$oid"],"username":raw["username"],"enabled":raw["enabled"],"admin":raw["admin"],"super_admin":raw["super_admin"]}))
        }).await.unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, OP, None)))
    }

    /// One narrowly typed variable write with independent local policy and a
    /// final complete private baseline/authority check. No SDK defaults or retry.
    pub(crate) async fn variable_write_checked<T>(
        &self,
        request: T,
        baseline: &serde_json::Value,
    ) -> Result<Result<serde_json::Value, AppError>, AppError>
    where
        T: Serialize + KomodoWriteRequest,
    {
        let operation = match T::req_type() {
            op @ ("CreateVariable"
            | "UpdateVariableValue"
            | "UpdateVariableDescription"
            | "DeleteVariable") => op,
            _ => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "variable_write",
                    None,
                ));
            }
        };
        let parameters = serde_json::to_value(request)
            .map_err(|_| AppError::new(ErrorKind::InvalidArguments, operation, None))?;
        crate::variables::guard_write(self, operation, &parameters, baseline).await?;
        Ok(tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.write_request,
                parameters,
                operation,
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))))
    }

    /// Administration uses its own closed whitelist and final private authority /
    /// baseline check. Generic configuration writes cannot issue credentials or
    /// bypass this boundary; response authentication objects stay raw and private.
    pub(crate) async fn access_write_checked<T>(
        &self,
        request: T,
        proof: &serde_json::Value,
    ) -> Result<Result<serde_json::Value, AppError>, AppError>
    where
        T: Serialize + KomodoWriteRequest,
    {
        let operation = match T::req_type() {
            op @ ("CreateServiceUser"
            | "UpdateServiceUserDescription"
            | "UpdateUserBasePermissions"
            | "UpdateUserAdmin"
            | "DeleteUser"
            | "CreateUserGroup"
            | "RenameUserGroup"
            | "DeleteUserGroup"
            | "AddUserToUserGroup"
            | "RemoveUserFromUserGroup"
            | "SetEveryoneUserGroup"
            | "UpdatePermissionOnTarget"
            | "UpdatePermissionOnResourceType") => op,
            _ => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "access_write",
                    None,
                ));
            }
        };
        let parameters = serde_json::to_value(request)
            .map_err(|_| AppError::new(ErrorKind::InvalidArguments, operation, None))?;
        crate::access::guard_write(self, operation, &parameters, proof).await?;
        Ok(tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.write_request,
                parameters,
                operation,
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))))
    }

    /// One typed configuration submission. No redirects, mutation retries or
    /// credential fallback; the same deadlines and streaming body cap apply.
    pub(crate) async fn write<T>(&self, request: T) -> Result<T::Response, AppError>
    where
        T: Serialize + KomodoWriteRequest,
        T::Response: DeserializeOwned,
    {
        self.write_checked(request).await?
    }

    /// Outer errors rejected before a write was attempted; inner errors may
    /// follow submission. New definition handlers preserve that distinction.
    pub(crate) async fn write_checked<T>(
        &self,
        request: T,
    ) -> Result<Result<T::Response, AppError>, AppError>
    where
        T: Serialize + KomodoWriteRequest,
        T::Response: DeserializeOwned,
    {
        let operation = match T::req_type() {
            "UpdateStack" => "UpdateStack",
            "CreateStack" => "CreateStack",
            "CreateDeployment" => "CreateDeployment",
            "UpdateDeployment" => "UpdateDeployment",
            "CreateServer" => "CreateServer",
            "UpdateServer" => "UpdateServer",
            "CreateRepo" => "CreateRepo",
            "UpdateRepo" => "UpdateRepo",
            "CreateResourceSync" => "CreateResourceSync",
            "UpdateResourceSync" => "UpdateResourceSync",
            "CreateBuilder" => "CreateBuilder",
            "UpdateBuilder" => "UpdateBuilder",
            "CreateBuild" => "CreateBuild",
            "UpdateBuild" => "UpdateBuild",
            "CreateProcedure" => "CreateProcedure",
            "UpdateProcedure" => "UpdateProcedure",
            "CreateAction" => "CreateAction",
            "UpdateAction" => "UpdateAction",
            "CreateAlerter" => "CreateAlerter",
            "UpdateAlerter" => "UpdateAlerter",
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "write", None)),
        };
        crate::configuration::require_edit(self)?;
        let parameters = serde_json::to_value(request).map_err(|_| {
            AppError::arguments(operation, "Use a supported typed configuration request")
        })?;
        crate::resource_syncs::guard_write(self, operation, &parameters)?;
        crate::builders::guard_write(self, operation, &parameters).await?;
        crate::procedures::guard_write(self, operation, &parameters)?;
        crate::actions::guard_write(self, operation, &parameters)?;
        crate::alerters::guard_write(self, operation, &parameters).await?;
        crate::build_configuration::guard_write(self, operation, &parameters).await?;
        crate::repositories::require_execution_effect_operate(
            self,
            (operation == "CreateRepo" && parameters["config"]["webhook_enabled"] != false)
                || (operation == "UpdateRepo"
                    && crate::repositories::update_requires_operate(&parameters["config"])),
            operation,
        )?;
        // Enforce Core's implicit execution boundary at the shared transport too.
        // Do not rely on a previously observed auto_update policy: it can race.
        crate::configuration::require_source_operate(
            self,
            operation == "UpdateStack"
                && ["repo", "linked_repo", "file_contents", "files_on_host"]
                    .iter()
                    .any(|field| parameters["config"].get(field).is_some()),
        )?;
        crate::deployment_configuration::require_image_operate(
            self,
            operation == "UpdateDeployment" && parameters["config"].get("image").is_some(),
        )?;
        crate::server_configuration::require_connection_operate(
            self,
            (operation == "UpdateServer"
                && ["address", "enabled", "insecure_tls"]
                    .iter()
                    .any(|f| parameters["config"].get(f).is_some()))
                || (operation == "CreateServer" && parameters["config"]["enabled"] == true),
        )?;
        let expected_config = parameters
            .get("config")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .ok_or_else(|| {
                AppError::arguments(operation, "Use a typed partial configuration object")
            })?;
        let expected_initial_key = (operation == "CreateServer").then(|| {
            parameters["public_key"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        });
        Ok(
            tokio::time::timeout(self.upstream.policy.operation_timeout, async {
                let response: serde_json::Value = self
                    .request_bounded_body(
                        &self.upstream.write_request,
                        parameters,
                        T::req_type(),
                        operation,
                    )
                    .await?;
                // SDK defaults must not turn an absent echoed field into a verified save.
                if !response["config"].is_object()
                    || !expected_config
                        .iter()
                        .all(|(key, value)| response["config"].get(key) == Some(value))
                    || expected_initial_key.as_ref().is_some_and(|key| {
                        response["info"]["public_key"].as_str() != Some(key.as_str())
                    })
                {
                    return Err(AppError::new(ErrorKind::InvalidResponse, operation, None));
                }
                serde_json::from_value(response)
                    .map_err(|_| AppError::new(ErrorKind::InvalidResponse, operation, None))
            })
            .await
            .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))),
        )
    }

    /// One explicit cache/source refresh, separately gated by operate. Core
    /// decides Execute permission; this cannot submit a definition or RunSync.
    pub(crate) async fn refresh_sync_pending(
        &self,
        sync: &str,
    ) -> Result<serde_json::Value, AppError> {
        const OP: &str = "RefreshResourceSyncPending";
        if !self.capabilities().allows(Capability::Operate) {
            return Err(AppError::new(ErrorKind::CapabilityDisabled, OP, None));
        }
        if sync.len() != 24 || !sync.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AppError::arguments(
                OP,
                "Refresh requires one resolved stable ResourceSync ID",
            ));
        }
        let request = komodo_client::api::write::RefreshResourceSyncPending { sync: sync.into() };
        tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(&self.upstream.write_request, request, OP, OP),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, OP, None)))
    }

    /// A single typed destructive write. Its raw acknowledgement is retained
    /// privately so missing fields cannot be manufactured by SDK defaults.
    pub(crate) async fn delete<T>(&self, request: T) -> Result<serde_json::Value, AppError>
    where
        T: Serialize + KomodoWriteRequest,
    {
        let operation = match T::req_type() {
            "DeleteStack" => "DeleteStack",
            "DeleteDeployment" => "DeleteDeployment",
            "DeleteServer" => "DeleteServer",
            "DeleteTag" => "DeleteTag",
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "delete", None)),
        };
        crate::deletion::require_apply(self)?;
        tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.write_request,
                request,
                T::req_type(),
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// Typed tag operations, retaining raw acknowledgements before SDK defaults.
    pub(crate) async fn tag_write<T>(&self, request: T) -> Result<serde_json::Value, AppError>
    where
        T: Serialize + KomodoWriteRequest,
    {
        let operation = match T::req_type() {
            op @ ("CreateTag" | "RenameTag" | "UpdateResourceMeta") => op,
            _ => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "tag_write",
                    None,
                ));
            }
        };
        crate::configuration::require_edit(self)?;
        let mut params = serde_json::to_value(request)
            .map_err(|_| AppError::arguments(operation, "Use a typed tag request"))?;
        if operation == "UpdateResourceMeta" {
            if !params["description"].is_null()
                || !params["template"].is_null()
                || !params["tags"].as_array().is_some_and(|a| {
                    a.len() <= 64
                        && a.iter()
                            .all(|v| v.as_str().is_some_and(crate::deletion::object_id))
                })
                || !matches!(
                    params["target"]["type"].as_str(),
                    Some("Stack" | "Deployment" | "Server")
                )
                || !params["target"]["id"]
                    .as_str()
                    .is_some_and(crate::deletion::object_id)
            {
                return Err(AppError::arguments(
                    operation,
                    "Only exact stable tag IDs and one managed resource's tags may be updated",
                ));
            }
            params
                .as_object_mut()
                .expect("typed object")
                .remove("description");
            params
                .as_object_mut()
                .expect("typed object")
                .remove("template");
        } else if operation == "CreateTag" && params["color"].is_null() {
            params
                .as_object_mut()
                .expect("typed object")
                .remove("color");
        }
        tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.write_request,
                params,
                T::req_type(),
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// One explicit typed submission, over the existing authenticated pool.
    /// Every uncertain failure is returned without retrying or changing credentials.
    pub(crate) async fn execute<T>(
        &self,
        request: T,
    ) -> Result<Result<T::Response, AppError>, AppError>
    where
        T: Serialize + KomodoExecuteRequest,
        T::Response: DeserializeOwned,
    {
        let operation = match T::req_type() {
            "DeployStack" => "DeployStack",
            "Deploy" => "Deploy",
            "StartStack" => "StartStack",
            "StopStack" => "StopStack",
            "RestartStack" => "RestartStack",
            "StartDeployment" => "StartDeployment",
            "StopDeployment" => "StopDeployment",
            "RestartDeployment" => "RestartDeployment",
            "CloneRepo" => "CloneRepo",
            "PullRepo" => "PullRepo",
            "RunBuild" => "RunBuild",
            "CancelBuild" => "CancelBuild",
            "RunProcedure" => "RunProcedure",
            "CancelProcedure" => "CancelProcedure",
            "RunAction" => "RunAction",
            "CancelAction" => "CancelAction",
            "TestAlerter" => "TestAlerter",
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "execute", None)),
        };
        if !self.capabilities().allows(Capability::Operate) {
            return Err(AppError::new(
                ErrorKind::CapabilityDisabled,
                operation,
                None,
            ));
        }
        let params = serde_json::to_value(&request)
            .map_err(|_| AppError::arguments(operation, "Use a typed execution request"))?;
        crate::build_operations::guard_execute(self, operation, &params).await?;
        crate::procedure_operations::guard_execute(self, operation, &params).await?;
        crate::action_operations::guard_execute(self, operation, &params).await?;
        crate::alerters::guard_execute(self, operation, &params).await?;
        Ok(tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.execute_request,
                request,
                T::req_type(),
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))))
    }

    /// Closed prune transport: the ordinary operate/configuration surfaces cannot
    /// invoke it, and the complete private review is checked again before send.
    pub(crate) async fn prune_checked<T>(
        &self,
        request: T,
        proof: &serde_json::Value,
    ) -> Result<Result<komodo_client::entities::update::Update, AppError>, AppError>
    where
        T: Serialize + KomodoExecuteRequest<Response = komodo_client::entities::update::Update>,
    {
        let operation = match T::req_type() {
            op @ ("PruneImages" | "PruneContainers" | "PruneNetworks" | "PruneVolumes"
            | "PruneSystem") => op,
            _ => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "docker_prune",
                    None,
                ));
            }
        };
        let params = serde_json::to_value(request)
            .map_err(|_| AppError::new(ErrorKind::InvalidArguments, operation, None))?;
        crate::pruning::guard_execute(self, operation, &params, proof).await?;
        Ok(tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(&self.upstream.execute_request, params, operation, operation),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))))
    }

    /// Single-use reviewed sync transport; unavailable to generic execute<T>.
    pub(crate) async fn execute_reviewed_sync(
        &self,
        authorized: crate::sync_plans::AuthorizedSync,
    ) -> Result<Result<komodo_client::entities::update::Update, AppError>, AppError> {
        // RunSync remains absent from generic execute<T>. This one-use private
        // authority can only be constructed after consuming and rechecking a
        // complete review; both immutable local grants are checked here too.
        crate::configuration::require_edit(self)?;
        crate::operations::require_operate(self, "RunSync")?;
        let request = authorized.into_request();
        crate::sync_plans::validate_request(&request)?;
        Ok(tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.execute_request,
                request,
                "RunSync",
                "RunSync",
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, "RunSync", None))))
    }

    /// Keep diagnosis inputs bounded without changing existing inventory calls.
    pub(crate) fn bounded_evidence(&self) -> Self {
        Self {
            bound_reads: true,
            ..self.clone()
        }
    }

    /// Full reference scans must distinguish missing reference fields from SDK
    /// defaults. Keep the raw bounded response private until its shape is checked.
    pub(crate) async fn read_references<T>(&self, request: T) -> Result<serde_json::Value, AppError>
    where
        T: Serialize + KomodoReadRequest,
    {
        let operation = match T::req_type() {
            op @ ("ListTags" | "ListVariables" | "GetVariable" | "ListSecrets" | "ListServers"
            | "ListStacks" | "ListDeployments" | "ListSwarms" | "ListBuilds"
            | "ListRepos" | "ListBuilders" | "ListAlerters" | "ListProcedures"
            | "ListActions" | "ListResourceSyncs" | "GetSwarm" | "GetBuild"
            | "GetProcedure" | "GetAction" | "GetResourceSync") => op,
            op @ ("ListUsers"
            | "FindUser"
            | "ListUserGroups"
            | "GetUserGroup"
            | "ListPermissions"
            | "ListUserTargetPermissions"
            | "GetPermission") => op,
            "ListFullBuilders" => "ListFullBuilders",
            "ListFullAlerters" => "ListFullAlerters",
            "ListFullRepos" => "ListFullRepos",
            "ListFullStacks" => "ListFullStacks",
            "ListFullDeployments" => "ListFullDeployments",
            "GetBuilder" => "GetBuilder",
            "GetAlerter" => "GetAlerter",
            "GetServer" => "GetServer",
            "GetRepo" => "GetRepo",
            "GetStack" => "GetStack",
            "GetDeployment" => "GetDeployment",
            op @ ("ListTerminals"
            | "InspectContainer"
            | "InspectStackContainer"
            | "InspectDeploymentContainer"
            | "ListStackServices") => op,
            "ListGitProviderAccounts" => "ListGitProviderAccounts",
            "ListImageRegistryAccounts" => "ListImageRegistryAccounts",
            "ListGitProvidersFromConfig" => "ListGitProvidersFromConfig",
            "ListImageRegistriesFromConfig" => "ListImageRegistriesFromConfig",
            _ => {
                return Err(AppError::new(
                    ErrorKind::UnsupportedTarget,
                    "dependency_read",
                    None,
                ));
            }
        };
        tokio::time::timeout(
            self.upstream.policy.operation_timeout,
            self.request_bounded_body(
                &self.upstream.bounded_request,
                request,
                T::req_type(),
                operation,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// Typed read entry point with the existing SDK contract and safe failures.
    pub async fn read<T>(&self, request: T) -> Result<T::Response, AppError>
    where
        T: Serialize + KomodoReadRequest,
        T::Response: DeserializeOwned,
    {
        let operation = match T::req_type() {
            "GetVersion" => "GetVersion",
            "ListVariables" => "ListVariables",
            "GetVariable" => "GetVariable",
            "ListSecrets" => "ListSecrets",
            "ListServers" => "ListServers",
            "GetServer" => "GetServer",
            "ListStacks" => "ListStacks",
            "GetStack" => "GetStack",
            "ListDeployments" => "ListDeployments",
            "GetDeployment" => "GetDeployment",
            "GetSwarm" => "GetSwarm",
            "ListSwarms" => "ListSwarms",
            "ListImages" => "ListImages",
            "InspectImage" => "InspectImage",
            "ListImageHistory" => "ListImageHistory",
            "ListNetworks" => "ListNetworks",
            "InspectNetwork" => "InspectNetwork",
            "ListVolumes" => "ListVolumes",
            "InspectVolume" => "InspectVolume",
            "InspectSwarm" => "InspectSwarm",
            "ListSwarmNodes" => "ListSwarmNodes",
            "InspectSwarmNode" => "InspectSwarmNode",
            "ListSwarmServices" => "ListSwarmServices",
            "InspectSwarmService" => "InspectSwarmService",
            "ListSwarmTasks" => "ListSwarmTasks",
            "InspectSwarmTask" => "InspectSwarmTask",
            "GetSwarmServiceLog" => "GetSwarmServiceLog",
            "GetRepo" => "GetRepo",
            "ListRepos" => "ListRepos",
            "GetBuild" => "GetBuild",
            "ListBuilds" => "ListBuilds",
            "GetBuildActionState" => "GetBuildActionState",
            "GetProcedure" => "GetProcedure",
            "ListProcedures" => "ListProcedures",
            "GetProcedureActionState" => "GetProcedureActionState",
            "GetAction" => "GetAction",
            "ListActions" => "ListActions",
            "GetActionActionState" => "GetActionActionState",
            "ListSchedules" => "ListSchedules",
            "ListAlerters" => "ListAlerters",
            "ListResourceSyncs" => "ListResourceSyncs",
            "GetResourceSync" => "GetResourceSync",
            "ListUserGroups" => "ListUserGroups",
            "ExportResourcesToToml" => "ExportResourcesToToml",
            "GetBuilder" => "GetBuilder",
            "ListBuilders" => "ListBuilders",
            "GetAlerter" => "GetAlerter",
            "ListFullBuilders" => "ListFullBuilders",
            "ListFullAlerters" => "ListFullAlerters",
            "ListFullRepos" => "ListFullRepos",
            "ListFullStacks" => "ListFullStacks",
            "ListFullDeployments" => "ListFullDeployments",
            "ListContainers" => "ListContainers",
            "InspectContainer" => "InspectContainer",
            "GetServerState" => "GetServerState",
            "GetSystemStats" => "GetSystemStats",
            "ListAllStackServices" => "ListAllStackServices",
            "ListAlerts" => "ListAlerts",
            "ListUpdates" => "ListUpdates",
            "GetUpdate" => "GetUpdate",
            "GetContainerLog" => "GetContainerLog",
            "GetDeploymentLog" => "GetDeploymentLog",
            "GetStackLog" => "GetStackLog",
            _ => "read",
        };
        let read = async {
            if self.bound_reads
                || matches!(
                    T::req_type(),
                    "GetContainerLog"
                        | "ListVariables"
                        | "GetVariable"
                        | "ListSecrets"
                        | "ListImages"
                        | "InspectImage"
                        | "ListImageHistory"
                        | "ListNetworks"
                        | "InspectNetwork"
                        | "ListVolumes"
                        | "InspectVolume"
                        | "ListSwarms"
                        | "GetSwarm"
                        | "InspectSwarm"
                        | "ListSwarmNodes"
                        | "InspectSwarmNode"
                        | "ListSwarmServices"
                        | "InspectSwarmService"
                        | "ListSwarmTasks"
                        | "InspectSwarmTask"
                        | "GetSwarmServiceLog"
                        | "ExportResourcesToToml"
                        | "GetDeploymentLog"
                        | "GetStackLog"
                        | "ListAlerts"
                        | "ListUpdates"
                        | "GetUpdate"
                )
            {
                self.read_bounded_body(request, operation).await
            } else {
                self.upstream
                    .sdk
                    .read(request)
                    .await
                    .map_err(|error| AppError::from_upstream(operation, error))
            }
        };
        match tokio::time::timeout(self.upstream.policy.operation_timeout, read).await {
            Err(_) => Err(AppError::new(ErrorKind::Timeout, operation, None)),
            Ok(result) => result,
        }
    }

    async fn read_bounded_body<T>(
        &self,
        request: T,
        operation: &'static str,
    ) -> Result<T::Response, AppError>
    where
        T: Serialize + KomodoReadRequest,
        T::Response: DeserializeOwned,
    {
        self.request_bounded_body(
            &self.upstream.bounded_request,
            request,
            T::req_type(),
            operation,
        )
        .await
    }

    async fn request_bounded_body<T, R>(
        &self,
        builder: &reqwest::RequestBuilder,
        request: T,
        request_type: &'static str,
        operation: &'static str,
    ) -> Result<R, AppError>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        let body = serde_json::json!({"type":request_type,"params":request});
        if request_type == "ListSecrets" && !body["params"]["target"].is_null() {
            return Err(AppError::arguments(
                "ListSecrets",
                "Only configured Core secret key names with target=None are supported; Server/Builder expansion and configured values are not exposed",
            ));
        }
        let inspected_object = match operation {
            "InspectContainer" => body["params"]["container"].as_str(),
            "InspectImage" | "ListImageHistory" => body["params"]["image"].as_str(),
            "InspectNetwork" => body["params"]["network"].as_str(),
            "InspectVolume" => body["params"]["volume"].as_str(),
            _ => None,
        };
        let mut response = self
            .clone_request(builder, operation)?
            .json(&body)
            .send()
            .await
            .map_err(|error| AppError::from_upstream(operation, error.into()))?;
        self.decode_bounded_body(&mut response, operation, inspected_object)
            .await
    }

    fn clone_request(
        &self,
        builder: &reqwest::RequestBuilder,
        operation: &'static str,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        builder
            .try_clone()
            .ok_or_else(|| AppError::new(ErrorKind::InvalidResponse, operation, None))
    }

    async fn decode_bounded_body<R: DeserializeOwned>(
        &self,
        response: &mut reqwest::Response,
        operation: &'static str,
        inspected_object: Option<&str>,
    ) -> Result<R, AppError> {
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return Err(AppError::from_http_failure(operation, status, &[]));
        }
        let too_large = || AppError::response_too_large(operation, status);
        if response
            .content_length()
            .is_some_and(|size| size > MAX_EVIDENCE_RESPONSE_BYTES as u64)
        {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            AppError::new(
                if status.is_success() {
                    ErrorKind::InvalidResponse
                } else {
                    ErrorKind::Upstream
                },
                operation,
                Some(status),
            )
        })? {
            if chunk.len() > MAX_EVIDENCE_RESPONSE_BYTES - body.len() {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            if operation == "InspectContainer" {
                return Err(AppError::from_container_http_failure(
                    status,
                    &body,
                    inspected_object,
                ));
            }
            if matches!(
                operation,
                "ListImages"
                    | "InspectImage"
                    | "ListImageHistory"
                    | "ListNetworks"
                    | "InspectNetwork"
                    | "ListVolumes"
                    | "InspectVolume"
            ) {
                return Err(AppError::from_docker_http_failure(
                    operation,
                    status,
                    &body,
                    inspected_object,
                ));
            }
            return Err(AppError::from_http_failure(operation, status, &body));
        }
        serde_json::from_slice(&body)
            .map_err(|_| AppError::new(ErrorKind::InvalidResponse, operation, Some(status)))
    }

    pub async fn detect_version(&self) -> Result<CoreVersion, AppError> {
        let response = self.read(GetVersion {}).await?;
        semver::Version::parse(&response.version)
            .map_err(|_| AppError::new(ErrorKind::InvalidResponse, "GetVersion", None))?;
        Ok(CoreVersion(
            self.upstream.redactor.redact(&response.version),
        ))
    }

    pub(crate) fn redact(&self, value: &str) -> String {
        self.upstream.redactor.redact(value)
    }

    pub(crate) fn redaction_marker(&self) -> &str {
        self.upstream.redactor.marker()
    }
}

#[cfg(test)]
mod write_policy_tests {
    use super::*;
    use komodo_client::{
        api::write::{CreateServer, UpdateDeployment, UpdateServer, UpdateStack},
        entities::{
            deployment::{DeploymentImage, PartialDeploymentConfig},
            server::PartialServerConfig,
            stack::PartialStackConfig,
        },
    };

    #[tokio::test]
    async fn shared_variable_transport_guards_effects_and_forbidden_routes_before_io() {
        use komodo_client::api::write::{
            CreateVariable, DeleteVariable, UpdateVariableDescription, UpdateVariableIsSecret,
            UpdateVariableValue,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for operate in [false, true] {
            for edit in [false, true] {
                let client = CoreClient::new(
                    Config::parse(
                        &format!("http://{}", listener.local_addr().unwrap()),
                        "fixture-key",
                        "fixture-secret",
                    )
                    .unwrap()
                    .with_capabilities(CapabilityPolicy::new(operate, edit)),
                    RequestPolicy::default(),
                )
                .unwrap();
                if !operate || !edit {
                    assert!(
                        matches!(client.variable_write_checked(CreateVariable{name:"APP_MODE".into(),value:"blue".into(),description:String::new(),is_secret:false},&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
                    );
                    assert!(
                        matches!(client.variable_write_checked(UpdateVariableValue{name:"APP_MODE".into(),value:"green".into()},&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
                    );
                    assert!(
                        matches!(client.variable_write_checked(DeleteVariable{name:"APP_MODE".into()},&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
                    );
                }
                if !edit {
                    assert!(
                        matches!(client.variable_write_checked(UpdateVariableDescription{name:"APP_MODE".into(),description:String::new()},&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
                    );
                }
                assert!(
                    client
                        .variable_write_checked(
                            CreateVariable {
                                name: "NEW_SECRET".into(),
                                value: "forbidden".into(),
                                description: String::new(),
                                is_secret: true
                            },
                            &serde_json::Value::Null
                        )
                        .await
                        .is_err()
                );
                assert!(
                    matches!(client.variable_write_checked(UpdateVariableIsSecret{name:"APP_MODE".into(),is_secret:false},&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::UnsupportedTarget)
                );
                assert!(
                    matches!(client.write(CreateVariable{name:"APP_MODE".into(),value:"blue".into(),description:String::new(),is_secret:false}).await,Err(e)if e.kind==ErrorKind::UnsupportedTarget)
                );
                assert!(
                    matches!(client.delete(DeleteVariable{name:"APP_MODE".into()}).await,Err(e)if e.kind==ErrorKind::UnsupportedTarget)
                );
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_secret_name_reads_refuse_server_and_builder_expansion_before_io() {
        use komodo_client::{api::read::ListSecrets, entities::ResourceTarget};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(true, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        for target in [
            ResourceTarget::Server("000000000000000000000001".into()),
            ResourceTarget::Builder("000000000000000000000002".into()),
        ] {
            assert!(
                matches!(client.read(ListSecrets{target:Some(target.clone())}).await,Err(e)if e.kind==ErrorKind::InvalidArguments)
            );
            assert!(
                matches!(client.read_references(ListSecrets{target:Some(target)}).await,Err(e)if e.kind==ErrorKind::InvalidArguments)
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_variable_payload_cannot_smuggle_unclassified_fields_before_io() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(true, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        for (operation, params) in [
            (
                "CreateVariable",
                serde_json::json!({"name":"APP_MODE","value":"blue","description":"","is_secret":false,"future_field":"forbidden"}),
            ),
            (
                "UpdateVariableValue",
                serde_json::json!({"name":"APP_MODE","value":"blue","is_secret":false}),
            ),
            (
                "UpdateVariableDescription",
                serde_json::json!({"name":"APP_MODE","description":"metadata","value":"forbidden"}),
            ),
            (
                "DeleteVariable",
                serde_json::json!({"name":"APP_MODE","all":true}),
            ),
        ] {
            assert!(
                matches!(crate::variables::guard_write(&client,operation,&params,&serde_json::Value::Null).await,Err(e)if e.kind==ErrorKind::InvalidArguments)
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn refresh_transport_requires_operate_and_exact_id_and_has_no_generic_write_escape() {
        use komodo_client::api::write::RefreshResourceSyncPending;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for (operate, edit) in [(false, false), (false, true), (true, false), (true, true)] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
                RequestPolicy::default(),
            )
            .unwrap();
            if !operate {
                assert!(
                    matches!(client.refresh_sync_pending("00000000000000000000000b").await,Err(e) if e.kind==ErrorKind::CapabilityDisabled)
                );
            }
            assert!(
                client
                    .refresh_sync_pending("unresolved-name")
                    .await
                    .is_err()
            );
            assert!(
                client
                    .write(RefreshResourceSyncPending {
                        sync: "00000000000000000000000b".into()
                    })
                    .await
                    .is_err()
            );
            assert!(
                client
                    .execute(komodo_client::api::execute::RunSync {
                        sync: "00000000000000000000000b".into(),
                        resource_type: None,
                        resources: None
                    })
                    .await
                    .is_err(),
                "generic execution must never mint reviewed sync authority"
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn refresh_timeout_preserves_one_submission_without_retry() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(true, false)),
            RequestPolicy {
                connect_timeout: Duration::from_secs(1),
                operation_timeout: Duration::from_millis(100),
            },
        )
        .unwrap();
        let observer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !String::from_utf8_lossy(&bytes).contains("RefreshResourceSyncPending") {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0 && bytes.len() + n <= 8192);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let request = String::from_utf8_lossy(&bytes);
            assert!(
                request.contains("POST /write") && request.contains("RefreshResourceSyncPending")
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
        });
        assert!(
            matches!(client.refresh_sync_pending("00000000000000000000000b").await,Err(e) if e.kind==ErrorKind::Timeout)
        );
        observer.await.unwrap();
    }

    #[tokio::test]
    async fn shared_procedure_transport_rejects_unsafe_defaults_and_empty_update_without_operate() {
        use komodo_client::api::write::{CreateProcedure, UpdateProcedure};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(false, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        for r in [
            client
                .write(CreateProcedure {
                    name: "inherited-unsafe-automation".into(),
                    config: Default::default(),
                })
                .await,
            client
                .write(UpdateProcedure {
                    id: "000000000000000000000030".into(),
                    config: Default::default(),
                })
                .await,
        ] {
            assert!(matches!(r,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_action_transport_rejects_unsafe_defaults_and_empty_update_without_operate() {
        use komodo_client::api::write::{CreateAction, UpdateAction};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(false, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        for r in [
            client
                .write(CreateAction {
                    name: "inherited-unsafe-automation".into(),
                    config: Default::default(),
                })
                .await,
            client
                .write(UpdateAction {
                    id: "000000000000000000000030".into(),
                    config: Default::default(),
                })
                .await,
        ] {
            assert!(matches!(r,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_schedule_partials_require_both_grants_before_any_io() {
        use komodo_client::{
            api::write::{UpdateAction, UpdateProcedure},
            entities::{action::PartialActionConfig, procedure::PartialProcedureConfig},
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for (operate, edit) in [(false, false), (false, true), (true, false)] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
                RequestPolicy::default(),
            )
            .unwrap();
            let action = client
                .write(UpdateAction {
                    id: "000000000000000000000040".into(),
                    config: PartialActionConfig {
                        schedule_enabled: Some(true),
                        ..Default::default()
                    },
                })
                .await;
            assert!(matches!(action,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
            let procedure = client
                .write(UpdateProcedure {
                    id: "000000000000000000000030".into(),
                    config: PartialProcedureConfig {
                        schedule: Some("0 0 9 * * *".into()),
                        ..Default::default()
                    },
                })
                .await;
            assert!(matches!(procedure,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_saved_execute_requires_operate_before_preflight_io() {
        use komodo_client::api::execute::{
            CancelAction, CancelBuild, CancelProcedure, RunAction, RunBuild, RunProcedure,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for edit in [false, true] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(false, edit)),
                RequestPolicy::default(),
            )
            .unwrap();
            for r in [
                client
                    .execute(RunAction {
                        action: "000000000000000000000040".into(),
                        args: None,
                    })
                    .await,
                client
                    .execute(CancelAction {
                        action: "000000000000000000000040".into(),
                        update_id: Some("000000000000000000000100".into()),
                    })
                    .await,
                client
                    .execute(RunBuild {
                        build: "000000000000000000000011".into(),
                    })
                    .await,
                client
                    .execute(CancelBuild {
                        build: "000000000000000000000011".into(),
                    })
                    .await,
                client
                    .execute(RunProcedure {
                        procedure: "000000000000000000000030".into(),
                    })
                    .await,
                client
                    .execute(CancelProcedure {
                        procedure: "000000000000000000000030".into(),
                        update_id: None,
                    })
                    .await,
            ] {
                assert!(matches!(r,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_delete_transport_requires_both_grants_without_opening_a_connection() {
        use komodo_client::api::write::{DeleteDeployment, DeleteServer, DeleteStack, DeleteTag};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for (operate, edit) in [(false, false), (true, false), (false, true)] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
                RequestPolicy::default(),
            )
            .unwrap();
            let id = "000000000000000000000001".to_owned();
            for r in [
                client.delete(DeleteStack { id: id.clone() }).await,
                client.delete(DeleteDeployment { id: id.clone() }).await,
                client.delete(DeleteServer { id: id.clone() }).await,
                client.delete(DeleteTag { id }).await,
            ] {
                assert!(matches!(r,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_tag_transport_requires_configuration_and_refuses_unrelated_metadata_before_io()
    {
        use komodo_client::{
            api::write::{CreateTag, RenameTag, UpdateResourceMeta},
            entities::ResourceTarget,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for operate in [false, true] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, false)),
                RequestPolicy::default(),
            )
            .unwrap();
            for r in [
                client
                    .tag_write(CreateTag {
                        name: "tag".into(),
                        color: None,
                    })
                    .await,
                client
                    .tag_write(RenameTag {
                        id: "000000000000000000007001".into(),
                        name: "new".into(),
                    })
                    .await,
                client
                    .tag_write(UpdateResourceMeta {
                        target: ResourceTarget::Stack("000000000000000000000002".into()),
                        description: None,
                        template: None,
                        tags: Some(vec![]),
                    })
                    .await,
            ] {
                assert!(matches!(r,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
            }
        }
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(false, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        let r = client
            .tag_write(UpdateResourceMeta {
                target: ResourceTarget::Stack("000000000000000000000002".into()),
                description: Some("unrelated".into()),
                template: None,
                tags: Some(vec![]),
            })
            .await;
        assert!(matches!(r,Err(e)if e.kind==ErrorKind::InvalidArguments));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_sync_transport_rejects_external_writeback_even_with_both_grants() {
        use komodo_client::api::write::{CommitSync, WriteSyncFileContents};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CoreClient::new(
            Config::parse(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-key",
                "fixture-secret",
            )
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(true, true)),
            RequestPolicy::default(),
        )
        .unwrap();
        let commit = client
            .write(CommitSync {
                sync: "owned-sync".into(),
            })
            .await;
        let file = client
            .write(WriteSyncFileContents {
                sync: "owned-sync".into(),
                resource_path: String::new(),
                file_path: "owned.toml".into(),
                contents: "trusted".into(),
            })
            .await;
        assert!(matches!(commit,Err(e)if e.kind == ErrorKind::UnsupportedTarget));
        assert!(matches!(file,Err(e)if e.kind == ErrorKind::UnsupportedTarget));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_transport_denies_implicit_execution_fields_without_both_grants() {
        use komodo_client::{
            api::write::{CreateRepo, CreateResourceSync, UpdateRepo, UpdateResourceSync},
            entities::{repo::PartialRepoConfig, sync::PartialResourceSyncConfig},
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for (operate, edit) in [(false, true), (true, false), (false, false)] {
            let client = CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
                RequestPolicy::default(),
            )
            .unwrap();
            for enabled in [None, Some(true)] {
                let sync_result = client
                    .write(CreateResourceSync {
                        name: "fixture-sync".into(),
                        config: PartialResourceSyncConfig {
                            webhook_enabled: enabled,
                            ..Default::default()
                        },
                    })
                    .await;
                assert!(matches!(sync_result,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
                let result = client
                    .write(CreateRepo {
                        name: "fixture".into(),
                        config: PartialRepoConfig {
                            webhook_enabled: enabled,
                            ..Default::default()
                        },
                    })
                    .await;
                assert!(matches!(result,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
            }
            for config in [
                serde_json::json!({"file_contents":"[[server]]\nname='trusted'"}),
                serde_json::json!({"linked_repo":"00000000000000000000000a"}),
                serde_json::json!({"repo":"fixture/source"}),
                serde_json::json!({"managed":true}),
                serde_json::json!({"delete":true}),
                serde_json::json!({"match_tags":[]}),
                serde_json::json!({"include_resources":false}),
                serde_json::json!({"include_variables":true}),
                serde_json::json!({"include_user_groups":true}),
                serde_json::json!({"webhook_enabled":true}),
                serde_json::json!({"webhook_secret":"trusted-secret"}),
            ] {
                let result = client
                    .write(UpdateResourceSync {
                        id: "00000000000000000000000b".into(),
                        config: serde_json::from_value(config).unwrap(),
                    })
                    .await;
                assert!(matches!(result,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
            }
            for config in [
                serde_json::json!({"webhook_enabled":true}),
                serde_json::json!({"repo":"fixture/changed"}),
                serde_json::json!({"branch":"next"}),
                serde_json::json!({"server_id":"000000000000000000000001"}),
                serde_json::json!({"git_account":"changed-reference"}),
                serde_json::json!({"path":"changed"}),
                serde_json::json!({"environment":"TOKEN=[[REFERENCE]]"}),
                serde_json::json!({"on_pull":{"path":"","command":"printf changed","shell_mode":true}}),
                serde_json::json!({"webhook_enabled":false,"on_clone":{"path":"","command":"printf changed","shell_mode":true}}),
            ] {
                let result = client
                    .write(UpdateRepo {
                        id: "00000000000000000000000a".into(),
                        config: serde_json::from_value::<PartialRepoConfig>(config).unwrap(),
                    })
                    .await;
                assert!(matches!(result,Err(e)if e.kind==ErrorKind::CapabilityDisabled));
            }
            for config in [
                PartialStackConfig {
                    repo: Some(String::new()),
                    ..Default::default()
                },
                PartialStackConfig {
                    linked_repo: Some(String::new()),
                    ..Default::default()
                },
                PartialStackConfig {
                    file_contents: Some(String::new()),
                    ..Default::default()
                },
                PartialStackConfig {
                    files_on_host: Some(false),
                    ..Default::default()
                },
            ] {
                let result = client
                    .write(UpdateStack {
                        id: "000000000000000000000002".into(),
                        config,
                    })
                    .await;
                assert!(matches!(result,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
            }
            for image in [
                DeploymentImage::Image {
                    image: "busybox:1.37.0".into(),
                },
                DeploymentImage::Build {
                    build_id: "00000000000000000000000b".into(),
                    version: Default::default(),
                },
            ] {
                let result = client
                    .write(UpdateDeployment {
                        id: "000000000000000000000003".into(),
                        config: PartialDeploymentConfig {
                            image: Some(image),
                            ..Default::default()
                        },
                    })
                    .await;
                assert!(matches!(result,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
            }
            for config in [
                PartialServerConfig {
                    address: Some(String::new()),
                    ..Default::default()
                },
                PartialServerConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
                PartialServerConfig {
                    insecure_tls: Some(false),
                    ..Default::default()
                },
            ] {
                let result = client
                    .write(UpdateServer {
                        id: "000000000000000000000001".into(),
                        config,
                    })
                    .await;
                assert!(matches!(result,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
            }
            let result = client
                .write(CreateServer {
                    name: "fixture".into(),
                    config: PartialServerConfig {
                        enabled: Some(true),
                        ..Default::default()
                    },
                    public_key: Some("explicit-key".into()),
                })
                .await;
            assert!(matches!(result,Err(e) if e.kind==ErrorKind::CapabilityDisabled));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_definition_transport_rejects_variants_and_effects_before_connection() {
        use komodo_client::{
            api::write::{CreateBuild, CreateBuilder, UpdateBuild, UpdateBuilder},
            entities::{
                build::PartialBuildConfig,
                builder::{PartialBuilderConfig, PartialServerBuilderConfig},
            },
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = |operate, edit| {
            CoreClient::new(
                Config::parse(
                    &format!("http://{}", listener.local_addr().unwrap()),
                    "fixture-key",
                    "fixture-secret",
                )
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
                RequestPolicy::default(),
            )
            .unwrap()
        };
        for variant in [
            PartialBuilderConfig::Aws(Default::default()),
            PartialBuilderConfig::Url(Default::default()),
        ] {
            assert!(
                matches!(client(true,true).write(CreateBuilder {name:"fixture".into(),config:variant.clone()}).await,Err(e)if e.kind==ErrorKind::UnsupportedTarget)
            );
            assert!(
                matches!(client(true,true).write(UpdateBuilder {id:"000000000000000000000010".into(),config:variant}).await,Err(e)if e.kind==ErrorKind::UnsupportedTarget)
            );
        }
        for (operate, edit) in [(false, true), (true, false), (false, false)] {
            let c = client(operate, edit);
            assert!(
                matches!(c.write(UpdateBuilder {id:"000000000000000000000010".into(),config:PartialBuilderConfig::Server(PartialServerBuilderConfig {server_ids:Some(vec!["000000000000000000000001".into()])})}).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
            );
            assert!(
                matches!(c.write(CreateBuild {name:"fixture".into(),config:PartialBuildConfig {repo:Some("fixture/workload".into()),webhook_enabled:Some(false),..Default::default()}}).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
            );
            assert!(
                matches!(c.write(UpdateBuild {id:"000000000000000000000011".into(),config:PartialBuildConfig {image_tag:Some("next".into()),..Default::default()}}).await,Err(e)if e.kind==ErrorKind::CapabilityDisabled)
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
}
