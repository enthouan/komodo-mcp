//! Pinned wire contract with raw admin/secret bodies and captured mutation counts.
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
use komodo_client::{
    api::write::{CreateVariable, DeleteVariable, UpdateVariableDescription, UpdateVariableValue},
    entities::{
        Operation, ResourceTarget,
        update::{Log, Update, UpdateStatus},
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

pub const PRIVATE: [&str; 4] = [
    "external-variable-secret-9d2a",
    "private-admin-password-hash-8fa",
    "private-unknown-auth-token",
    "0123aabbccddeeff00112233445566778899aabb",
];
pub const UPDATE: &str = "000000000000000000000092";
#[derive(Clone, Copy, Default)]
pub enum Reply {
    #[default]
    Valid,
    Denied,
    Lost,
    Malformed,
    MissingClassification,
    WrongEntry,
    Oversized,
    Redirect,
    BeforeError,
}
pub struct Data {
    pub entries: BTreeMap<String, Value>,
    pub role: Value,
    pub requests: Vec<Request>,
    pub reply: Reply,
    pub read_status: Option<u16>,
    pub role_status: Option<u16>,
    pub read_override: Option<Value>,
    pub post_write_read_status: Option<u16>,
    pub list_reads: usize,
    pub drift_on_list: Option<(usize, Value)>,
    pub replace_after_write: Option<Value>,
    pub update: Value,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn variable(name: &str, value: &str, secret: bool) -> Value {
    json!({"name":name,"value":value,"description":"metadata","is_secret":secret})
}
pub fn history(op: Operation) -> Value {
    serde_json::to_value(Update {
        id: UPDATE.into(),
        operation: op,
        target: ResourceTarget::System("system".into()),
        status: UpdateStatus::Complete,
        success: false,
        commit_hash: PRIVATE[3].into(),
        logs: vec![Log {
            stage: PRIVATE[0].into(),
            stderr: PRIVATE[0].into(),
            stdout: PRIVATE[0].into(),
            command: PRIVATE[0].into(),
            success: false,
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
        let mut ordinary = variable("APP_MODE", "blue", false);
        ordinary["extra_private"] = json!(PRIVATE[2]);
        let entries = BTreeMap::from([
            ("APP_MODE".into(), ordinary),
            (
                "API_CREDENTIAL".into(),
                variable("API_CREDENTIAL", PRIVATE[0], true),
            ),
        ]);
        let data = Arc::new(Mutex::new(Data {
            entries,
            role: json!({"_id":{"$oid":"000000000000000000000077"},"username":"variable-admin","enabled":true,"admin":true,"super_admin":false,"config":{"type":"Local","data":{"password":PRIVATE[1]}},"linked_logins":{"private":PRIVATE[2]},"totp":{"secret":PRIVATE[2]}}),
            requests: vec![],
            reply: Reply::Valid,
            read_status: None,
            role_status: None,
            read_override: None,
            post_write_read_status: None,
            list_reads: 0,
            drift_on_list: None,
            replace_after_write: None,
            update: history(Operation::CreateVariable),
            bad_credentials: false,
        }));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/write", post(write))
            .route("/komodo/user", get(user))
            .with_state(data.clone());
        Self {
            address,
            data,
            _task: Task(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            })),
        }
    }
    pub fn writes(&self) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == "write")
            .cloned()
            .collect()
    }
}
fn authenticated(d: &mut Data, h: &HeaderMap) -> bool {
    let valid = h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(KEY)
        && h.get("x-api-secret").and_then(|v| v.to_str().ok()) == Some(SECRET);
    d.bad_credentials |= !valid;
    valid
}
fn failure(status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        format!("{} {KEY} {SECRET}", PRIVATE[0]),
    )
        .into_response()
}
async fn user(State(data): State<Arc<Mutex<Data>>>, headers: HeaderMap) -> Response {
    let mut d = data.lock().unwrap();
    if !authenticated(&mut d, &headers) {
        return failure(401);
    }
    d.requests.push(Request {
        path: "user",
        operation: "CurrentCaller".into(),
        params: json!({}),
    });
    if let Some(status) = d.role_status {
        return failure(status);
    }
    Json(d.role.clone()).into_response()
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    if !authenticated(&mut d, &headers) {
        return failure(401);
    }
    let op = body["type"].as_str().unwrap();
    let p = &body["params"];
    d.requests.push(Request {
        path: "read",
        operation: op.into(),
        params: p.clone(),
    });
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if op == "ListVariables" {
        d.list_reads += 1;
        let n = d.list_reads;
        if let Some((at, v)) = d.drift_on_list.clone()
            && at == n
        {
            d.entries.insert(v["name"].as_str().unwrap().into(), v);
        }
    }
    if let Some(status) = d.read_status {
        return failure(status);
    }
    match op {
        "ListVariables"=>Json(d.read_override.clone().unwrap_or_else(||json!(d.entries.values().collect::<Vec<_>>()))).into_response(),
        "ListSecrets"=>{assert!(p["target"].is_null());Json(json!(["CORE_KEY","SECOND.KEY"])).into_response()},
        "GetUpdate"=>{assert_eq!(p["id"],UPDATE);Json(d.update.clone()).into_response()},
        "ListUpdates"=>Json(json!({"updates":[{"id":UPDATE,"operation":d.update["operation"],"target":d.update["target"],"start_ts":0,"success":false,"operator":"fixture-admin","username":"fixture-admin","status":"Complete","other_data":""}],"next_page":null})).into_response(),
        "ExportResourcesToToml"=>Json(json!({"toml":format!("[[variable]]\nname = \"API_CREDENTIAL\"\nvalue = \"{}\"\nis_secret = true\n\n[[variable]]\nname = \"APP_MODE\"\nvalue = \"ordinary-export-value\"\nis_secret = false\n",PRIVATE[0])})).into_response(),
        _=>panic!("unexpected read operation {op}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    if !authenticated(&mut d, &headers) {
        return failure(401);
    }
    let op = body["type"].as_str().unwrap();
    let p = body["params"].clone();
    d.requests.push(Request {
        path: "write",
        operation: op.into(),
        params: p.clone(),
    });
    if matches!(d.reply, Reply::Denied) {
        return failure(403);
    }
    if matches!(d.reply, Reply::BeforeError) {
        return failure(500);
    }
    let name = p["name"].as_str().unwrap();
    let mut ack = match op {
        "CreateVariable" => {
            let typed: CreateVariable = serde_json::from_value(p.clone()).unwrap();
            assert!(!typed.is_secret);
            assert!(!d.entries.contains_key(name));
            let v = serde_json::to_value(typed).unwrap();
            d.entries.insert(name.into(), v.clone());
            v
        }
        "UpdateVariableValue" => {
            let typed: UpdateVariableValue = serde_json::from_value(p.clone()).unwrap();
            let v = d.entries.get_mut(name).unwrap();
            v["value"] = json!(typed.value);
            v.clone()
        }
        "UpdateVariableDescription" => {
            let typed: UpdateVariableDescription = serde_json::from_value(p.clone()).unwrap();
            let v = d.entries.get_mut(name).unwrap();
            v["description"] = json!(typed.description);
            v.clone()
        }
        "DeleteVariable" => {
            let _: DeleteVariable = serde_json::from_value(p.clone()).unwrap();
            d.entries.remove(name).unwrap()
        }
        _ => panic!("unexpected variable write {op}"),
    };
    if let Some(value) = d.replace_after_write.clone() {
        d.entries
            .insert(value["name"].as_str().unwrap().into(), value);
    }
    d.read_status = d.post_write_read_status;
    match d.reply {
        Reply::Valid => Json(ack).into_response(),
        Reply::Lost => failure(502),
        Reply::Malformed => (StatusCode::OK, "{").into_response(),
        Reply::MissingClassification => {
            ack.as_object_mut().unwrap().remove("is_secret");
            Json(ack).into_response()
        }
        Reply::WrongEntry => {
            ack["name"] = json!("OTHER_NAME");
            Json(ack).into_response()
        }
        Reply::Oversized => (StatusCode::OK, "x".repeat(2 * 1024 * 1024 + 1)).into_response(),
        Reply::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", "/komodo/write")],
            PRIVATE[0],
        )
            .into_response(),
        Reply::Denied | Reply::BeforeError => unreachable!(),
    }
}
