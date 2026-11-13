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
        let operation = match T::req_type() {
            "UpdateStack" => "UpdateStack",
            "CreateStack" => "CreateStack",
            "CreateDeployment" => "CreateDeployment",
            "UpdateDeployment" => "UpdateDeployment",
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "write", None)),
        };
        crate::configuration::require_edit(self)?;
        let parameters = serde_json::to_value(request).map_err(|_| {
            AppError::arguments(operation, "Use a supported typed configuration request")
        })?;
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
        let expected_config = parameters
            .get("config")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .ok_or_else(|| {
                AppError::arguments(operation, "Use a typed partial configuration object")
            })?;
        tokio::time::timeout(self.0.policy.operation_timeout, async {
            let response: serde_json::Value = self
                .request_bounded_body(&self.0.write_request, parameters, T::req_type(), operation)
                .await?;
            // SDK defaults must not turn an absent echoed field into a verified save.
            if !response["config"].is_object()
                || !expected_config
                    .iter()
                    .all(|(key, value)| response["config"].get(key) == Some(value))
            {
                return Err(AppError::new(ErrorKind::InvalidResponse, operation, None));
            }
            serde_json::from_value(response)
                .map_err(|_| AppError::new(ErrorKind::InvalidResponse, operation, None))
        })
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// One explicit typed submission, over the existing authenticated pool.
    /// Every uncertain failure is returned without retrying or changing credentials.
    pub(crate) async fn execute<T>(&self, request: T) -> Result<T::Response, AppError>
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
            _ => return Err(AppError::new(ErrorKind::UnsupportedTarget, "execute", None)),
        };
        if !self.capabilities().allows(Capability::Operate) {
            return Err(AppError::new(
                ErrorKind::CapabilityDisabled,
                operation,
                None,
            ));
        }
        tokio::time::timeout(
            self.0.policy.operation_timeout,
            self.request_bounded_body(&self.0.execute_request, request, T::req_type(), operation),
        )
        .await
        .unwrap_or_else(|_| Err(AppError::new(ErrorKind::Timeout, operation, None)))
    }

    /// Keep diagnosis inputs bounded without changing existing inventory calls.
    pub(crate) fn bounded_evidence(&self) -> Self {
        Self(Arc::clone(&self.0), true)
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
        let mut response = self
            .clone_request(builder, operation)?
            .json(&serde_json::json!({"type":request_type,"params":request}))
            .send()
            .await
            .map_err(|error| AppError::from_upstream(operation, error.into()))?;
        self.decode_bounded_body(&mut response, operation).await
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
        api::write::{UpdateDeployment, UpdateStack},
        entities::{
            deployment::{DeploymentImage, PartialDeploymentConfig},
            stack::PartialStackConfig,
        },
    };

    #[tokio::test]
    async fn shared_transport_denies_implicit_execution_fields_without_both_grants() {
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
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
}
