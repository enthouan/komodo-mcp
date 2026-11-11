use std::{fmt, sync::Arc, time::Duration};

use komodo_client::{
    KomodoClient,
    api::read::{GetVersion, KomodoReadRequest, ListServers, ListServersResponse},
};
use reqwest::{Client, redirect::Policy};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    config::Config,
    diagnostics::Redactor,
    error::{AppError, ErrorKind},
};

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
    policy: RequestPolicy,
    redactor: Redactor,
}

/// Clones share the same SDK and reqwest connection pool. No mutable request state.
#[derive(Clone)]
pub struct CoreClient(Arc<Inner>);

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
        let sdk = KomodoClient::new(config.address, config.key, config.secret).set_reqwest(http);
        Ok(Self(Arc::new(Inner {
            sdk,
            policy,
            redactor,
        })))
    }

    /// The only SDK entry point exposed to tools: typed reads with safe failures.
    pub async fn read<T>(&self, request: T) -> Result<T::Response, AppError>
    where
        T: Serialize + KomodoReadRequest,
        T::Response: DeserializeOwned,
    {
        let operation = match T::req_type() {
            "GetVersion" => "GetVersion",
            "ListServers" => "ListServers",
            "GetServer" => "GetServer",
            _ => "read",
        };
        match tokio::time::timeout(self.0.policy.operation_timeout, self.0.sdk.read(request)).await
        {
            Err(_) => Err(AppError::new(ErrorKind::Timeout, operation, None)),
            Ok(Err(error)) => Err(AppError::from_upstream(operation, error)),
            Ok(Ok(response)) => Ok(response),
        }
    }

    pub async fn detect_version(&self) -> Result<CoreVersion, AppError> {
        let response = self.read(GetVersion {}).await?;
        semver::Version::parse(&response.version)
            .map_err(|_| AppError::new(ErrorKind::InvalidResponse, "GetVersion", None))?;
        Ok(CoreVersion(self.0.redactor.redact(&response.version)))
    }

    /// One finite page for foundation tests; issue #3 adds public inventory tools.
    pub async fn list_servers(&self) -> Result<ListServersResponse, AppError> {
        self.read(ListServers {
            limit: Some(30),
            ..Default::default()
        })
        .await
    }
}
