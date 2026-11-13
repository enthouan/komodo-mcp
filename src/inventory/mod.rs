//! Bounded read-only inventory and exact target resolution over Core 2.3.2.

mod types;
pub use types::*;

use komodo_client::{
    api::read::{
        GetDeployment, GetRepo, GetServer, GetStack, GetSwarm, ListDeployments, ListRepos,
        ListServers, ListStacks, ListSwarms,
    },
    entities::{
        ResourceTargetVariant,
        deployment::{Deployment, DeploymentListItem, DeploymentState as CoreDeploymentState},
        repo::{Repo, RepoListItem},
        resource::ResourceListItem,
        server::{Server, ServerListItem, ServerState as CoreServerState},
        stack::{Stack, StackListItem, StackState as CoreStackState},
        swarm::{Swarm, SwarmListItem},
    },
};
use rmcp::model::{CallToolResult, JsonObject};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    client::CoreClient,
    error::{AppError, Candidate, ErrorKind},
};

const RESOLUTION_LIMIT: u64 = 100;
const NAME_SEARCH_PAGES: u64 = 4;
const MAX_CANDIDATES: usize = 5;

/// AppError also has structured content, so advertise both shapes. A success-only
/// schema would reject the foundation's typed error results in strict clients.
pub(crate) fn output_schema<T: rmcp::schemars::JsonSchema + 'static>() -> std::sync::Arc<JsonObject>
{
    let mut schema = rmcp::handler::server::tool::schema_for_output::<ToolOutput<T>>()
        .as_ref()
        .clone();
    schema.insert("type".into(), Value::String("object".into()));
    std::sync::Arc::new(schema)
}

fn invalid(operation: &'static str) -> AppError {
    AppError::new(ErrorKind::InvalidResponse, operation, None)
}

fn changed(operation: &'static str) -> AppError {
    AppError {
        correction: Some(
            "The resource changed during inspection; list this family again and retry with its current ID",
        ),
        ..invalid(operation)
    }
}

/// Parse at our boundary: SDK serde diagnostics can echo arbitrary argument values.
/// Discard those diagnostics instead of returning them through rmcp's extractor.
pub(crate) fn parse<T: DeserializeOwned>(
    arguments: JsonObject,
    operation: &'static str,
) -> Result<T, AppError> {
    serde_json::from_value(Value::Object(arguments)).map_err(|_| {
        AppError::arguments(operation, "Use the documented field names and JSON types")
    })
}

pub(crate) fn tool_result<T: Serialize>(
    client: &CoreClient,
    result: Result<T, AppError>,
) -> CallToolResult {
    match result {
        Ok(projected) => {
            let value = projected_value(client, &projected);
            // rmcp generates text from this same value, preserving compatibility.
            CallToolResult::structured(value)
        }
        Err(error) => error.into_tool_result(),
    }
}

/// The exact structured success projection, including final string redaction.
/// Composed readers use this when enforcing serialized output byte limits.
pub(crate) fn projected_value<T: Serialize>(client: &CoreClient, projected: &T) -> Value {
    let mut value = serde_json::to_value(projected).expect("projection is JSON serializable");
    redact_strings(client, &mut value);
    value
}

fn redact_strings(client: &CoreClient, value: &mut Value) {
    match value {
        Value::String(text) => *text = client.redact(text),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| redact_strings(client, item)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| redact_strings(client, item)),
        _ => {}
    }
}

impl ListInput {
    /// Validate the requested offset AND the possible lookahead before any I/O.
    fn validate(self, operation: &'static str) -> Result<u64, AppError> {
        let error = || {
            AppError::arguments(
                operation,
                "Use page_size from 1 through 100 and page values whose current and next offsets fit in an unsigned machine integer",
            )
        };
        if !(1..=100).contains(&self.page_size) {
            return Err(error());
        }
        let next = self.page.checked_add(1).ok_or_else(error)?;
        for page in [self.page, next] {
            let offset = page.checked_mul(self.page_size).ok_or_else(error)?;
            usize::try_from(offset).map_err(|_| error())?;
        }
        Ok(next)
    }
}

#[derive(Clone)]
enum Selector {
    Id(String),
    Name(String),
}

fn is_object_id(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl DetailInput {
    fn validate(self, operation: &'static str) -> Result<Selector, AppError> {
        match (self.id, self.name) {
            (Some(id), None) if is_object_id(&id) => Ok(Selector::Id(id)),
            (None, Some(name)) if !name.trim().is_empty() => Ok(Selector::Name(name)),
            _ => Err(AppError::arguments(
                operation,
                "Provide exactly one nonempty name or a 24-character hexadecimal id; omit the other selector",
            )),
        }
    }
}

#[derive(Clone, Default)]
struct Search {
    names: Vec<String>,
    terms: Vec<String>,
}

async fn paginate<T, F, Fut>(
    input: ListInput,
    operation: &'static str,
    mut fetch: F,
) -> Result<Page<T>, AppError>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<T>, AppError>>,
{
    let next = input.validate(operation)?;
    let items = fetch(input.page, input.page_size).await?;
    if items.len() as u64 > input.page_size {
        return Err(invalid(operation));
    }
    let has_more = if items.len() as u64 == input.page_size {
        let probe = fetch(next, input.page_size).await?;
        if probe.len() as u64 > input.page_size {
            return Err(invalid(operation));
        }
        !probe.is_empty()
    } else {
        false
    };
    Ok(Page {
        items,
        page: input.page,
        page_size: input.page_size,
        has_more,
        next_page: has_more.then_some(next),
    })
}

async fn resolve<I, F, Fut>(
    client: &CoreClient,
    selector: &Selector,
    operation: &'static str,
    mut fetch: F,
) -> Result<ResourceListItem<I>, AppError>
where
    F: FnMut(Search, u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<ResourceListItem<I>>, AppError>>,
{
    // Core interprets ObjectId-shaped ResourceQuery.names as IDs. For an explicit
    // name use escaped substring terms, then verify case-sensitive equality.
    let (search, max_pages) = match selector {
        Selector::Name(name) if is_object_id(name) => (
            Search {
                terms: vec![name.clone()],
                ..Default::default()
            },
            NAME_SEARCH_PAGES,
        ),
        Selector::Name(name) | Selector::Id(name) => (
            Search {
                names: vec![name.clone()],
                ..Default::default()
            },
            1,
        ),
    };
    let mut matches = Vec::new();
    for page in 0..max_pages {
        let items = fetch(search.clone(), page, RESOLUTION_LIMIT).await?;
        let count = items.len() as u64;
        if count > RESOLUTION_LIMIT {
            return Err(invalid(operation));
        }
        for item in items {
            let exact = match selector {
                Selector::Id(id) => item.id.eq_ignore_ascii_case(id),
                Selector::Name(name) => item.name == *name,
            };
            if exact {
                if !is_object_id(&item.id) || item.name.trim().is_empty() {
                    return Err(invalid(operation));
                }
                if matches
                    .iter()
                    .any(|other: &ResourceListItem<I>| other.id == item.id)
                {
                    return Err(invalid(operation));
                }
                matches.push(item);
            }
        }
        if matches.len() > 1 {
            return Err(AppError {
                candidates: Some(
                    matches
                        .iter()
                        .take(MAX_CANDIDATES)
                        .map(|item| Candidate {
                            id: client.redact(&item.id),
                            name: client.redact(&item.name),
                        })
                        .collect(),
                ),
                ..AppError::new(ErrorKind::AmbiguousTarget, operation, None)
            });
        }
        if count < RESOLUTION_LIMIT {
            return matches
                .pop()
                .ok_or_else(|| AppError::new(ErrorKind::NotFound, operation, None));
        }
    }
    Err(AppError::new(
        ErrorKind::IncompleteResolution,
        operation,
        None,
    ))
}

// Each reader builds an official typed request with positive limits and Name ASC.
macro_rules! reader {
    ($function:ident, $request:ident, $item:ty, $family:ident) => {
        async fn $function(
            client: &CoreClient,
            search: Search,
            page: u64,
            limit: u64,
        ) -> Result<Vec<$item>, AppError> {
            let mut request = $request {
                page,
                limit: Some(limit),
                sort_desc: false,
                ..Default::default()
            };
            request.query.names = search.names;
            request.query.terms = search.terms;
            let items = client.read(request).await?;
            if items.len() as u64 > limit
                || items.iter().any(|item| {
                    !matches!(item.resource_type, ResourceTargetVariant::$family)
                        || !is_object_id(&item.id)
                        || item.name.trim().is_empty()
                })
            {
                return Err(invalid(stringify!($request)));
            }
            Ok(items)
        }
    };
}
reader!(servers, ListServers, ServerListItem, Server);
reader!(stacks, ListStacks, StackListItem, Stack);
reader!(deployments, ListDeployments, DeploymentListItem, Deployment);
reader!(swarms, ListSwarms, SwarmListItem, Swarm);
reader!(repos, ListRepos, RepoListItem, Repo);

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

fn target(
    server_id: &str,
    server_name: &str,
    swarm_id: &str,
    swarm_name: &str,
) -> Option<DeploymentTarget> {
    if !swarm_id.is_empty() {
        Some(DeploymentTarget {
            kind: TargetKind::Swarm,
            id: swarm_id.into(),
            name: nonempty(swarm_name),
        })
    } else if !server_id.is_empty() {
        Some(DeploymentTarget {
            kind: TargetKind::Server,
            id: server_id.into(),
            name: nonempty(server_name),
        })
    } else {
        None
    }
}

fn active_target<'a>(server: &'a str, swarm: &'a str) -> (Option<&'static str>, &'a str) {
    if !swarm.is_empty() {
        (Some("swarm"), swarm)
    } else if !server.is_empty() {
        (Some("server"), server)
    } else {
        (None, "")
    }
}

fn check_identity<I>(
    summary: &ResourceListItem<I>,
    id: &str,
    name: &str,
    template: bool,
    operation: &'static str,
) -> Result<(), AppError> {
    if summary.id != id || summary.name != name || summary.template != template {
        return Err(changed(operation));
    }
    Ok(())
}

fn server_summary(item: ServerListItem) -> ServerSummary {
    ServerSummary {
        id: item.id,
        name: item.name,
        template: item.template,
        state: match item.info.state {
            CoreServerState::Ok => ServerState::Ok,
            CoreServerState::NotOk => ServerState::NotOk,
            CoreServerState::Disabled => ServerState::Disabled,
        },
        region: nonempty(&item.info.region),
        periphery_version: item.info.version.as_deref().and_then(nonempty),
    }
}

fn stack_summary(item: StackListItem) -> StackSummary {
    StackSummary {
        target: target(
            &item.info.server_id,
            &item.info.server_name,
            &item.info.swarm_id,
            &item.info.swarm_name,
        ),
        service_count: item.info.services.len(),
        state: match item.info.state {
            CoreStackState::Deploying => StackState::Deploying,
            CoreStackState::Running => StackState::Running,
            CoreStackState::Paused => StackState::Paused,
            CoreStackState::Stopped => StackState::Stopped,
            CoreStackState::Created => StackState::Created,
            CoreStackState::Restarting => StackState::Restarting,
            CoreStackState::Dead => StackState::Dead,
            CoreStackState::Removing => StackState::Removing,
            CoreStackState::Unhealthy => StackState::Unhealthy,
            CoreStackState::Down => StackState::Down,
            CoreStackState::Unknown => StackState::Unknown,
        },
        id: item.id,
        name: item.name,
        template: item.template,
    }
}

fn deployment_summary(item: DeploymentListItem) -> DeploymentSummary {
    DeploymentSummary {
        target: target(
            &item.info.server_id,
            &item.info.server_name,
            &item.info.swarm_id,
            &item.info.swarm_name,
        ),
        image: nonempty(&item.info.image),
        runtime_name: nonempty(item.custom_name()),
        update_available: item.info.update_available,
        state: match item.info.state {
            CoreDeploymentState::Deploying => DeploymentState::Deploying,
            CoreDeploymentState::Running => DeploymentState::Running,
            CoreDeploymentState::Created => DeploymentState::Created,
            CoreDeploymentState::Restarting => DeploymentState::Restarting,
            CoreDeploymentState::Stopping => DeploymentState::Stopping,
            CoreDeploymentState::Removing => DeploymentState::Removing,
            CoreDeploymentState::Paused => DeploymentState::Paused,
            CoreDeploymentState::Exited => DeploymentState::Exited,
            CoreDeploymentState::Dead => DeploymentState::Dead,
            CoreDeploymentState::Unhealthy => DeploymentState::Unhealthy,
            CoreDeploymentState::NotDeployed => DeploymentState::NotDeployed,
            CoreDeploymentState::Unknown => DeploymentState::Unknown,
        },
        id: item.id,
        name: item.name,
        template: item.template,
    }
}

fn map_page<T, U>(page: Page<T>, project: impl Fn(T) -> U) -> Page<U> {
    Page {
        items: page.items.into_iter().map(project).collect(),
        page: page.page,
        page_size: page.page_size,
        has_more: page.has_more,
        next_page: page.next_page,
    }
}

pub async fn list_servers(
    client: &CoreClient,
    input: ListInput,
) -> Result<Page<ServerSummary>, AppError> {
    Ok(map_page(
        paginate(input, "ListServers", |page, limit| {
            servers(client, Search::default(), page, limit)
        })
        .await?,
        server_summary,
    ))
}
pub async fn list_stacks(
    client: &CoreClient,
    input: ListInput,
) -> Result<Page<StackSummary>, AppError> {
    Ok(map_page(
        paginate(input, "ListStacks", |page, limit| {
            stacks(client, Search::default(), page, limit)
        })
        .await?,
        stack_summary,
    ))
}
pub async fn list_deployments(
    client: &CoreClient,
    input: ListInput,
) -> Result<Page<DeploymentSummary>, AppError> {
    Ok(map_page(
        paginate(input, "ListDeployments", |page, limit| {
            deployments(client, Search::default(), page, limit)
        })
        .await?,
        deployment_summary,
    ))
}

pub async fn get_server(client: &CoreClient, input: DetailInput) -> Result<ServerDetail, AppError> {
    let (summary, full) = resolve_server(client, input).await?;
    Ok(ServerDetail {
        summary: server_summary(summary),
        enabled: full.config.enabled,
    })
}

pub(crate) async fn resolve_server(
    client: &CoreClient,
    input: DetailInput,
) -> Result<(ServerListItem, Server), AppError> {
    let selector = input.validate("GetServer")?;
    let summary = resolve(client, &selector, "ListServers", |search, page, limit| {
        servers(client, search, page, limit)
    })
    .await?;
    let full = client
        .read(GetServer {
            server: summary.id.clone(),
        })
        .await?;
    check_identity(&summary, &full.id, &full.name, full.template, "GetServer")?;
    Ok((summary, full))
}

pub(crate) async fn resolve_swarm(
    client: &CoreClient,
    input: DetailInput,
) -> Result<(SwarmListItem, Swarm), AppError> {
    let selector = input.validate("GetSwarm")?;
    let summary = resolve(client, &selector, "ListSwarms", |search, page, limit| {
        swarms(client, search, page, limit)
    })
    .await?;
    let full = client
        .read(GetSwarm {
            swarm: summary.id.clone(),
        })
        .await?;
    check_identity(&summary, &full.id, &full.name, full.template, "GetSwarm")?;
    Ok((summary, full))
}

pub(crate) async fn resolve_repo(
    client: &CoreClient,
    input: DetailInput,
) -> Result<(RepoListItem, Repo), AppError> {
    let selector = input.validate("GetRepo")?;
    let summary = resolve(client, &selector, "ListRepos", |search, page, limit| {
        repos(client, search, page, limit)
    })
    .await?;
    let full = client
        .read(GetRepo {
            repo: summary.id.clone(),
        })
        .await?;
    check_identity(&summary, &full.id, &full.name, full.template, "GetRepo")?;
    Ok((summary, full))
}
pub async fn get_stack(client: &CoreClient, input: DetailInput) -> Result<StackDetail, AppError> {
    get_stack_observation(client, input, 0)
        .await
        .map(|(detail, _)| detail)
}

/// Internal correlation metadata from the same two reads as get_stack. Keep
/// expected service names out of the public inventory projection.
pub(crate) async fn get_stack_observation(
    client: &CoreClient,
    input: DetailInput,
    max_services: usize,
) -> Result<(StackDetail, Option<std::collections::BTreeSet<String>>), AppError> {
    let (summary, full) = resolve_stack(client, input).await?;
    let project_name = nonempty(&full.project_name(false));
    let expected_services = (summary.info.services.len() <= max_services
        && summary.info.services.iter().all(|service| {
            service.service.len() <= 128 && client.redact(&service.service) == service.service
        }))
    .then(|| {
        summary
            .info
            .services
            .iter()
            .map(|service| service.service.clone())
            .collect()
    });
    Ok((
        StackDetail {
            summary: stack_summary(summary),
            project_name,
        },
        expected_services,
    ))
}

/// Raw identities/configuration stay internal. Mutations select stable IDs and
/// exact Core service identifiers, never redacted or clipped display fields.
pub(crate) async fn resolve_stack(
    client: &CoreClient,
    input: DetailInput,
) -> Result<(StackListItem, Stack), AppError> {
    let selector = input.validate("GetStack")?;
    let summary = resolve(client, &selector, "ListStacks", |search, page, limit| {
        stacks(client, search, page, limit)
    })
    .await?;
    let full = client
        .read(GetStack {
            stack: summary.id.clone(),
        })
        .await?;
    check_identity(&summary, &full.id, &full.name, full.template, "GetStack")?;
    if active_target(&summary.info.server_id, &summary.info.swarm_id)
        != active_target(&full.config.server_id, &full.config.swarm_id)
    {
        return Err(changed("GetStack"));
    }
    Ok((summary, full))
}
pub async fn get_deployment(
    client: &CoreClient,
    input: DetailInput,
) -> Result<DeploymentDetail, AppError> {
    let (summary, _) = resolve_deployment(client, input).await?;
    Ok(DeploymentDetail {
        summary: deployment_summary(summary),
    })
}

pub(crate) async fn resolve_deployment(
    client: &CoreClient,
    input: DetailInput,
) -> Result<(DeploymentListItem, Deployment), AppError> {
    let selector = input.validate("GetDeployment")?;
    let summary = resolve(
        client,
        &selector,
        "ListDeployments",
        |search, page, limit| deployments(client, search, page, limit),
    )
    .await?;
    let full = client
        .read(GetDeployment {
            deployment: summary.id.clone(),
        })
        .await?;
    check_identity(
        &summary,
        &full.id,
        &full.name,
        full.template,
        "GetDeployment",
    )?;
    if active_target(&summary.info.server_id, &summary.info.swarm_id)
        != active_target(&full.config.server_id, &full.config.swarm_id)
    {
        return Err(changed("GetDeployment"));
    }
    Ok((summary, full))
}
