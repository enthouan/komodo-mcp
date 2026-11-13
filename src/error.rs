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
    StalePreview,
    InvalidPreview,
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
            Self::StalePreview => "stale_preview",
            Self::InvalidPreview => "invalid_preview",
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
            Self::StalePreview => {
                "The authoritative definition changed. Inspect it and prepare a new preview; no write was submitted."
            }
            Self::InvalidPreview => {
                "The preview is missing, expired, consumed, or does not match this target and patch. Prepare a new preview; no write was submitted."
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
            "DeleteStack" | "DeleteDeployment" | "DeleteServer" | "DeleteTag" => Some(
                "Deletion or cleanup may already have occurred. Inspect the exact resource/tag ID and runtime or assignment effects separately; never automatically repeat deletion.",
            ),
            "UpdateStack" | "CreateStack" | "UpdateDeployment" | "CreateDeployment"
            | "CreateServer" | "UpdateServer" | "CreateTag" | "RenameTag"
            | "UpdateResourceMeta" => Some(
                "The configuration may already have been saved. Inspect the definition; never automatically repeat the write.",
            ),
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
            error.message = if matches!(
                operation,
                "UpdateStack"
                    | "CreateStack"
                    | "UpdateDeployment"
                    | "CreateDeployment"
                    | "CreateServer"
                    | "UpdateServer"
            ) {
                "Core configuration response exceeded its input budget."
            } else if matches!(
                operation,
                "DeleteStack" | "DeleteDeployment" | "DeleteServer"
            ) {
                "Core deletion acknowledgement exceeded its input budget."
            } else if matches!(operation, "DeployStack" | "Deploy") {
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
                && matches!(operation, "RenameTag" | "DeleteTag")
            {
                "Core denied this tag change; rename/delete requires this tag's owner or an admin. Keep the same identity; no retry or credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "CreateTag" {
                "Core denied tag creation; check the enabled caller and Core's non-admin creation policy. No retry or credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "UpdateResourceMeta" {
                "Core denied assignment; check Write on the exact resource for the same enabled identity. No retry or credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "CreateStack" {
                "Core denied Stack creation; check the enabled user's creation permissions, Core's non-admin creation policy, and Read/Attach on the target and linked Repo. No credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied
                && matches!(
                    operation,
                    "DeleteStack" | "DeleteDeployment" | "DeleteServer"
                )
            {
                "Core denied deletion; check Write access on this exact resource for the same enabled identity. No retry, credential fallback or unrelated deletion was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "CreateServer" {
                "Core denied Server creation; check the enabled user's create_server_permissions and Core's non-admin creation policy, or use an authorized admin. No credential fallback or provisioning was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "UpdateServer" {
                "Core denied Server editing; check Write access on this exact Server for the same enabled identity. No key update, rotation, credential fallback or provisioning was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "CreateDeployment" {
                "Core denied Deployment creation; check the enabled user's creation permissions, Core's non-admin creation policy, and Read/Attach on the target and Build. No credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "UpdateDeployment" {
                "Core denied configuration editing; check Write access on this Deployment and Read/Attach on any new target or Build for the same identity. No credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied && operation == "UpdateStack" {
                "Core denied configuration editing; check Write access on this Stack and Read/Attach on any new target or linked Repo for the same identity. No credential fallback was attempted."
            } else if kind == ErrorKind::PermissionDenied
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

    pub(crate) fn from_container_http_failure(
        status: StatusCode,
        body: &[u8],
        locator: Option<&str>,
    ) -> Self {
        let error = Self::from_http_failure("InspectContainer", status, body);
        if error.kind != ErrorKind::NotFound {
            return error;
        }
        let exact = locator
            .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .is_some_and(|id| {
                let expected =
                    format!("Docker responded with status code 404: No such container: {id}");
                serde_json::from_slice::<serde_json::Value>(body).is_ok_and(|v| {
                    v["error"] == expected
                        || v["trace"]
                            .as_array()
                            .is_some_and(|trace| trace.iter().any(|s| s == &expected))
                })
            });
        if exact {
            error
        } else {
            Self::new(ErrorKind::Upstream, "InspectContainer", Some(status))
        }
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
    // Periphery 2.3.2 uses Bollard 0.21.0's exact Docker 404 message.
    // Inspection is issued by full validated IDs. No generic 'not found'
    // substring, HTTP 404, empty cache, or missing Server establishes absence.
    if operation == "InspectContainer" {
        return message
            .strip_prefix("Docker responded with status code 404: No such container: ")
            .is_some_and(|id| id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()));
    }
    let resource = match operation {
        "GetServer" => "Server",
        "GetStack" => "Stack",
        "GetDeployment" => "Deployment",
        "GetSwarm" => "Swarm",
        "GetRepo" => "Repo",
        "GetBuild" => "Build",
        "GetBuilder" => "Builder",
        "GetAlerter" => "Alerter",
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
    if message == "user must be tag owner or admin" {
        return true;
    }
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
        ["Read", "Write", "Execute"].iter().any(|level| {
            let prefix=format!("User does not have required permissions on this {resource}. Must have at least {level} permissions");
            message == prefix || message == format!("{prefix}, as well as these specific permissions: [Attach]")
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
        } else if let Some(correction) = self.correction {
            write!(f, " {correction}.")?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    #[test]
    fn container_absence_requires_exact_known_id_and_pinned_failure_shape() {
        let id = "11".repeat(32);
        let missing = |suffix: &str| {
            serde_json::to_vec(&serde_json::json!({"error":format!("Docker responded with status code 404: No such container: {suffix}")})).unwrap()
        };
        assert_eq!(
            AppError::from_container_http_failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                &missing(&id),
                Some(&id)
            )
            .kind,
            ErrorKind::NotFound
        );
        for (status,body,locator) in [
            (StatusCode::INTERNAL_SERVER_ERROR,missing(&"22".repeat(32)),Some(id.as_str())),
            (StatusCode::INTERNAL_SERVER_ERROR,missing(&id),Some("short-id")),
            (StatusCode::NOT_FOUND,missing(&id),Some(id.as_str())),
            (StatusCode::INTERNAL_SERVER_ERROR,serde_json::to_vec(&serde_json::json!({"error":"Did not find any Server matching 000000000000000000000001"})).unwrap(),Some(id.as_str())),
        ] {
            assert_ne!(AppError::from_container_http_failure(status,&body,locator).kind,ErrorKind::NotFound);
        }
    }
}
