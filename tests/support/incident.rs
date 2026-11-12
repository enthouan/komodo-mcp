//! Shared read-only log fixture for production-stdio redaction cases.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use super::{KEY, SECRET};

pub fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/core-2.3.2/incident.json")).unwrap()
}

pub fn wire() -> Value {
    serde_json::from_str(include_str!("../fixtures/core-2.3.2/incident-wire.json")).unwrap()
}

#[derive(Default)]
pub struct Options {
    pub overrides: BTreeMap<String, Value>,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub operation: String,
    pub params: Value,
}

struct Inner {
    options: Options,
    requests: Mutex<Vec<Request>>,
}

pub struct IncidentCore {
    pub address: String,
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl IncidentCore {
    pub async fn start(options: Options) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let inner = Arc::new(Inner {
            options,
            requests: Mutex::new(vec![]),
        });
        let app = Router::new()
            .route("/komodo/read", post(read))
            .with_state(inner.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            address,
            inner,
            task,
        }
    }

    pub fn api_key(&self) -> &str {
        self.inner.options.api_key.as_deref().unwrap_or(KEY)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.inner.requests.lock().unwrap().clone()
    }
}

impl Drop for IncidentCore {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if headers.get("x-api-key").and_then(|s| s.to_str().ok())
        != Some(inner.options.api_key.as_deref().unwrap_or(KEY))
        || headers.get("x-api-secret").and_then(|s| s.to_str().ok()) != Some(SECRET)
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"synthetic authentication rejected"})),
        );
    }
    let operation = envelope["type"].as_str().unwrap_or("");
    let params = &envelope["params"];
    inner.requests.lock().unwrap().push(Request {
        operation: operation.into(),
        params: params.clone(),
    });
    let supported = matches!(
        operation,
        "GetStackLog" | "GetContainerLog" | "GetDeploymentLog"
    );
    if operation == "GetVersion" {
        return (StatusCode::OK, Json(json!({"version":"2.3.2"})));
    }
    if !supported
        || !params["tail"]
            .as_u64()
            .is_some_and(|n| (1..=500).contains(&n))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"fixture rejects unsupported or unbounded read"})),
        );
    }
    let log = inner
        .options
        .overrides
        .get(operation)
        .cloned()
        .unwrap_or_else(|| wire()["log"].clone());
    (StatusCode::OK, Json(log))
}
