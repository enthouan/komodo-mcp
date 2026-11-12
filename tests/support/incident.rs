//! Shared unordered incident mock, with observed read/concurrency evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use super::{KEY, SECRET};

pub fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/core-2.3.2/incident.json")).unwrap()
}
pub fn wire() -> Value {
    serde_json::from_str(include_str!("../fixtures/core-2.3.2/incident-wire.json")).unwrap()
}

#[derive(Clone, Copy, Default)]
pub enum Mode {
    #[default]
    Broken,
    Healthy,
    Inconclusive,
}

#[derive(Clone, Default)]
pub struct Options {
    pub mode: Mode,
    pub failures: BTreeSet<String>,
    pub delay: Duration,
    pub overrides: BTreeMap<String, Value>,
    pub api_key: Option<String>,
    pub chunked: bool,
    pub swarm: bool,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub operation: String,
    pub params: Value,
}

struct Inner {
    options: Options,
    requests: Mutex<Vec<Request>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

pub struct IncidentCore {
    pub address: String,
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl IncidentCore {
    pub async fn start(options: Options) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let inner = Arc::new(Inner {
            options,
            requests: Mutex::new(vec![]),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/komodo/read", post(read))
            .with_state(inner.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            address,
            inner,
            task,
        }
    }
    pub fn client(&self, deadline: Duration) -> CoreClient {
        CoreClient::new(
            Config::parse(&self.address, self.api_key(), SECRET).unwrap(),
            RequestPolicy {
                connect_timeout: deadline,
                operation_timeout: deadline,
            },
        )
        .unwrap()
    }
    pub fn api_key(&self) -> &str {
        self.inner.options.api_key.as_deref().unwrap_or(KEY)
    }
    pub fn requests(&self) -> Vec<Request> {
        self.inner.requests.lock().unwrap().clone()
    }
    pub fn peak(&self) -> usize {
        self.inner.peak.load(Ordering::SeqCst)
    }
}
impl Drop for IncidentCore {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn read(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    envelope: Json<Value>,
) -> Response {
    let mut response = read_typed(State(inner.clone()), headers, envelope)
        .await
        .into_response();
    if inner.options.chunked {
        // An unknown-size body makes HTTP/1.1 select real chunked framing.
        // Merely setting Transfer-Encoding on a known-size body can produce
        // conflicting Content-Length headers instead of exercising chunk reads.
        let (mut parts, body) = response.into_parts();
        parts.headers.remove("content-length");
        response = Response::from_parts(
            parts,
            axum::body::Body::from_stream(body.into_data_stream()),
        );
    }
    response
}

async fn read_typed(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if headers.get("x-api-key").and_then(|s| s.to_str().ok())
        != Some(inner.options.api_key.as_deref().unwrap_or(KEY))
        || headers.get("x-api-secret").and_then(|s| s.to_str().ok()) != Some(SECRET)
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"synthetic authentication rejected"})),
        );
    }
    let op = envelope["type"].as_str().unwrap_or("");
    let params = &envelope["params"];
    inner.requests.lock().unwrap().push(Request {
        operation: op.into(),
        params: params.clone(),
    });
    if !matches!(
        op,
        "GetVersion"
            | "ListServers"
            | "ListStacks"
            | "ListDeployments"
            | "GetStack"
            | "GetServerState"
            | "GetSystemStats"
            | "ListAllStackServices"
            | "ListUpdates"
            | "ListAlerts"
            | "GetUpdate"
            | "GetStackLog"
            | "GetContainerLog"
            | "GetDeploymentLog"
    ) || (matches!(op, "GetStackLog" | "GetContainerLog" | "GetDeploymentLog")
        && !params["tail"]
            .as_u64()
            .is_some_and(|tail| (1..=500).contains(&tail)))
        || (op == "GetStackLog"
            && inner.options.swarm
            && params["services"]
                .as_array()
                .is_none_or(|services| services.len() != 1))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"fixture rejects unsupported or unbounded read"})),
        );
    }
    let count = inner.active.fetch_add(1, Ordering::SeqCst) + 1;
    inner.peak.fetch_max(count, Ordering::SeqCst);
    let _active = Active(&inner.active);
    tokio::time::sleep(inner.options.delay).await;
    if inner.options.failures.contains(op)
        || (matches!(inner.options.mode, Mode::Inconclusive)
            && matches!(op, "ListUpdates" | "ListAlerts" | "GetStackLog"))
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":format!("synthetic failure {KEY} {SECRET}")})),
        );
    }
    if let Some(value) = inner.options.overrides.get(op) {
        return (StatusCode::OK, Json(value.clone()));
    }
    let f = fixture();
    let mut w = wire();
    if inner.options.swarm {
        w["stacks"][0]["info"]["server_id"] = json!("");
        w["stacks"][0]["info"]["swarm_id"] = json!("000000000000000000000009");
        w["stacks"][0]["info"]["swarm_name"] = json!("fixture-swarm");
        w["stack_detail"]["config"]["server_id"] = json!("");
        w["stack_detail"]["config"]["swarm_id"] = json!("000000000000000000000009");
    }
    match inner.options.mode {
        Mode::Broken => (),
        Mode::Healthy => {
            w["stacks"][0]["info"]["state"] = json!("running");
            w["services"][0]["state"] = json!("Running");
            w["updates"] = json!([]);
            w["alerts"] = json!([]);
            w["log"]["stdout"] = json!("web | ready\n");
        }
        Mode::Inconclusive => {
            w["stacks"][0]["info"]["state"] = json!("unknown");
            for item in w["services"].as_array_mut().unwrap() {
                item["state"] = json!("Unknown");
            }
        }
    }
    let ok = |value| (StatusCode::OK, Json(value));
    match op {
        "GetVersion" => ok(json!({"version":"2.3.2"})),
        "ListServers" => ok(list(&w["servers"], params)),
        "ListStacks" => ok(list(&w["stacks"], params)),
        "ListDeployments" => ok(list(&w["deployments"], params)),
        "GetStack" if params["stack"] == f["stack"]["id"] => ok(w["stack_detail"].clone()),
        "GetServerState" | "GetSystemStats"
            if params["server"] == f["unavailable_server"]["id"] =>
        {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"synthetic unavailable server"})),
            )
        }
        "GetServerState" if params["server"] == f["unknown_server"]["id"] => {
            ok(json!({"status":"Disabled"}))
        }
        "GetServerState" => ok(w["server_state"].clone()),
        "GetSystemStats" => ok(w["stats"].clone()),
        "ListAllStackServices" => {
            let selected = params["stacks"].as_array().cloned().unwrap_or_default();
            if selected == vec![f["stack"]["id"].clone()] {
                ok(w["services"].clone())
            } else if selected == vec![f["healthy_stack"]["id"].clone()] {
                let mut services = w["services"].clone();
                for item in services.as_array_mut().unwrap() {
                    item["stack_id"] = f["healthy_stack"]["id"].clone();
                    item["stack_name"] = f["healthy_stack"]["name"].clone();
                    item["state"] = json!("Running");
                }
                ok(services)
            } else {
                ok(json!([]))
            }
        }
        "ListUpdates" | "ListAlerts" => {
            let key = if op == "ListUpdates" {
                "updates"
            } else {
                "alerts"
            };
            let matching = params["query"]["target.id"].is_null()
                || params["query"]["target.id"] == f["stack"]["id"];
            ok(
                json!({key: if matching && params["page"].as_u64().unwrap_or(0) == 0 { w[key].clone() } else { json!([]) }, "next_page":null}),
            )
        }
        "GetUpdate" if params["id"] == f["update"]["id"] => ok(w["update"].clone()),
        "GetContainerLog" | "GetDeploymentLog" => ok(w["log"].clone()),
        "GetStackLog" if params["stack"] == f["stack"]["id"] => {
            let mut log = w["log"].clone();
            if let Some(services) = params["services"].as_array()
                && !services.is_empty()
                && !services.contains(&f["affected_service"])
            {
                log["stdout"] = json!("db | ready\n");
            }
            ok(log)
        }
        _ => (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"fixture rejects unsupported read or mutation"})),
        ),
    }
}

fn list(items: &Value, params: &Value) -> Value {
    let mut items: Vec<Value> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            params["query"]["names"].as_array().is_none_or(|names| {
                names.is_empty()
                    || names
                        .iter()
                        .any(|name| *name == item["id"] || *name == item["name"])
            })
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let size = params["limit"].as_u64().unwrap_or(30) as usize;
    assert!(size > 0 && size <= 101, "unbounded incident inventory read");
    let start = params["page"].as_u64().unwrap_or(0) as usize * size;
    json!(items.into_iter().skip(start).take(size).collect::<Vec<_>>())
}
