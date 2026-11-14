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
        CreateDeployment, CreateServer, CreateStack, DeleteDeployment, DeleteServer, DeleteStack,
        UpdateDeployment, UpdateServer, UpdateStack,
    },
    entities::{
        Operation, ResourceTarget,
        alerter::Alerter,
        build::Build,
        builder::Builder,
        deployment::Deployment,
        repo::{Repo, RepoListItemInfo},
        server::{Server, ServerInfo},
        stack::Stack,
        swarm::{Swarm, SwarmListItemInfo},
        update::{Log, Update, UpdateStatus},
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

#[derive(Clone, Copy, Default)]
#[allow(
    dead_code,
    reason = "Shared reply fixture; each integration binary selects its applicable failure shapes"
)]
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
    #[allow(dead_code, reason = "Tag-specific required acknowledgement fields")]
    MissingTagOwner,
    #[allow(dead_code, reason = "Tag-specific required acknowledgement fields")]
    WrongTagOwner,
    #[allow(dead_code, reason = "Tag-specific required acknowledgement fields")]
    MissingTagColor,
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
    pub deletion_enabled: bool,
    pub tags_enabled: bool,
    pub tags: BTreeMap<String, Value>,
    pub tag_owner_denied: bool,
    pub tag_partial_cleanup_error: bool,
    pub tag_drop_on_assign: bool,
    pub tag_change_metadata: bool,
    pub containers: BTreeMap<String, Value>,
    pub extra_references: BTreeMap<String, Vec<Value>>,
    pub deletion_updates: Vec<Value>,
    pub deletion_history_visible: bool,
    pub cleanup_skip: bool,
    pub cleanup_fail: bool,
    pub stack_failure_success_log: bool,
    pub reference_status: Option<u16>,
    pub runtime_unreachable: bool,
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
            deletion_enabled: false,
            tags_enabled: false,
            tags: BTreeMap::new(),
            tag_owner_denied: false,
            tag_partial_cleanup_error: false,
            tag_drop_on_assign: false,
            tag_change_metadata: false,
            containers: BTreeMap::new(),
            extra_references: BTreeMap::new(),
            deletion_updates: vec![],
            deletion_history_visible: false,
            cleanup_skip: false,
            cleanup_fail: false,
            stack_failure_success_log: false,
            reference_status: None,
            runtime_unreachable: false,
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
    #[allow(
        dead_code,
        reason = "Specialized deletion scenarios share this fixture"
    )]
    pub async fn start_deletion() -> Self {
        let core = Self::start().await;
        {
            let mut d = core.data.lock().unwrap();
            d.deletion_enabled = true;
            d.stack =
                serde_json::to_value(serde_json::from_value::<Stack>(d.stack.clone()).unwrap())
                    .unwrap();
            d.stack["config"]["project_name"] = json!("desired-project");
            d.stack["info"]["deployed_project_name"] = json!("running-project");
            d.deployment["config"]["custom_name"] = json!("desired-deployment");
            d.deployment["info"]["deployed_name"] = json!("running-deployment");
            d.server["config"]["enabled"] = json!(true);
            for (byte, name, project) in [
                ("11", "running-project-web", Some("running-project")),
                ("22", "running-project-orphan", Some("running-project")),
                ("33", "running-deployment", None),
            ] {
                let id = byte.repeat(32);
                let labels = project.map_or(json!({}), |p| json!({"com.docker.compose.project":p}));
                d.containers.insert(id.clone(),json!({"Id":id,"Name":format!("/{name}"),"State":{"Status":"running","Running":true},"Config":{"Labels":labels,"Env":["PASSWORD=deletion-private-container-environment"]}}));
            }
            d.repo["config"]["server_id"] = json!(SERVER);
            let builder:Builder=serde_json::from_value(json!({"_id":{"$oid":"00000000000000000000000c"},"name":"dependent-builder","template":false,"config":{"type":"Server","params":{"server_ids":[SERVER]}}})).unwrap();
            let alerter:Alerter=serde_json::from_value(json!({"_id":{"$oid":"00000000000000000000000d"},"name":"dependent-alerter","template":false,"config":{"enabled":false,"resources":[{"type":"Stack","id":resource_id(&d.stack)},{"type":"Deployment","id":resource_id(&d.deployment)},{"type":"Server","id":SERVER}],"except_resources":[{"type":"Server","id":SERVER}]}})).unwrap();
            d.extra_references.insert(
                "builder".into(),
                vec![serde_json::to_value(builder).unwrap()],
            );
            d.extra_references.insert(
                "alerter".into(),
                vec![serde_json::to_value(alerter).unwrap()],
            );
        }
        core
    }
    #[allow(dead_code, reason = "Specialized tag scenarios share this fixture")]
    pub async fn start_tags() -> Self {
        let core = Self::start_deletion().await;
        {
            let mut d = core.data.lock().unwrap();
            d.tags_enabled = true;
            for (id, name) in [
                ("000000000000000000007001", "keep"),
                ("000000000000000000007002", "old-tag"),
            ] {
                d.tags.insert(
                    id.into(),
                    json!({"_id":{"$oid":id},"name":name,"owner":"fixture-owner","color":"Slate"}),
                );
            }
            d.stack["tags"] = json!(["000000000000000000007001"]);
            for (family, id) in [
                ("procedure", "00000000000000000000000e"),
                ("action", "00000000000000000000000f"),
                ("resource_sync", "000000000000000000000010"),
            ] {
                d.extra_references.insert(family.into(),vec![json!({"_id":{"$oid":id},"name":format!("dependent-{family}"),"template":false,"description":"","tags":[],"config":{},"info":{}})]);
            }
        }
        core
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
    if data.tags_enabled
        && let Some(response) = tag_read(&data, &value)
    {
        return response;
    }
    if data.deletion_enabled
        && let Some(response) = deletion_read(&data, &value)
    {
        return response;
    }
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
    let raw = raw.into_iter().filter(|v| !v.is_null()).collect::<Vec<_>>();
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
            && query["tags"].as_array().unwrap().iter().all(|tag|v["tags"].as_array().is_some_and(|tags|tags.contains(tag)))
    }).map(|v| {
        let mut result=match operation {
            "ListStacks"=>Family::Stack.summary(),
            "ListServers"=>Family::Server.summary(),
            "ListDeployments"=>Family::Deployment.summary(),
            "ListBuilds"=>json!({"type":"Build","tags":[],"template":false,"info":{"state":"Ok","last_built_at":0,"version":{"major":1,"minor":2,"patch":3},"builder_id":"","files_on_host":false,"dockerfile_contents":true,"linked_repo":"","linked_repo_name":"","git_provider":"github.com","repo":"","branch":"main","repo_link":"","built_hash":null,"latest_hash":null,"image_registry_domain":null}}),
            "ListSwarms"=>json!({"type":"Swarm","tags":[],"template":false,"info":SwarmListItemInfo::default()}),
            _=>json!({"type":"Repo","tags":[],"template":false,"info":RepoListItemInfo::default()}),
        };
        result["id"]=resource_id(v).clone();result["name"]=v["name"].clone();if let Some(tags)=v.get("tags"){result["tags"]=tags.clone();}
        if operation=="ListServers" {
            result["info"]["region"]=v["config"]["region"].clone();
            if data.deletion_enabled {result["info"]["state"]=json!(if v["config"]["enabled"]==false {"Disabled"}else if data.runtime_unreachable {"NotOk"}else{"Ok"});}
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
    let (reply, saved, tag_owner_denied, tag_cleanup_error) = {
        let mut data = data.lock().unwrap();
        record(&mut data, &headers, "write", &value);
        let may_save = !matches!(
            data.reply,
            WriteReply::Denied
                | WriteReply::WriteDenied
                | WriteReply::AttachDenied
                | WriteReply::Redirect
        );
        let tag_owner_denied = data.tags_enabled
            && data.tag_owner_denied
            && matches!(value["type"].as_str(), Some("RenameTag" | "DeleteTag"));
        let saved = match value["type"].as_str().unwrap() {
            "CreateTag" | "RenameTag" | "DeleteTag" | "UpdateResourceMeta" => {
                tag_write(&mut data, &value, may_save && !tag_owner_denied)
            }
            "DeleteStack" | "DeleteDeployment" | "DeleteServer" => {
                deletion_write(&mut data, &value, may_save)
            }
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
        (
            data.reply,
            saved,
            tag_owner_denied,
            data.tags_enabled && data.tag_partial_cleanup_error && value["type"] == "DeleteTag",
        )
    };
    if tag_owner_denied {
        return (StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":"user must be tag owner or admin","trace":[format!("{KEY} {SECRET}")]}))).into_response();
    }
    if tag_cleanup_error {
        return (StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":"failed to remove tag from resources","trace":[format!("{KEY} {SECRET} tag-private-cleanup-error")]}))).into_response();
    }
    match reply {
        WriteReply::MissingTagOwner=>{let mut s=saved;s.as_object_mut().unwrap().remove("owner");Json(s).into_response()},
        WriteReply::WrongTagOwner=>{let mut s=saved;s["owner"]=json!("other-owner");Json(s).into_response()},
        WriteReply::MissingTagColor=>{let mut s=saved;s.as_object_mut().unwrap().remove("color");Json(s).into_response()},
        WriteReply::Valid => Json(saved).into_response(),
        WriteReply::WriteDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("User does not have required permissions on this {}. Must have at least Write permissions",if value["type"].as_str().unwrap().ends_with("Deployment"){"Deployment"}else if value["type"].as_str().unwrap().ends_with("Server"){"Server"}else{"Stack"}),"trace":[format!("{KEY} {SECRET}")]}))).into_response(),
        WriteReply::AttachDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"User does not have required permissions on this Server. Must have at least Read permissions, as well as these specific permissions: [Attach]","trace":[format!("{KEY} {SECRET}")]}))).into_response(),
        WriteReply::Delayed => {
            // Deadline tests use a one-second request budget: give setup reads
            // scheduling margin, while this write deterministically exceeds it.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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
            if let Some(config)=value["params"].get("config").and_then(Value::as_object) {
                let key=config.keys().next().unwrap();s["config"].as_object_mut().unwrap().remove(key);
            } else {s.as_object_mut().unwrap().remove("info");}
            Json(s).into_response()
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

fn reference_rows(d: &Data, family: &str) -> Vec<Value> {
    let mut result = match family {
        "stack" => std::iter::once(d.stack.clone())
            .chain(d.created.values().cloned())
            .collect(),
        "deployment" => std::iter::once(d.deployment.clone())
            .chain(d.created_deployments.values().cloned())
            .collect(),
        "repo" => vec![d.repo.clone()],
        "server" => std::iter::once(d.server.clone())
            .chain(d.created_servers.values().cloned())
            .collect(),
        "swarm" => vec![d.swarm.clone()],
        "build" => vec![d.build.clone()],
        _ => vec![],
    };
    result.extend(
        d.extra_references
            .get(family)
            .into_iter()
            .flatten()
            .cloned(),
    );
    result.retain(|v| !v.is_null());
    result.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
    result
}
fn cache_container(v: &Value) -> Value {
    json!({"server_id":SERVER,"name":v["Name"].as_str().unwrap().trim_start_matches('/'),"id":v["Id"],"state":v["State"]["Status"]})
}

fn tag_read(d: &Data, request: &Value) -> Option<Response> {
    let op = request["type"].as_str().unwrap();
    let p = &request["params"];
    if op == "ListTags" {
        fn matches(t: &Value, q: &Value) -> bool {
            if let Some(or) = q["$or"].as_array() {
                return or.iter().any(|q| matches(t, q));
            }
            if let Some(name) = q["name"].as_str() {
                return t["name"] == name;
            }
            let id = q["_id"]
                .get("$oid")
                .and_then(Value::as_str)
                .or_else(|| q["_id"].as_str());
            id.is_some_and(|id| resource_id(t).as_str().unwrap().eq_ignore_ascii_case(id))
        }
        let q = &p["query"];
        assert!(
            q.is_object() && !q.as_object().unwrap().is_empty(),
            "tag lookup must be exact and bounded by generated selectors"
        );
        return Some(
            Json(json!(
                d.tags
                    .values()
                    .filter(|t| matches(t, q))
                    .cloned()
                    .collect::<Vec<_>>()
            ))
            .into_response(),
        );
    }
    let family = match op {
        "ListBuilders" | "GetBuilder" => "builder",
        "ListAlerters" | "GetAlerter" => "alerter",
        "ListProcedures" | "GetProcedure" => "procedure",
        "ListActions" | "GetAction" => "action",
        "ListResourceSyncs" | "GetResourceSync" => "resource_sync",
        _ => return None,
    };
    if let Some(status) = d.reference_status {
        return Some(
            (
                StatusCode::from_u16(status).unwrap(),
                format!("{KEY} {SECRET}"),
            )
                .into_response(),
        );
    }
    let rows = reference_rows(d, family);
    if op.starts_with("Get") {
        let key = if family == "resource_sync" {
            "sync"
        } else {
            family
        };
        return Some(match rows.into_iter().find(|v| resource_id(v) == &p[key]) {
            Some(v) => Json(v).into_response(),
            None => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"fixture reference missing"})),
            )
                .into_response(),
        });
    }
    let limit = p["limit"].as_u64().unwrap() as usize;
    assert!(limit > 0);
    let page = p["page"].as_u64().unwrap() as usize;
    let kind = match family {
        "builder" => "Builder",
        "alerter" => "Alerter",
        "procedure" => "Procedure",
        "action" => "Action",
        _ => "ResourceSync",
    };
    Some(Json(json!(rows.into_iter().filter(|v|p["query"]["tags"].as_array().unwrap().iter().all(|tag|v["tags"].as_array().is_some_and(|tags|tags.contains(tag)))).skip(page*limit).take(limit).map(|v|json!({"id":resource_id(&v),"name":v["name"],"template":v["template"],"tags":v["tags"],"type":kind,"info":{}})).collect::<Vec<_>>())).into_response())
}

fn pull_tag(d: &mut Data, id: &str, partial: bool) {
    for v in std::iter::once(&mut d.stack).chain(d.created.values_mut()) {
        if let Some(tags) = v["tags"].as_array_mut() {
            tags.retain(|t| t != id);
        }
    }
    if partial {
        return;
    }
    for v in std::iter::once(&mut d.server)
        .chain(d.created_servers.values_mut())
        .chain(std::iter::once(&mut d.deployment))
        .chain(d.created_deployments.values_mut())
        .chain(std::iter::once(&mut d.repo))
        .chain(std::iter::once(&mut d.build))
        .chain(d.extra_references.values_mut().flat_map(|v| v.iter_mut()))
    {
        if let Some(tags) = v["tags"].as_array_mut() {
            tags.retain(|t| t != id);
        }
    }
    // Pinned Core's cleanup list omits Swarm.
}
fn tag_write(d: &mut Data, request: &Value, may_save: bool) -> Value {
    use komodo_client::api::write::{CreateTag, DeleteTag, RenameTag, UpdateResourceMeta};
    let p = &request["params"];
    match request["type"].as_str().unwrap() {
        "CreateTag" => {
            let typed: CreateTag = serde_json::from_value(p.clone()).unwrap();
            assert_eq!(p, &json!({"name":typed.name}));
            let id = format!("{:024x}", 0x7100 + d.tags.len());
            let tag = json!({"_id":{"$oid":id},"name":typed.name,"owner":"fixture-owner","color":"Slate"});
            if may_save {
                assert!(!d.tags.values().any(|t| t["name"] == tag["name"]));
                d.tags.insert(id, tag.clone());
            }
            tag
        }
        "RenameTag" => {
            let typed: RenameTag = serde_json::from_value(p.clone()).unwrap();
            assert_eq!(serde_json::to_value(&typed).unwrap(), *p);
            let old = d.tags.get(&typed.id).unwrap().clone();
            if may_save {
                d.tags.get_mut(&typed.id).unwrap()["name"] = json!(typed.name);
                d.tags[&typed.id].clone()
            } else {
                old
            }
        }
        "DeleteTag" => {
            let typed: DeleteTag = serde_json::from_value(p.clone()).unwrap();
            assert_eq!(serde_json::to_value(&typed).unwrap(), *p);
            let tag = d.tags.get(&typed.id).unwrap().clone();
            if may_save {
                pull_tag(d, &typed.id, d.tag_partial_cleanup_error);
                if !d.tag_partial_cleanup_error {
                    d.tags.remove(&typed.id);
                }
            }
            tag
        }
        "UpdateResourceMeta" => {
            let typed: UpdateResourceMeta = serde_json::from_value(p.clone()).unwrap();
            assert!(typed.description.is_none() && typed.template.is_none());
            assert_eq!(
                p.as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["tags", "target"])
            );
            let mut tags = typed.tags.unwrap();
            if may_save {
                if d.tag_drop_on_assign
                    && let Some(id) = tags.last()
                {
                    d.tags.remove(id);
                }
                tags.retain(|id| d.tags.contains_key(id));
                let target = match p["target"]["type"].as_str().unwrap() {
                    "Stack" => &mut d.stack,
                    "Deployment" => &mut d.deployment,
                    "Server" => &mut d.server,
                    _ => panic!("unsupported tag target"),
                };
                assert_eq!(resource_id(target), &p["target"]["id"]);
                target["tags"] = json!(tags);
                if d.tag_change_metadata {
                    target["description"] = json!("tag-private-unrelated-metadata");
                }
            }
            json!({})
        }
        _ => panic!("unexpected tag write"),
    }
}

fn deletion_read(d: &Data, request: &Value) -> Option<Response> {
    let op = request["type"].as_str().unwrap();
    let p = &request["params"];
    if let Some(family) = match op {
        "ListFullStacks" => Some("stack"),
        "ListFullDeployments" => Some("deployment"),
        "ListFullRepos" => Some("repo"),
        "ListFullBuilders" => Some("builder"),
        "ListFullAlerters" => Some("alerter"),
        _ => None,
    } {
        if let Some(status) = d.reference_status {
            return Some(
                (
                    StatusCode::from_u16(status).unwrap(),
                    format!("{KEY} {SECRET}"),
                )
                    .into_response(),
            );
        }
        let limit = p["limit"].as_u64().unwrap() as usize;
        assert!(limit > 0);
        let page = p["page"].as_u64().unwrap() as usize;
        return Some(
            Json(json!(
                reference_rows(d, family)
                    .into_iter()
                    .skip(page * limit)
                    .take(limit)
                    .collect::<Vec<_>>()
            ))
            .into_response(),
        );
    }
    if let Some((family, key)) = match op {
        "GetAlerter" => Some(("alerter", "alerter")),
        "GetBuilder" => Some(("builder", "builder")),
        _ => None,
    } {
        let item = reference_rows(d, family)
            .into_iter()
            .find(|v| resource_id(v) == &p[key]);
        return Some(match item {Some(v)=>Json(v).into_response(),None=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("Did not find any {} matching {}",if family=="alerter"{"Alerter"}else{"Builder"},p[key].as_str().unwrap())}))).into_response()});
    }
    match op {
        "ListContainers" => Some(
            Json(json!(
                d.containers
                    .values()
                    .map(cache_container)
                    .collect::<Vec<_>>()
            ))
            .into_response(),
        ),
        "InspectContainer" => {
            if d.server.is_null() {
                return Some(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":format!("Did not find any Server matching {SERVER}")})),
                    )
                        .into_response(),
                );
            }
            if d.runtime_unreachable || d.server["config"]["enabled"] == false {
                return Some((StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":"Cannot inspect container: server is NotOk","trace":[format!("{KEY} {SECRET}")]}))).into_response());
            }
            let locator = p["container"].as_str().unwrap();
            let item = d.containers.values().find(|v| {
                v["Id"] == locator || v["Name"].as_str().unwrap().trim_start_matches('/') == locator
            });
            Some(match item {Some(v)=>Json(v.clone()).into_response(),None=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("Docker responded with status code 404: No such container: {locator}"),"trace":[format!("{KEY} {SECRET}")]}))).into_response()})
        }
        "ListAllStackServices" => {
            let services=d.containers.values().filter(|v|v["Config"]["Labels"]["com.docker.compose.project"]=="running-project" && v["Name"]!="/running-project-orphan").map(|v|json!({"stack_id":resource_id(&d.stack),"stack_name":d.stack["name"],"service":"web","image":"busybox:1.37.0","container":cache_container(v),"swarm_service":null,"state":"Running","image_digests":null})).collect::<Vec<_>>();
            Some(Json(json!(services)).into_response())
        }
        "ListUpdates" => {
            let query = &p["query"];
            let target_type = &query["target.type"];
            let target_id = &query["target.id"];
            let operation = &query["operation"];
            let updates = if !d.deletion_history_visible && !d.deletion_updates.is_empty() {
                vec![]
            } else {
                d.deletion_updates.iter().filter(|u|&u["target"]["type"]==target_type && &u["target"]["id"]==target_id && &u["operation"]==operation).map(|u|json!({"id":resource_id(u),"operation":u["operation"],"target":u["target"],"start_ts":u["start_ts"],"success":u["success"],"operator":"fixture-user","username":"fixture-user","status":u["status"],"other_data":""})).collect()
            };
            Some(Json(json!({"updates":updates,"next_page":null})).into_response())
        }
        "GetUpdate" => Some(if !d.deletion_history_visible {
            (StatusCode::FORBIDDEN, format!("{KEY} {SECRET}")).into_response()
        } else {
            match d
                .deletion_updates
                .iter()
                .find(|v| resource_id(v) == &p["id"])
            {
                Some(v) => Json(v.clone()).into_response(),
                None => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":"no update exists with given id"})),
                )
                    .into_response(),
            }
        }),
        _ => None,
    }
}
fn deletion_write(d: &mut Data, request: &Value, may_save: bool) -> Value {
    let op = request["type"].as_str().unwrap();
    let id = request["params"]["id"].as_str().unwrap();
    let saved = match op {
        "DeleteStack" => {
            let typed: DeleteStack = serde_json::from_value(request["params"].clone()).unwrap();
            assert_eq!(typed.id, id);
            d.stack.clone()
        }
        "DeleteDeployment" => {
            let typed: DeleteDeployment =
                serde_json::from_value(request["params"].clone()).unwrap();
            assert_eq!(typed.id, id);
            d.deployment.clone()
        }
        "DeleteServer" => {
            let typed: DeleteServer = serde_json::from_value(request["params"].clone()).unwrap();
            assert_eq!(typed.id, id);
            d.server.clone()
        }
        _ => panic!("unexpected deletion"),
    };
    assert_eq!(resource_id(&saved), id);
    if !may_save {
        return saved;
    }
    let mut logs = vec![];
    if op != "DeleteServer" {
        if d.cleanup_skip {
            logs.push(Log::simple(
                if op == "DeleteStack" {
                    "Destroy Stack"
                } else {
                    "Remove Container"
                },
                if op == "DeleteStack" {
                    "Skipping stack destroy, Server is disabled."
                } else {
                    "Skipping container removal, server is disabled."
                }
                .into(),
            ));
        } else if d.cleanup_fail || d.stack_failure_success_log {
            logs.push(if d.stack_failure_success_log {
                Log::simple(
                    "Failed to destroy stack",
                    "deletion-private-cleanup-error sensitive-config-credential-marker".into(),
                )
            } else {
                Log::error(
                    "Remove Container",
                    "deletion-private-cleanup-error sensitive-config-credential-marker".into(),
                )
            });
        } else if saved["config"]["swarm_id"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
        {
            if op == "DeleteStack" {
                d.containers.retain(|_, v| {
                    v["Config"]["Labels"]["com.docker.compose.project"] != "running-project"
                });
            } else {
                d.containers
                    .retain(|_, v| v["Name"] != "/running-deployment");
            }
            logs.push(Log::simple(
                if op == "DeleteStack" {
                    "Compose Command"
                } else {
                    "Remove Container"
                },
                "cleanup command done".into(),
            ));
        }
    }
    let (target, operation) = match op {
        "DeleteStack" => {
            d.stack = Value::Null;
            (ResourceTarget::Stack(id.into()), Operation::DeleteStack)
        }
        "DeleteDeployment" => {
            d.deployment = Value::Null;
            (
                ResourceTarget::Deployment(id.into()),
                Operation::DeleteDeployment,
            )
        }
        _ => {
            d.server = Value::Null;
            for value in std::iter::once(&mut d.stack)
                .chain(d.created.values_mut())
                .chain(std::iter::once(&mut d.deployment))
                .chain(d.created_deployments.values_mut())
                .chain(std::iter::once(&mut d.repo))
            {
                if value["config"]["server_id"] == id {
                    value["config"]["server_id"] = json!("");
                }
            }
            // Match Core's legacy scalar query; current vector references remain.
            for value in d.extra_references.get_mut("builder").into_iter().flatten() {
                if value["config"]["params"]["server_id"] == id {
                    value["config"]["params"]["server_id"] = json!("");
                }
            }
            (ResourceTarget::Server(id.into()), Operation::DeleteServer)
        }
    };
    let kind = op.strip_prefix("Delete").unwrap();
    for a in d.extra_references.get_mut("alerter").into_iter().flatten() {
        for field in ["resources", "except_resources"] {
            a["config"][field]
                .as_array_mut()
                .unwrap()
                .retain(|t| t["type"] != kind || t["id"] != id);
        }
    }
    logs.push(Log::simple(
        "Deleted Toml",
        format!("deletion-private-archived-definition {KEY} {SECRET} PASSWORD=never-return-this"),
    ));
    let mut update = Update {
        id: format!("{:024x}", 0x500 + d.deletion_updates.len()),
        operation,
        target,
        start_ts: 1,
        operator: "fixture-user".into(),
        logs,
        status: UpdateStatus::Complete,
        ..Default::default()
    };
    update.finalize();
    d.deletion_updates
        .push(serde_json::to_value(update).unwrap());
    saved
}
