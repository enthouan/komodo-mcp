//! Authenticated pinned-wire fixture. Records every route and separates saves/tests.
#![allow(dead_code)]
use crate::support::{
    KEY, SECRET,
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
    alerter::AlerterConfig,
    resource::Resource,
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;
pub const ID: &str = "000000000000000000000081";
pub const UPDATE: &str = "000000000000000000000091";
pub const PRIVATE: [&str; 5] = [
    "private-provider-response-18a",
    "private-endpoint-token-19b",
    "private-ntfy-recipient@example.test",
    "private-admin-password-20c",
    "0123456789abcdef0123456789abcdef01234567",
];
#[derive(Clone, Copy, Default)]
pub enum Reply {
    #[default]
    Valid,
    Denied,
    Lost,
    Malformed,
    WrongTarget,
    WrongId,
    MissingEcho,
    Oversized,
    Redirect,
    BeforeError,
}
pub struct Data {
    pub entries: BTreeMap<String, Value>,
    pub role: Value,
    pub requests: Vec<Request>,
    pub bad_credentials: bool,
    pub read_status: Option<u16>,
    pub server_read_status: Option<u16>,
    pub server: Value,
    pub user_status: Option<u16>,
    pub write_reply: Reply,
    pub test_reply: Reply,
    pub get_count: usize,
    pub drift: Option<(usize, Value)>,
    pub post_write_drift: Option<Value>,
    pub post_write_read_status: Option<u16>,
    pub update: Value,
    pub history_status: Option<u16>,
    pub history_override: Option<Value>,
    pub read_override: Option<Value>,
    pub last_written: Option<String>,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn resource(name: &str, kind: &str) -> Value {
    let mut c = serde_json::to_value(AlerterConfig::default()).unwrap();
    c["enabled"] = json!(true);
    c["endpoint"] = json!({"type":kind,"params":{"url":format!("https://example.test/{}/[[ENDPOINT_REF]]",PRIVATE[1])}});
    if kind == "Ntfy" {
        c["endpoint"]["params"]["email"] = json!(PRIVATE[2]);
    }
    let a: Resource<AlerterConfig, ()> = Resource {
        id: ID.into(),
        name: name.into(),
        config: serde_json::from_value(c).unwrap(),
        ..Default::default()
    };
    let mut v = serde_json::to_value(a).unwrap();
    v["config"]["private_extension"] = json!(PRIVATE[1]);
    v
}
pub fn update(op: Operation, success: bool) -> Value {
    serde_json::to_value(Update {
        id: UPDATE.into(),
        operation: op,
        target: ResourceTarget::Alerter(ID.into()),
        status: UpdateStatus::Complete,
        success,
        commit_hash: PRIVATE[4].into(),
        logs: vec![Log {
            stage: PRIVATE[0].into(),
            stdout: PRIVATE[1].into(),
            stderr: format!("{} {} {}", PRIVATE[0], PRIVATE[1], PRIVATE[2]),
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let data = Arc::new(Mutex::new(Data {
            entries: BTreeMap::from([(ID.into(), resource("owned-alerter", "Ntfy"))]),
            role: json!({"_id":{"$oid":"000000000000000000000077"},"username":"fixture-admin","enabled":true,"admin":true,"super_admin":false,"config":{"type":"Local","data":{"password":PRIVATE[3]}}}),
            requests: vec![],
            bad_credentials: false,
            read_status: None,
            server_read_status: None,
            server: crate::support::inventory::Family::Server.detail(),
            user_status: None,
            write_reply: Reply::Valid,
            test_reply: Reply::Valid,
            get_count: 0,
            drift: None,
            post_write_drift: None,
            post_write_read_status: None,
            update: update(Operation::TestAlerter, true),
            history_status: None,
            history_override: None,
            read_override: None,
            last_written: None,
        }));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/write", post(write))
            .route("/komodo/execute", post(execute))
            .route("/komodo/user", get(user))
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
    pub fn mutations(&self, path: &str) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
}
fn failure(s: u16) -> Response {
    (
        StatusCode::from_u16(s).unwrap(),
        format!("{} {KEY} {SECRET}", PRIVATE[0]),
    )
        .into_response()
}
fn record(d: &mut Data, h: &HeaderMap, path: &'static str, op: &str, p: Value) -> bool {
    let ok = h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(KEY)
        && h.get("x-api-secret").and_then(|v| v.to_str().ok()) == Some(SECRET);
    d.bad_credentials |= !ok;
    d.requests.push(Request {
        path,
        operation: op.into(),
        params: p,
    });
    ok
}
async fn fallback(State(data): State<Arc<Mutex<Data>>>) -> Response {
    data.lock().unwrap().requests.push(Request {
        path: "unexpected",
        operation: "unexpected".into(),
        params: Value::Null,
    });
    failure(404)
}
async fn user(State(data): State<Arc<Mutex<Data>>>, h: HeaderMap) -> Response {
    let mut d = data.lock().unwrap();
    if !record(&mut d, &h, "user", "CurrentCaller", json!({})) {
        return failure(401);
    }
    if let Some(s) = d.user_status {
        return failure(s);
    }
    Json(d.role.clone()).into_response()
}
fn listed(v: &Value) -> Value {
    json!({"id":v["_id"]["$oid"],"name":v["name"],"template":v["template"],"type":"Alerter","tags":[],"updated_at":0,"info":{"enabled":v["config"]["enabled"],"endpoint_type":v["config"]["endpoint"]["type"]}})
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    let op = b["type"].as_str().unwrap();
    let p = &b["params"];
    if !record(&mut d, &h, "read", op, p.clone()) {
        return failure(401);
    }
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if let Some(s) = d.read_status {
        return failure(s);
    }
    if matches!(op, "ListServers" | "GetServer")
        && let Some(status) = d.server_read_status
    {
        return failure(status);
    }
    match op {
        "ListServers"=>Json(json!([crate::support::inventory::Family::Server.summary()])).into_response(),
        "GetServer"=>Json(d.server.clone()).into_response(),
        "ExportResourcesToToml"=>Json(json!({"toml":format!("[[alerter]]\nname = \"owned-alerter\"\n[alerter.config]\nenabled = true\n[alerter.config.endpoint]\ntype = \"Ntfy\"\n[alerter.config.endpoint.params]\nurl = \"https://example.test/{}\"\nemail = \"{}\"\n",PRIVATE[1],PRIVATE[2])})).into_response(),
        "ListAlerters" => {
            assert!(p["limit"].as_u64().unwrap() > 0);
            let page = p["page"].as_u64().unwrap() as usize;
            let limit = p["limit"].as_u64().unwrap() as usize;
            let mut entries = d
                .entries
                .values()
                .filter(|v| {
                    p["query"]["names"].as_array().is_none_or(|names| {
                        names.is_empty()
                            || names
                                .iter()
                                .any(|n| n == &v["name"] || n == &v["_id"]["$oid"])
                    })
                })
                .map(listed)
                .collect::<Vec<_>>();
            entries.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
            Json(d.read_override.clone().unwrap_or_else(|| {
                json!(
                    entries
                        .into_iter()
                        .skip(page * limit)
                        .take(limit)
                        .collect::<Vec<_>>()
                )
            }))
            .into_response()
        }
        "GetAlerter" => {
            d.get_count += 1;
            let n = d.get_count;
            if let Some((at, v)) = d.drift.clone()
                && at == n
            {
                d.entries.insert(ID.into(), v);
            }
            match d.entries.get(p["alerter"].as_str().unwrap()) {
                Some(v) => {
                    Json(d.read_override.clone().unwrap_or_else(|| v.clone())).into_response()
                }
                None => failure(404),
            }
        }
        "GetUpdate" => {
            assert_eq!(p["id"], UPDATE);
            if let Some(s) = d.history_status {
                return failure(s);
            }
            Json(
                d.history_override
                    .clone()
                    .unwrap_or_else(|| d.update.clone()),
            )
            .into_response()
        }
        "ListUpdates" => {
            if let Some(s) = d.history_status {
                return failure(s);
            }
            Json(json!({"updates":[{"id":UPDATE,"operation":d.update["operation"],"target":d.update["target"],"start_ts":0,"success":d.update["success"],"operator":"fixture-admin","username":"fixture-admin","status":"Complete","other_data":""}],"next_page":null})).into_response()
        }
        _ => failure(404),
    }
}
fn reply(kind: Reply, mut ack: Value, write: bool) -> Response {
    match kind {
        Reply::Valid => Json(ack).into_response(),
        Reply::Denied => failure(403),
        Reply::Lost => failure(502),
        Reply::BeforeError => failure(500),
        Reply::Malformed => (StatusCode::OK, "{").into_response(),
        Reply::WrongTarget => {
            ack["target"] = json!({"type":"Alerter","id":"000000000000000000000082"});
            Json(ack).into_response()
        }
        Reply::WrongId => {
            if write {
                ack["_id"]["$oid"] = json!("000000000000000000000082");
            } else {
                ack["_id"] = json!({"$oid":"invalid"});
            }
            Json(ack).into_response()
        }
        Reply::MissingEcho => {
            ack["config"].as_object_mut().unwrap().remove("enabled");
            Json(ack).into_response()
        }
        Reply::Oversized => (StatusCode::OK, "x".repeat(2 * 1024 * 1024 + 1)).into_response(),
        Reply::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", "/komodo/execute")],
            PRIVATE[0],
        )
            .into_response(),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    let op = b["type"].as_str().unwrap();
    let p = &b["params"];
    if !record(&mut d, &h, "write", op, p.clone()) {
        return failure(401);
    }
    let kind = d.write_reply;
    if matches!(kind, Reply::Denied | Reply::BeforeError) {
        return reply(kind, Value::Null, true);
    }
    let id = if op == "CreateAlerter" {
        let mut v = resource(p["name"].as_str().unwrap(), "Custom");
        v["_id"]["$oid"] = json!("000000000000000000000082");
        v["config"] = serde_json::to_value(AlerterConfig::default()).unwrap();
        d.entries.insert("000000000000000000000082".into(), v);
        "000000000000000000000082"
    } else {
        assert_eq!(op, "UpdateAlerter");
        p["id"].as_str().unwrap()
    };
    let v = d.entries.get_mut(id).unwrap();
    for (k, value) in p["config"].as_object().unwrap() {
        v["config"][k] = value.clone();
    }
    let ack = v.clone();
    d.last_written = Some(id.into());
    if let Some(v) = d.post_write_drift.clone() {
        d.entries.insert(id.into(), v);
    }
    d.read_status = d.post_write_read_status;
    reply(kind, ack, true)
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    let op = b["type"].as_str().unwrap();
    let p = &b["params"];
    if !record(&mut d, &h, "execute", op, p.clone()) {
        return failure(401);
    }
    assert_eq!(op, "TestAlerter");
    assert_eq!(p, &json!({"alerter":ID}));
    reply(d.test_reply, d.update.clone(), false)
}
