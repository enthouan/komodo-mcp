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
use komodo_client::api::write::UpdateStack;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
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
    Delayed,
}
pub struct Data {
    pub stack: Value,
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
        let data = Arc::new(Mutex::new(Data {
            stack,
            requests: vec![],
            reply: WriteReply::Valid,
            read_status: None,
            bad_credentials: false,
        }));
        let app = Router::new()
            .route("/read", post(read))
            .route("/write", post(write))
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
    match value["type"].as_str().unwrap() {
        "GetVersion" => Json(json!({"version":"2.3.2"})).into_response(),
        "ListStacks" => {
            let names = &value["params"]["query"]["names"];
            let mut summary = Family::Stack.summary();
            summary["name"] = data.stack["name"].clone();
            let matches = names.as_array().unwrap().is_empty()
                || names
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n == &summary["name"] || n == &summary["id"]);
            Json(if matches { json!([summary]) } else { json!([]) }).into_response()
        }
        "GetStack" => Json(data.stack.clone()).into_response(),
        other => panic!("unexpected read {other}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    let (reply, saved) = {
        let mut data = data.lock().unwrap();
        record(&mut data, &headers, "write", &value);
        assert_eq!(value["type"], "UpdateStack");
        let typed: UpdateStack = serde_json::from_value(value["params"].clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), value["params"]);
        if !matches!(
            data.reply,
            WriteReply::Denied | WriteReply::WriteDenied | WriteReply::Redirect
        ) {
            for (key, v) in value["params"]["config"].as_object().unwrap() {
                data.stack["config"][key] = v.clone();
            }
        }
        (data.reply, data.stack.clone())
    };
    match reply {
        WriteReply::Valid => Json(saved).into_response(),
        WriteReply::WriteDenied => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"User does not have required permissions on this Stack. Must have at least Write permissions","trace":[format!("{KEY} {SECRET}")]}))).into_response(),
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
