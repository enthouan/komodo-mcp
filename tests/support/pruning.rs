//! Pinned-wire deterministic prune faults; never a real Docker engine.
#![allow(dead_code)]
use crate::support::{
    KEY, SECRET,
    inventory::Family,
    operations::{Request, Task},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use komodo_client::entities::{
    Operation, ResourceTarget,
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub const SERVER: &str = "000000000000000000000001";
pub const USER: &str = "000000000000000000000077";
pub const UPDATE: &str = "000000000000000000000091";
pub const PRIVATE: [&str; 4] = [
    "private-prune-daemon-output-21a",
    "private-prune-connection-reference-21b",
    "private-prune-password-hash-21c",
    "0123456789abcdef0123456789abcdef01234567",
];
#[derive(Clone, Copy, Default)]
pub enum Reply {
    #[default]
    Valid,
    Denied,
    Busy,
    Stalled,
    Lost,
    Malformed,
    Oversized,
    Redirect,
    WrongTarget,
    WrongOperation,
    WrongId,
    BeforeError,
}
pub struct Data {
    pub requests: Vec<Request>,
    pub server: Value,
    pub summaries: Vec<Value>,
    pub caller: Value,
    pub permission: Value,
    pub reply: Reply,
    pub bad_credentials: bool,
    pub accepted_prunes: usize,
    pub update: Value,
    pub update_success: bool,
    pub history_status: Option<u16>,
    pub server_reads: usize,
    pub permission_reads: usize,
    pub caller_reads: usize,
    pub drift_server: Option<(usize, Value)>,
    pub drift_permission: Option<(usize, Value)>,
    pub drift_caller: Option<(usize, Value)>,
    pub inventory_status: Option<u16>,
    pub read_status: Option<u16>,
    pub reads: BTreeMap<String, Value>,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn update(op: &str, success: bool) -> Value {
    let operation: Operation = serde_json::from_value(json!(op)).unwrap();
    serde_json::to_value(Update {
        id: UPDATE.into(),
        operation,
        target: ResourceTarget::Server(SERVER.into()),
        status: UpdateStatus::Complete,
        success,
        commit_hash: PRIVATE[3].into(),
        logs: vec![Log {
            stage: PRIVATE[0].into(),
            stdout: PRIVATE[0].into(),
            stderr: PRIVATE[0].into(),
            command: PRIVATE[1].into(),
            success,
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap()
}
impl Core {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let mut server = Family::Server.detail();
        server["config"]["private_extension"] = json!(PRIVATE[1]);
        let mut summary = Family::Server.summary();
        summary["info"]["state"] = json!("Ok");
        let data = Arc::new(Mutex::new(Data {
            requests: vec![],
            server,
            summaries: vec![summary],
            caller: json!({"_id":{"$oid":USER},"username":"scoped-operator","enabled":true,"admin":false,"super_admin":false,"config":{"type":"Local","data":{"password":PRIVATE[2]}}}),
            permission: json!({"level":"Execute","specific":[]}),
            reply: Reply::Valid,
            bad_credentials: false,
            accepted_prunes: 0,
            update: update("PruneImages", true),
            update_success: true,
            history_status: None,
            server_reads: 0,
            permission_reads: 0,
            caller_reads: 0,
            drift_server: None,
            drift_permission: None,
            drift_caller: None,
            inventory_status: None,
            read_status: None,
            reads: BTreeMap::new(),
        }));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/user", get(caller))
            .route("/komodo/execute", post(execute))
            .route("/komodo/write", post(unexpected))
            .route("/komodo/terminal/execute", post(unexpected))
            .fallback(fallback)
            .with_state(data.clone());
        Self {
            address,
            data,
            _task: Task(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            })),
        }
    }
    pub fn requests(&self, path: &str) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
    pub fn executions(&self) -> Vec<Request> {
        self.requests("execute")
    }
}
fn failure(status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        format!("{} {KEY} {SECRET}", PRIVATE[0]),
    )
        .into_response()
}
fn record(d: &mut Data, h: &HeaderMap, path: &'static str, op: &str, params: Value) -> bool {
    let ok = h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(KEY)
        && h.get("x-api-secret").and_then(|v| v.to_str().ok()) == Some(SECRET);
    d.bad_credentials |= !ok;
    d.requests.push(Request {
        path,
        operation: op.into(),
        params,
    });
    ok
}
async fn fallback(State(d): State<Arc<Mutex<Data>>>) -> Response {
    d.lock().unwrap().requests.push(Request {
        path: "unexpected",
        operation: "unexpected".into(),
        params: Value::Null,
    });
    failure(404)
}
async fn unexpected(
    State(d): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(
        &mut d.lock().unwrap(),
        &h,
        "unexpected",
        v["type"].as_str().unwrap_or("terminal"),
        v["params"].clone(),
    );
    failure(403)
}
async fn caller(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap) -> Response {
    let mut d = d.lock().unwrap();
    if !record(&mut d, &h, "user", "CurrentCaller", json!({})) {
        return failure(401);
    };
    d.caller_reads += 1;
    if let Some((at, v)) = d.drift_caller.clone()
        && at == d.caller_reads
    {
        d.caller = v;
    }
    Json(d.caller.clone()).into_response()
}
async fn read(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = d.lock().unwrap();
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if !record(&mut d, &h, "read", op, p.clone()) {
        return failure(401);
    }
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if let Some(s) = d.read_status {
        return failure(s);
    }
    if let Some(v) = d.reads.get(op) {
        return Json(v.clone()).into_response();
    }
    match op {
        "ListServers" => {
            let a = d.summaries.iter().filter(|v| {
                p["query"]["names"].as_array().is_none_or(|a| {
                    a.is_empty() || a.iter().any(|x| x == &v["name"] || x == &v["id"])
                })
            });
            let limit = p["limit"].as_u64().unwrap() as usize;
            assert!(limit > 0);
            let page = p["page"].as_u64().unwrap() as usize;
            Json(json!(a.skip(page * limit).take(limit).collect::<Vec<_>>())).into_response()
        }
        "GetServer" => {
            d.server_reads += 1;
            if let Some((at, v)) = d.drift_server.clone()
                && at == d.server_reads
            {
                d.server = v;
            }
            if p["server"] != SERVER {
                return failure(404);
            }
            Json(d.server.clone()).into_response()
        }
        "GetPermission" => {
            assert_eq!(p["target"], json!({"type":"Server","id":SERVER}));
            d.permission_reads += 1;
            if let Some((at, v)) = d.drift_permission.clone()
                && at == d.permission_reads
            {
                d.permission = v;
            }
            Json(d.permission.clone()).into_response()
        }
        "GetServerState" => Json(json!({"status":"Ok"})).into_response(),
        "ListImages" | "ListNetworks" | "ListVolumes" | "ListContainers" => {
            if let Some(s) = d.inventory_status {
                return failure(s);
            }
            assert_eq!(p["server"], SERVER);
            Json(json!([])).into_response()
        }
        "GetUpdate" => {
            assert_eq!(p["id"], UPDATE);
            if let Some(s) = d.history_status {
                return failure(s);
            }
            Json(d.update.clone()).into_response()
        }
        "ListUpdates" => {
            if let Some(s) = d.history_status {
                return failure(s);
            }
            Json(json!({"updates":[{"id":UPDATE,"operation":d.update["operation"],"target":d.update["target"],"start_ts":0,"success":d.update["success"],"operator":USER,"username":"scoped-operator","status":"Complete","other_data":""}],"next_page":null})).into_response()
        }
        _ => {
            d.requests.last_mut().unwrap().path = "unexpected";
            failure(404)
        }
    }
}
async fn execute(
    State(state): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let (stalled, response) = {
        let mut d = state.lock().unwrap();
        (
            matches!(d.reply, Reply::Stalled),
            execute_inner(&mut d, h, v),
        )
    };
    if stalled {
        tokio::time::sleep(std::time::Duration::from_secs(16)).await;
    }
    response
}
fn execute_inner(d: &mut Data, h: HeaderMap, v: Value) -> Response {
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if !record(d, &h, "execute", op, p.clone()) {
        return failure(401);
    }
    assert!(matches!(
        op,
        "PruneImages" | "PruneContainers" | "PruneNetworks" | "PruneVolumes" | "PruneSystem"
    ));
    assert_eq!(*p, json!({"server":SERVER}));
    if matches!(d.reply, Reply::Denied) {
        return failure(403);
    }
    if matches!(d.reply, Reply::Busy) {
        return failure(409);
    }
    if matches!(d.reply, Reply::BeforeError) {
        return failure(500);
    }
    d.accepted_prunes += 1;
    d.update = update(op, d.update_success);
    let mut ack = d.update.clone();
    match d.reply {
        Reply::Valid | Reply::Stalled => Json(ack).into_response(),
        Reply::Lost => failure(502),
        Reply::Malformed => (StatusCode::OK, "{").into_response(),
        Reply::Oversized => (StatusCode::OK, "x".repeat(2 * 1024 * 1024 + 1)).into_response(),
        Reply::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", "/komodo/execute")],
            PRIVATE[0],
        )
            .into_response(),
        Reply::WrongTarget => {
            ack["target"]["id"] = json!("000000000000000000000099");
            Json(ack).into_response()
        }
        Reply::WrongOperation => {
            ack["operation"] = json!("PruneBuildx");
            Json(ack).into_response()
        }
        Reply::WrongId => {
            ack["_id"] = json!({"$oid":"invalid"});
            Json(ack).into_response()
        }
        Reply::Denied | Reply::Busy | Reply::BeforeError => unreachable!(),
    }
}
