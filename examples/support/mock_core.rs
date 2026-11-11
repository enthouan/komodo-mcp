//! Repeated, independently ordered reads over the tagged synthetic wire fixtures.
#![allow(dead_code)] // Shared by examples and integration test binaries.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
};
use komodo_client::{
    api::read::{
        GetDeployment, GetServer, GetStack, GetVersion, ListDeployments, ListServers, ListStacks,
    },
    entities::{
        deployment::{Deployment, DeploymentListItem},
        permission::{Permission, PermissionLevel},
        server::{Server, ServerListItem},
        stack::{Stack, StackListItem},
        user::User,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};

pub const KEY: &str = "fixture-API-KEY-7d42";
pub const SECRET: &str = "fixture-API-SECRET-a913";
pub const DEFAULT_SERVER: &str = "fixture-server";
pub const DEADLINE: Duration = Duration::from_secs(25);
const VERSION: &str = include_str!("../../tests/fixtures/core-2.3.2/version.json");

#[derive(Deserialize)]
#[serde(tag = "type", content = "params")]
enum Read {
    GetVersion(GetVersion),
    ListServers(ListServers),
    GetServer(GetServer),
    ListStacks(ListStacks),
    GetStack(GetStack),
    ListDeployments(ListDeployments),
    GetDeployment(GetDeployment),
}

#[derive(Clone)]
struct Resource {
    summary: Value,
    detail: Value,
}
#[derive(Clone)]
struct Inventory {
    servers: Vec<Resource>,
    stacks: Vec<Resource>,
    deployments: Vec<Resource>,
}

fn fixture(source: &str) -> Value {
    serde_json::from_str(source).expect("checked-in JSON fixture")
}

impl Inventory {
    fn load() -> Result<Self> {
        let manifest = fixture(include_str!("../../tests/fixtures/core-2.3.2/demo.json"));
        let server_base =
            fixture(include_str!("../../tests/fixtures/core-2.3.2/servers.json"))[0].clone();
        let server_detail = fixture(include_str!(
            "../../tests/fixtures/core-2.3.2/server-detail.json"
        ));
        let stack_base =
            fixture(include_str!("../../tests/fixtures/core-2.3.2/stacks.json"))[0].clone();
        let stack_detail = fixture(include_str!(
            "../../tests/fixtures/core-2.3.2/stack-detail.json"
        ));
        let mut inventory = Self {
            servers: Vec::new(),
            stacks: Vec::new(),
            deployments: vec![Resource {
                summary: fixture(include_str!(
                    "../../tests/fixtures/core-2.3.2/deployments.json"
                ))[0]
                    .clone(),
                detail: fixture(include_str!(
                    "../../tests/fixtures/core-2.3.2/deployment-detail.json"
                )),
            }],
        };
        for spec in manifest["servers"].as_array().context("demo servers")? {
            let mut summary = server_base.clone();
            let mut detail = server_detail.clone();
            for field in ["id", "name"] {
                summary[field] = spec[field].clone();
            }
            detail["_id"]["$oid"] = spec["id"].clone();
            detail["name"] = spec["name"].clone();
            inventory.servers.push(Resource { summary, detail });
        }
        for spec in manifest["stacks"].as_array().context("demo stacks")? {
            let mut summary = stack_base.clone();
            let mut detail = stack_detail.clone();
            for field in ["id", "name"] {
                summary[field] = spec[field].clone();
            }
            detail["_id"]["$oid"] = spec["id"].clone();
            detail["name"] = spec["name"].clone();
            for field in ["server_id", "swarm_id"] {
                summary["info"][field] = spec[field].clone();
                detail["config"][field] = spec[field].clone();
            }
            summary["info"]["server_name"] = inventory
                .servers
                .iter()
                .find(|r| r.summary["id"] == spec["server_id"])
                .map(|r| r.summary["name"].clone())
                .unwrap_or(json!(""));
            summary["info"]["swarm_name"] = json!(if spec["swarm_id"] == "" {
                ""
            } else {
                "fixture-swarm"
            });
            summary["info"]["state"] = spec["state"].clone();
            summary["info"]["status"] = spec["state"].clone();
            detail["config"]["project_name"] = spec["name"].clone();
            detail["info"]["deployed_project_name"] = spec["name"].clone();
            inventory.stacks.push(Resource { summary, detail });
        }
        // Validate the assembled wire shapes and every visible resource's Read grant.
        let user: User =
            serde_json::from_str(include_str!("../../tests/fixtures/core-2.3.2/reader.json"))?;
        ensure!(
            user.enabled && !user.admin && !user.super_admin,
            "expected enabled non-admin identity"
        );
        let grants: Vec<Permission> = serde_json::from_str(include_str!(
            "../../tests/fixtures/core-2.3.2/demo-permissions.json"
        ))?;
        for grant in &grants {
            ensure!(
                grant.level == PermissionLevel::Read && grant.specific.is_empty(),
                "Read grants only"
            );
            ensure!(
                serde_json::to_value(&grant.user_target)? == json!({"type":"User","id":user.id}),
                "wrong grant identity"
            );
        }
        for (family, resources) in [
            ("Server", &inventory.servers),
            ("Stack", &inventory.stacks),
            ("Deployment", &inventory.deployments),
        ] {
            for resource in resources {
                ensure!(
                    grants
                        .iter()
                        .any(|g| serde_json::to_value(&g.resource_target).ok()
                            == Some(json!({"type":family,"id":resource.summary["id"]}))),
                    "missing resource Read grant"
                );
                match family {
                    "Server" => {
                        serde_json::from_value::<ServerListItem>(resource.summary.clone())?;
                        serde_json::from_value::<Server>(resource.detail.clone())?;
                    }
                    "Stack" => {
                        serde_json::from_value::<StackListItem>(resource.summary.clone())?;
                        serde_json::from_value::<Stack>(resource.detail.clone())?;
                    }
                    _ => {
                        serde_json::from_value::<DeploymentListItem>(resource.summary.clone())?;
                        serde_json::from_value::<Deployment>(resource.detail.clone())?;
                    }
                }
            }
        }
        Ok(inventory)
    }
}

#[derive(Clone, Debug)]
pub struct RequestEvidence {
    pub operation: String,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}
#[derive(Default)]
struct Observations {
    requests: Vec<RequestEvidence>,
    unexpected: Vec<&'static str>,
    rejected_credentials: usize,
}
#[derive(Clone)]
struct AppState {
    inventory: Inventory,
    observations: Arc<Mutex<Observations>>,
    fail_operation: Option<&'static str>,
}

pub struct MockCore {
    pub address: String,
    observations: Arc<Mutex<Observations>>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}
impl MockCore {
    pub async fn start(bind: SocketAddr) -> Result<Self> {
        Self::start_with_failure(bind, None).await
    }
    pub async fn start_with_failure(
        bind: SocketAddr,
        fail_operation: Option<&'static str>,
    ) -> Result<Self> {
        ensure!(
            bind.ip().is_loopback(),
            "mock Core requires a loopback bind address"
        );
        let inventory = Inventory::load()?;
        let listener = timeout(DEADLINE, TcpListener::bind(bind))
            .await
            .context("mock bind deadline")??;
        let address = format!("http://{}/komodo", listener.local_addr()?);
        let observations = Arc::new(Mutex::new(Observations::default()));
        let state = AppState {
            inventory,
            observations: observations.clone(),
            fail_operation,
        };
        let app = Router::new()
            .fallback(handle)
            .layer(DefaultBodyLimit::max(64 * 1024))
            .with_state(state);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        // Returning this bound address is the in-process readiness signal.
        Ok(Self {
            address,
            observations,
            stop: Some(stop),
            task,
        })
    }
    pub fn requests(&self) -> Vec<RequestEvidence> {
        self.observations.lock().unwrap().requests.clone()
    }
    pub fn rejected_credentials(&self) -> usize {
        self.observations.lock().unwrap().rejected_credentials
    }
    pub fn verify_reads(&self) -> Result<()> {
        let observations = self.observations.lock().unwrap();
        ensure!(
            observations.unexpected.is_empty(),
            "mock observed unsupported requests: {:?}",
            observations.unexpected
        );
        ensure!(
            observations.requests.iter().all(|r| matches!(
                r.operation.as_str(),
                "GetVersion"
                    | "ListServers"
                    | "GetServer"
                    | "ListStacks"
                    | "GetStack"
                    | "ListDeployments"
                    | "GetDeployment"
            )),
            "non-read request observed"
        );
        Ok(())
    }
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match timeout(Duration::from_secs(5), &mut self.task).await {
            Ok(result) => result.context("mock task")?.context("mock listener"),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                anyhow::bail!("mock shutdown deadline");
            }
        }
    }
}
impl Drop for MockCore {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, axum::Json<Value>) {
    (
        status,
        axum::Json(json!({"error":message.into(),"trace":[]})),
    )
}
async fn handle(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if method != Method::POST || uri.path() != "/komodo/read" || uri.query().is_some() {
        state
            .observations
            .lock()
            .unwrap()
            .unexpected
            .push("non-read endpoint");
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "mock supports only POST /komodo/read",
        );
    }
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some(KEY)
        || headers.get("x-api-secret").and_then(|v| v.to_str().ok()) != Some(SECRET)
    {
        state.observations.lock().unwrap().rejected_credentials += 1;
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(fixture(include_str!(
                "../../tests/fixtures/core-2.3.2/authentication.json"
            ))),
        );
    }
    let read = match serde_json::from_slice::<Read>(&body) {
        Ok(read) => read,
        Err(_) => {
            state
                .observations
                .lock()
                .unwrap()
                .unexpected
                .push("unsupported read envelope");
            return error(StatusCode::BAD_REQUEST, "unsupported mock read envelope");
        }
    };
    let (operation, params, resources, selector) = match read {
        Read::GetVersion(_) => ("GetVersion", json!({}), None, None),
        Read::ListServers(p) => (
            "ListServers",
            json!(p),
            Some(&state.inventory.servers),
            None,
        ),
        Read::GetServer(p) => (
            "GetServer",
            json!({}),
            Some(&state.inventory.servers),
            Some(p.server),
        ),
        Read::ListStacks(p) => ("ListStacks", json!(p), Some(&state.inventory.stacks), None),
        Read::GetStack(p) => (
            "GetStack",
            json!({}),
            Some(&state.inventory.stacks),
            Some(p.stack),
        ),
        Read::ListDeployments(p) => (
            "ListDeployments",
            json!(p),
            Some(&state.inventory.deployments),
            None,
        ),
        Read::GetDeployment(p) => (
            "GetDeployment",
            json!({}),
            Some(&state.inventory.deployments),
            Some(p.deployment),
        ),
    };
    state
        .observations
        .lock()
        .unwrap()
        .requests
        .push(RequestEvidence {
            operation: operation.into(),
            page: params["page"].as_u64(),
            limit: params["limit"].as_u64(),
        });
    if state.fail_operation == Some(operation) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "injected synthetic read failure",
        );
    }
    let Some(resources) = resources else {
        return (StatusCode::OK, axum::Json(fixture(VERSION)));
    };
    if let Some(selector) = selector {
        if selector == "restricted-server" {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(fixture(include_str!(
                    "../../tests/fixtures/core-2.3.2/permission.json"
                ))),
            );
        }
        return match resources
            .iter()
            .find(|r| r.summary["id"] == selector || r.summary["name"] == selector)
        {
            Some(resource) => (StatusCode::OK, axum::Json(resource.detail.clone())),
            None => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "Did not find any {} matching {selector}",
                    operation.trim_start_matches("Get")
                ),
            ),
        };
    }
    match list(resources, &params) {
        Ok(items) => (StatusCode::OK, axum::Json(json!(items))),
        Err(_) => {
            state
                .observations
                .lock()
                .unwrap()
                .unexpected
                .push("unsupported list options");
            error(StatusCode::BAD_REQUEST, "unsupported mock list options")
        }
    }
}

fn is_id(name: &str) -> bool {
    name.len() == 24 && name.bytes().all(|b| b.is_ascii_hexdigit())
}
fn list(resources: &[Resource], params: &Value) -> Result<Vec<Value>> {
    // Production tools use Name ordering and common resource filters. Other sorts
    // and family-specific filters are deliberately rejected instead of approximated.
    ensure!(params["sort_by"] == "Name", "only Name ordering supported");
    let query = &params["query"];
    let specific = &query["specific"];
    ensure!(
        specific
            .as_object()
            .is_none_or(|s| s.values().all(|v| v == &json!([]) || v == &json!(false))),
        "specific filter unsupported"
    );
    let names: Vec<&str> = query["names"]
        .as_array()
        .context("query names")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let (ids, names): (Vec<_>, Vec<_>) = names.into_iter().partition(|name| is_id(name));
    let terms = query["terms"].as_array().context("query terms")?;
    let tags = query["tags"].as_array().context("query tags")?;
    let mut items: Vec<_> = resources.iter().filter(|r| {
        let name = r.summary["name"].as_str().unwrap();
        (ids.is_empty() || ids.iter().any(|id| r.summary["id"].as_str().unwrap().eq_ignore_ascii_case(id)))
            && (names.is_empty() || names.contains(&name))
            && terms.iter().all(|t| name.to_lowercase().contains(&t.as_str().unwrap_or("").to_lowercase()))
            && tags.is_empty() // Every demo resource is untagged, so Any and All are empty.
            && match query["templates"].as_str() { Some("Only") => r.summary["template"] == true, Some("Exclude") => r.summary["template"] != true, _ => true }
    }).map(|r| r.summary.clone()).collect();
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    if params["sort_desc"] == true {
        items.reverse();
    }
    let limit = params["limit"].as_u64().unwrap_or(30);
    if limit == 0 {
        return Ok(items);
    }
    let offset = params["page"]
        .as_u64()
        .context("page")?
        .checked_mul(limit)
        .context("page overflow")?;
    Ok(items
        .into_iter()
        .skip(usize::try_from(offset)?)
        .take(usize::try_from(limit)?)
        .collect())
}
