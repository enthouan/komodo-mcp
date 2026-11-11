use std::fmt;

use reqwest::StatusCode;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

/// Stable public categories. No variant contains a raw upstream error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Configuration,
    Authentication,
    PermissionDenied,
    Timeout,
    Unreachable,
    Upstream,
    InvalidResponse,
}

impl ErrorKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Authentication => "authentication",
            Self::PermissionDenied => "permission_denied",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Upstream => "upstream",
            Self::InvalidResponse => "invalid_response",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Configuration => "Correct the indicated configuration field.",
            Self::Authentication => {
                "Core rejected authentication; check the API key, secret, and enabled user."
            }
            Self::PermissionDenied => {
                "Core denied access; grant the user the required resource read permission."
            }
            Self::Timeout => {
                "The Core request exceeded its deadline; check connectivity and Core health."
            }
            Self::Unreachable => {
                "Unable to reach Core; check KOMODO_ADDRESS, connectivity, and TLS certificates."
            }
            Self::Upstream => "Core returned an unexpected failure; check Core or proxy health.",
            Self::InvalidResponse => {
                "Core returned invalid or incompatible data; check the Core release and proxy configuration."
            }
        }
    }
}

/// Only application-owned messages and selected, nonsensitive metadata survive.
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: &'static str,
    pub operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction: Option<&'static str>,
}

impl AppError {
    pub(crate) fn new(
        kind: ErrorKind,
        operation: &'static str,
        status: Option<StatusCode>,
    ) -> Self {
        Self {
            kind,
            message: kind.message(),
            operation,
            status: status.map(|status| status.as_u16()),
            field: None,
            correction: None,
        }
    }

    pub(crate) fn configuration(field: &'static str, correction: &'static str) -> Self {
        Self {
            field: Some(field),
            correction: Some(correction),
            ..Self::new(ErrorKind::Configuration, "configure", None)
        }
    }

    /// Tool execution failures use `isError`, so the model can inspect and recover.
    /// Protocol/dispatch errors (e.g. unknown tools) remain rmcp ErrorData errors.
    pub fn into_tool_result(self) -> CallToolResult {
        let value = serde_json::to_value(&self).expect("application error is JSON serializable");
        let mut result = CallToolResult::error(vec![ContentBlock::text(self.to_string())]);
        result.structured_content = Some(value);
        result
    }

    pub(crate) fn from_upstream(operation: &'static str, error: anyhow::Error) -> Self {
        // The SDK attaches the typed HTTP status as anyhow context. Body errors
        // can be strings; never propagate, store, trace, or format this chain.
        let status = error.downcast_ref::<StatusCode>().copied();
        let transport = error.downcast_ref::<reqwest::Error>();
        let kind = if transport.is_some_and(reqwest::Error::is_timeout) {
            ErrorKind::Timeout
        } else if status == Some(StatusCode::UNAUTHORIZED) {
            ErrorKind::Authentication
        } else if status == Some(StatusCode::FORBIDDEN)
            || (status == Some(StatusCode::INTERNAL_SERVER_ERROR)
                && core_2_3_2_permission_denial(&error))
        {
            ErrorKind::PermissionDenied
        } else if transport.is_some_and(reqwest::Error::is_connect) {
            ErrorKind::Unreachable
        } else if status.is_some_and(|status| status.is_success()) {
            ErrorKind::InvalidResponse
        } else {
            ErrorKind::Upstream
        };
        Self::new(kind, operation, status)
    }
}

// Core v2.3.2 permission::get_check_permissions returns anyhow -> mogh_error
// with the default HTTP 500. Match a complete known helper message, never a
// generic substring such as "permission" in an unrelated upstream failure.
fn core_2_3_2_permission_denial(error: &anyhow::Error) -> bool {
    const RESOURCE_TYPES: &[&str] = &[
        "Server",
        "Swarm",
        "Stack",
        "Deployment",
        "Build",
        "Repo",
        "Procedure",
        "Action",
        "ResourceSync",
        "Builder",
        "Alerter",
    ];
    error.chain().any(|cause| {
        let message = cause.to_string();
        RESOURCE_TYPES.iter().any(|resource| {
            message == format!("User does not have required permissions on this {resource}. Must have at least Read permissions")
        })
    })
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}): {}",
            self.kind.id(),
            self.operation,
            self.message
        )?;
        if let Some(status) = self.status {
            write!(f, " HTTP {status}.")?;
        }
        if let (Some(field), Some(correction)) = (self.field, self.correction) {
            write!(f, " {field} {correction}.")?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}
