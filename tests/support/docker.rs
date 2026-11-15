//! Owned loopback Core wire fixture, not a Docker daemon or permission engine.
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
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub const SERVER: &str = "000000000000000000000001";
pub const IMAGE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const NETWORK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const PRIVATE: [&str; 10] = [
    "docker-env-private",
    "docker-label-private",
    "docker-build-command-private",
    "docker-options-private",
    "docker-user-private",
    "docker-password-private",
    "docker-driver-status-private",
    "docker-mountpoint-private",
    "docker-cluster-secret-private",
    "docker-error-private",
];
pub struct Data {
    pub requests: Vec<Request>,
    pub replies: BTreeMap<String, Value>,
    pub errors: BTreeMap<String, (u16, Value)>,
    pub oversized: Option<String>,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn image(id: &str, tag: &str) -> Value {
    json!({"id":id,"parent_id":"","name":tag,"tags":[tag],"digests":[],"created":123,"size":4096,"in_use":true})
}
pub fn network(id: &str, name: &str) -> Value {
    json!({"id":id,"name":name,"created":"2026-11-15T00:00:00Z","scope":"local","driver":"bridge","enable_ipv6":false,"internal":false,"attachable":true,"ingress":false,"in_use":true})
}
pub fn volume(name: &str) -> Value {
    json!({"name":name,"driver":"local","mountpoint":PRIVATE[7],"created":"2026-11-15T00:00:00Z","scope":"local","size":-1,"in_use":false})
}
impl Core {
    pub async fn start() -> Self {
        let mut replies = BTreeMap::new();
        let mut summary = Family::Server.summary();
        summary["info"]["state"] = json!("Ok");
        replies.insert("ListServers".into(), json!([summary]));
        replies.insert("GetServer".into(), Family::Server.detail());
        replies.insert(
            "ListImages".into(),
            json!([image(IMAGE, "fixture/web:latest")]),
        );
        replies.insert("InspectImage".into(),json!({"Id":IMAGE,"RepoTags":["fixture/web:latest",format!("{}:{}@registry.invalid/web",PRIVATE[4],PRIVATE[5])],"RepoDigests":[],"Created":"2026-11-15T00:00:00Z","Architecture":"amd64","Os":"linux","Size":4096,"Config":{"Env":[format!("TOKEN={}",PRIVATE[0])],"Labels":{"x":PRIVATE[1]},"Cmd":[PRIVATE[2]],"Entrypoint":[PRIVATE[2]]},"Author":PRIVATE[4],"Comment":PRIVATE[1],"Descriptor":{"urls":[PRIVATE[5]],"annotations":{"key":PRIVATE[1]},"data":PRIVATE[0]},"GraphDriver":{"Name":"overlay2","Data":{"path":PRIVATE[7]}},"RootFS":{"Type":"layers","Layers":[IMAGE]}}));
        replies.insert("ListImageHistory".into(),json!([{"Id":IMAGE,"Created":123,"CreatedBy":PRIVATE[2],"Tags":[PRIVATE[5]],"Size":4096,"Comment":PRIVATE[1]},{"Id":"<missing>","Created":122,"CreatedBy":PRIVATE[2],"Tags":[],"Size":-1,"Comment":PRIVATE[1]}]));
        replies.insert(
            "ListNetworks".into(),
            json!([network(NETWORK, "fixture-net")]),
        );
        replies.insert("InspectNetwork".into(),json!({"Id":NETWORK,"Name":"fixture-net","Driver":"bridge","Scope":"local","Created":"2026-11-15T00:00:00Z","EnableIPv6":false,"Internal":false,"Attachable":true,"Ingress":false,"Containers":[{"ContainerID":"cc","Name":PRIVATE[4],"EndpointID":PRIVATE[5]}],"Options":{"password":PRIVATE[3]},"Labels":{"secret":PRIVATE[1]},"IPAM":{"Driver":"default","Config":[],"Options":{"token":PRIVATE[3]}}}));
        replies.insert("ListVolumes".into(), json!([volume("fixture-volume")]));
        replies.insert("InspectVolume".into(),json!({"Name":"fixture-volume","Driver":"local","Mountpoint":PRIVATE[7],"CreatedAt":"2026-11-15T00:00:00Z","Scope":"local","Options":{"password":PRIVATE[3]},"Labels":{"secret":PRIVATE[1]},"Status":[PRIVATE[6]],"ClusterVolume":{"ID":"ccccccccccccccccccccccccc","Spec":{"AccessMode":{"Secrets":[{"Key":PRIVATE[8],"Secret":PRIVATE[8]}]}},"Info":{"VolumeContext":{"secret":PRIVATE[8]}}},"UsageData":{"Size":-1,"RefCount":2}}));
        let data = Arc::new(Mutex::new(Data {
            requests: vec![],
            replies,
            errors: BTreeMap::new(),
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
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if let Some((status, body)) = d.errors.get(op) {
        return (StatusCode::from_u16(*status).unwrap(), Json(body.clone())).into_response();
    }
    if d.oversized.as_deref() == Some(op) {
        return Json(json!({"private":PRIVATE[0].repeat(200_000)})).into_response();
    }
    let mut result = d
        .replies
        .get(op)
        .unwrap_or_else(|| panic!("unexpected {op}"))
        .clone();
    if op == "ListServers" {
        if let Some(names) = v["params"]["query"]["names"]
            .as_array()
            .filter(|v| !v.is_empty())
        {
            result
                .as_array_mut()
                .unwrap()
                .retain(|s| names.iter().any(|n| *n == s["id"] || *n == s["name"]));
        }
        let limit = v["params"]["limit"].as_u64().unwrap() as usize;
        assert!(limit > 0);
        let offset = v["params"]["page"].as_u64().unwrap() as usize * limit;
        result = json!(
            result
                .as_array()
                .unwrap()
                .iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>()
        );
    }
    Json(result).into_response()
}
