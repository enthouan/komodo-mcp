//! Owned mutable Core wire fixture; no external sources or execution are accessed.
#![allow(dead_code)] // Shared by definition and pending-plan acceptance targets.
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
    routing::post,
};
use komodo_client::entities::{
    repo::{Repo, RepoConfig, RepoListItemInfo},
    sync::{ResourceSync, ResourceSyncConfig},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

pub const SYNC: &str = "00000000000000000000000b";
pub const REPO: &str = "00000000000000000000000a";
pub const PRIVATE: [&str; 5] = [
    "sync-private-account",
    "sync-webhook-secret",
    "inline-private-value",
    "pending-private-error",
    "linked-repo-secret",
];
#[derive(Clone, Copy, Default)]
pub enum Fault {
    #[default]
    None,
    Denied,
    Lost,
    MissingRequested,
    ChangeUnrelated,
    ReadBackDenied,
    MissingRaw,
}
pub struct Data {
    pub syncs: BTreeMap<String, Value>,
    pub repo: Value,
    pub requests: Vec<Request>,
    pub fault: Fault,
    pub deny_create: bool,
    pub deny_attach: bool,
    pub deny_read: bool,
    pub bad_credentials: bool,
    pub server: Value,
    pub variables: Value,
    pub deny_metadata: bool,
    pub deny_refresh: bool,
    pub refresh_reply: Option<Value>,
    pub version: String,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
impl Core {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let mut sync = serde_json::to_value(ResourceSync {
            id: SYNC.into(),
            name: "fixture-sync".into(),
            config: ResourceSyncConfig {
                git_account: PRIVATE[0].into(),
                webhook_secret: PRIVATE[1].into(),
                webhook_enabled: false,
                file_contents: format!(
                    "[[variable]]\nname='owned-variable'\nvalue='{}'\n",
                    PRIVATE[2]
                ),
                resource_path: vec!["resources.toml".into()],
                match_tags: vec!["owned".into()],
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        sync["info"]["pending_error"] = json!(PRIVATE[3]);
        sync["info"]["remote_contents"] =
            json!([{"resource_path":"","path":"private-file","contents":PRIVATE[2]}]);
        let repo = serde_json::to_value(Repo {
            id: REPO.into(),
            name: "source-repo".into(),
            config: RepoConfig {
                repo: "fixture/source".into(),
                git_account: PRIVATE[4].into(),
                webhook_secret: PRIVATE[4].into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let data = Arc::new(Mutex::new(Data {
            syncs: BTreeMap::from([(SYNC.into(), sync)]),
            repo,
            requests: vec![],
            fault: Fault::None,
            deny_create: false,
            deny_attach: false,
            deny_read: false,
            bad_credentials: false,
            server: Family::Server.detail(),
            variables: json!([]),
            deny_metadata: false,
            deny_refresh: false,
            refresh_reply: None,
            version: "2.3.2".into(),
        }));
        let app = Router::new()
            .route("/read", post(read))
            .route("/write", post(write))
            .route("/execute", post(execute))
            .with_state(data.clone());
        let task = Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Self {
            address,
            data,
            _task: task,
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
    pub fn saved(&self, id: &str) -> Value {
        self.data.lock().unwrap().syncs[id].clone()
    }
}
fn record(d: &mut Data, h: &HeaderMap, path: &'static str, v: &Value) {
    d.bad_credentials |= h.get("x-api-key").and_then(|v| v.to_str().ok()) != Some(KEY)
        || h.get("x-api-secret").and_then(|v| v.to_str().ok()) != Some(SECRET);
    d.requests.push(Request {
        path,
        operation: v["type"].as_str().unwrap().into(),
        params: v["params"].clone(),
    });
}
fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":format!("denied {KEY} {SECRET}")})),
    )
        .into_response()
}
fn filtered(mut items: Vec<Value>, p: &Value) -> Vec<Value> {
    let names = p["query"]["names"].as_array().unwrap();
    let terms = p["query"]["terms"].as_array().unwrap();
    items.retain(|v| {
        (names.is_empty()
            || names
                .iter()
                .any(|n| n == &v["id"] || (n.as_str().unwrap().len() != 24 && n == &v["name"])))
            && terms
                .iter()
                .all(|t| v["name"].as_str().unwrap().contains(t.as_str().unwrap()))
    });
    assert_eq!(p["sort_desc"], false);
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let limit = p["limit"].as_u64().unwrap() as usize;
    assert!((1..=100).contains(&limit));
    items
        .into_iter()
        .skip(p["page"].as_u64().unwrap() as usize * limit)
        .take(limit)
        .collect()
}
fn item(v: &Value) -> Value {
    let c = &v["config"];
    json!({"id":v["_id"]["$oid"],"name":v["name"],"type":"ResourceSync","template":v["template"],"tags":[],"info":{
        "last_sync_ts":0,"files_on_host":c["files_on_host"],"file_contents":!c["file_contents"].as_str().unwrap().is_empty(),"managed":c["managed"],"resource_path":c["resource_path"],"linked_repo":c["linked_repo"],"linked_repo_name":"source-repo","git_provider":c["git_provider"],"repo":c["repo"],"branch":c["branch"],"repo_link":"https://private.invalid/source","last_sync_hash":null,"last_sync_message":PRIVATE[3],"state":"Ok"
    }})
}
fn repo_item(v: &Value) -> Value {
    let c = &v["config"];
    let info = RepoListItemInfo {
        repo: c["repo"].as_str().unwrap().into(),
        branch: c["branch"].as_str().unwrap().into(),
        git_provider: c["git_provider"].as_str().unwrap().into(),
        ..Default::default()
    };
    json!({"id":v["_id"]["$oid"],"name":v["name"],"type":"Repo","template":v["template"],"tags":[],"info":info})
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "read", &v);
    let p = &v["params"];
    match v["type"].as_str().unwrap() {
        "GetVersion" => Json(json!({"version":d.version})).into_response(),
        "ListResourceSyncs" => {
            Json(filtered(d.syncs.values().map(item).collect(), p)).into_response()
        }
        "GetResourceSync" => {
            if d.deny_read
                || (matches!(d.fault, Fault::ReadBackDenied)
                    && d.requests.iter().any(|r| r.path == "write"))
            {
                return denied();
            }
            let mut saved = d.syncs[p["sync"].as_str().unwrap()].clone();
            if matches!(d.fault, Fault::MissingRaw) && d.requests.iter().any(|r| r.path == "write")
            {
                saved["config"]
                    .as_object_mut()
                    .unwrap()
                    .remove("webhook_secret");
            }
            Json(saved).into_response()
        }
        "ListRepos" => Json(filtered(vec![repo_item(&d.repo)], p)).into_response(),
        "GetRepo" => Json(d.repo.clone()).into_response(),
        "ListVariables" => {
            if d.deny_metadata {
                return denied();
            }
            Json(d.variables.clone()).into_response()
        }
        "ListServers" => {
            if d.deny_metadata {
                return denied();
            }
            let mut item = Family::Server.summary();
            item["id"] = d.server["_id"]["$oid"].clone();
            item["name"] = d.server["name"].clone();
            item["template"] = d.server["template"].clone();
            Json(filtered(vec![item], p)).into_response()
        }
        "GetServer" => {
            if d.deny_metadata {
                return denied();
            }
            Json(d.server.clone()).into_response()
        }
        other => panic!("unexpected read {other}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "write", &v);
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if op == "RefreshResourceSyncPending" {
        if d.deny_refresh {
            return denied();
        }
        let id = p["sync"].as_str().unwrap();
        if let Some(reply) = d.refresh_reply.clone() {
            d.syncs.insert(id.into(), reply.clone());
            return Json(reply).into_response();
        }
        if matches!(d.fault, Fault::Lost) {
            return (StatusCode::OK, "lost refresh acknowledgement").into_response();
        }
        return Json(d.syncs[id].clone()).into_response();
    }
    let config = p["config"].as_object().unwrap();
    assert!(config.values().all(|v| !v.is_null()));
    if matches!(d.fault, Fault::Denied)
        || (op == "CreateResourceSync" && d.deny_create)
        || (d.deny_attach && config.get("linked_repo").is_some_and(|v| v != ""))
    {
        return denied();
    }
    let mut saved = if op == "CreateResourceSync" {
        serde_json::to_value(ResourceSync {
            id: format!("{:024x}", 256 + d.syncs.len()),
            name: p["name"].as_str().unwrap().into(),
            ..Default::default()
        })
        .unwrap()
    } else {
        assert_eq!(op, "UpdateResourceSync");
        d.syncs[p["id"].as_str().unwrap()].clone()
    };
    saved["config"]
        .as_object_mut()
        .unwrap()
        .extend(config.clone());
    if matches!(d.fault, Fault::ChangeUnrelated) {
        saved["config"]["webhook_secret"] = json!("changed-upstream-secret");
    }
    let id = saved["_id"]["$oid"].as_str().unwrap().to_string();
    d.syncs.insert(id, saved.clone());
    match d.fault {
        Fault::Lost => (StatusCode::OK, "invalid acknowledgement").into_response(),
        Fault::MissingRequested => {
            saved["config"].as_object_mut().unwrap().remove("managed");
            Json(saved).into_response()
        }
        _ => Json(saved).into_response(),
    }
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(&mut data.lock().unwrap(), &headers, "execute", &v);
    panic!("definition editing must not submit any execute request")
}
