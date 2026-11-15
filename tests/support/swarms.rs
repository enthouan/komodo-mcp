//! Pinned Core 2.3.2 wire behavior, including cache ambiguity and fresh inspect.
use crate::support::{
    KEY, SECRET,
    operations::{Request, Task},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_client::entities::swarm::{Swarm, SwarmConfig};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

pub const SWARM: &str = "000000000000000000000050";
pub const MANAGER: &str = "000000000000000000000001";
pub const NODE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaa";
pub const WORKER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbb";
pub const SERVICE: &str = "ccccccccccccccccccccccccc";
pub const OTHER_SERVICE: &str = "ddddddddddddddddddddddddd";
pub const RUNNING: &str = "eeeeeeeeeeeeeeeeeeeeeeeee";
pub const PENDING: &str = "fffffffffffffffffffffffff";
pub const FAILED: &str = "ggggggggggggggggggggggggg";
pub const HISTORY: &str = "hhhhhhhhhhhhhhhhhhhhhhhhh";
pub const PRIVATE: [&str; 7] = [
    "swarm-join-private",
    "swarm-ca-private",
    "swarm-env-private",
    "swarm-label-private",
    "swarm-registry-private",
    "swarm-error-private",
    "swarm-config-private",
];
pub struct Data {
    pub requests: Vec<Request>,
    pub swarms: Vec<Value>,
    pub full: Value,
    pub nodes: Vec<Value>,
    pub services: Vec<Value>,
    pub tasks: Vec<Value>,
    pub details: BTreeMap<String, Value>,
    pub denied: BTreeSet<String>,
    pub unsupported: BTreeSet<String>,
    pub oversized: Option<String>,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn node(id: &str, name: &str, role: &str, availability: &str) -> Value {
    json!({"ID":id,"Name":name,"Hostname":name,"Role":role,"Availability":availability,"State":"ready","Labels":{"TOKEN":PRIVATE[3]}})
}
pub fn service(id: &str, name: &str) -> Value {
    json!({"ID":id,"Name":name,"Image":format!("{}:password@registry/image",PRIVATE[4]),"Mode":"Replicated","Replicas":3,"DesiredTasks":3,"RunningTasks":1,"Configs":[PRIVATE[6]],"Secrets":[PRIVATE[2]],"State":"Unhealthy"})
}
pub fn task(id: &str, state: &str, desired: &str, node: &str, service: &str) -> Value {
    json!({"ID":id,"Name":format!("fixture.{}",&id[..2]),"ServiceID":service,"NodeID":node,"State":state,"DesiredState":desired,"Configs":[PRIVATE[6]],"Secrets":[PRIVATE[2]]})
}
impl Core {
    pub async fn start() -> Self {
        let full = serde_json::to_value(Swarm {
            id: SWARM.into(),
            name: "fixture-swarm".into(),
            description: PRIVATE[5].into(),
            config: SwarmConfig {
                server_ids: vec![MANAGER.into()],
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let nodes = vec![
            node(NODE, "manager", "manager", "active"),
            node(WORKER, "worker", "worker", "drain"),
        ];
        let services = vec![
            service(SERVICE, "fixture-web"),
            service(OTHER_SERVICE, "unrelated"),
        ];
        let tasks = vec![
            task(RUNNING, "running", "running", NODE, SERVICE),
            task(PENDING, "pending", "running", "", SERVICE),
            task(FAILED, "failed", "shutdown", WORKER, SERVICE),
            task(HISTORY, "running", "shutdown", NODE, SERVICE),
        ];
        let mut details = BTreeMap::new();
        for n in &nodes {
            details.insert(n["ID"].as_str().unwrap().into(), json!({"ID":n["ID"],"Spec":{"Name":n["Name"],"Role":n["Role"],"Availability":n["Availability"],"Labels":{"secret":PRIVATE[3]}},"Description":{"Hostname":n["Hostname"]},"Status":{"State":"ready","Message":PRIVATE[5]},"ManagerStatus":{"Leader":true,"Reachability":"reachable"}}));
        }
        for s in &services {
            details.insert(s["ID"].as_str().unwrap().into(), json!({"ID":s["ID"],"Mode":"Replicated","Replicas":3,"State":"Unhealthy","Spec":{"Name":s["Name"],"Mode":{"Replicated":{"Replicas":3}},"Labels":{"password":PRIVATE[3]},"TaskTemplate":{"ContainerSpec":{"Env":[format!("SECRET={}",PRIVATE[2])],"Image":PRIVATE[4],"Secrets":[{"SecretID":PRIVATE[2]}]}}},"UpdateStatus":{"State":"paused","Message":PRIVATE[5]}}));
        }
        for t in &tasks {
            details.insert(t["ID"].as_str().unwrap().into(), json!({"ID":t["ID"],"Name":t["Name"],"ServiceID":t["ServiceID"],"NodeID":t["NodeID"],"DesiredState":t["DesiredState"],"Slot":1,"Spec":{"ContainerSpec":{"Env":[format!("TOKEN={}",PRIVATE[2])],"Image":PRIVATE[4]}},"Status":{"State":t["State"],"Message":PRIVATE[5],"Err":if t["ID"]==PENDING{format!("no suitable node (scheduling constraints not satisfied) {}",PRIVATE[5])}else if t["ID"]==FAILED{format!("task: non-zero exit (23) {}",PRIVATE[5])}else{String::new()},"ContainerStatus":{"ExitCode":if t["ID"]==FAILED{23}else{0}}}}));
        }
        let data = Arc::new(Mutex::new(Data {
            requests: vec![],
            swarms: vec![
                json!({"id":SWARM,"type":"Swarm","name":"fixture-swarm","template":false,"tags":[],"info":{"server_ids":[MANAGER],"state":"Unhealthy","err":{"error":PRIVATE[5],"trace":[]}}}),
            ],
            full,
            nodes,
            services,
            tasks,
            details,
            denied: BTreeSet::new(),
            unsupported: BTreeSet::new(),
            oversized: None,
            bad_credentials: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
    pub fn assert_readonly(&self) {
        assert!(self.requests("write").is_empty());
        assert!(self.requests("execute").is_empty());
        assert!(!self.data.lock().unwrap().bad_credentials);
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
async fn write(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    record(&mut d.lock().unwrap(), &h, "write", &v);
    StatusCode::FORBIDDEN.into_response()
}
async fn execute(
    State(d): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(&mut d.lock().unwrap(), &h, "execute", &v);
    StatusCode::FORBIDDEN.into_response()
}
async fn read(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = d.lock().unwrap();
    record(&mut d, &h, "read", &v);
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if d.denied.contains(op) {
        let suffix = if op == "GetSwarmServiceLog" {
            ", as well as these specific permissions: [Logs]"
        } else if op.starts_with("Inspect") {
            ", as well as these specific permissions: [Inspect]"
        } else {
            ""
        };
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("User does not have required permissions on this Swarm. Must have at least Read permissions{suffix}"),"trace":[PRIVATE[5]]}))).into_response();
    }
    if d.unsupported.contains(op) {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error":PRIVATE[5]})),
        )
            .into_response();
    }
    if d.oversized.as_deref() == Some(op) {
        return Json(json!({"private":PRIVATE[2].repeat(200_000)})).into_response();
    }
    let result = match op {
        "GetVersion" => json!({"version":"2.3.2"}),
        "ListSwarms" => {
            let mut items = d.swarms.clone();
            if let Some(names) = p["query"]["names"].as_array().filter(|v| !v.is_empty()) {
                items.retain(|s| names.iter().any(|n| *n == s["name"] || *n == s["id"]));
            }
            if let Some(terms) = p["query"]["terms"].as_array().filter(|v| !v.is_empty()) {
                items.retain(|s| {
                    terms
                        .iter()
                        .all(|n| s["name"].as_str().unwrap().contains(n.as_str().unwrap()))
                });
            }
            let size = p["limit"].as_u64().unwrap() as usize;
            assert!(size > 0);
            let offset = p["page"].as_u64().unwrap() as usize * size;
            json!(
                items
                    .into_iter()
                    .skip(offset)
                    .take(size)
                    .collect::<Vec<_>>()
            )
        }
        "GetSwarm" => d.full.clone(),
        "InspectSwarm" => {
            json!({"ID":"iiiiiiiiiiiiiiiiiiiiiiiii","JoinTokens":{"Worker":PRIVATE[0],"Manager":PRIVATE[0]},"Spec":{"CAConfig":{"SigningCAKey":PRIVATE[1]},"Labels":{"secret":PRIVATE[3]}},"RootRotationInProgress":false})
        }
        "ListSwarmNodes" => json!(d.nodes),
        "ListSwarmServices" => json!(d.services),
        "ListSwarmTasks" => json!(d.tasks),
        "InspectSwarmNode" | "InspectSwarmService" | "InspectSwarmTask" => {
            let field = if op == "InspectSwarmNode" {
                "node"
            } else if op == "InspectSwarmService" {
                "service"
            } else {
                "task"
            };
            d.details
                .get(p[field].as_str().unwrap())
                .cloned()
                .unwrap_or(Value::Null)
        }
        "GetSwarmServiceLog" => {
            assert_eq!(p["service"], SERVICE);
            assert_eq!(p["details"], false);
            json!({"stage":"log","command":PRIVATE[4],"stdout":format!("hello\nTOKEN={}\nsecret={}\n{} done\n",PRIVATE[2],SECRET,KEY),"stderr":"","success":true,"start_ts":1,"end_ts":2})
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":format!("unexpected {op}")})),
            )
                .into_response();
        }
    };
    Json(result).into_response()
}
