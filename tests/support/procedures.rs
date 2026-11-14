//! Owned pinned-wire fixture; actual Core authorization is covered by live smoke.
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
pub use definitions::{BUILD, DEPLOYMENT, REPO};
use komodo_client::entities::{
    Operation, ResourceTarget, komodo_timestamp,
    procedure::{EnabledExecution, Procedure, ProcedureConfig, ProcedureStage},
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
pub const PROCEDURE: &str = "000000000000000000000030";
pub const ACTION: &str = "000000000000000000000040";
pub const CHILD: &str = "0000000000000000000000c1";
pub const PRIVATE: [&str; 9] = [
    definitions::PRIVATE[0],
    definitions::PRIVATE[1],
    definitions::PRIVATE[2],
    definitions::PRIVATE[3],
    definitions::PRIVATE[4],
    definitions::PRIVATE[5],
    definitions::PRIVATE[6],
    "procedure-private-arguments",
    "procedure-private-webhook",
];
#[derive(Clone, Copy, Default)]
pub enum Outcome {
    #[default]
    Success,
    PartialFailure,
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
#[derive(Clone, Copy, Default)]
pub enum Fault {
    #[default]
    None,
    Denied,
    Lost,
    MissingEcho,
    ChangeUnrelated,
    ReadbackDenied,
}
pub struct Data {
    pub procedures: BTreeMap<String, Value>,
    pub action: Value,
    pub updates: BTreeMap<String, Value>,
    pub requests: Vec<Request>,
    pub outcome: Outcome,
    pub ack: Ack,
    pub fault: Fault,
    pub admin: bool,
    pub editor_execute: bool,
    pub can_create: bool,
    pub running: bool,
    pub fail_updates: bool,
    pub update_delay: Duration,
    pub cancel_finishes_original: bool,
    pub cancel_original_succeeds: bool,
    pub replace_on_cancel: bool,
    pub replace_at_active_read: Option<usize>,
    pub active_reads: usize,
    pub incomplete_active: bool,
    pub deny_procedure_read: bool,
    pub fail_observation_after_cancel: bool,
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
fn execution(v: Value) -> komodo_client::api::execute::Execution {
    serde_json::from_value(v).unwrap()
}
pub fn saved() -> Value {
    serde_json::to_value(Procedure{id:PROCEDURE.into(),name:"fixture-procedure".into(),config:ProcedureConfig {
        stages:vec![
            ProcedureStage{name:"prepare".into(),enabled:true,executions:vec![EnabledExecution{enabled:true,execution:execution(json!({"type":"Sleep","params":{"duration_ms":1}}))},EnabledExecution{enabled:false,execution:execution(json!({"type":"RunAction","params":{"action":ACTION,"args":{"private":PRIVATE[7]}}}))}]},
            ProcedureStage{name:"parallel".into(),enabled:true,executions:vec![EnabledExecution{enabled:true,execution:execution(json!({"type":"RunBuild","params":{"build":BUILD}}))},EnabledExecution{enabled:true,execution:execution(json!({"type":"RunAction","params":{"action":ACTION,"args":{"private":PRIVATE[7]}}}))}]},
            ProcedureStage{name:"disabled-stage".into(),enabled:false,executions:vec![EnabledExecution{enabled:true,execution:execution(json!({"type":"PullRepo","params":{"repo":REPO}}))}]},
            ProcedureStage{name:"deploy".into(),enabled:true,executions:vec![EnabledExecution{enabled:true,execution:execution(json!({"type":"Deploy","params":{"deployment":DEPLOYMENT}}))}]},
        ],webhook_secret:PRIVATE[8].into(),schedule:"0 */5 * * * *".into(),schedule_enabled:true,schedule_timezone:"Europe/London".into(),..Default::default()
    },..Default::default()}).unwrap()
}
impl Core {
    pub async fn start() -> Self {
        let definitions = Arc::new(definitions::Core::start().await);
        let data = Arc::new(Mutex::new(Data {
            procedures: BTreeMap::from([(PROCEDURE.into(), saved())]),
            action: json!({"_id":{"$oid":ACTION},"name":"fixture-action","template":false,"config":{"code":PRIVATE[7]}}),
            updates: BTreeMap::new(),
            requests: vec![],
            outcome: Outcome::Success,
            ack: Ack::Valid,
            fault: Fault::None,
            admin: true,
            editor_execute: true,
            can_create: true,
            running: false,
            fail_updates: false,
            update_delay: Duration::ZERO,
            cancel_finishes_original: false,
            cancel_original_succeeds: false,
            replace_on_cancel: false,
            replace_at_active_read: None,
            active_reads: 0,
            incomplete_active: false,
            deny_procedure_read: false,
            fail_observation_after_cancel: false,
            bad_credentials: false,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/read", post(read))
            .route("/write", post(write))
            .route("/execute", post(execute))
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
    pub fn procedure(&self) -> Value {
        self.data.lock().unwrap().procedures[PROCEDURE].clone()
    }
    pub fn seed_run(&self, status: UpdateStatus) -> String {
        let mut d = self.data.lock().unwrap();
        let u = update(&d, Operation::RunProcedure, status, false, vec![]);
        let id = u["_id"]["$oid"].as_str().unwrap().to_owned();
        d.running = status != UpdateStatus::Complete;
        d.updates.insert(id.clone(), u);
        id
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
fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":format!("denied key={KEY} secret={SECRET}")})),
    )
        .into_response()
}
fn listed(v: &Value) -> Value {
    let value = json!({"id":v["_id"]["$oid"],"name":v["name"],"template":v["template"].as_bool().unwrap_or(false),"tags":[],"type":"Procedure","info":{"stages":v["config"]["stages"].as_array().unwrap().len(),"state":"Unknown","last_run_at":null,"next_scheduled_run":null,"schedule_error":null}});
    let item: komodo_client::entities::procedure::ProcedureListItem =
        serde_json::from_value(value).expect("pinned Procedure list fixture");
    serde_json::to_value(item).unwrap()
}
async fn forward(s: &StateData, path: &str, h: HeaderMap, v: Value) -> Response {
    let r = s
        .http
        .post(format!("{}{path}", s.definitions.address))
        .headers(h)
        .json(&v)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(r.status().as_u16()).unwrap();
    let body = r.bytes().await.unwrap();
    (status, body).into_response()
}
async fn read(State(s): State<StateData>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let (reply, delay) = {
        let mut d = s.data.lock().unwrap();
        record(&mut d, &h, "read", &v);
        let p = &v["params"];
        let operation = v["type"].as_str().unwrap();
        let delay = if operation == "GetUpdate" {
            d.update_delay
        } else {
            Duration::ZERO
        };
        let reply = match operation {
            "ListProcedures" => {
                let names = p["query"]["names"].as_array().unwrap();
                let terms = p["query"]["terms"].as_array().unwrap();
                assert_eq!(p["sort_desc"], false);
                let limit = p["limit"].as_u64().unwrap() as usize;
                assert!((1..=100).contains(&limit));
                let mut values = d
                    .procedures
                    .values()
                    .filter(|v| {
                        (names.is_empty()
                            || names
                                .iter()
                                .any(|n| n == &v["name"] || n == &v["_id"]["$oid"]))
                            && terms
                                .iter()
                                .all(|t| v["name"].as_str().unwrap().contains(t.as_str().unwrap()))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                values.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Some(
                    Json(Value::Array(
                        values
                            .iter()
                            .skip(p["page"].as_u64().unwrap() as usize * limit)
                            .take(limit)
                            .map(listed)
                            .collect(),
                    ))
                    .into_response(),
                )
            }
            "GetProcedure" => {
                if d.deny_procedure_read {
                    Some(denied())
                } else {
                    Some(
                        d.procedures
                            .get(p["procedure"].as_str().unwrap())
                            .map_or_else(
                                || (StatusCode::NOT_FOUND, "missing procedure").into_response(),
                                |r| Json(r.clone()).into_response(),
                            ),
                    )
                }
            }
            "GetAction" => Some(Json(d.action.clone()).into_response()),
            "GetProcedureActionState" => {
                assert_eq!(p["procedure"], PROCEDURE);
                Some(Json(json!({"running":d.running})).into_response())
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
                assert_eq!(q["target.type"], "Procedure");
                assert_eq!(q["target.id"], PROCEDURE);
                let active = !q["status"].is_null();
                if active {
                    assert_eq!(q["operation"], "RunProcedure");
                    assert_eq!(q["status"]["$in"], json!(["Queued", "InProgress"]));
                    d.active_reads += 1;
                    if d.replace_at_active_read == Some(d.active_reads) {
                        replace(&mut d);
                    }
                }
                let mut values = d
                    .updates
                    .values()
                    .filter(|u| {
                        u["target"]["id"] == PROCEDURE
                            && (!active
                                || (u["operation"] == "RunProcedure" && u["status"] != "Complete"))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if active && d.incomplete_active {
                    values = (0..100)
                        .map(|n| {
                            let mut u = update(
                                &d,
                                Operation::RunProcedure,
                                UpdateStatus::InProgress,
                                false,
                                vec![],
                            );
                            u["_id"]["$oid"] = json!(format!("{:024x}", 0x500 + n));
                            u
                        })
                        .collect();
                }
                values.sort_by(|a, b| b["start_ts"].as_i64().cmp(&a["start_ts"].as_i64()));
                let page = p["page"].as_u64().unwrap() as usize;
                let values=values.into_iter().skip(page*100).take(100).map(|u|json!({"id":u["_id"]["$oid"],"operation":u["operation"],"start_ts":u["start_ts"],"success":u["success"],"username":"fixture-operator","operator":u["operator"],"target":u["target"],"status":u["status"]})).collect::<Vec<_>>();
                Some(Json(json!({"next_page":if values.len()==100{Some(page+1)}else{None},"updates":values})).into_response())
            }
            _ => None,
        };
        (reply, delay)
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    if let Some(r) = reply {
        r
    } else {
        forward(&s, "/read", h, v).await
    }
}
async fn write(State(s): State<StateData>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = s.data.lock().unwrap();
    record(&mut d, &h, "write", &v);
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    assert!(matches!(op, "CreateProcedure" | "UpdateProcedure"));
    let patch = p["config"].as_object().unwrap();
    if matches!(d.fault, Fault::Denied) || (op == "CreateProcedure" && !d.can_create) {
        return denied();
    }
    // Core validates all supplied executions, including disabled entries.
    if let Some(stages) = patch.get("stages") {
        for e in stages
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["executions"].as_array().unwrap())
        {
            let kind = e["execution"]["type"].as_str().unwrap();
            if (!d.admin
                && (kind.starts_with("Batch")
                    || matches!(kind, "GlobalAutoUpdate" | "ClearRepoCache")))
                || (!d.editor_execute && !matches!(kind, "None" | "Sleep"))
            {
                return denied();
            }
        }
    }
    let id = if op == "CreateProcedure" {
        format!("{:024x}", 0x30 + d.procedures.len())
    } else {
        p["id"].as_str().unwrap().to_owned()
    };
    let mut value = if op == "CreateProcedure" {
        serde_json::to_value(Procedure {
            id: id.clone(),
            name: p["name"].as_str().unwrap().into(),
            ..Default::default()
        })
        .unwrap()
    } else {
        d.procedures[&id].clone()
    };
    for (k, x) in patch {
        value["config"][k] = x.clone();
    }
    let canonical: Procedure = serde_json::from_value(value).unwrap();
    let value = serde_json::to_value(canonical).unwrap();
    d.procedures.insert(id, value.clone());
    let mut reply = value;
    match d.fault {
        Fault::Lost => {
            return (StatusCode::SERVICE_UNAVAILABLE, "reply lost after save").into_response();
        }
        Fault::MissingEcho => {
            reply["config"]
                .as_object_mut()
                .unwrap()
                .remove(patch.keys().next().unwrap());
        }
        Fault::ChangeUnrelated => {
            reply["config"]["webhook_secret"] = json!("changed");
            d.procedures
                .get_mut(reply["_id"]["$oid"].as_str().unwrap())
                .unwrap()["config"]["webhook_secret"] = json!("changed");
        }
        Fault::ReadbackDenied => d.deny_procedure_read = true,
        _ => {}
    }
    Json(reply).into_response()
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
        target: ResourceTarget::Procedure(PROCEDURE.into()),
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
        .filter(|u| u["operation"] == "RunProcedure" && u["status"] != "Complete")
    {
        u["status"] = json!("Complete");
        u["success"] = json!(true);
    }
    let u = update(
        d,
        Operation::RunProcedure,
        UpdateStatus::InProgress,
        false,
        vec![],
    );
    let id = u["_id"]["$oid"].as_str().unwrap().to_owned();
    d.updates.insert(id, u);
    d.running = true;
}
fn span(kind: &str, text: &str) -> String {
    format!(
        "<span style=\"{}\">{text}</span>",
        if kind == "bold" {
            "font-weight: bolder"
        } else if kind == "green" {
            "color: var(--mantine-color-green-6)"
        } else if kind == "red" {
            "color: var(--mantine-color-red-6)"
        } else {
            "color: var(--mantine-color-dimmed)"
        }
    )
}
fn parent_logs(failed: bool, running: bool) -> Vec<Log> {
    let info = span("dim", "INFO");
    let mut lines = vec![
        format!(
            "{info}: executing procedure '{}'",
            span("bold", "fixture-procedure")
        ),
        format!("{info}: Executing stage: '{}'", span("bold", "prepare")),
        format!("{info}: Executing: Sleep(Sleep {{ duration_ms: 1 }})"),
        format!(
            "{info}: {} execution in 1ms: Sleep(Sleep {{ duration_ms: 1 }})",
            span("green", "Finished")
        ),
        format!(
            "{info}: {} stage '{}' execution in 1ms",
            span("green", "Finished"),
            span("bold", "prepare")
        ),
        format!("{info}: Executing stage: '{}'", span("bold", "parallel")),
        format!("{info}: Executing: RunBuild(RunBuild {{ build: \"{BUILD}\" }})"),
        format!(
            "{info}: Executing: RunAction(RunAction {{ action: \"{ACTION}\", args: Some({{\"private\": String(\"{}\")}}) }})",
            PRIVATE[7]
        ),
    ];
    if !running {
        lines.push(format!("{info}: {} execution in 4ms: RunAction(RunAction {{ action: \"{ACTION}\", args: Some({{\"private\": String(\"{}\")}}) }})",span("green","Finished"),PRIVATE[7]));
        lines.push(format!(
            "{info}: {} execution in 8ms: RunBuild(RunBuild {{ build: \"{BUILD}\" }})",
            span("green", "Finished")
        ));
        if !failed {
            lines.push(format!(
                "{info}: {} stage '{}' execution in 8ms",
                span("green", "Finished"),
                span("bold", "parallel")
            ));
            lines.push(format!(
                "{info}: Executing stage: '{}'",
                span("bold", "deploy")
            ));
            lines.push(format!("{info}: Executing: Deploy(Deploy {{ deployment: \"{DEPLOYMENT}\", stop_signal: None, stop_time: None }})"));
            lines.push(format!(
                "{info}: {} execution in 3ms: Deploy(Deploy {{ deployment: \"{DEPLOYMENT}\" }})",
                span("green", "Finished")
            ));
            lines.push(format!(
                "{info}: {} stage '{}' execution in 3ms",
                span("green", "Finished"),
                span("bold", "deploy")
            ));
        }
    }
    let mut logs = vec![Log::simple("Execute procedure", lines.join("\n"))];
    if failed {
        logs.push(Log::error("Execution error",format!("{}: Failed stage '{}' execution after 8ms\n\n{}:\n\t{}: Failed on RunAction(RunAction {{ action: \"{ACTION}\", args: Some({{\"private\": String(\"{}\")}}) }})\n\t{}: {}: Execution not successful, see update: 'http://core.invalid/updates/{CHILD}'",span("red","ERROR"),span("bold","parallel"),span("dim","TRACE"),span("dim","1"),PRIVATE[7],span("dim","2"),span("red","ERROR"))));
    } else if !running {
        logs.push(Log::simple(
            "Execution ok",
            "Procedure completed with no errors".into(),
        ));
    }
    logs
}
async fn execute(State(s): State<StateData>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = s.data.lock().unwrap();
    record(&mut d, &h, "execute", &v);
    let op = v["type"].as_str().unwrap();
    assert_eq!(v["params"]["procedure"], PROCEDURE);
    if matches!(d.ack, Ack::Denied) {
        return denied();
    }
    let mut u = if op == "RunProcedure" {
        let busy = d.running;
        let running = matches!(d.outcome, Outcome::Running) && !busy;
        let failed = matches!(d.outcome, Outcome::PartialFailure) || busy;
        let logs = if busy {
            vec![Log::error(
                "Task Error",
                "Procedure is already running".into(),
            )]
        } else {
            parent_logs(failed, running)
        };
        if failed && !busy {
            let child = serde_json::to_value(Update {
                id: CHILD.into(),
                operation: Operation::RunAction,
                status: UpdateStatus::Complete,
                success: false,
                target: ResourceTarget::Action(ACTION.into()),
                operator: "000000000000000000000001".into(),
                logs: vec![Log::error("Script Error", "Action child failed".into())],
                ..Default::default()
            })
            .unwrap();
            d.updates.insert(CHILD.into(), child);
        }
        if !busy {
            d.running = running;
        }
        update(
            &d,
            Operation::RunProcedure,
            if running {
                UpdateStatus::InProgress
            } else {
                UpdateStatus::Complete
            },
            !failed && !running,
            logs,
        )
    } else {
        assert_eq!(op, "CancelProcedure");
        assert!(v["params"]["update_id"].is_null());
        if d.replace_on_cancel {
            replace(&mut d);
        }
        if d.cancel_finishes_original {
            let success = d.cancel_original_succeeds;
            for u in d
                .updates
                .values_mut()
                .filter(|u| u["operation"] == "RunProcedure" && u["status"] != "Complete")
            {
                u["status"] = json!("Complete");
                u["success"] = json!(success);
                if !success {
                    u["logs"] = json!([
                        Log::simple(
                            "Execute procedure",
                            format!(
                                "{}: Cancelled before stage: '{}'",
                                span("red", "ERROR"),
                                span("bold", "deploy")
                            )
                        ),
                        Log::error(
                            "Execution error",
                            "Procedure cancelled before completion".into()
                        )
                    ]);
                }
            }
            d.running = false;
        }
        let u = update(
            &d,
            Operation::CancelProcedure,
            UpdateStatus::Complete,
            true,
            vec![Log::simple(
                "Cancel Triggered",
                "Procedure exits after currently running stage completes".into(),
            )],
        );
        if d.fail_observation_after_cancel {
            d.fail_updates = true;
        }
        u
    };
    let id = u["_id"]["$oid"].as_str().unwrap().to_owned();
    d.updates.insert(id, u.clone());
    if matches!(d.ack, Ack::Lost) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "lost acknowledgement after acceptance",
        )
            .into_response();
    }
    u["status"] = json!("Queued");
    u["logs"] = json!([]);
    u["end_ts"] = Value::Null;
    match d.ack {
        Ack::WrongTarget => u["target"]["id"] = json!("000000000000000000000099"),
        Ack::WrongOperation => u["operation"] = json!("RunBuild"),
        Ack::MissingId => {
            u.as_object_mut().unwrap().remove("_id");
        }
        _ => {}
    }
    Json(u).into_response()
}
