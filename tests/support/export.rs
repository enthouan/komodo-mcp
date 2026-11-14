//! Scoped export wire fixture: privileged raw TOML is deliberately secret-bearing.
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
    Version, action::Action, alerter::Alerter, build::Build, builder::Builder,
    deployment::Deployment, procedure::Procedure, repo::Repo, server::Server, stack::Stack,
    swarm::Swarm, sync::ResourceSync,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

pub const PRIVATE: [&str; 5] = [
    "unrecognized-private-value",
    "inline-private-value",
    "variable-private-value",
    "metadata-private-value",
    "unknown-key-private-value",
];
pub const GROUP: &str = "000000000000000000000020";
pub const KINDS: [(&str, &str, &str); 11] = [
    ("server", "Server", "server"),
    ("swarm", "Swarm", "swarm"),
    ("stack", "Stack", "stack"),
    ("deployment", "Deployment", "deployment"),
    ("build", "Build", "build"),
    ("repo", "Repo", "repo"),
    ("builder", "Builder", "builder"),
    ("alerter", "Alerter", "alerter"),
    ("procedure", "Procedure", "procedure"),
    ("action", "Action", "action"),
    ("resource_sync", "ResourceSync", "sync"),
];
pub fn id(index: usize) -> String {
    format!("{:024x}", index + 1)
}
pub fn input(index: usize) -> Value {
    json!({"kind":KINDS[index].0,"target":{"id":id(index)}})
}

pub struct Data {
    pub resources: BTreeMap<String, Value>,
    pub requests: Vec<Request>,
    pub source: Option<String>,
    pub body: Option<String>,
    pub denied: Option<String>,
    pub ambiguous: bool,
    pub missing: bool,
    pub omit_sync: bool,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
impl Core {
    pub async fn start() -> Self {
        let mut resources = BTreeMap::new();
        for (index, (_, kind, _)) in KINDS.iter().enumerate() {
            let id = id(index);
            let name = format!("selected-{}", KINDS[index].0);
            macro_rules! resource {
                ($t:ident) => {
                    serde_json::to_value($t {
                        id,
                        name,
                        ..Default::default()
                    })
                    .unwrap()
                };
            }
            let full = match *kind {
                "Server" => resource!(Server),
                "Swarm" => resource!(Swarm),
                "Stack" => resource!(Stack),
                "Deployment" => resource!(Deployment),
                "Build" => resource!(Build),
                "Repo" => resource!(Repo),
                "Builder" => resource!(Builder),
                "Alerter" => resource!(Alerter),
                "Procedure" => resource!(Procedure),
                "Action" => resource!(Action),
                "ResourceSync" => resource!(ResourceSync),
                _ => unreachable!(),
            };
            resources.insert((*kind).into(), full);
        }
        let data = Arc::new(Mutex::new(Data {
            resources,
            requests: vec![],
            source: None,
            body: None,
            denied: None,
            ambiguous: false,
            missing: false,
            omit_sync: false,
            bad_credentials: false,
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
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
    pub fn requests(&self) -> Vec<Request> {
        self.data.lock().unwrap().requests.clone()
    }
    pub fn assert_reads_only(&self) {
        let d = self.data.lock().unwrap();
        assert!(!d.bad_credentials);
        assert!(d.requests.iter().all(|r| r.path == "read"));
        assert!(
            d.requests
                .iter()
                .all(|r| r.operation != "ExportAllResourcesToToml")
        );
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
fn listed(kind: &str, full: &Value) -> Value {
    let mut info = json!({"state":"Unknown","server_id":"","server_name":"","swarm_id":"","swarm_name":"","server_ids":[],"builder_id":"","last_pulled_at":0,"last_built_at":0,"files_on_host":false,"file_contents":false,"dockerfile_contents":false,"linked_repo":"","git_provider":"github.com","repo":"fixture/repo","branch":"main","repo_link":"","services":[],"auto_update_all_services":false,"project_missing":false,"missing_files":[],"custom_name":"","image":"","update_available":false,"version":Version::default(),"builder_type":"Server","enabled":false,"endpoint_type":"Custom","stages":0,"last_sync_ts":0,"managed":false,"resource_path":[]});
    if kind == "Server" {
        info = Family::Server.summary()["info"].clone();
    }
    if kind == "Stack" {
        info["state"] = json!(komodo_client::entities::stack::StackState::Unknown);
    }
    if kind == "Deployment" {
        info["state"] = json!(komodo_client::entities::deployment::DeploymentState::Unknown);
    }
    if kind == "Swarm" {
        info["state"] = json!(komodo_client::entities::swarm::SwarmState::Unknown);
    }
    let value = json!({"id":full["_id"]["$oid"],"type":kind,"name":full["name"],"template":full["template"],"tags":[],"info":info});
    if kind == "Stack" {
        serde_json::from_value::<komodo_client::entities::stack::StackListItem>(value.clone())
            .expect("valid pinned Stack list fixture");
    }
    value
}
async fn read(
    State(state): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = state.lock().unwrap();
    record(&mut d, &h, "read", &v);
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if d.denied.as_deref() == Some(op) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":format!("denied {SECRET} {}",PRIVATE[0])})),
        )
            .into_response();
    }
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if op == "ListUserGroups" {
        return Json(json!([{"_id":{"$oid":GROUP},"name":"selected-group"}])).into_response();
    }
    if op == "ExportResourcesToToml" {
        if let Some(body) = &d.body {
            return (StatusCode::OK, body.clone()).into_response();
        }
        assert!(p["existing"].is_null());
        let source = if let Some(source) = &d.source {
            source.clone()
        } else {
            let mut source = format!("# {}\n", PRIVATE[3]);
            for target in p["targets"].as_array().unwrap() {
                let kind = target["type"].as_str().unwrap();
                let id = target["id"].as_str().unwrap();
                if kind == "ResourceSync" && d.omit_sync {
                    continue;
                }
                let full = &d.resources[kind];
                assert_eq!(id, full["_id"]["$oid"]);
                let header = KINDS.iter().find(|k| k.1 == kind).unwrap().0;
                source += &format!(
                    "[[{header}]]\nname = {}\ndescription = {}\ntags = ['owned']\n[{header}.config]\n",
                    json!(full["name"].as_str().unwrap()),
                    json!(PRIVATE[3])
                );
                if kind == "Builder" {
                    source += "type = 'Server'\n";
                }
                source += &format!(
                    "environment = {}\nfile_contents = {}\n{} = {}\n",
                    json!(format!("PASSWORD={}", PRIVATE[0])),
                    json!(format!("services:\n  app:\n    secret: {}", PRIVATE[1])),
                    json!(PRIVATE[4]),
                    json!(SECRET)
                );
                if kind == "Server" {
                    source += "enabled = false\n";
                }
                if kind == "Stack" {
                    source += "auto_pull = false\nserver = 'outside-selection'\n";
                }
                if kind == "ResourceSync" {
                    source += "managed = true\ndelete = false\nresource_tags = []\n";
                }
            }
            if p["include_variables"] == true {
                for (name, secret) in [("SECRET_VAR", true), ("UNMARKED_VAR", false)] {
                    source += &format!(
                        "[[variable]]\nname='{name}'\nvalue={}\nis_secret={secret}\n",
                        json!(PRIVATE[2])
                    );
                }
            }
            for g in p["user_groups"].as_array().unwrap() {
                assert_eq!(g, GROUP);
                source += "[[user_group]]\nname='selected-group'\neveryone=false\nusers=['private-user']\n";
            }
            source
        };
        return Json(json!({"toml":source})).into_response();
    }
    for (_, kind, field) in KINDS {
        if op == format!("Get{kind}") {
            assert_eq!(p[field], d.resources[kind]["_id"]["$oid"]);
            return Json(d.resources[kind].clone()).into_response();
        }
        let plural = if kind == "ResourceSync" {
            "ResourceSyncs".into()
        } else {
            format!("{kind}s")
        };
        if op == format!("List{plural}") {
            assert_eq!(p["limit"], 100);
            assert_eq!(p["sort_desc"], false);
            let full = &d.resources[kind];
            let q = &p["query"];
            let matches = q["names"].as_array().unwrap().iter().any(|s| {
                let s = s.as_str().unwrap();
                if s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                    full["_id"]["$oid"] == s
                } else {
                    full["name"] == s
                }
            }) || q["terms"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| full["name"].as_str().unwrap().contains(s.as_str().unwrap()));
            let mut rows = vec![];
            if matches && !d.missing {
                rows.push(listed(kind, full));
            }
            if d.ambiguous && !rows.is_empty() {
                let mut other = rows[0].clone();
                other["id"] = json!("000000000000000000000099");
                rows.push(other);
            }
            return Json(json!(rows)).into_response();
        }
    }
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"unexpected typed read"})),
    )
        .into_response()
}
async fn write(
    State(state): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(&mut state.lock().unwrap(), &h, "write", &v);
    StatusCode::FORBIDDEN.into_response()
}
async fn execute(
    State(state): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(&mut state.lock().unwrap(), &h, "execute", &v);
    StatusCode::FORBIDDEN.into_response()
}
