use std::{fmt, sync::Arc, time::Duration};

use komodo_client::{
    KomodoClient,
    api::read::{GetVersion, KomodoReadRequest},
};
use reqwest::{Client, redirect::Policy};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    capabilities::CapabilityPolicy,
    config::Config,
    diagnostics::Redactor,
    error::{AppError, ErrorKind},
};

/// JSON body budget for log and incident-history reads, before typed decoding.
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
        let sdk = KomodoClient::new(config.address, config.key, config.secret).set_reqwest(http);
        Ok(Self(
            Arc::new(Inner {
                sdk,
                bounded_request,
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

    /// Keep diagnosis inputs bounded without changing existing inventory calls.
    pub(crate) fn bounded_evidence(&self) -> Self {
        Self(Arc::clone(&self.0), true)
    }

    /// The only Core entry point exposed to tools: typed reads with safe failures.
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
        let mut response = self
            .0
            .bounded_request
            .try_clone()
            .ok_or_else(|| AppError::new(ErrorKind::InvalidResponse, operation, None))?
            .json(&serde_json::json!({"type":T::req_type(),"params":request}))
            .send()
            .await
            .map_err(|error| AppError::from_upstream(operation, error.into()))?;
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
