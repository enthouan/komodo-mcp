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

struct Inner {
    sdk: KomodoClient,
    bounded_request: reqwest::RequestBuilder,
    execute_request: reqwest::RequestBuilder,
    write_request: reqwest::RequestBuilder,
    previews: crate::configuration::PreviewStore,
    policy: RequestPolicy,
    redactor: Redactor,
    capabilities: CapabilityPolicy,
}

/// Clones share the same SDK and reqwest connection pool. No mutable request state.
#[derive(Clone)]
pub struct CoreClient(Arc<Inner>, bool);

impl fmt::Debug for CoreClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreClient")
            .field("policy", &self.0.policy)
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
        let sdk = KomodoClient::new(config.address, config.key, config.secret).set_reqwest(http);
        Ok(Self(
            Arc::new(Inner {
                sdk,
                bounded_request,
                execute_request,
                write_request,
                previews: Default::default(),
                policy,
                redactor,
                capabilities,
            }),
            false,
        ))
    }

    pub fn capabilities(&self) -> CapabilityPolicy {
        self.0.capabilities
    }

    pub(crate) fn previews(&self) -> &crate::configuration::PreviewStore {
        &self.0.previews
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
            "CreateBuilder" => "CreateBuilder",
            "UpdateBuilder" => "UpdateBuilder",
            "CreateBuild" => "CreateBuild",
            "UpdateBuild" => "UpdateBuild",
            "CreateProcedure" => "CreateProcedure",
            "UpdateProcedure" => "UpdateProcedure",
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "write", None)),
        };
        crate::configuration::require_edit(self)?;
        let parameters = serde_json::to_value(request).map_err(|_| {
            AppError::arguments(operation, "Use a supported typed configuration request")
        })?;
        crate::builders::guard_write(self, operation, &parameters).await?;
        crate::procedures::guard_write(self, operation, &parameters)?;
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
            tokio::time::timeout(self.0.policy.operation_timeout, async {
                let response: serde_json::Value = self
                    .request_bounded_body(
                        &self.0.write_request,
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
            self.0.policy.operation_timeout,
            self.request_bounded_body(&self.0.write_request, request, T::req_type(), operation),
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
            self.0.policy.operation_timeout,
            self.request_bounded_body(&self.0.write_request, params, T::req_type(), operation),
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
        Ok(tokio::time::timeout(
            self.0.policy.operation_timeout,
            self.request_bounded_body(&self.0.execute_request, request, T::req_type(), operation),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None))))
    }

    /// Keep diagnosis inputs bounded without changing existing inventory calls.
    pub(crate) fn bounded_evidence(&self) -> Self {
        Self(Arc::clone(&self.0), true)
    }

    /// Full reference scans must distinguish missing reference fields from SDK
    /// defaults. Keep the raw bounded response private until its shape is checked.
    pub(crate) async fn read_references<T>(&self, request: T) -> Result<serde_json::Value, AppError>
    where
        T: Serialize + KomodoReadRequest,
    {
        let operation = match T::req_type() {
            op @ ("ListTags" | "ListServers" | "ListStacks" | "ListDeployments" | "ListSwarms"
            | "ListBuilds" | "ListRepos" | "ListBuilders" | "ListAlerters"
            | "ListProcedures" | "ListActions" | "ListResourceSyncs" | "GetSwarm"
            | "GetBuild" | "GetProcedure" | "GetAction" | "GetResourceSync") => op,
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
            self.0.policy.operation_timeout,
            self.request_bounded_body(&self.0.bounded_request, request, T::req_type(), operation),
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
            "ListServers" => "ListServers",
            "GetServer" => "GetServer",
            "ListStacks" => "ListStacks",
            "GetStack" => "GetStack",
            "ListDeployments" => "ListDeployments",
            "GetDeployment" => "GetDeployment",
            "GetSwarm" => "GetSwarm",
            "ListSwarms" => "ListSwarms",
            "GetRepo" => "GetRepo",
            "ListRepos" => "ListRepos",
            "GetBuild" => "GetBuild",
            "ListBuilds" => "ListBuilds",
            "GetBuildActionState" => "GetBuildActionState",
            "GetProcedure" => "GetProcedure",
            "ListProcedures" => "ListProcedures",
            "GetProcedureActionState" => "GetProcedureActionState",
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
            if self.1
                || matches!(
                    T::req_type(),
                    "GetContainerLog"
                        | "GetDeploymentLog"
                        | "GetStackLog"
                        | "ListAlerts"
                        | "ListUpdates"
                        | "GetUpdate"
                )
            {
                self.read_bounded_body(request, operation).await
            } else {
                self.0
                    .sdk
                    .read(request)
                    .await
                    .map_err(|error| AppError::from_upstream(operation, error))
            }
        };
        match tokio::time::timeout(self.0.policy.operation_timeout, read).await {
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
        self.request_bounded_body(&self.0.bounded_request, request, T::req_type(), operation)
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
        let inspected_container = (operation == "InspectContainer")
            .then(|| body["params"]["container"].as_str())
            .flatten();
        let mut response = self
            .clone_request(builder, operation)?
            .json(&body)
            .send()
            .await
            .map_err(|error| AppError::from_upstream(operation, error.into()))?;
        self.decode_bounded_body(&mut response, operation, inspected_container)
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
        inspected_container: Option<&str>,
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
                    inspected_container,
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
        Ok(CoreVersion(self.0.redactor.redact(&response.version)))
    }

    pub(crate) fn redact(&self, value: &str) -> String {
        self.0.redactor.redact(value)
    }

    pub(crate) fn redaction_marker(&self) -> &str {
        self.0.redactor.marker()
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
    async fn shared_saved_execute_requires_operate_before_preflight_io() {
        use komodo_client::api::execute::{CancelBuild, CancelProcedure, RunBuild, RunProcedure};
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
    async fn shared_transport_denies_implicit_execution_fields_without_both_grants() {
        use komodo_client::{
            api::write::{CreateRepo, UpdateRepo},
            entities::repo::PartialRepoConfig,
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
