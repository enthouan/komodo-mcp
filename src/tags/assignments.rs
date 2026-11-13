//! Positive bounded tag-assignment reads across the pinned cleanup families.
use super::{ids, invalid};
use crate::{
    client::CoreClient,
    configuration::bounded_text,
    deletion::{Coverage, ResourceKind, raw_id},
    error::{AppError, ErrorKind},
};
use komodo_client::api::read::{
    GetAction, GetAlerter, GetBuild, GetBuilder, GetDeployment, GetProcedure, GetRepo,
    GetResourceSync, GetServer, GetStack, GetSwarm, ListActions, ListAlerters, ListBuilders,
    ListBuilds, ListDeployments, ListProcedures, ListRepos, ListResourceSyncs, ListServers,
    ListStacks, ListSwarms,
};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, JsonSchema)]
pub struct Assignment {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub template: bool,
    pub tags_before: Vec<String>,
    pub expected_effect: &'static str,
    pub observation: Option<String>,
    pub tags_after: Option<Vec<String>>,
    pub unrelated_tags_preserved: Option<bool>,
    pub error: Option<AppError>,
}
const FAMILIES: [&str; 11] = [
    "server",
    "stack",
    "deployment",
    "swarm",
    "build",
    "repo",
    "builder",
    "alerter",
    "procedure",
    "action",
    "resource_sync",
];
fn core_kind(family: &str) -> &'static str {
    match family {
        "server" => "Server",
        "stack" => "Stack",
        "deployment" => "Deployment",
        "swarm" => "Swarm",
        "build" => "Build",
        "repo" => "Repo",
        "builder" => "Builder",
        "alerter" => "Alerter",
        "procedure" => "Procedure",
        "action" => "Action",
        "resource_sync" => "ResourceSync",
        _ => unreachable!("known family"),
    }
}
async fn page(
    client: &CoreClient,
    family: &str,
    tag: &str,
    page: u64,
    limit: u64,
) -> Result<Value, AppError> {
    macro_rules! list {
        ($t:ident) => {{
            let mut r = $t {
                page,
                limit: Some(limit),
                ..Default::default()
            };
            r.query.tags = vec![tag.into()];
            client.read_references(r).await
        }};
    }
    match family {
        "server" => list!(ListServers),
        "stack" => list!(ListStacks),
        "deployment" => list!(ListDeployments),
        "swarm" => list!(ListSwarms),
        "build" => list!(ListBuilds),
        "repo" => list!(ListRepos),
        "builder" => list!(ListBuilders),
        "alerter" => list!(ListAlerters),
        "procedure" => list!(ListProcedures),
        "action" => list!(ListActions),
        "resource_sync" => list!(ListResourceSyncs),
        _ => Err(invalid()),
    }
}
fn decode(
    client: &CoreClient,
    family: &str,
    tag: &str,
    raw: &Value,
    limit: u64,
) -> Result<Vec<Assignment>, AppError> {
    let list = raw
        .as_array()
        .filter(|v| v.len() <= limit as usize)
        .ok_or_else(invalid)?;
    let mut seen = BTreeSet::new();
    let mut result = vec![];
    for item in list {
        let id = raw_id(item)?;
        let name = item["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(invalid)?;
        let template = item["template"].as_bool().ok_or_else(invalid)?;
        let before = ids(&item["tags"])?;
        if item["type"] != core_kind(family)
            || !before.iter().any(|t| t == tag)
            || !seen.insert(id.clone())
        {
            return Err(invalid());
        }
        result.push(Assignment {
            kind: family.into(),
            id,
            name: bounded_text(client, name, 256),
            template,
            tags_before: before,
            expected_effect: if family == "swarm" {
                "Pinned Core omits Swarm records; this tag assignment can remain dangling"
            } else {
                "Core removes this exact tag ID; unrelated assignments should remain"
            },
            observation: None,
            tags_after: None,
            unrelated_tags_preserved: None,
            error: None,
        });
    }
    Ok(result)
}
pub(super) struct Scan {
    pub assignments: Vec<Assignment>,
    pub coverage: Vec<Coverage>,
    pub snapshot: Value,
}
pub(super) async fn scan(client: &CoreClient, tag: &str) -> Result<Scan, AppError> {
    let mut assignments = vec![];
    let mut coverage = vec![];
    let mut snapshot = vec![];
    for family in FAMILIES {
        let mut c = Coverage {
            family: family.into(),
            visible_records_examined: 0,
            visible_scan_complete: false,
            hidden_records_possible: true,
            error: None,
        };
        let mut seen = BTreeSet::new();
        for page_index in 0..2 {
            let raw = match page(client, family, tag, page_index, 20).await {
                Ok(v) => v,
                Err(e) if e.kind == ErrorKind::InvalidResponse => return Err(e),
                Err(e) => {
                    c.error = Some(e);
                    break;
                }
            };
            let rows = decode(client, family, tag, &raw, 20)?;
            let count = rows.len();
            for row in rows {
                if !seen.insert(row.id.clone()) {
                    return Err(invalid());
                }
                let raw_item = raw
                    .as_array()
                    .expect("list")
                    .iter()
                    .find(|v| raw_id(v).is_ok_and(|id| id == row.id))
                    .expect("validated row");
                snapshot.push(json!({"kind":family,"id":row.id,"name":raw_item["name"],"template":row.template,"tags":row.tags_before}));
                assignments.push(row);
                c.visible_records_examined += 1;
            }
            if count < 20 {
                c.visible_scan_complete = true;
                break;
            }
        }
        coverage.push(c);
    }
    assignments.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    snapshot.sort_by_key(Value::to_string);
    Ok(Scan {
        assignments,
        snapshot: json!({"assignments":snapshot,"coverage":coverage}),
        coverage,
    })
}
pub(super) async fn find_page(
    client: &CoreClient,
    kind: ResourceKind,
    tag: &str,
    page_index: u64,
    limit: u64,
) -> Result<Vec<Assignment>, AppError> {
    let family = match kind {
        ResourceKind::Stack => "stack",
        ResourceKind::Deployment => "deployment",
        ResourceKind::Server => "server",
    };
    let raw = page(client, family, tag, page_index, limit).await?;
    decode(client, family, tag, &raw, limit)
}
async fn get(client: &CoreClient, r: &Assignment) -> Result<Value, AppError> {
    match r.kind.as_str() {
        "server" => {
            client
                .read_references(GetServer {
                    server: r.id.clone(),
                })
                .await
        }
        "stack" => {
            client
                .read_references(GetStack {
                    stack: r.id.clone(),
                })
                .await
        }
        "deployment" => {
            client
                .read_references(GetDeployment {
                    deployment: r.id.clone(),
                })
                .await
        }
        "swarm" => {
            client
                .read_references(GetSwarm {
                    swarm: r.id.clone(),
                })
                .await
        }
        "build" => {
            client
                .read_references(GetBuild {
                    build: r.id.clone(),
                })
                .await
        }
        "repo" => client.read_references(GetRepo { repo: r.id.clone() }).await,
        "builder" => {
            client
                .read_references(GetBuilder {
                    builder: r.id.clone(),
                })
                .await
        }
        "alerter" => {
            client
                .read_references(GetAlerter {
                    alerter: r.id.clone(),
                })
                .await
        }
        "procedure" => {
            client
                .read_references(GetProcedure {
                    procedure: r.id.clone(),
                })
                .await
        }
        "action" => {
            client
                .read_references(GetAction {
                    action: r.id.clone(),
                })
                .await
        }
        "resource_sync" => {
            client
                .read_references(GetResourceSync { sync: r.id.clone() })
                .await
        }
        _ => Err(invalid()),
    }
}
pub(super) async fn observe(client: &CoreClient, tag: &str, assignments: &mut [Assignment]) {
    for r in assignments {
        let observed = async {
            let value = get(client, r).await?;
            if raw_id(&value)? != r.id {
                return Err(invalid());
            }
            let after = ids(&value["tags"])?;
            let still = after.iter().any(|id| id == tag);
            r.unrelated_tags_preserved = Some(
                after
                    .iter()
                    .filter(|id| id.as_str() != tag)
                    .eq(r.tags_before.iter().filter(|id| id.as_str() != tag)),
            );
            r.tags_after = Some(after);
            Ok(if still {
                "tag_still_assigned"
            } else {
                "tag_no_longer_assigned"
            })
        }
        .await;
        match observed {
            Ok(state) => r.observation = Some(state.into()),
            Err(e) => {
                r.observation = Some(
                    if e.kind == ErrorKind::NotFound {
                        "resource_not_found"
                    } else {
                        "assignment_observation_unavailable"
                    }
                    .into(),
                );
                r.error = Some(e);
            }
        }
    }
}
