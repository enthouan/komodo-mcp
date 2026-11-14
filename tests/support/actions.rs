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
use komodo_client::entities::{
    FileFormat, Operation, ResourceTarget,
    action::{Action, ActionConfig},
    komodo_timestamp,
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
pub const ACTION: &str = "000000000000000000000040";

pub const PRIVATE: [&str; 11] = [
    definitions::PRIVATE[0],
    definitions::PRIVATE[1],
    definitions::PRIVATE[2],
    definitions::PRIVATE[3],
    definitions::PRIVATE[4],
    definitions::PRIVATE[5],
    definitions::PRIVATE[6],
    "action-private-arguments",
    "action-private-webhook",
    "action-private-source",
    "action-private-per-run",
];
#[derive(Clone, Copy, Default)]
pub enum Outcome {
    #[default]
    Success,
    Failure,
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
    pub actions: BTreeMap<String, Value>,
    pub updates: BTreeMap<String, Value>,
    pub requests: Vec<Request>,
    pub outcome: Outcome,
    pub ack: Ack,
    pub fault: Fault,
    pub admin: bool,
    pub can_edit: bool,
    pub can_execute: bool,
    pub merged_arguments: Vec<Value>,
    pub executed_sources: Vec<String>,
    pub update_reads: usize,
    pub finish_at_update_read: Option<usize>,
    pub can_create: bool,
    pub fail_updates: bool,
    pub update_delay: Duration,
    pub cancel_finishes_original: bool,
    pub cancel_original_succeeds: bool,
    pub replace_on_cancel: bool,
    pub deny_action_read: bool,
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
pub fn saved() -> Value {
    serde_json::to_value(Action {
        id: ACTION.into(),
        name: "fixture-action".into(),
        config: ActionConfig {
            file_contents: format!("// {}\nconsole.log(ARGS);\n", PRIVATE[9]),
            arguments: format!("PRIVATE={}\nDEFAULT=retained\n", PRIVATE[7]),
            arguments_format: FileFormat::KeyValue,
            webhook_secret: PRIVATE[8].into(),
            schedule: "0 */5 * * * *".into(),
            schedule_enabled: true,
            schedule_timezone: "Europe/London".into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap()
}
impl Core {
    pub async fn start() -> Self {
        let definitions = Arc::new(definitions::Core::start().await);
        let data = Arc::new(Mutex::new(Data {
            actions: BTreeMap::from([(ACTION.into(), saved())]),
            updates: BTreeMap::new(),
            requests: vec![],
            outcome: Outcome::Success,
            ack: Ack::Valid,
            fault: Fault::None,
            admin: true,
            can_edit: true,
            can_execute: true,
            merged_arguments: vec![],
            executed_sources: vec![],
            update_reads: 0,
            finish_at_update_read: None,
            can_create: true,
            fail_updates: false,
            update_delay: Duration::ZERO,
            cancel_finishes_original: false,
            cancel_original_succeeds: false,
            replace_on_cancel: false,
            deny_action_read: false,
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
    pub fn action(&self) -> Value {
        self.data.lock().unwrap().actions[ACTION].clone()
    }
    pub fn seed_run(&self, status: UpdateStatus) -> String {
        let mut d = self.data.lock().unwrap();
        let u = update(&d, Operation::RunAction, status, false, vec![]);
        let id = u["_id"]["$oid"].as_str().unwrap().to_owned();
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
    let value = json!({"id":v["_id"]["$oid"],"name":v["name"],"template":v["template"].as_bool().unwrap_or(false),"tags":[],"type":"Action","info":{"state":"Unknown","last_run_at":null,"next_scheduled_run":null,"schedule_error":null}});
    let item: komodo_client::entities::action::ActionListItem =
        serde_json::from_value(value).expect("pinned Action list fixture");
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
            "ListActions" => {
                let names = p["query"]["names"].as_array().unwrap();
                let terms = p["query"]["terms"].as_array().unwrap();
                assert_eq!(p["sort_desc"], false);
                let limit = p["limit"].as_u64().unwrap() as usize;
                assert!((1..=100).contains(&limit));
                let mut values = d
                    .actions
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
            "GetAction" => {
                if d.deny_action_read {
                    Some(denied())
                } else {
                    Some(d.actions.get(p["action"].as_str().unwrap()).map_or_else(
                        || (StatusCode::NOT_FOUND, "missing action").into_response(),
                        |r| Json(r.clone()).into_response(),
                    ))
                }
            }
            "GetActionActionState" => {
                assert_eq!(p["action"], ACTION);
                Some(Json(json!({"running":d.updates.values().filter(|u|u["operation"]=="RunAction"&&u["status"]!="Complete").count()})).into_response())
            }
            "GetUpdate" => {
                d.update_reads += 1;
                if d.finish_at_update_read == Some(d.update_reads)
                    && let Some(u) = d.updates.get_mut(p["id"].as_str().unwrap())
                {
                    u["status"] = json!("Complete");
                    u["success"] = json!(true);
                }
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
                assert_eq!(q["target.type"], "Action");
                assert_eq!(q["target.id"], ACTION);
                assert!(
                    q["status"].is_null(),
                    "Action cancellation must not use resource-level history selection"
                );
                let mut values = d
                    .updates
                    .values()
                    .filter(|u| u["target"]["id"] == ACTION)
                    .cloned()
                    .collect::<Vec<_>>();
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
    assert!(matches!(op, "CreateAction" | "UpdateAction"));
    let patch = p["config"].as_object().unwrap();
    if matches!(d.fault, Fault::Denied) || (op == "CreateAction" && !d.can_create) {
        return denied();
    }
    if (op == "CreateAction" && !d.admin) || !d.can_edit {
        return denied();
    }
    let id = if op == "CreateAction" {
        format!("{:024x}", 0x40 + d.actions.len())
    } else {
        p["id"].as_str().unwrap().to_owned()
    };
    let mut value = if op == "CreateAction" {
        serde_json::to_value(Action {
            id: id.clone(),
            name: p["name"].as_str().unwrap().into(),
            ..Default::default()
        })
        .unwrap()
    } else {
        d.actions[&id].clone()
    };
    for (k, x) in patch {
        value["config"][k] = x.clone();
    }
    let canonical: Action = serde_json::from_value(value).unwrap();
    let value = serde_json::to_value(canonical).unwrap();
    d.actions.insert(id, value.clone());
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
            d.actions
                .get_mut(reply["_id"]["$oid"].as_str().unwrap())
                .unwrap()["config"]["webhook_secret"] = json!("changed");
        }
        Fault::ReadbackDenied => d.deny_action_read = true,
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
        target: ResourceTarget::Action(ACTION.into()),
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
        .filter(|u| u["operation"] == "RunAction" && u["status"] != "Complete")
    {
        u["status"] = json!("Complete");
        u["success"] = json!(true);
    }
    let u = update(
        d,
        Operation::RunAction,
        UpdateStatus::InProgress,
        false,
        vec![],
    );
    let id = u["_id"]["$oid"].as_str().unwrap().to_owned();
    d.updates.insert(id, u);
}
// Mirrors mogh_config 1.1.0 merge_objects(true,true), pinned Core dependency.
// Expectations are checked independently in the stdio tests; no MCP-side merge.
fn merge(target: &mut Value, source: Value) -> Result<(), ()> {
    let target = target.as_object_mut().ok_or(())?;
    for (k, v) in source.as_object().ok_or(())? {
        match target.get_mut(k) {
            Some(t @ Value::Object(_)) => merge(t, v.clone())?,
            Some(Value::Array(a)) => a.extend(v.as_array().ok_or(())?.iter().cloned()),
            _ => {
                target.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(())
}
fn defaults(v: &Value) -> Value {
    if v["arguments_format"] == "json" {
        serde_json::from_str(v["arguments"].as_str().unwrap()).unwrap()
    } else {
        let map = v["arguments"]
            .as_str()
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), json!(v)))
            .collect::<serde_json::Map<_, _>>();
        json!(map)
    }
}
async fn execute(State(s): State<StateData>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let mut d = s.data.lock().unwrap();
    record(&mut d, &h, "execute", &v);
    let op = v["type"].as_str().unwrap();
    assert_eq!(v["params"]["action"], ACTION);
    if matches!(d.ack, Ack::Denied) {
        return denied();
    }
    if !d.can_execute {
        return denied();
    }
    let mut u = if op == "RunAction" {
        let source = d.actions[ACTION]["config"]["file_contents"]
            .as_str()
            .unwrap()
            .to_owned();
        d.executed_sources.push(source);
        let mut merged = defaults(&d.actions[ACTION]["config"]);
        let args = v["params"]
            .get("args")
            .filter(|x| !x.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let valid = merge(&mut merged, args).is_ok();
        if valid {
            d.merged_arguments.push(merged);
        }
        let running = matches!(d.outcome, Outcome::Running) && valid;
        let failed = matches!(d.outcome, Outcome::Failure) || !valid;
        let logs = if !valid {
            vec![Log::error(
                "Task Error",
                format!("Failed to merge request args: {}", PRIVATE[10]),
            )]
        } else {
            vec![Log {
                stage: "Execute Action".into(),
                success: !failed,
                stdout: format!("plain {} {} ephemeral-api-secret", PRIVATE[7], PRIVATE[10]),
                stderr: if failed {
                    format!(
                        "unlabelled {} {} ephemeral-api-key",
                        PRIVATE[9], PRIVATE[10]
                    )
                } else {
                    String::new()
                },
                ..Default::default()
            }]
        };
        update(
            &d,
            Operation::RunAction,
            if running {
                UpdateStatus::InProgress
            } else {
                UpdateStatus::Complete
            },
            !failed && !running,
            logs,
        )
    } else {
        assert_eq!(op, "CancelAction");
        let id = v["params"]["update_id"]
            .as_str()
            .expect("explicit update ID required")
            .to_owned();
        // Core's cancellation token is keyed by this exact ID. Deliberately do
        // not validate Action association here: production MCP must enforce it.
        if d.replace_on_cancel {
            replace(&mut d);
        }
        let active = d
            .updates
            .get(&id)
            .is_some_and(|u| u["status"] != "Complete");
        if active && d.cancel_finishes_original {
            let succeeds = d.cancel_original_succeeds;
            let u = d.updates.get_mut(&id).unwrap();
            u["status"] = json!("Complete");
            u["success"] = json!(succeeds);
            if !succeeds {
                u["logs"] = json!([Log::error(
                    "Execute Action",
                    "Command cancelled (process group killed)".into()
                )]);
            }
        }
        let u = update(
            &d,
            Operation::CancelAction,
            UpdateStatus::Complete,
            active,
            vec![if active {
                Log::simple(
                    "Cancel Triggered",
                    "The action cancel has been triggered.".into(),
                )
            } else {
                Log::error(
                    "Task Error",
                    "No active cancellation token for this update ID".into(),
                )
            }],
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
