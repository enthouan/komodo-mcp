//! Mutable pinned-wire fixture; captures the actual authenticated HTTP bodies.
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
use komodo_client::{
    api::write::{CreateStack, UpdateStack},
    entities::{
        repo::{Repo, RepoListItemInfo},
        stack::Stack,
        swarm::{Swarm, SwarmListItemInfo},
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

#[derive(Clone, Copy, Default)]
pub enum WriteReply {
    #[default]
    Valid,
    Malformed,
    Oversized,
    Denied,
    WrongTarget,
    Redirect,
    Lost,
    WriteDenied,
    AttachDenied,
    Delayed,
}
pub const SERVER: &str = "000000000000000000000001";
pub const SWARM: &str = "000000000000000000000009";
pub const REPO: &str = "00000000000000000000000a";
pub struct Data {
    pub stack: Value,
    pub created: BTreeMap<String, Value>,
    pub server: Value,
    pub swarm: Value,
    pub repo: Value,
    pub requests: Vec<Request>,
    pub reply: WriteReply,
    pub read_status: Option<u16>,
    pub bad_credentials: bool,
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
        let mut stack = Family::Stack.detail();
        stack["config"]["auto_pull"] = json!(false);
        stack["config"]["ignore_services"] = json!(["init"]);
        stack["config"]["environment"] =
            json!("PASSWORD=[[SECRET_REFERENCE]]\nOTHER=sensitive-environment-marker");
        let swarm = serde_json::to_value(Swarm {
            id: SWARM.into(),
            name: "fixture-swarm".into(),
            ..Default::default()
        })
        .unwrap();
        let repo = serde_json::to_value(Repo {
            id: REPO.into(),
            name: "fixture-repo".into(),
            ..Default::default()
        })
        .unwrap();
        let data = Arc::new(Mutex::new(Data {
            stack,
            created: BTreeMap::new(),
            server: Family::Server.detail(),
            swarm,
            repo,
            requests: vec![],
            reply: WriteReply::Valid,
            read_status: None,
            bad_credentials: false,
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
fn record(data: &mut Data, headers: &HeaderMap, path: &'static str, value: &Value) {
    data.bad_credentials |= headers.get("x-api-key").and_then(|s| s.to_str().ok()) != Some(KEY)
        || headers.get("x-api-secret").and_then(|s| s.to_str().ok()) != Some(SECRET);
    data.requests.push(Request {
        path,
        operation: value["type"].as_str().unwrap().into(),
        params: value["params"].clone(),
    });
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    let mut data = data.lock().unwrap();
    record(&mut data, &headers, "read", &value);
    if let Some(status) = data.read_status {
        return (
            StatusCode::from_u16(status).unwrap(),
            format!("{KEY} {SECRET} sensitive-environment-marker"),
        )
            .into_response();
    }
    let operation = value["type"].as_str().unwrap();
    if operation == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    let raw = match operation {
        "ListStacks" | "GetStack" => std::iter::once(data.stack.clone())
            .chain(data.created.values().cloned())
            .collect::<Vec<_>>(),
        "ListServers" | "GetServer" => vec![data.server.clone()],
        "ListSwarms" | "GetSwarm" => vec![data.swarm.clone()],
        "ListRepos" | "GetRepo" => vec![data.repo.clone()],
        _ => panic!("unexpected read {operation}"),
    };
    if operation.starts_with("Get") {
        let key = match operation {
            "GetStack" => "stack",
            "GetServer" => "server",
            "GetSwarm" => "swarm",
            _ => "repo",
        };
        if let Some(item) = raw
            .into_iter()
            .find(|v| resource_id(v) == &value["params"][key])
        {
            return Json(item).into_response();
        }
        return (StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("Did not find any Stack matching {}",value["params"][key].as_str().unwrap())}))).into_response();
    }
    let query = &value["params"]["query"];
    let mut items=raw.iter().filter(|v| {
        let names=query["names"].as_array().unwrap();
        (names.is_empty() || names.iter().any(|n|n==&v["name"] || n==resource_id(v)))
            && query["terms"].as_array().unwrap().iter().all(|term|v["name"].as_str().unwrap().contains(term.as_str().unwrap()))
    }).map(|v| {
        let mut result=match operation {
            "ListStacks"=>Family::Stack.summary(),
            "ListServers"=>Family::Server.summary(),
            "ListSwarms"=>json!({"type":"Swarm","tags":[],"template":false,"info":SwarmListItemInfo::default()}),
            _=>json!({"type":"Repo","tags":[],"template":false,"info":RepoListItemInfo::default()}),
        };
        result["id"]=resource_id(v).clone();result["name"]=v["name"].clone();
        if operation=="ListStacks" {
            result["info"]["server_id"]=v["config"].get("server_id").cloned().unwrap_or(json!(""));
            result["info"]["swarm_id"]=v["config"].get("swarm_id").cloned().unwrap_or(json!(""));
            if data.created.contains_key(resource_id(v).as_str().unwrap()) {result["info"]["state"]=json!("down");result["info"]["services"]=json!([]);}
        }
        result
    }).collect::<Vec<_>>();
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let limit = value["params"]["limit"].as_u64().unwrap() as usize;
    let offset = value["params"]["page"].as_u64().unwrap() as usize * limit;
    Json(
        items
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>(),
    )
    .into_response()
}
fn resource_id(value: &Value) -> &Value {
    value.get("id").unwrap_or(&value["_id"]["$oid"])
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    record(&mut data.lock().unwrap(), &headers, "execute", &value);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected fixture execution",
    )
        .into_response()
}

async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    let (reply, saved) = {
        let mut data = data.lock().unwrap();
        record(&mut data, &headers, "write", &value);
        let may_save = !matches!(
            data.reply,
            WriteReply::Denied
                | WriteReply::WriteDenied
                | WriteReply::AttachDenied
                | WriteReply::Redirect
        );
        let saved = match value["type"].as_str().unwrap() {
            "UpdateStack" => {
                let typed: UpdateStack = serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let target = if typed.id == resource_id(&data.stack).as_str().unwrap() {
                    &mut data.stack
                } else {
                    data.created.get_mut(&typed.id).unwrap()
                };
                if may_save {
                    for (key, v) in value["params"]["config"].as_object().unwrap() {
                        target["config"][key] = v.clone();
                    }
                }
                target.clone()
            }
            "CreateStack" => {
                let typed: CreateStack = serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let id = format!("{:024x}", 0x100 + data.created.len());
                let saved = serde_json::to_value(Stack {
                    id: id.clone(),
                    name: typed.name,
                    config: typed.config.into(),
                    ..Default::default()
                })
                .unwrap();
                if may_save {
                    data.created.insert(id, saved.clone());
                }
                saved
            }
            other => panic!("unexpected write {other}"),
        };
        (data.reply, saved)
    };
    match reply {
        WriteReply::Valid => Json(saved).into_response(),
        WriteReply::WriteDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"User does not have required permissions on this Stack. Must have at least Write permissions","trace":[format!("{KEY} {SECRET}")]}))).into_response(),
        WriteReply::AttachDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"User does not have required permissions on this Server. Must have at least Read permissions, as well as these specific permissions: [Attach]","trace":[format!("{KEY} {SECRET}")]}))).into_response(),
        WriteReply::Delayed => {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            Json(saved).into_response()
        }
        WriteReply::Denied => (
            StatusCode::FORBIDDEN,
            format!("{KEY} {SECRET} sensitive-environment-marker"),
        )
            .into_response(),
        WriteReply::Malformed => {
            (StatusCode::OK, format!("{{invalid {KEY} {SECRET}")).into_response()
        }
        WriteReply::Oversized => (StatusCode::OK, "x".repeat(2 * 1024 * 1024 + 1)).into_response(),
        WriteReply::WrongTarget => {
            let mut s = saved;
            s["_id"] = json!({"$oid":"000000000000000000000099"});
            Json(s).into_response()
        }
        WriteReply::Redirect => {
            (StatusCode::TEMPORARY_REDIRECT, [("location", "/write")], "").into_response()
        }
        // Hyper closes this owned fixture connection without returning an ACK.
        WriteReply::Lost => panic!("fixture intentionally dropped an accepted response"),
    }
}
