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
    api::write::{
        CreateDeployment, CreateServer, CreateStack, UpdateDeployment, UpdateServer, UpdateStack,
    },
    entities::{
        build::Build,
        deployment::Deployment,
        repo::{Repo, RepoListItemInfo},
        server::{Server, ServerInfo},
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
    WrongName,
    Template,
    MissingConfig,
    MissingRequested,
    #[allow(
        dead_code,
        reason = "Server-specific response used by its integration binary"
    )]
    WrongTrust,
    #[allow(
        dead_code,
        reason = "Server-specific response used by its integration binary"
    )]
    MissingTrust,
    Redirect,
    Lost,
    WriteDenied,
    AttachDenied,
    Delayed,
}
pub const SERVER: &str = "000000000000000000000001";
pub const SWARM: &str = "000000000000000000000009";
pub const REPO: &str = "00000000000000000000000a";
pub const BUILD: &str = "00000000000000000000000b";
pub struct Data {
    pub stack: Value,
    pub created: BTreeMap<String, Value>,
    pub deployment: Value,
    pub created_deployments: BTreeMap<String, Value>,
    pub observed_deployment_image: String,
    pub build: Value,
    pub server: Value,
    pub created_servers: BTreeMap<String, Value>,
    pub post_write_read_status: Option<u16>,
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
        let mut deployment = Family::Deployment.detail();
        deployment["config"]["image"] = json!({"type":"Image","params":{"image":"busybox:1.37.0"}});
        deployment["config"]["ports"] = json!("9000:9000");
        deployment["config"]["environment"] =
            json!("TOKEN=[[DEPLOY_SECRET_REFERENCE]]\nPRIVATE=deployment-private-environment");
        deployment["config"]["image_registry_account"] =
            json!("private-registry-account-reference");
        let deployment =
            serde_json::to_value(serde_json::from_value::<Deployment>(deployment).unwrap())
                .unwrap();
        let build = serde_json::to_value(Build {
            id: BUILD.into(),
            name: "fixture-build".into(),
            ..Default::default()
        })
        .unwrap();
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
            deployment,
            created_deployments: BTreeMap::new(),
            observed_deployment_image: "busybox:1.36.1".into(),
            build,
            server: serde_json::to_value(
                serde_json::from_value::<Server>(Family::Server.detail()).unwrap(),
            )
            .unwrap(),
            created_servers: BTreeMap::new(),
            post_write_read_status: None,
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
    let read_status = data.read_status.or_else(|| {
        data.requests
            .iter()
            .any(|r| r.path == "write")
            .then_some(data.post_write_read_status)
            .flatten()
    });
    if let Some(status) = read_status {
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
        "ListServers" | "GetServer" => std::iter::once(data.server.clone())
            .chain(data.created_servers.values().cloned())
            .collect(),
        "ListDeployments" | "GetDeployment" => std::iter::once(data.deployment.clone())
            .chain(data.created_deployments.values().cloned())
            .collect(),
        "ListBuilds" | "GetBuild" => vec![data.build.clone()],
        "ListSwarms" | "GetSwarm" => vec![data.swarm.clone()],
        "ListRepos" | "GetRepo" => vec![data.repo.clone()],
        _ => panic!("unexpected read {operation}"),
    };
    if operation.starts_with("Get") {
        let key = match operation {
            "GetStack" => "stack",
            "GetServer" => "server",
            "GetSwarm" => "swarm",
            "GetDeployment" => "deployment",
            "GetBuild" => "build",
            _ => "repo",
        };
        if let Some(item) = raw
            .into_iter()
            .find(|v| resource_id(v) == &value["params"][key])
        {
            return Json(item).into_response();
        }
        let kind = operation.strip_prefix("Get").unwrap();
        return (StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("Did not find any {kind} matching {}",value["params"][key].as_str().unwrap())}))).into_response();
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
            "ListDeployments"=>Family::Deployment.summary(),
            "ListBuilds"=>json!({"type":"Build","tags":[],"template":false,"info":{"state":"Ok","last_built_at":0,"version":{"major":1,"minor":2,"patch":3},"builder_id":"","files_on_host":false,"dockerfile_contents":true,"linked_repo":"","linked_repo_name":"","git_provider":"github.com","repo":"","branch":"main","repo_link":"","built_hash":null,"latest_hash":null,"image_registry_domain":null}}),
            "ListSwarms"=>json!({"type":"Swarm","tags":[],"template":false,"info":SwarmListItemInfo::default()}),
            _=>json!({"type":"Repo","tags":[],"template":false,"info":RepoListItemInfo::default()}),
        };
        result["id"]=resource_id(v).clone();result["name"]=v["name"].clone();
        if operation=="ListServers" {
            result["info"]["region"]=v["config"]["region"].clone();
            if data.created_servers.contains_key(resource_id(v).as_str().unwrap()) {result["info"]["state"]=json!(if v["config"]["enabled"]==false {"Disabled"} else {"NotOk"});}
        }
        if operation=="ListStacks" {
            result["info"]["server_id"]=v["config"].get("server_id").cloned().unwrap_or(json!(""));
            result["info"]["swarm_id"]=v["config"].get("swarm_id").cloned().unwrap_or(json!(""));
            if data.created.contains_key(resource_id(v).as_str().unwrap()) {result["info"]["state"]=json!("down");result["info"]["services"]=json!([]);}
        }
        if operation=="ListDeployments" {
            result["info"]["server_id"]=v["config"]["server_id"].clone();
            result["info"]["swarm_id"]=v["config"]["swarm_id"].clone();
            if data.created_deployments.contains_key(resource_id(v).as_str().unwrap()) {
                result["info"]["state"]=json!("not_deployed");
                result["info"]["image"]=v["config"]["image"]["params"].get("image").cloned().unwrap_or(json!("fixture-build:latest"));
            } else { result["info"]["image"]=json!(data.observed_deployment_image); }
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
            "UpdateDeployment" => {
                let typed: UpdateDeployment =
                    serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let target = if typed.id == resource_id(&data.deployment).as_str().unwrap() {
                    &mut data.deployment
                } else {
                    data.created_deployments.get_mut(&typed.id).unwrap()
                };
                if may_save {
                    for (key, v) in value["params"]["config"].as_object().unwrap() {
                        target["config"][key] = v.clone();
                    }
                }
                target.clone()
            }
            "CreateDeployment" => {
                let typed: CreateDeployment =
                    serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let id = format!("{:024x}", 0x200 + data.created_deployments.len());
                let saved = serde_json::to_value(Deployment {
                    id: id.clone(),
                    name: typed.name,
                    config: typed.config.into(),
                    ..Default::default()
                })
                .unwrap();
                if may_save {
                    data.created_deployments.insert(id, saved.clone());
                }
                saved
            }
            "UpdateServer" => {
                let typed: UpdateServer = serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let target = if typed.id == resource_id(&data.server).as_str().unwrap() {
                    &mut data.server
                } else {
                    data.created_servers.get_mut(&typed.id).unwrap()
                };
                if may_save {
                    for (key, v) in value["params"]["config"].as_object().unwrap() {
                        target["config"][key] = v.clone();
                    }
                }
                target.clone()
            }
            "CreateServer" => {
                let typed: CreateServer = serde_json::from_value(value["params"].clone()).unwrap();
                assert_eq!(serde_json::to_value(&typed).unwrap(), value["params"]);
                let id = format!("{:024x}", 0x300 + data.created_servers.len());
                let saved = serde_json::to_value(Server {
                    id: id.clone(),
                    name: typed.name,
                    config: typed.config.into(),
                    info: ServerInfo {
                        public_key: typed.public_key.unwrap_or_default(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap();
                if may_save {
                    data.created_servers.insert(id, saved.clone());
                }
                saved
            }
            other => panic!("unexpected write {other}"),
        };
        (data.reply, saved)
    };
    match reply {
        WriteReply::Valid => Json(saved).into_response(),
        WriteReply::WriteDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("User does not have required permissions on this {}. Must have at least Write permissions",if value["type"].as_str().unwrap().ends_with("Deployment"){"Deployment"}else if value["type"].as_str().unwrap().ends_with("Server"){"Server"}else{"Stack"}),"trace":[format!("{KEY} {SECRET}")]}))).into_response(),
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
        WriteReply::WrongName => {
            let mut s=saved;s["name"]=json!("other-resource");Json(s).into_response()
        }
        WriteReply::Template => {
            let mut s=saved;s["template"]=json!(true);Json(s).into_response()
        }
        WriteReply::MissingConfig => {
            let mut s=saved;s.as_object_mut().unwrap().remove("config");Json(s).into_response()
        }
        WriteReply::MissingRequested => {
            let mut s=saved;
            let key=value["params"]["config"].as_object().unwrap().keys().next().unwrap();
            s["config"].as_object_mut().unwrap().remove(key);Json(s).into_response()
        }
        WriteReply::WrongTrust => {let mut s=saved;s["info"]["public_key"]=json!("unexpected-accepted-public-key");Json(s).into_response()}
        WriteReply::MissingTrust => {let mut s=saved;s["info"].as_object_mut().unwrap().remove("public_key");Json(s).into_response()}
        WriteReply::Redirect => {
            (StatusCode::TEMPORARY_REDIRECT, [("location", "/write")], "").into_response()
        }
        // Hyper closes this owned fixture connection without returning an ACK.
        WriteReply::Lost => panic!("fixture intentionally dropped an accepted response"),
    }
}
