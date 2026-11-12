//! Bounded fleet observations and a reusable, read-only stack service reader.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};

use komodo_client::{
    api::read::{
        GetServerState, GetSystemStats, KomodoReadRequest, ListAllStackServices, ListDeployments,
        ListServers, ListStacks,
    },
    entities::{
        deployment::{DeploymentListItem, DeploymentState},
        docker::container::ContainerStateStatusEnum,
        server::{ServerListItem, ServerState},
        stack::{StackListItem, StackServiceState},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    task::JoinSet,
    time::{Instant, timeout, timeout_at},
};

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
};

/// Per-call bound, including independently retained server-state and stats reads.
pub const MAX_CORE_CONCURRENCY: usize = 4;
pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(2);
pub const FLEET_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_SERVICES: usize = 100;
const DISPLAY_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetHealthInput {
    /// Zero-based page in each inventory family; continue with the same size.
    #[serde(default)]
    pub page: u64,
    /// Maximum resources per family, 1..100. A full page may have a continuation.
    #[serde(default = "default_page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}
fn default_page_size() -> u64 {
    30
}
impl Default for FleetHealthInput {
    fn default() -> Self {
        Self {
            page: 0,
            page_size: default_page_size(),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthGap {
    pub source: String,
    pub resource_id: Option<String>,
    pub error: AppError,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Utilization {
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub disk_percent: Option<f64>,
    /// Core's cache refresh timestamp in milliseconds, not the time of this call.
    pub refresh_ts: i64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerHealth {
    pub id: String,
    pub name: String,
    pub template: bool,
    /// Inventory state, replaced by GetServerState when that read succeeds.
    /// `health` and `gaps` disclose whether the current check was available.
    pub state: String,
    pub health: HealthStatus,
    pub utilization: Option<Utilization>,
    pub gaps: Vec<HealthGap>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceHealth {
    pub service: String,
    pub state: String,
    pub health: HealthStatus,
    pub container_name: Option<String>,
    pub container_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackServices {
    pub stack_id: String,
    pub services: Vec<ServiceHealth>,
    /// At least one service was omitted. A truncated healthy set is inconclusive.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkloadHealth {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub health: HealthStatus,
    /// A Swarm association takes precedence over a Server association.
    pub server_id: Option<String>,
    pub swarm_id: Option<String>,
    /// Expected distinct Stack service names from inventory; None for Deployments.
    pub service_count: Option<usize>,
    pub services: Vec<ServiceHealth>,
    pub services_truncated: bool,
    pub gaps: Vec<HealthGap>,
    #[serde(skip)]
    expected_service_names: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetHealth {
    pub servers: Vec<ServerHealth>,
    pub stacks: Vec<WorkloadHealth>,
    pub deployments: Vec<WorkloadHealth>,
    /// Failed inventory families are recorded here; successful families survive.
    pub gaps: Vec<HealthGap>,
    pub page: u64,
    pub page_size: u64,
    /// True if any family filled its page. Continue to establish exhaustion.
    pub has_more: bool,
    pub next_page: Option<u64>,
}

fn invalid(operation: &'static str) -> AppError {
    AppError::new(ErrorKind::InvalidResponse, operation, None)
}
fn timed_out(operation: &'static str) -> AppError {
    AppError::new(ErrorKind::Timeout, operation, None)
}
fn valid_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|x| x.is_ascii_hexdigit())
}
fn display(client: &CoreClient, text: &str) -> String {
    let mut text = client.redact(text);
    if text.len() > DISPLAY_BYTES {
        let mut end = DISPLAY_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}
fn gap(source: &'static str, id: Option<&str>, error: AppError) -> HealthGap {
    HealthGap {
        source: source.into(),
        resource_id: id.map(str::to_owned),
        error,
    }
}

async fn read<T>(
    client: &CoreClient,
    request: T,
    operation: &'static str,
) -> Result<T::Response, AppError>
where
    T: Serialize + KomodoReadRequest,
    T::Response: DeserializeOwned,
{
    timeout(SOURCE_TIMEOUT, client.read(request))
        .await
        .map_err(|_| timed_out(operation))?
}

/// One Core page of at most 101 services, with at most 100 projected results.
/// Core interprets `stacks` through ResourceQuery.names, which accepts stable IDs.
pub async fn stack_services(client: &CoreClient, id: &str) -> Result<StackServices, AppError> {
    if !valid_id(id) {
        return Err(AppError::arguments(
            "ListAllStackServices",
            "Provide a 24-character hexadecimal stack ID",
        ));
    }
    let services = read(
        client,
        ListAllStackServices {
            stacks: vec![id.into()],
            tags: vec![],
            terms: vec![],
            state: vec![],
            page: 0,
            limit: Some(MAX_SERVICES as u64 + 1),
        },
        "ListAllStackServices",
    )
    .await?;
    if services.len() > MAX_SERVICES + 1 || services.iter().any(|s| s.stack_id != id) {
        return Err(invalid("ListAllStackServices"));
    }
    let truncated = services.len() > MAX_SERVICES;
    let mut services: Vec<_> = services
        .into_iter()
        .take(MAX_SERVICES)
        .map(|service| {
            let mut health = match service.state {
                StackServiceState::Healthy | StackServiceState::Running => HealthStatus::Healthy,
                StackServiceState::Unknown => HealthStatus::Unknown,
                _ => HealthStatus::Unhealthy,
            };
            if let Some(container) = &service.container {
                // Docker puts healthcheck state in its summary's status, while the
                // lifecycle state can remain Running. Do not confuse the two.
                if container
                    .status
                    .as_deref()
                    .is_some_and(|s| s.trim_end().ends_with("(unhealthy)"))
                    || !matches!(
                        container.state,
                        ContainerStateStatusEnum::Running | ContainerStateStatusEnum::Empty
                    )
                {
                    health = HealthStatus::Unhealthy;
                } else if (container.state == ContainerStateStatusEnum::Empty
                    && health != HealthStatus::Unhealthy)
                    || (health == HealthStatus::Healthy
                        && container
                            .status
                            .as_deref()
                            .is_some_and(|s| s.trim_end().ends_with("(health: starting)")))
                {
                    health = HealthStatus::Unknown;
                }
            }
            ServiceHealth {
                service: display(client, &service.service),
                state: service.state.to_string().to_ascii_lowercase(),
                health,
                container_name: service.container.as_ref().map(|c| display(client, &c.name)),
                container_status: service
                    .container
                    .as_ref()
                    .and_then(|c| c.status.as_deref())
                    .map(|s| display(client, s)),
            }
        })
        .collect();
    services.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(StackServices {
        stack_id: id.into(),
        services,
        truncated,
    })
}

fn percent(value: f64) -> Option<f64> {
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}
fn ratio(used: f64, total: f64) -> Option<f64> {
    if !used.is_finite() || used < 0.0 || !total.is_finite() || total <= 0.0 {
        return None;
    }
    percent(100.0 * used / total)
}
fn workload_state(state: &str) -> HealthStatus {
    match state {
        "running" => HealthStatus::Healthy,
        "unknown" | "deploying" => HealthStatus::Unknown,
        _ => HealthStatus::Unhealthy,
    }
}
fn target(server: String, swarm: String) -> (Option<String>, Option<String>) {
    if !swarm.is_empty() {
        (None, valid_id(&swarm).then_some(swarm))
    } else {
        (valid_id(&server).then_some(server), None)
    }
}
fn stack(client: &CoreClient, item: StackListItem) -> WorkloadHealth {
    let state = item.info.state.to_string();
    let names: BTreeSet<_> = item
        .info
        .services
        .iter()
        .map(|s| s.service.as_str())
        .collect();
    let service_count = names.len();
    let expected_service_names = names
        .into_iter()
        .map(|name| display(client, name))
        .collect();
    let (server_id, swarm_id) = target(item.info.server_id, item.info.swarm_id);
    WorkloadHealth {
        id: item.id,
        name: display(client, &item.name),
        template: item.template,
        health: if item.template {
            HealthStatus::Unknown
        } else {
            workload_state(&state)
        },
        state,
        server_id,
        swarm_id,
        service_count: Some(service_count),
        services: vec![],
        services_truncated: false,
        gaps: vec![],
        expected_service_names,
    }
}
fn deployment(client: &CoreClient, item: DeploymentListItem) -> WorkloadHealth {
    let state = item.info.state.to_string();
    let health = if item.template {
        HealthStatus::Unknown
    } else if item.info.state == DeploymentState::Running
        && item
            .info
            .status
            .as_deref()
            .is_some_and(|s| s.trim_end().ends_with("(unhealthy)"))
    {
        HealthStatus::Unhealthy
    } else if item.info.state == DeploymentState::Running
        && item
            .info
            .status
            .as_deref()
            .is_some_and(|s| s.trim_end().ends_with("(health: starting)"))
    {
        HealthStatus::Unknown
    } else {
        workload_state(&state)
    };
    let (server_id, swarm_id) = target(item.info.server_id, item.info.swarm_id);
    WorkloadHealth {
        id: item.id,
        name: display(client, &item.name),
        template: item.template,
        state,
        health,
        server_id,
        swarm_id,
        service_count: None,
        services: vec![],
        services_truncated: false,
        gaps: vec![],
        expected_service_names: BTreeSet::new(),
    }
}
fn server(client: &CoreClient, item: ServerListItem) -> ServerHealth {
    ServerHealth {
        id: item.id,
        name: display(client, &item.name),
        template: item.template,
        state: server_state(item.info.state).into(),
        health: HealthStatus::Unknown,
        utilization: None,
        gaps: vec![],
    }
}
fn server_state(state: ServerState) -> &'static str {
    match state {
        ServerState::Ok => "ok",
        ServerState::NotOk => "not_ok",
        ServerState::Disabled => "disabled",
    }
}

#[derive(Clone)]
enum Job {
    ServerState(usize, String),
    Stats(usize, String),
    Stack(usize, String),
}
enum Observation {
    ServerState(Result<ServerState, AppError>),
    Stats(Result<Utilization, AppError>),
    Stack(Result<StackServices, AppError>),
}

async fn observe(client: CoreClient, job: Job) -> Observation {
    match job {
        Job::ServerState(_, id) => Observation::ServerState(
            read(&client, GetServerState { server: id }, "GetServerState")
                .await
                .map(|s| s.status),
        ),
        Job::Stats(_, id) => {
            let stats = read(&client, GetSystemStats { server: id }, "GetSystemStats").await;
            Observation::Stats(stats.map(|s| {
                let valid_disks = s.disks.iter().all(|d| {
                    d.used_gb.is_finite()
                        && d.used_gb >= 0.0
                        && d.total_gb.is_finite()
                        && d.total_gb > 0.0
                });
                Utilization {
                    cpu_percent: percent(s.cpu_perc.into()),
                    memory_percent: ratio(s.mem_used_gb, s.mem_total_gb),
                    disk_percent: valid_disks
                        .then(|| {
                            ratio(
                                s.disks.iter().map(|d| d.used_gb).sum(),
                                s.disks.iter().map(|d| d.total_gb).sum(),
                            )
                        })
                        .flatten(),
                    refresh_ts: s.refresh_ts,
                }
            }))
        }
        Job::Stack(_, id) => Observation::Stack(stack_services(&client, &id).await),
    }
}

fn apply(report: &mut FleetHealth, job: &Job, observation: Observation) {
    match (job, observation) {
        (Job::ServerState(index, id), Observation::ServerState(state)) => {
            let server = &mut report.servers[*index];
            match state {
                Ok(state) => {
                    server.state = server_state(state).into();
                    server.health = match state {
                        ServerState::Ok => HealthStatus::Healthy,
                        ServerState::NotOk => HealthStatus::Unavailable,
                        ServerState::Disabled => HealthStatus::Unknown,
                    };
                }
                Err(error) => {
                    server.health = HealthStatus::Unavailable;
                    server.gaps.push(gap("server_state", Some(id), error));
                }
            }
        }
        (Job::Stats(index, id), Observation::Stats(stats)) => {
            let server = &mut report.servers[*index];
            match stats {
                Ok(stats) => server.utilization = Some(stats),
                Err(error) => server.gaps.push(gap("system_stats", Some(id), error)),
            }
        }
        (Job::Stack(index, id), Observation::Stack(observed)) => {
            let stack = &mut report.stacks[*index];
            match observed {
                Ok(observed) => {
                    stack.services = observed.services;
                    stack.services_truncated = observed.truncated;
                    // Compose replicas may repeat a service name. Compare the
                    // distinct names with inventory, rather than replica count.
                    let observed_names: BTreeSet<_> =
                        stack.services.iter().map(|s| s.service.clone()).collect();
                    let complete = Some(observed_names.len()) == stack.service_count
                        && observed_names == stack.expected_service_names
                        && !stack.services_truncated;
                    if !complete {
                        stack.gaps.push(gap(
                            "service_inventory",
                            Some(id),
                            invalid("ListAllStackServices"),
                        ));
                    }
                    if stack
                        .services
                        .iter()
                        .any(|s| s.health == HealthStatus::Unhealthy)
                    {
                        stack.health = HealthStatus::Unhealthy;
                    } else if stack.health != HealthStatus::Unhealthy
                        && (stack.services.is_empty()
                            || !complete
                            || stack
                                .services
                                .iter()
                                .any(|s| s.health != HealthStatus::Healthy))
                    {
                        stack.health = HealthStatus::Unknown;
                    }
                }
                Err(error) => {
                    if stack.health != HealthStatus::Unhealthy {
                        stack.health = HealthStatus::Unknown;
                    }
                    stack.gaps.push(gap("stack_services", Some(id), error));
                }
            }
        }
        _ => unreachable!("observation must match its job"),
    }
}
fn timeout_job(report: &mut FleetHealth, job: &Job, started: bool) {
    let observation = match job {
        Job::ServerState(..) => Observation::ServerState(Err(timed_out("GetServerState"))),
        Job::Stats(..) => Observation::Stats(Err(timed_out("GetSystemStats"))),
        Job::Stack(..) => Observation::Stack(Err(timed_out("ListAllStackServices"))),
    };
    apply(report, job, observation);
    // A job still in the queue never observed its server. Distinguish that
    // unknown state from an attempted state read that failed or timed out.
    if !started && let Job::ServerState(index, _) = job {
        report.servers[*index].health = HealthStatus::Unknown;
    }
}

/// Three bounded inventory reads followed by a bounded worker pool. A ten-second
/// overall deadline cancels unfinished reads and marks every unchecked resource.
pub async fn fleet_health(
    client: &CoreClient,
    input: FleetHealthInput,
) -> Result<FleetHealth, AppError> {
    let next = input.page.checked_add(1);
    if !(1..=100).contains(&input.page_size)
        || next
            .and_then(|p| p.checked_mul(input.page_size))
            .and_then(|offset| usize::try_from(offset).ok())
            .is_none()
    {
        return Err(AppError::arguments(
            "fleet_health",
            "Use page_size 1..100 and a page whose next offset fits an unsigned machine integer",
        ));
    }
    let deadline = Instant::now() + FLEET_TIMEOUT;
    let (servers, stacks, deployments) = tokio::join!(
        read(
            client,
            ListServers {
                page: input.page,
                limit: Some(input.page_size),
                ..Default::default()
            },
            "ListServers"
        ),
        read(
            client,
            ListStacks {
                page: input.page,
                limit: Some(input.page_size),
                ..Default::default()
            },
            "ListStacks"
        ),
        read(
            client,
            ListDeployments {
                page: input.page,
                limit: Some(input.page_size),
                ..Default::default()
            },
            "ListDeployments"
        ),
    );
    let mut report = FleetHealth {
        servers: vec![],
        stacks: vec![],
        deployments: vec![],
        gaps: vec![],
        page: input.page,
        page_size: input.page_size,
        has_more: false,
        next_page: None,
    };
    macro_rules! family {
        ($result:expr, $field:ident, $project:ident, $source:literal, $op:literal) => {
            match $result {
                Ok(items)
                    if items.len() as u64 <= input.page_size
                        && items.iter().all(|i| valid_id(&i.id))
                        && {
                            let mut ids = std::collections::HashSet::new();
                            items.iter().all(|i| ids.insert(&i.id))
                        } =>
                {
                    report.has_more |= items.len() as u64 == input.page_size;
                    report.$field = items
                        .into_iter()
                        .map(|item| $project(client, item))
                        .collect();
                }
                Ok(_) => report.gaps.push(gap($source, None, invalid($op))),
                Err(error) => report.gaps.push(gap($source, None, error)),
            }
        };
    }
    family!(servers, servers, server, "servers", "ListServers");
    family!(stacks, stacks, stack, "stacks", "ListStacks");
    family!(
        deployments,
        deployments,
        deployment,
        "deployments",
        "ListDeployments"
    );
    report.next_page = report.has_more.then_some(next.unwrap());
    let mut pending = VecDeque::new();
    for index in 0..report.servers.len().max(report.stacks.len()) {
        if let Some(server) = report.servers.get(index).filter(|s| !s.template) {
            pending.push_back(Job::ServerState(index, server.id.clone()));
        }
        if let Some(stack) = report.stacks.get(index).filter(|s| !s.template) {
            pending.push_back(Job::Stack(index, stack.id.clone()));
        }
        if let Some(server) = report.servers.get(index).filter(|s| !s.template) {
            pending.push_back(Job::Stats(index, server.id.clone()));
        }
    }
    let mut tasks = JoinSet::new();
    let mut active = BTreeMap::new();
    let mut serial = 0_u64;
    loop {
        if Instant::now() >= deadline {
            tasks.abort_all();
            break;
        }
        while tasks.len() < MAX_CORE_CONCURRENCY && !pending.is_empty() {
            let job = pending.pop_front().unwrap();
            let client = client.clone();
            active.insert(serial, job.clone());
            let key = serial;
            serial += 1;
            tasks.spawn(async move { (key, observe(client, job).await) });
        }
        if tasks.is_empty() {
            break;
        }
        match timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(Ok((key, observation)))) => {
                let job = active.remove(&key).expect("active observation");
                apply(&mut report, &job, observation);
            }
            _ => {
                tasks.abort_all();
                break;
            }
        }
    }
    for job in active.values() {
        timeout_job(&mut report, job, true);
    }
    for job in &pending {
        timeout_job(&mut report, job, false);
    }
    Ok(report)
}
