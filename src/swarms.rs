//! Read-only Swarm observations. Core lists are caches, not Docker snapshots.

use std::{collections::BTreeSet, time::Duration};

use komodo_client::{
    api::read::{
        GetSwarmServiceLog, InspectSwarm, InspectSwarmNode, InspectSwarmService, InspectSwarmTask,
        ListSwarmNodes, ListSwarmServices, ListSwarmTasks, ListSwarms,
    },
    entities::{
        ResourceTargetVariant,
        docker::{
            node::{SwarmNode, SwarmNodeListItem},
            service::{SwarmService, SwarmServiceListItem},
            task::{SwarmTask, SwarmTaskListItem, TaskState},
        },
        swarm::SwarmListItem,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
    client::CoreClient,
    error::{AppError, Candidate, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
    logs::{self, LogTarget, LogTargetKind, WorkloadLogs},
};

pub const MAX_SCAN: usize = 512;
const MAX_OUTPUT: usize = 96 * 1024;
const CACHE_LIMIT: &str = "Core cache; no generation timestamp or completeness guarantee. Empty may mean no objects, missing cache, or a suppressed Periphery poll failure. Local pagination does not bound Core/Docker enumeration. Separate reads are not an atomic snapshot.";
const FRESH_LIMIT: &str = "Fresh exact-object inspection through a configured Swarm manager; no cache generation or atomic relationship to other reads. Raw Docker configuration and error text are omitted.";

fn default_size() -> u64 {
    30
}
fn default_tail() -> u64 {
    100
}
fn default_bytes() -> usize {
    16_384
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTargetInput {
    pub target: DetailInput,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}

/// Docker IDs are full 25-character Swarm IDs, never ID prefixes.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectSelector {
    #[serde(default, deserialize_with = "crate::configuration::present")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "crate::configuration::present")]
    pub name: Option<String>,
}

impl JsonSchema for ObjectSelector {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SwarmObjectSelector".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"object", "properties":{
            "id":{"type":"string","pattern":"^[a-z0-9]{25}$"},
            "name":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[a-zA-Z0-9_.][a-zA-Z0-9_.-]*$"}
        },"oneOf":[{"required":["id"]},{"required":["name"]}],"additionalProperties":false})
    }
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectInput {
    pub target: DetailInput,
    pub object: ObjectSelector,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TasksInput {
    pub target: DetailInput,
    /// Omit for all cached tasks; an invalid/missing service never widens scope.
    #[serde(default, deserialize_with = "crate::configuration::present")]
    #[schemars(with = "ObjectSelector")]
    pub service: Option<ObjectSelector>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogsInput {
    pub target: DetailInput,
    pub service: ObjectSelector,
    #[serde(default = "default_tail")]
    #[schemars(range(min = 1, max = 500))]
    pub tail: u64,
    #[serde(default = "default_bytes")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_bytes: usize,
    #[serde(default)]
    pub timestamps: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct SwarmSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub manager_server_ids: Vec<String>,
    pub manager_servers_incomplete: bool,
    pub infrastructure_error_present: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct Observation<T> {
    pub swarm: SwarmSummary,
    pub source: &'static str,
    pub evidence_limit: &'static str,
    pub data: T,
}

#[derive(Serialize, JsonSchema)]
pub struct SwarmDetail {
    pub docker_swarm_id: String,
    pub root_rotation_in_progress: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
pub struct ObjectPage<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub next_page: Option<u64>,
    pub observed_count: usize,
    /// True when the local 512-object scan was clipped. Cache coverage remains
    /// unverified even when this is false; see the observation's evidence_limit.
    pub scan_incomplete: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Node {
    pub id: String,
    pub name: Option<String>,
    pub hostname: Option<String>,
    pub role: Option<String>,
    pub availability: Option<String>,
    pub state: Option<String>,
    pub leader: Option<bool>,
    pub reachability: Option<String>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Service {
    pub id: String,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub desired_replicas: Option<u64>,
    /// Docker often supplies this only on the cached list, not fresh inspect.
    pub running_tasks: Option<u64>,
    pub state: String,
    pub update_state: Option<String>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Task {
    pub id: String,
    pub name: Option<String>,
    pub service_id: Option<String>,
    pub node_id: Option<String>,
    pub state: Option<String>,
    pub desired_state: Option<String>,
    /// current = desired running; historical = desired shutdown/remove;
    /// unknown = Core did not establish either. Not a healthy-replica count.
    pub replica_evidence: &'static str,
    pub slot: Option<i64>,
    pub exit_code: Option<i64>,
    /// Conservative application-owned classification; never raw Docker errors.
    pub failure_reason: Option<&'static str>,
}

#[derive(Serialize, JsonSchema)]
pub struct Diagnosis {
    pub service: Service,
    pub cached_service: Service,
    pub cached_tasks: Vec<Task>,
    pub fresh_tasks: Vec<Task>,
    pub cached_nodes: Vec<Node>,
    pub fresh_nodes: Vec<Node>,
    pub cached_current_running: usize,
    pub cached_pending: usize,
    pub cached_unclassified_tasks: usize,
    pub historical_tasks: usize,
    pub under_replicated_in_cache: Option<bool>,
    pub scan_incomplete: bool,
    pub fresh_task_coverage_incomplete: bool,
    pub fresh_node_coverage_incomplete: bool,
    pub cached_display_incomplete: bool,
    pub gaps: Vec<AppError>,
    pub evidence_limit: &'static str,
}

fn invalid(op: &'static str) -> AppError {
    AppError::new(ErrorKind::InvalidResponse, op, None)
}
fn bounded_text(client: &CoreClient, value: &str, limit: usize) -> String {
    let redacted = client.redact(value);
    if redacted != value {
        return client.redaction_marker().into();
    }
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn unique<T>(items: &[T], id: impl Fn(&T) -> &str) -> Result<(), AppError> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(id(item)) {
            return Err(invalid("swarm_cache"));
        }
    }
    Ok(())
}
fn docker_id(value: &str) -> bool {
    value.len() == 25
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}
fn required_id(value: Option<&str>, op: &'static str) -> Result<String, AppError> {
    value
        .filter(|v| docker_id(v))
        .map(str::to_owned)
        .ok_or_else(|| invalid(op))
}
fn optional_id(value: Option<&str>, op: &'static str) -> Result<Option<String>, AppError> {
    value
        .filter(|v| !v.is_empty())
        .map(|v| required_id(Some(v), op))
        .transpose()
}
fn safe(client: &CoreClient, value: Option<&str>) -> Option<String> {
    value.map(|v| bounded_text(client, v, 128))
}
fn enum_text<T: Serialize>(value: Option<T>) -> Option<String> {
    value.and_then(|v| {
        serde_json::to_value(v)
            .ok()?
            .as_str()
            .map(|s| s.to_ascii_lowercase())
    })
}
fn finish<T: Serialize>(client: &CoreClient, value: T) -> Result<T, AppError> {
    if serde_json::to_vec(&inventory::projected_value(client, &value))
        .map_or(true, |v| v.len() > MAX_OUTPUT)
    {
        return Err(AppError::new(
            ErrorKind::ResponseTooLarge,
            "swarm_observation",
            None,
        ));
    }
    Ok(value)
}
fn target_input(target: &DetailInput) -> Result<(), AppError> {
    if target
        .name
        .as_ref()
        .is_some_and(|n| n.len() > 128 || n.chars().any(char::is_control))
    {
        return Err(AppError::arguments(
            "GetSwarm",
            "Use a bounded exact Swarm name or stable resource ID",
        ));
    }
    Ok(())
}
fn summary(client: &CoreClient, item: SwarmListItem) -> Result<SwarmSummary, AppError> {
    if item.resource_type != ResourceTargetVariant::Swarm
        || item.id.len() != 24
        || !item.id.bytes().all(|b| b.is_ascii_hexdigit())
        || item.name.trim().is_empty()
    {
        return Err(invalid("ListSwarms"));
    }
    let incomplete = item.info.server_ids.len() > 16;
    let managers = item
        .info
        .server_ids
        .into_iter()
        .take(16)
        .collect::<Vec<_>>();
    if managers
        .iter()
        .any(|id| id.len() != 24 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(invalid("ListSwarms"));
    }
    Ok(SwarmSummary {
        id: item.id,
        name: bounded_text(client, &item.name, 128),
        template: item.template,
        state: item.info.state.to_string().to_ascii_lowercase(),
        manager_server_ids: managers,
        manager_servers_incomplete: incomplete,
        infrastructure_error_present: item.info.err.is_some(),
    })
}
async fn resolve(client: &CoreClient, target: DetailInput) -> Result<SwarmSummary, AppError> {
    target_input(&target)?;
    let (mut item, full) = inventory::resolve_swarm(client, target).await?;
    if full.template || full.config.server_ids.is_empty() {
        return Err(AppError::new(
            ErrorKind::UnsupportedTarget,
            "GetSwarm",
            None,
        ));
    }
    item.info.server_ids = full.config.server_ids;
    summary(client, item)
}
fn observe<T>(swarm: SwarmSummary, data: T, fresh: bool) -> Observation<T> {
    Observation {
        swarm,
        source: if fresh { "fresh_inspect" } else { "core_cache" },
        evidence_limit: if fresh { FRESH_LIMIT } else { CACHE_LIMIT },
        data,
    }
}
fn offsets(page: u64, size: u64) -> Result<usize, AppError> {
    if !(1..=100).contains(&size) || page.checked_mul(size).is_none_or(|v| v > MAX_SCAN as u64) {
        return Err(AppError::arguments(
            "swarm_list",
            "Use page_size 1–100 and an offset at most 512; lists are locally bounded cache observations",
        ));
    }
    Ok((page * size) as usize)
}
fn page<T>(
    items: Vec<T>,
    index: u64,
    size: u64,
    observed: usize,
) -> Result<ObjectPage<T>, AppError> {
    let offset = offsets(index, size)?;
    let count = items.len();
    Ok(ObjectPage {
        items: items.into_iter().skip(offset).take(size as usize).collect(),
        page: index,
        page_size: size,
        next_page: (offset + (size as usize) < count).then_some(index + 1),
        observed_count: observed,
        scan_incomplete: observed > MAX_SCAN,
    })
}
fn selector(input: &ObjectSelector) -> Result<(), AppError> {
    match (&input.id, &input.name) {
        (Some(id), None) if docker_id(id) => Ok(()),
        (None, Some(name))
            if !name.is_empty()
                && name.len() <= 128
                && !name.starts_with('-')
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) =>
        {
            Ok(())
        }
        _ => Err(AppError::arguments(
            "swarm_select",
            "Provide exactly one full 25-character lowercase Docker Swarm ID or exact ASCII object name (1–128 letters, digits, dots, underscores or hyphens)",
        )),
    }
}
fn select<T>(
    client: &CoreClient,
    items: Vec<T>,
    input: &ObjectSelector,
    identity: impl Fn(&T) -> (&str, Option<&str>),
) -> Result<T, AppError> {
    selector(input)?;
    if items.len() > MAX_SCAN {
        return Err(AppError::new(
            ErrorKind::IncompleteResolution,
            "swarm_select",
            None,
        ));
    }
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for item in items {
        let (id, name) = identity(&item);
        if !docker_id(id) || !seen.insert(id.to_owned()) {
            return Err(invalid("swarm_select"));
        }
        if input.id.as_deref().is_some_and(|v| v == id)
            || input.name.as_deref().is_some_and(|v| Some(v) == name)
        {
            matches.push(item);
        }
    }
    if matches.len() > 1 {
        return Err(AppError {
            candidates: Some(
                matches
                    .iter()
                    .take(5)
                    .map(|item| {
                        let (id, name) = identity(item);
                        Candidate {
                            id: id.into(),
                            name: bounded_text(client, name.unwrap_or_default(), 128),
                        }
                    })
                    .collect(),
            ),
            ..AppError::new(ErrorKind::AmbiguousTarget, "swarm_select", None)
        });
    }
    matches
        .pop()
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, "swarm_select", None))
}
fn node_list(client: &CoreClient, v: SwarmNodeListItem) -> Result<Node, AppError> {
    Ok(Node {
        id: required_id(v.id.as_deref(), "ListSwarmNodes")?,
        name: safe(client, v.name.as_deref()),
        hostname: safe(client, v.hostname.as_deref()),
        role: enum_text(v.role),
        availability: enum_text(v.availability),
        state: enum_text(v.state),
        leader: None,
        reachability: None,
    })
}
fn node_detail(client: &CoreClient, v: SwarmNode) -> Result<Node, AppError> {
    let spec = v.spec.unwrap_or_default();
    let status = v.status.unwrap_or_default();
    let manager = v.manager_status.unwrap_or_default();
    Ok(Node {
        id: required_id(v.id.as_deref(), "InspectSwarmNode")?,
        name: safe(client, spec.name.as_deref()),
        hostname: safe(client, v.description.and_then(|v| v.hostname).as_deref()),
        role: enum_text(spec.role),
        availability: enum_text(spec.availability),
        state: enum_text(status.state),
        leader: manager.leader,
        reachability: enum_text(manager.reachability),
    })
}
pub(crate) fn service_list(
    client: &CoreClient,
    v: SwarmServiceListItem,
) -> Result<Service, AppError> {
    Ok(Service {
        id: required_id(v.id.as_deref(), "ListSwarmServices")?,
        name: safe(client, v.name.as_deref()),
        mode: enum_text(v.mode),
        desired_replicas: v
            .desired_tasks
            .or(v.replicas.and_then(|v| u64::try_from(v).ok())),
        running_tasks: v.running_tasks,
        state: v.state.to_string().to_ascii_lowercase(),
        update_state: None,
    })
}
pub(crate) fn service_detail(client: &CoreClient, v: SwarmService) -> Result<Service, AppError> {
    let spec = v.spec.unwrap_or_default();
    let replicas = v.replicas.or_else(|| {
        spec.mode
            .and_then(|v| v.replicated)
            .and_then(|v| v.replicas)
    });
    Ok(Service {
        id: required_id(v.id.as_deref(), "InspectSwarmService")?,
        name: safe(client, spec.name.as_deref()),
        mode: enum_text(v.mode),
        desired_replicas: replicas.and_then(|v| u64::try_from(v).ok()),
        running_tasks: v.service_status.and_then(|v| v.running_tasks),
        state: v.state.to_string().to_ascii_lowercase(),
        update_state: enum_text(v.update_status.and_then(|v| v.state)),
    })
}
fn replica(desired: Option<TaskState>) -> &'static str {
    match desired {
        Some(TaskState::RUNNING) => "current",
        Some(TaskState::SHUTDOWN | TaskState::REMOVE) => "historical",
        _ => "unknown",
    }
}
pub(crate) fn task_list(client: &CoreClient, v: SwarmTaskListItem) -> Result<Task, AppError> {
    Ok(Task {
        id: required_id(v.id.as_deref(), "ListSwarmTasks")?,
        name: safe(client, v.name.as_deref()),
        service_id: optional_id(v.service_id.as_deref(), "ListSwarmTasks")?,
        node_id: optional_id(v.node_id.as_deref(), "ListSwarmTasks")?,
        state: enum_text(v.state),
        desired_state: enum_text(v.desired_state),
        replica_evidence: replica(v.desired_state),
        slot: None,
        exit_code: None,
        failure_reason: None,
    })
}
fn failure(message: Option<&str>, state: Option<TaskState>) -> Option<&'static str> {
    let s = message.unwrap_or_default().to_ascii_lowercase();
    if s.contains("insufficient resources") {
        Some("insufficient_resources")
    } else if s.contains("scheduling constraints") || s.contains("constraints not satisfied") {
        Some("placement_constraints_unsatisfied")
    } else if s.contains("no suitable node") {
        Some("no_suitable_node")
    } else if s.contains("no such image")
        || s.contains("pull access denied")
        || s.contains("manifest unknown")
    {
        Some("image_unavailable")
    } else if s.contains("non-zero exit") {
        Some("nonzero_exit")
    } else if !s.is_empty() {
        Some("upstream_failure_details_omitted")
    } else if matches!(
        state,
        Some(TaskState::FAILED | TaskState::REJECTED | TaskState::ORPHANED)
    ) {
        Some("task_failed")
    } else {
        None
    }
}
pub(crate) fn task_detail(client: &CoreClient, v: SwarmTask) -> Result<Task, AppError> {
    let status = v.status.unwrap_or_default();
    Ok(Task {
        id: required_id(v.id.as_deref(), "InspectSwarmTask")?,
        name: safe(client, v.name.as_deref()),
        service_id: optional_id(v.service_id.as_deref(), "InspectSwarmTask")?,
        node_id: optional_id(v.node_id.as_deref(), "InspectSwarmTask")?,
        state: enum_text(status.state),
        desired_state: enum_text(v.desired_state),
        replica_evidence: replica(v.desired_state),
        slot: v.slot,
        exit_code: status.container_status.and_then(|v| v.exit_code),
        failure_reason: failure(
            status.err.as_deref().filter(|s| !s.is_empty()).or_else(|| {
                (status.state != Some(TaskState::RUNNING))
                    .then_some(status.message.as_deref())
                    .flatten()
            }),
            status.state,
        ),
    })
}

pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<SwarmSummary>, AppError> {
    let client = client.bounded_evidence();
    inventory::paginate(input, "ListSwarms", |page, size| {
        let client = client.clone();
        async move {
            client
                .read(ListSwarms {
                    page,
                    limit: Some(size),
                    ..Default::default()
                })
                .await?
                .into_iter()
                .map(|v| summary(&client, v))
                .collect()
        }
    })
    .await
    .and_then(|v| finish(&client, v))
}
pub async fn get(
    client: &CoreClient,
    input: DetailInput,
) -> Result<Observation<SwarmDetail>, AppError> {
    let client = client.bounded_evidence();
    let swarm = resolve(&client, input).await?;
    let v = client
        .read(InspectSwarm {
            swarm: swarm.id.clone(),
        })
        .await?;
    let data = SwarmDetail {
        docker_swarm_id: required_id(v.id.as_deref(), "InspectSwarm")?,
        root_rotation_in_progress: v.root_rotation_in_progress,
    };
    finish(&client, observe(swarm, data, false))
}

fn node_hostname(v: &SwarmNode) -> Option<&str> {
    v.description.as_ref().and_then(|d| d.hostname.as_deref())
}
fn service_name(v: &SwarmService) -> Option<&str> {
    v.spec.as_ref().and_then(|s| s.name.as_deref())
}

macro_rules! objects {
    ($list:ident, $get:ident, $request:ident, $inspect:ident, $field:ident, $out:ty, $project:ident, $detail:ident, $name:ident, $fresh_name:ident) => {
        pub async fn $list(
            client: &CoreClient,
            input: ListTargetInput,
        ) -> Result<Observation<ObjectPage<$out>>, AppError> {
            offsets(input.page, input.page_size)?;
            let client = client.bounded_evidence();
            let swarm = resolve(&client, input.target).await?;
            let values = client
                .read($request {
                    swarm: swarm.id.clone(),
                })
                .await?;
            let count = values.len();
            let items = values
                .into_iter()
                .take(MAX_SCAN)
                .map(|v| $project(&client, v))
                .collect::<Result<Vec<_>, _>>()?;
            unique(&items, |v| &v.id)?;
            finish(
                &client,
                observe(
                    swarm,
                    page(items, input.page, input.page_size, count)?,
                    false,
                ),
            )
        }
        pub async fn $get(
            client: &CoreClient,
            input: ObjectInput,
        ) -> Result<Observation<$out>, AppError> {
            selector(&input.object)?;
            let client = client.bounded_evidence();
            let swarm = resolve(&client, input.target).await?;
            let items = client
                .read($request {
                    swarm: swarm.id.clone(),
                })
                .await?;
            let selected = select(&client, items, &input.object, |v| {
                (v.id.as_deref().unwrap_or_default(), v.$name.as_deref())
            })?;
            let id = selected
                .id
                .as_deref()
                .ok_or_else(|| invalid(stringify!($request)))?;
            let raw = client
                .read($inspect {
                    swarm: swarm.id.clone(),
                    $field: id.into(),
                })
                .await?;
            if input
                .object
                .name
                .as_deref()
                .is_some_and(|n| ($fresh_name)(&raw) != Some(n))
            {
                return Err(invalid(stringify!($inspect)));
            }
            let fresh = $detail(&client, raw)?;
            if fresh.id != id {
                return Err(invalid(stringify!($inspect)));
            }
            finish(&client, observe(swarm, fresh, true))
        }
    };
}
objects!(
    nodes,
    node,
    ListSwarmNodes,
    InspectSwarmNode,
    node,
    Node,
    node_list,
    node_detail,
    hostname,
    node_hostname
);
objects!(
    services,
    service,
    ListSwarmServices,
    InspectSwarmService,
    service,
    Service,
    service_list,
    service_detail,
    name,
    service_name
);

async fn selected_service(
    client: &CoreClient,
    swarm: &SwarmSummary,
    input: &ObjectSelector,
) -> Result<SwarmServiceListItem, AppError> {
    let items = client
        .read(ListSwarmServices {
            swarm: swarm.id.clone(),
        })
        .await?;
    select(client, items, input, |v| {
        (v.id.as_deref().unwrap_or_default(), v.name.as_deref())
    })
}
pub async fn tasks(
    client: &CoreClient,
    input: TasksInput,
) -> Result<Observation<ObjectPage<Task>>, AppError> {
    offsets(input.page, input.page_size)?;
    if let Some(s) = &input.service {
        selector(s)?;
    }
    let client = client.bounded_evidence();
    let swarm = resolve(&client, input.target).await?;
    let service = if let Some(s) = input.service {
        selected_service(&client, &swarm, &s).await?.id
    } else {
        None
    };
    let items = client
        .read(ListSwarmTasks {
            swarm: swarm.id.clone(),
        })
        .await?;
    let count = items.len();
    let tasks = items
        .into_iter()
        .take(MAX_SCAN)
        .map(|v| task_list(&client, v))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|v| service.is_none() || v.service_id == service)
        .collect::<Vec<_>>();
    unique(&tasks, |v| &v.id)?;
    finish(
        &client,
        observe(
            swarm,
            page(tasks, input.page, input.page_size, count)?,
            false,
        ),
    )
}
pub async fn task(client: &CoreClient, input: ObjectInput) -> Result<Observation<Task>, AppError> {
    selector(&input.object)?;
    let client = client.bounded_evidence();
    let swarm = resolve(&client, input.target).await?;
    let items = client
        .read(ListSwarmTasks {
            swarm: swarm.id.clone(),
        })
        .await?;
    let selected = select(&client, items, &input.object, |v| {
        (v.id.as_deref().unwrap_or_default(), v.name.as_deref())
    })?;
    let id = selected
        .id
        .as_deref()
        .ok_or_else(|| invalid("ListSwarmTasks"))?;
    let raw = client
        .read(InspectSwarmTask {
            swarm: swarm.id.clone(),
            task: id.into(),
        })
        .await?;
    if input
        .object
        .name
        .as_deref()
        .is_some_and(|n| raw.name.as_deref() != Some(n))
    {
        return Err(invalid("InspectSwarmTask"));
    }
    let fresh = task_detail(&client, raw)?;
    if fresh.id != id || fresh.service_id != selected.service_id {
        return Err(invalid("InspectSwarmTask"));
    }
    finish(&client, observe(swarm, fresh, true))
}
pub async fn service_logs(client: &CoreClient, input: LogsInput) -> Result<WorkloadLogs, AppError> {
    selector(&input.service)?;
    if !(1..=logs::MAX_TAIL).contains(&input.tail)
        || !(1..=logs::MAX_BYTES).contains(&input.max_bytes)
    {
        return Err(AppError::arguments(
            "GetSwarmServiceLog",
            "Use tail 1–500 and max_bytes 1–65536",
        ));
    }
    let client = client.bounded_evidence();
    let swarm = resolve(&client, input.target).await?;
    let service = selected_service(&client, &swarm, &input.service).await?;
    let id = required_id(service.id.as_deref(), "ListSwarmServices")?;
    let log = client
        .read(GetSwarmServiceLog {
            swarm: swarm.id.clone(),
            service: id.clone(),
            tail: input.tail,
            timestamps: input.timestamps,
            no_task_ids: false,
            no_resolve: true,
            details: false,
        })
        .await?;
    logs::project(
        &client,
        log,
        LogTarget {
            kind: LogTargetKind::SwarmService,
            id: swarm.id,
            container: None,
            services: vec![id],
        },
        input.tail,
        input.max_bytes,
        input.timestamps,
        "GetSwarmServiceLog",
    )
}

pub async fn diagnose(
    client: &CoreClient,
    input: ObjectInput,
) -> Result<Observation<Diagnosis>, AppError> {
    selector(&input.object)?;
    tokio::time::timeout(
        Duration::from_secs(30),
        diagnose_inner(&client.bounded_evidence(), input),
    )
    .await
    .unwrap_or_else(|_| {
        Err(AppError::new(
            ErrorKind::Timeout,
            "diagnose_swarm_service",
            None,
        ))
    })
}
async fn diagnose_inner(
    client: &CoreClient,
    input: ObjectInput,
) -> Result<Observation<Diagnosis>, AppError> {
    let swarm = resolve(client, input.target).await?;
    let cached = selected_service(client, &swarm, &input.object).await?;
    let id = required_id(cached.id.as_deref(), "ListSwarmServices")?;
    let raw = client
        .read(InspectSwarmService {
            swarm: swarm.id.clone(),
            service: id.clone(),
        })
        .await?;
    if input
        .object
        .name
        .as_deref()
        .is_some_and(|n| service_name(&raw) != Some(n))
    {
        return Err(invalid("InspectSwarmService"));
    }
    let fresh = service_detail(client, raw)?;
    if fresh.id != id {
        return Err(invalid("InspectSwarmService"));
    }
    let mut gaps = Vec::new();
    let tasks = match client
        .read(ListSwarmTasks {
            swarm: swarm.id.clone(),
        })
        .await
    {
        Ok(v) => Some(v),
        Err(e) => {
            gaps.push(e);
            None
        }
    };
    let nodes = match client
        .read(ListSwarmNodes {
            swarm: swarm.id.clone(),
        })
        .await
    {
        Ok(v) => Some(v),
        Err(e) => {
            gaps.push(e);
            None
        }
    };
    let scan_incomplete = tasks.as_ref().is_none_or(|v| v.len() > MAX_SCAN)
        || nodes.as_ref().is_none_or(|v| v.len() > MAX_SCAN);
    let cached_tasks = tasks
        .unwrap_or_default()
        .into_iter()
        .take(MAX_SCAN)
        .map(|v| task_list(client, v))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|t| t.service_id.as_deref() == Some(&id))
        .collect::<Vec<_>>();
    unique(&cached_tasks, |v| &v.id)?;
    let node_ids = cached_tasks
        .iter()
        .filter_map(|v| v.node_id.as_deref())
        .collect::<BTreeSet<_>>();
    let unassigned_pending = cached_tasks
        .iter()
        .any(|t| t.replica_evidence == "current" && t.node_id.is_none());
    let cached_nodes = nodes
        .unwrap_or_default()
        .into_iter()
        .take(MAX_SCAN)
        .map(|v| node_list(client, v))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|v| unassigned_pending || node_ids.contains(v.id.as_str()))
        .collect::<Vec<_>>();
    unique(&cached_nodes, |v| &v.id)?;
    let current_running = cached_tasks
        .iter()
        .filter(|t| t.replica_evidence == "current" && t.state.as_deref() == Some("running"))
        .count();
    let pending = cached_tasks
        .iter()
        .filter(|t| {
            t.replica_evidence == "current"
                && matches!(
                    t.state.as_deref(),
                    Some(
                        "new"
                            | "allocated"
                            | "pending"
                            | "assigned"
                            | "accepted"
                            | "preparing"
                            | "ready"
                            | "starting"
                    )
                )
        })
        .count();
    let unclassified = cached_tasks
        .iter()
        .filter(|t| t.replica_evidence == "unknown" || t.state.is_none())
        .count();
    let historical = cached_tasks
        .iter()
        .filter(|t| t.replica_evidence == "historical")
        .count();
    // Prefer pending/failed tasks; never let historical running tasks count as replicas.
    let mut candidates = cached_tasks.iter().collect::<Vec<_>>();
    candidates.sort_by_key(|t| {
        (
            t.state.as_deref() == Some("running"),
            t.replica_evidence == "historical",
        )
    });
    let mut fresh_tasks = Vec::new();
    for t in candidates.into_iter().take(8) {
        match client
            .read(InspectSwarmTask {
                swarm: swarm.id.clone(),
                task: t.id.clone(),
            })
            .await
            .and_then(|v| task_detail(client, v))
        {
            Ok(v) if v.id == t.id && v.service_id.as_deref() == Some(&id) => fresh_tasks.push(v),
            Ok(_) => gaps.push(invalid("InspectSwarmTask")),
            Err(e) => gaps.push(e),
        }
    }
    let mut fresh_nodes = Vec::new();
    for n in cached_nodes.iter().take(4) {
        match client
            .read(InspectSwarmNode {
                swarm: swarm.id.clone(),
                node: n.id.clone(),
            })
            .await
            .and_then(|v| node_detail(client, v))
        {
            Ok(v) if v.id == n.id => fresh_nodes.push(v),
            Ok(_) => gaps.push(invalid("InspectSwarmNode")),
            Err(e) => gaps.push(e),
        }
    }
    let fresh_task_coverage_incomplete = scan_incomplete || fresh_tasks.len() != cached_tasks.len();
    let fresh_node_coverage_incomplete = scan_incomplete
        || fresh_nodes.len() != cached_nodes.len()
        || node_ids
            .iter()
            .any(|id| !cached_nodes.iter().any(|n| n.id == *id));
    let under_replicated = (!scan_incomplete && unclassified == 0)
        .then(|| fresh.desired_replicas.map(|n| (current_running as u64) < n))
        .flatten();
    let cached_display_incomplete = cached_tasks.len() > 64 || cached_nodes.len() > 64;
    let diagnosis = Diagnosis {
        service: fresh,
        cached_service: service_list(client, cached)?,
        cached_current_running: current_running,
        cached_pending: pending,
        cached_unclassified_tasks: unclassified,
        historical_tasks: historical,
        under_replicated_in_cache: under_replicated,
        fresh_task_coverage_incomplete,
        fresh_node_coverage_incomplete,
        cached_display_incomplete,
        scan_incomplete,
        cached_tasks: cached_tasks.into_iter().take(64).collect(),
        fresh_tasks,
        cached_nodes: cached_nodes.into_iter().take(64).collect(),
        fresh_nodes,
        gaps,
        evidence_limit: "Task/node discovery is cached and may be incomplete even without local clipping. Display at most 64 cached tasks/nodes; fresh details cover at most eight tasks and four nodes, without a snapshot. Unassigned pending tasks include candidate nodes without claiming eligible placement. Desired replicas, execution, convergence and workload health are separate. Failure reasons are conservative classifications; arbitrary Docker error text is withheld.",
    };
    finish(client, observe(swarm, diagnosis, true))
}
