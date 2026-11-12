use std::fmt;

use reqwest::StatusCode;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// Stable public categories. No variant contains a raw upstream error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidArguments,
    NotFound,
    AmbiguousTarget,
    IncompleteResolution,
    Configuration,
    Authentication,
    PermissionDenied,
    Timeout,
    Unreachable,
    Upstream,
    InvalidResponse,
    ResponseTooLarge,
    CapabilityDisabled,
    UnsupportedTarget,
}

impl ErrorKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::NotFound => "not_found",
            Self::AmbiguousTarget => "ambiguous_target",
            Self::IncompleteResolution => "incomplete_resolution",
            Self::Configuration => "configuration",
            Self::Authentication => "authentication",
            Self::PermissionDenied => "permission_denied",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Upstream => "upstream",
            Self::InvalidResponse => "invalid_response",
            Self::ResponseTooLarge => "response_too_large",
            Self::CapabilityDisabled => "capability_disabled",
            Self::UnsupportedTarget => "unsupported_target",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::InvalidArguments => "Correct the tool arguments using its input schema.",
            Self::NotFound => {
                "Target was not found or is not visible; list this resource family, retry with a returned ID, and check Read access."
            }
            Self::AmbiguousTarget => {
                "Multiple visible targets match; retry with one explicit candidate ID."
            }
            Self::IncompleteResolution => {
                "The bounded name search could not establish a unique match; list this family and retry with an explicit ID."
            }
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
            Self::ResponseTooLarge => {
                "Core log data exceeded its input budget; request a smaller tail or narrower service filter."
            }
            Self::CapabilityDisabled => {
                "Enable the required local capability before requesting this operation."
            }
            Self::UnsupportedTarget => {
                "This workload or requested scope is unsupported; inspect the supported target matrix. No execution was submitted."
            }
        }
    }
}

/// Only application-owned messages and selected, nonsensitive metadata survive.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<Candidate>>,
}

/// A bounded ambiguity hint, never a raw upstream resource.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: String,
    pub name: String,
}

impl AppError {
    pub(crate) fn response_too_large(operation: &'static str, status: StatusCode) -> Self {
        let mut error = Self::new(ErrorKind::ResponseTooLarge, operation, Some(status));
        error.correction = match operation {
            "DeployStack" | "Deploy" => Some(
                "Submission may have been accepted. Inspect update history; never automatically repeat execution.",
            ),
            "GetUpdate" => Some(
                "Inspect the bounded summary with list_updates; Core's update detail cannot be narrowed.",
            ),
            "ListAlerts" | "ListUpdates" => {
                Some("Use a resource target filter or inspect another history page.")
            }
            "ListStacks" | "GetStack" | "ListAllStackServices" => {
                Some("Inspect this resource separately with fleet_health or get_stack.")
            }
            _ => None,
        };
        if error.correction.is_some() {
            error.message = if matches!(operation, "DeployStack" | "Deploy") {
                "Core execution acknowledgement exceeded its input budget."
            } else {
                "Core incident data exceeded its input budget."
            };
        }
        error
    }

    pub(crate) fn new(
        kind: ErrorKind,
        operation: &'static str,
        status: Option<StatusCode>,
    ) -> Self {
        Self {
            kind,
            message: if kind == ErrorKind::PermissionDenied
                && matches!(operation, "DeployStack" | "Deploy")
            {
                "Core denied execution; check Execute access on the selected workload for this identity."
            } else {
                kind.message()
            },
            operation,
            status: status.map(|status| status.as_u16()),
            field: None,
            correction: None,
            candidates: None,
        }
    }

    pub(crate) fn configuration(field: &'static str, correction: &'static str) -> Self {
        Self {
            field: Some(field),
            correction: Some(correction),
            ..Self::new(ErrorKind::Configuration, "configure", None)
        }
    }

    pub(crate) fn arguments(operation: &'static str, correction: &'static str) -> Self {
        Self {
            correction: Some(correction),
            ..Self::new(ErrorKind::InvalidArguments, operation, None)
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
        } else if status == Some(StatusCode::INTERNAL_SERVER_ERROR)
            && core_2_3_2_missing_target(operation, &error)
        {
            ErrorKind::NotFound
        } else if status.is_some_and(|status| status.is_success()) {
            ErrorKind::InvalidResponse
        } else {
            ErrorKind::Upstream
        };
        Self::new(kind, operation, status)
    }

    pub(crate) fn from_http_failure(
        operation: &'static str,
        status: StatusCode,
        body: &[u8],
    ) -> Self {
        // Match the pinned SDK's {error, trace} chain only after bounded reading.
        // Preserve known HTTP-500 failure classifications without
        // reconstructing, tracing, or returning an arbitrary error chain.
        #[derive(Deserialize)]
        struct CoreFailure {
            error: String,
            #[serde(default)]
            trace: Vec<String>,
        }
        let failure = (status == StatusCode::INTERNAL_SERVER_ERROR)
            .then(|| serde_json::from_slice::<CoreFailure>(body).ok())
            .flatten();
        let kind = if status == StatusCode::UNAUTHORIZED {
            ErrorKind::Authentication
        } else if status == StatusCode::FORBIDDEN
            || failure.as_ref().is_some_and(|failure| {
                core_2_3_2_permission_message(&failure.error)
                    || failure
                        .trace
                        .iter()
                        .any(|message| core_2_3_2_permission_message(message))
            })
        {
            ErrorKind::PermissionDenied
        } else if failure.as_ref().is_some_and(|failure| {
            let missing = |message: &str| {
                (operation == "GetUpdate" && message == "no update exists with given id")
                    || core_2_3_2_missing_message(operation, message)
            };
            missing(&failure.error) || failure.trace.iter().any(|message| missing(message))
        }) {
            ErrorKind::NotFound
        } else {
            ErrorKind::Upstream
        };
        Self::new(kind, operation, Some(status))
    }
}

// Only the tagged resource::get helper on the supported detail operations.
// Classification happens before discarding the chain; no target text survives.
fn core_2_3_2_missing_target(operation: &str, error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| core_2_3_2_missing_message(operation, &cause.to_string()))
}

fn core_2_3_2_missing_message(operation: &str, message: &str) -> bool {
    let resource = match operation {
        "GetServer" => "Server",
        "GetStack" => "Stack",
        "GetDeployment" => "Deployment",
        _ => return false,
    };
    let prefix = format!("Did not find any {resource} matching ");
    message.strip_prefix(&prefix).is_some_and(|target| {
        target.len() == 24 && target.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

// Core v2.3.2 permission::get_check_permissions returns anyhow -> mogh_error
// with the default HTTP 500. Match a complete known helper message, never a
// generic substring such as "permission" in an unrelated upstream failure.
fn core_2_3_2_permission_denial(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| core_2_3_2_permission_message(&cause.to_string()))
}

fn core_2_3_2_permission_message(message: &str) -> bool {
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
    RESOURCE_TYPES.iter().any(|resource| {
            message == format!("User does not have required permissions on this {resource}. Must have at least Read permissions")
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
        } else if let Some(correction) = self.correction {
            write!(f, " {correction}.")?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}
