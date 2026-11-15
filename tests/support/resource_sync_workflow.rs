//! Composed pinned-wire Core. Actual defaulting/source/runtime behavior is also
//! verified by the opt-in disposable Core smoke; this fixture is deterministic.
use crate::{exports, support, syncs};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_client::entities::{
    server::{Server, ServerConfig},
    toml::ResourcesToml,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::operations;

pub const SERVER: &str = "000000000000000000000001";
pub const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
pub struct Core {
    pub address: String,
    pub syncs: syncs::Core,
    export: exports::Core,
    workloads: operations::Core,
    pub source: Mutex<(String, String)>,
    pub requests: Mutex<Vec<operations::Request>>,
    http: reqwest::Client,
    task: Mutex<Option<operations::Task>>,
}
impl Core {
    pub async fn start(source: String) -> Arc<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let syncs = syncs::Core::start().await;
        {
            let mut d = syncs.data.lock().unwrap();
            let mut server: Server = serde_json::from_value(d.server.clone()).unwrap();
            server.id = SERVER.into();
            server.name = "workflow-server".into();
            server.config.auto_prune = false;
            d.server = serde_json::to_value(server).unwrap();
        }
        let c = Arc::new(Self {
            address: format!("http://{}", listener.local_addr().unwrap()),
            syncs,
            export: exports::Core::start().await,
            workloads: operations::Core::start(operations::Options {
                outcome: operations::Outcome::Succeeded,
                ..Default::default()
            })
            .await,
            source: Mutex::new((source, REVISION.into())),
            requests: Mutex::new(vec![]),
            http: reqwest::Client::new(),
            task: Mutex::new(None),
        });
        let app = Router::new()
            .route("/{path}", post(handle))
            .with_state(Arc::downgrade(&c));
        *c.task.lock().unwrap() = Some(operations::Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        })));
        c
    }
    pub fn count(&self, path: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
    fn document(&self, sync: &Value) -> (String, String) {
        if !sync["config"]["file_contents"].as_str().unwrap().is_empty() {
            (
                sync["config"]["file_contents"].as_str().unwrap().into(),
                String::new(),
            )
        } else {
            self.source.lock().unwrap().clone()
        }
    }
    fn refresh(&self, id: &str) -> Value {
        let mut d = self.syncs.data.lock().unwrap();
        let saved = d.syncs[id].clone();
        let (source, revision) = self.document(&saved);
        let desired: ResourcesToml = toml::from_str(&source).unwrap();
        assert_eq!(desired.servers.len(), 1);
        let desired_cfg: ServerConfig = desired.servers[0].config.clone().into();
        let original: Server = serde_json::from_value(d.server.clone()).unwrap();
        let current = format!(
            "[[server]]\nname={}\n[server.config]\n{}",
            json!(original.name),
            toml::to_string(&original.config).unwrap()
        );
        let changed = serde_json::to_value(&original.config).unwrap()
            != serde_json::to_value(desired_cfg).unwrap();
        let mut s = saved;
        s["info"]["pending_error"] = Value::Null;
        s["info"]["pending_deploy_error"] = Value::Null;
        s["info"]["remote_contents"] =
            json!([{"resource_path":"resources.toml","path":"resources.toml","contents":source}]);
        s["info"]["pending_hash"] =
            if s["config"]["file_contents"] != "" || s["config"]["files_on_host"] == true {
                Value::Null
            } else {
                json!(revision)
            };
        s["info"]["resource_updates"] = if changed {
            json!([{"target":{"type":"Server","id":SERVER},"data":{"type":"Update","data":{"current":current,"proposed":source}}}])
        } else {
            json!([])
        };
        d.syncs.insert(id.into(), s.clone());
        s
    }
}
async fn handle(
    State(weak): State<std::sync::Weak<Core>>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let c = weak.upgrade().unwrap();
    assert_eq!(headers["x-api-key"], support::KEY);
    assert_eq!(headers["x-api-secret"], support::SECRET);
    let path = match path.as_str() {
        "read" => "read",
        "write" => "write",
        "execute" => "execute",
        _ => panic!("unexpected path"),
    };
    let op = v["type"].as_str().unwrap();
    c.requests.lock().unwrap().push(operations::Request {
        path,
        operation: op.into(),
        params: v["params"].clone(),
    });
    if op == "ExportResourcesToToml" {
        let server = c.syncs.data.lock().unwrap().server.clone();
        let mut d = c.export.data.lock().unwrap();
        d.resources.insert("Server".into(), server.clone());
        d.source = Some(format!(
            "[[server]]\nname={}\n[server.config]\npasskey={}\nenabled=false\n[[variable]]\nname='WORKFLOW_VAR'\nvalue={}\nis_secret=false\n",
            server["name"],
            json!(syncs::PRIVATE[2]),
            json!(exports::PRIVATE[2])
        ));
    }
    if op == "RefreshResourceSyncPending" {
        let denied = c.syncs.data.lock().unwrap().deny_refresh;
        if !denied {
            let value = c.refresh(v["params"]["sync"].as_str().unwrap());
            c.syncs.data.lock().unwrap().refresh_reply = Some(value);
        }
    }
    let address = if op == "ExportResourcesToToml" {
        &c.export.address
    } else if matches!(
        op,
        "ListStacks"
            | "ListDeployments"
            | "ListAllStackServices"
            | "GetSystemStats"
            | "GetServerState"
    ) {
        &c.workloads.address
    } else {
        &c.syncs.address
    };
    let response = c
        .http
        .post(format!("{address}/{path}"))
        .headers(headers)
        .json(&v)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let bytes = response.bytes().await.unwrap();
    // The fixture can accept execution and then lose only its acknowledgement.
    // Preserve that malformed wire response instead of manufacturing a JSON ID.
    if path == "execute" && status.is_success() {
        let id = v["params"]["sync"].as_str().unwrap();
        let saved = c.syncs.saved(id);
        let (source, _) = c.document(&saved);
        let desired: ResourcesToml = toml::from_str(&source).unwrap();
        let config: ServerConfig = desired.servers[0].config.clone().into();
        c.syncs.data.lock().unwrap().server["config"] = serde_json::to_value(config).unwrap();
        c.refresh(id);
    }
    let Ok(mut reply) = serde_json::from_slice::<Value>(&bytes) else {
        return (status, bytes).into_response();
    };
    if status.is_success() && matches!(op, "CreateResourceSync" | "UpdateResourceSync") {
        let id = reply["_id"]["$oid"].as_str().unwrap().to_owned();
        reply = c.refresh(&id);
    } else if op == "RefreshResourceSyncPending" {
        c.syncs.data.lock().unwrap().refresh_reply = None;
    }
    (status, Json(reply)).into_response()
}
