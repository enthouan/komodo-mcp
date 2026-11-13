//! Fresh known-container observations. Cached absence never establishes cleanup.
use std::collections::BTreeSet;

use komodo_client::{
    api::read::{InspectContainer, ListAllStackServices, ListContainers},
    entities::docker::container::Container,
};
use serde_json::{Value, json};

use super::{ResourceKind, Selected, Workload, invalid};
use crate::{
    client::CoreClient,
    configuration::bounded_text,
    error::{AppError, ErrorKind},
};

const INSPECTIONS: usize = 8;
const CACHE_ITEMS: usize = 128;
pub(super) struct Evidence {
    pub workloads: Vec<Workload>,
    pub snapshot: Value,
    pub gaps: Vec<&'static str>,
    pub errors: Vec<AppError>,
}
fn docker_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
async fn inspect(client: &CoreClient, server: &str, locator: &str) -> Result<Container, AppError> {
    let c = client
        .read(InspectContainer {
            server: server.into(),
            container: locator.into(),
        })
        .await?;
    let id =
        c.id.as_deref()
            .filter(|s| docker_id(s))
            .ok_or_else(invalid)?;
    let name = c
        .name
        .as_deref()
        .filter(|s| !s.trim_start_matches('/').is_empty())
        .ok_or_else(invalid)?;
    if (docker_id(locator) && !id.eq_ignore_ascii_case(locator))
        || (!docker_id(locator) && name.trim_start_matches('/') != locator.trim_start_matches('/'))
    {
        return Err(invalid());
    }
    let state = c.state.as_ref().ok_or_else(invalid)?;
    if state.running.is_none()
        || serde_json::to_value(state.status)
            .expect("state")
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err(invalid());
    }
    Ok(c)
}
fn state(c: &Container) -> String {
    serde_json::to_value(c.state.as_ref().expect("validated state").status)
        .expect("state")
        .as_str()
        .expect("state string")
        .into()
}
fn error_signature(e: &AppError) -> Value {
    json!({"kind":e.kind,"operation":e.operation,"status":e.status})
}

pub(super) async fn before(client: &CoreClient, s: &Selected) -> Result<Evidence, AppError> {
    let mut e = Evidence {
        workloads: vec![],
        snapshot: json!({}),
        gaps: vec![
            "Runtime discovery includes Core cache evidence and at most eight fresh container inspections. Cached empty lists do not prove absence; hidden, new or uninspected workloads and Compose orphans can remain unknown.",
        ],
        errors: vec![],
    };
    if s.is_swarm {
        e.gaps.push("Swarm removal can be asynchronous; fresh Server-container verification does not cover Swarm services or stacks.");
        e.snapshot = json!({"mode":"swarm","state":s.state,"scope":s.runtime_name});
        return Ok(e);
    }
    let Some(server) = &s.server_id else {
        e.gaps.push(
            "No observable Server target is attached; runtime cleanup cannot be established.",
        );
        e.snapshot = json!({"mode":"no_server","state":s.state});
        return Ok(e);
    };
    let mut candidates = vec![];
    let mut service_names = BTreeSet::new();
    let mut reads = vec![];
    if s.target.kind == ResourceKind::Deployment {
        candidates.push(s.runtime_name.clone());
    } else {
        if s.target.kind == ResourceKind::Stack {
            match client
                .read(ListAllStackServices {
                    stacks: vec![s.target.id.clone()],
                    limit: Some(100),
                    tags: vec![],
                    terms: vec![],
                    state: vec![],
                    page: 0,
                })
                .await
            {
                Ok(services) => {
                    if services.len() > 100 {
                        return Err(AppError::arguments(
                            "resource_delete",
                            "At most 100 Stack service records can be inspected for deletion; inspect a larger Stack directly in Core",
                        ));
                    }
                    if services.len() == 100 {
                        e.gaps.push("Stack service page reached its positive limit; additional services may be unknown.");
                    }
                    let mut states = vec![];
                    for svc in services {
                        if svc.stack_id != s.target.id {
                            return Err(invalid());
                        }
                        if let Some(c) = svc.container {
                            if c.server_id.as_ref().is_some_and(|id| id != server) {
                                return Err(invalid());
                            }
                            states.push(json!({"service":svc.service,"name":c.name,"id":c.id,"state":c.state}));
                            service_names.insert(c.name.clone());
                            candidates.push(c.name);
                        }
                    }
                    states.sort_by_key(Value::to_string);
                    reads.push(json!({"stack_services":states}));
                }
                Err(err) if err.kind == ErrorKind::InvalidResponse => return Err(err),
                Err(err) => {
                    reads.push(json!({"stack_services_error":error_signature(&err)}));
                    e.errors.push(err);
                    e.gaps.push("Stack service observations were unavailable; the preview cannot enumerate all affected services.");
                }
            }
        }
        match client
            .read(ListContainers {
                server: server.clone(),
            })
            .await
        {
            Ok(mut containers) => {
                if containers.len() > CACHE_ITEMS {
                    e.gaps.push("Server cache inventory exceeded the retained container bound; additional workloads are unknown.");
                }
                containers.sort_by(|a, b| {
                    let priority = |name: &str| {
                        s.target.kind == ResourceKind::Stack && name.starts_with(&s.runtime_name)
                    };
                    (!priority(&a.name), &a.name).cmp(&(!priority(&b.name), &b.name))
                });
                let mut cached = vec![];
                for c in containers.into_iter().take(CACHE_ITEMS) {
                    if c.server_id.as_ref().is_some_and(|id| id != server) {
                        return Err(invalid());
                    }
                    if s.target.kind == ResourceKind::Server || c.name.starts_with(&s.runtime_name)
                    {
                        cached.push(json!({"name":c.name,"id":c.id,"state":c.state}));
                    }
                    candidates.push(c.name);
                }
                cached.sort_by_key(Value::to_string);
                reads.push(json!({"cached_containers":cached}));
            }
            Err(err) if err.kind == ErrorKind::InvalidResponse => return Err(err),
            Err(err) => {
                reads.push(json!({"container_cache_error":error_signature(&err)}));
                e.errors.push(err);
                e.gaps
                    .push("Server container-cache evidence was unavailable.");
            }
        }
    }
    // Service names and deployed-name lookups take precedence over sampled
    // cache entries; stable sorting avoids invalidation from list order alone.
    candidates.sort_by(|a, b| {
        let rank = |name: &str| {
            if service_names.contains(name) {
                0
            } else if s.target.kind == ResourceKind::Stack && name.starts_with(&s.runtime_name) {
                1
            } else {
                2
            }
        };
        (rank(a), a).cmp(&(rank(b), b))
    });
    let mut seen = BTreeSet::new();
    candidates.retain(|v| seen.insert(v.clone()));
    if candidates.len() > INSPECTIONS {
        e.gaps.push("Fresh container inspection limit reached; additional project containers or orphans may be affected.");
    }
    let mut inspected = vec![];
    for locator in candidates.iter().take(INSPECTIONS) {
        if locator.is_empty() || locator.len() > 256 {
            return Err(invalid());
        }
        match inspect(client, server, locator).await {
            Ok(c) => {
                let id = c.id.as_ref().expect("validated id").to_ascii_lowercase();
                let name = c
                    .name
                    .as_deref()
                    .expect("validated name")
                    .trim_start_matches('/');
                let status = state(&c);
                let project = c
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.get("com.docker.compose.project"));
                inspected.push(
                    json!({"locator":locator,"id":id,"name":name,"state":status,"project":project}),
                );
                if s.target.kind == ResourceKind::Stack && project != Some(&s.runtime_name) {
                    e.gaps.push("A cached service/container name did not corroborate the deployed Compose project label; it is not proof of this project's workload.");
                    continue;
                }
                if !e.workloads.iter().any(|w| w.container_id == id) {
                    e.workloads.push(Workload {
                        server_id: server.clone(),
                        container_id: id,
                        name: bounded_text(client, name, 256),
                        state_before: status,
                        observation: "fresh_inspection_before_delete".into(),
                        after_state: None,
                        error: None,
                    });
                }
            }
            Err(err) if err.kind == ErrorKind::InvalidResponse => return Err(err),
            Err(err) => {
                inspected.push(json!({"locator":locator,"error":error_signature(&err)}));
                e.errors.push(err);
                e.gaps.push("A fresh container inspection was unavailable or a cached container was already gone; runtime scope remains incomplete.");
            }
        }
    }
    e.workloads
        .sort_by(|a, b| a.container_id.cmp(&b.container_id));
    inspected.sort_by_key(Value::to_string);
    e.snapshot = json!({"state":s.state,"scope":s.runtime_name,"server":server,"reads":reads,"candidates":candidates,"inspected":inspected});
    Ok(e)
}

pub(super) async fn after(client: &CoreClient, workloads: &mut [Workload]) {
    for w in workloads {
        match inspect(client, &w.server_id, &w.container_id).await {
            Ok(c) => {
                w.observation = "fresh_inspection_still_present_after_attempt".into();
                w.after_state = Some(state(&c));
            }
            Err(err) if err.kind == ErrorKind::NotFound => {
                w.observation = "fresh_inspection_reports_known_id_absent".into();
            }
            Err(err) => {
                w.observation = "post_attempt_inspection_unavailable".into();
                w.error = Some(err);
            }
        }
    }
}
