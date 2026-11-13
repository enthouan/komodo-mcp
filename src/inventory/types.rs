//! Public inventory contracts. Only these allowlisted fields leave the tools.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// Both branches are returned directly as structuredContent. isError on the
/// enclosing rmcp result distinguishes success from the shared safe error type.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ToolOutput<T> {
    Success(T),
    Failure(crate::error::AppError),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Zero-based inventory page. Keep page_size unchanged when continuing.
    #[serde(default)]
    pub page: u64,
    /// Maximum items per page, from 1 through 100. Defaults to 30.
    #[serde(default = "default_page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}

fn default_page_size() -> u64 {
    30
}

impl Default for ListInput {
    fn default() -> Self {
        Self {
            page: 0,
            page_size: default_page_size(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailInput {
    #[serde(default, deserialize_with = "selector_value")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "selector_value")]
    pub name: Option<String>,
}

fn selector_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl JsonSchema for DetailInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DetailInput".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "pattern": "^[0-9a-fA-F]{24}$", "description": "Exact stable resource ID returned by a list tool."},
                "name": {"type": "string", "minLength": 1, "pattern": "\\S", "description": "Exact, case-sensitive name within this family; never interpreted as an ID."}
            },
            "oneOf": [{"required": ["id"]}, {"required": ["name"]}],
            "additionalProperties": false
        })
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub has_more: bool,
    /// Null if the lookahead observed exhaustion; this is not a snapshot token.
    pub next_page: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Server,
    Swarm,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentTarget {
    pub kind: TargetKind,
    pub id: String,
    /// Null if Core did not supply a display name. The ID remains usable.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    Ok,
    NotOk,
    Disabled,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StackState {
    Deploying,
    Running,
    Paused,
    Stopped,
    Created,
    Restarting,
    Dead,
    Removing,
    Unhealthy,
    Down,
    Unknown,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    Deploying,
    Running,
    Created,
    Restarting,
    Stopping,
    Removing,
    Paused,
    Exited,
    Dead,
    Unhealthy,
    NotDeployed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: ServerState,
    pub region: Option<String>,
    pub periphery_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerDetail {
    #[serde(flatten)]
    pub summary: ServerSummary,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: StackState,
    pub target: Option<DeploymentTarget>,
    /// Count of services reported by Core, not a count of running containers.
    pub service_count: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackDetail {
    #[serde(flatten)]
    pub summary: StackSummary,
    /// Deployed project name, then configured project name, then resource name.
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: DeploymentState,
    pub target: Option<DeploymentTarget>,
    /// Core's cached image, which can fall back to desired configuration when
    /// runtime cache data is missing. This does not verify the running image.
    pub image: Option<String>,
    pub runtime_name: Option<String>,
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentDetail {
    #[serde(flatten)]
    pub summary: DeploymentSummary,
}
