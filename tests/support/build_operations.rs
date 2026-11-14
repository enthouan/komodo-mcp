//! Execution/history overlay on the owned definition fixture. Survives MCP restarts.
use crate::{
    definitions,
    support::{
        KEY, SECRET,
        operations::{Request, Task},
    },
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
pub use definitions::{BUILD, BUILDER, PRIVATE};
use komodo_client::entities::{
    Operation, ResourceTarget,
    build::BuildConfig,
    komodo_timestamp,
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Copy, Default)]
pub enum Outcome {
    #[default]
    Success,
    HookFailed,
    BuildFailed,
    PushFailed,
    Running,
}
#[derive(Clone, Copy, Default)]
pub enum Ack {
    #[default]
    Valid,
    Lost,
    Denied,
    WrongTarget,
    WrongOperation,
    MissingId,
}
pub struct Data {
    pub updates: BTreeMap<String, Value>,
    pub requests: Vec<Request>,
    pub outcome: Outcome,
    pub ack: Ack,
    pub building: bool,
    pub fail_updates: bool,
    pub fail_observation_after_cancel: bool,
    pub update_delay: Duration,
    pub cancel_finishes_original: bool,
    pub replace_on_cancel: bool,
    pub replace_at_active_read: Option<usize>,
    pub active_reads: usize,
    pub incomplete_active: bool,
    pub switch_builder_at_read: Option<usize>,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub definitions: Arc<definitions::Core>,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
#[derive(Clone)]
struct StateData {
    definitions: Arc<definitions::Core>,
    data: Arc<Mutex<Data>>,
    http: reqwest::Client,
}
impl Core {
    pub async fn start() -> Self {
        let definitions = Arc::new(definitions::Core::start().await);
        definitions
            .data
            .lock()
            .unwrap()
            .builds
            .get_mut(BUILD)
            .unwrap()["config"]["version"] = json!({"major":1,"minor":2,"patch":2});
        let data = Arc::new(Mutex::new(Data {
            updates: BTreeMap::new(),
            requests: vec![],
            outcome: Outcome::Success,
            ack: Ack::Valid,
            building: false,
            fail_updates: false,
            fail_observation_after_cancel: false,
            update_delay: Duration::ZERO,
            cancel_finishes_original: false,
            replace_on_cancel: false,
            replace_at_active_read: None,
            active_reads: 0,
            incomplete_active: false,
            switch_builder_at_read: None,
            bad_credentials: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/read", post(read))
            .route("/execute", post(execute))
            .route("/write", post(write))
            .with_state(StateData {
                definitions: definitions.clone(),
                data: data.clone(),
                http: reqwest::Client::new(),
            });
        let task = Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Self {
            address,
            definitions,
            data,
            _task: task,
        }
    }
    pub fn executions(&self) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == "execute")
            .cloned()
            .collect()
    }
    pub fn reads(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == "read")
            .count()
    }
    pub fn seed_run(&self, status: UpdateStatus) -> String {
        let mut d = self.data.lock().unwrap();
        let u = update(&d, Operation::RunBuild, status, false, vec![]);
        let id = u["_id"]["$oid"].as_str().unwrap().into();
        d.building = status != UpdateStatus::Complete;
        d.updates.insert(id, u);
        d.updates.keys().next_back().unwrap().clone()
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
fn update(
    d: &Data,
    operation: Operation,
    status: UpdateStatus,
    success: bool,
    logs: Vec<Log>,
) -> Value {
    serde_json::to_value(Update {
        id: format!("{:024x}", 0x100 + d.updates.len()),
        operation,
        status,
        success,
        start_ts: komodo_timestamp() + d.updates.len() as i64,
        end_ts: (status == UpdateStatus::Complete).then(komodo_timestamp),
        target: ResourceTarget::Build(BUILD.into()),
        operator: "fixture-operator".into(),
        logs,
        ..Default::default()
    })
    .unwrap()
}
fn replace(d: &mut Data) {
    for u in d
        .updates
        .values_mut()
        .filter(|u| u["operation"] == "RunBuild" && u["status"] != "Complete")
    {
        u["status"] = json!("Complete");
        u["success"] = json!(true);
        u["end_ts"] = json!(komodo_timestamp());
    }
    let u = update(
        d,
        Operation::RunBuild,
        UpdateStatus::InProgress,
        false,
        vec![],
    );
    let id = u["_id"]["$oid"].as_str().unwrap().into();
    d.updates.insert(id, u);
    d.building = true;
}
fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":format!("permission denied key={KEY} secret={SECRET}")})),
    )
        .into_response()
}
async fn forward(state: &StateData, path: &str, headers: HeaderMap, v: Value) -> Response {
    let response = state
        .http
        .post(format!("{}{path}", state.definitions.address))
        .headers(headers)
        .json(&v)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let bytes = response.bytes().await.unwrap();
    (status, bytes).into_response()
}
async fn read(State(s): State<StateData>, headers: HeaderMap, Json(v): Json<Value>) -> Response {
    let (reply, delay) = {
        let mut d = s.data.lock().unwrap();
        record(&mut d, &headers, "read", &v);
        let op = v["type"].as_str().unwrap();
        let p = &v["params"];
        if op == "GetBuild"
            && d.switch_builder_at_read.is_some_and(|n| {
                d.requests
                    .iter()
                    .filter(|r| r.operation == "GetBuild")
                    .count()
                    == n
            })
        {
            s.definitions
                .data
                .lock()
                .unwrap()
                .builders
                .get_mut(BUILDER)
                .unwrap()["config"] = json!({"type":"Aws","params":{}});
        }
        let delay = if op == "GetUpdate" {
            d.update_delay
        } else {
            Duration::ZERO
        };
        let reply = match op {
            "GetBuildActionState" => {
                assert_eq!(p["build"], BUILD);
                Some(Json(json!({"building":d.building})).into_response())
            }
            "GetUpdate" => {
                if d.fail_updates {
                    Some((StatusCode::SERVICE_UNAVAILABLE, "history unavailable").into_response())
                } else {
                    Some(d.updates.get(p["id"].as_str().unwrap()).map_or_else(
                        || (StatusCode::NOT_FOUND, "missing update").into_response(),
                        |u| Json(u.clone()).into_response(),
                    ))
                }
            }
            "ListUpdates" => {
                let q = &p["query"];
                assert_eq!(q["target.type"], "Build");
                assert_eq!(q["target.id"], BUILD);
                let active = !q["status"].is_null();
                if active {
                    assert_eq!(q["operation"], "RunBuild");
                    assert_eq!(q["status"]["$in"], json!(["Queued", "InProgress"]));
                    d.active_reads += 1;
                    if d.replace_at_active_read == Some(d.active_reads) {
                        replace(&mut d);
                    }
                }
                let mut updates = d
                    .updates
                    .values()
                    .filter(|u| {
                        !active || (u["operation"] == "RunBuild" && u["status"] != "Complete")
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if active && d.incomplete_active {
                    updates = (0..100)
                        .map(|n| {
                            let mut u = update(
                                &d,
                                Operation::RunBuild,
                                UpdateStatus::InProgress,
                                false,
                                vec![],
                            );
                            u["_id"]["$oid"] = json!(format!("{:024x}", 0x500 + n));
                            u
                        })
                        .collect();
                }
                updates.sort_by(|a, b| b["start_ts"].as_i64().cmp(&a["start_ts"].as_i64()));
                let page = p["page"].as_u64().unwrap() as usize;
                let updates=updates.into_iter().skip(page*100).take(100).map(|u|json!({"id":u["_id"]["$oid"],"operation":u["operation"],"start_ts":u["start_ts"],"success":u["success"],"username":"fixture-operator","operator":u["operator"],"target":u["target"],"status":u["status"]})).collect::<Vec<_>>();
                Some(Json(json!({"next_page":if updates.len()==100 {Some(page+1)}else{None},"updates":updates})).into_response())
            }
            _ => None,
        };
        (reply, delay)
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    if let Some(reply) = reply {
        reply
    } else {
        forward(&s, "/read", headers, v).await
    }
}
async fn write(State(s): State<StateData>, headers: HeaderMap, Json(v): Json<Value>) -> Response {
    {
        let mut d = s.data.lock().unwrap();
        record(&mut d, &headers, "write", &v);
    }
    forward(&s, "/write", headers, v).await
}
async fn execute(State(s): State<StateData>, headers: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = s.data.lock().unwrap();
    record(&mut d, &headers, "execute", &v);
    let op = v["type"].as_str().unwrap();
    assert_eq!(v["params"], json!({"build":BUILD}));
    if matches!(d.ack, Ack::Denied) {
        return denied();
    }
    let mut u = if op == "RunBuild" {
        let busy = d.building;
        let mut logs = vec![];
        let terminal = !matches!(d.outcome, Outcome::Running) || busy;
        let success = matches!(d.outcome, Outcome::Success) && !busy;
        if busy {
            logs.push(Log::error("Build Busy", "build is already building".into()));
        } else {
            match d.outcome {
            Outcome::HookFailed=>logs.push(Log::error("Pre Build",format!("hook failed token={} https://user:{}@private.invalid/source",PRIVATE[2],PRIVATE[6]))),
            Outcome::BuildFailed=>logs.push(Log::error("Docker Build",format!("compiler failed token={}",PRIVATE[2]))),
            Outcome::PushFailed=>logs.push(Log {stage:"Docker Build".into(),stdout:"#10 naming to registry.invalid:5000/owned-fixture/fixture-image:1.2.3 done".into(),stderr:format!("#10 ERROR: failed to push registry.invalid:5000/owned-fixture/fixture-image:1.2.3 token={}",PRIVATE[6]),success:false,..Default::default()}),
            Outcome::Success=>logs.push(Log {stage:"Docker Build".into(),stdout:format!("#10 naming to registry.invalid:5000/owned-fixture/fixture-image:1.2.3 done\n#10 pushing manifest for registry.invalid:5000/owned-fixture/fixture-image:1.2.3@sha256:{} 0.1s done","a".repeat(64)),success:true,..Default::default()}),
            Outcome::Running=>{}
        }
        }
        let mut u = update(
            &d,
            Operation::RunBuild,
            if terminal {
                UpdateStatus::Complete
            } else {
                UpdateStatus::InProgress
            },
            success,
            logs,
        );
        let mut base = s.definitions.data.lock().unwrap();
        let cfg: BuildConfig =
            serde_json::from_value(base.builds[BUILD]["config"].clone()).unwrap();
        let mut version = cfg.version;
        if cfg.auto_increment_version {
            version.increment();
        }
        u["version"] = serde_json::to_value(version).unwrap();
        u["commit_hash"] = json!("cafed00");
        if success {
            base.builds.get_mut(BUILD).unwrap()["config"]["version"] =
                serde_json::to_value(version).unwrap();
        }
        if !busy {
            d.building = !terminal;
        }
        u
    } else {
        assert_eq!(op, "CancelBuild");
        if d.replace_on_cancel {
            replace(&mut d);
        }
        if d.cancel_finishes_original {
            for original in d
                .updates
                .values_mut()
                .filter(|u| u["operation"] == "RunBuild" && u["status"] != "Complete")
            {
                original["status"] = json!("Complete");
                original["success"] = json!(false);
                original["end_ts"] = json!(komodo_timestamp());
                original["logs"] = json!([Log::error(
                    "Build Cancelled",
                    "User cancelled build during image build step".into()
                )]);
            }
            d.building = false;
        }
        update(
            &d,
            Operation::CancelBuild,
            UpdateStatus::Complete,
            true,
            vec![Log::simple(
                "Cancel acknowledged",
                "Cancellation queued; original can still be running".into(),
            )],
        )
    };
    let id = u["_id"]["$oid"].as_str().unwrap().to_string();
    d.updates.insert(id, u.clone());
    if op == "CancelBuild" && d.fail_observation_after_cancel {
        d.fail_updates = true;
    }
    if matches!(d.ack, Ack::Lost) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "lost acknowledgement after acceptance",
        )
            .into_response();
    }
    // Core acknowledges before its asynchronous worker finishes.
    u["status"] = json!("Queued");
    u["logs"] = json!([]);
    u["end_ts"] = Value::Null;
    u.as_object_mut().unwrap().remove("version");
    u.as_object_mut().unwrap().remove("commit_hash");
    match d.ack {
        Ack::WrongTarget => u["target"] = json!({"type":"Build","id":"000000000000000000000099"}),
        Ack::WrongOperation => u["operation"] = json!("PullRepo"),
        Ack::MissingId => {
            u.as_object_mut().unwrap().remove("_id");
        }
        _ => {}
    }
    Json(u).into_response()
}
