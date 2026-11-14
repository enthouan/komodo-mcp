//! Deterministic owned Core wire fixture. Parser/timezone versions and algorithm
//! match Core 2.3.2; production MCP does not parse schedules. Live coverage is #93.
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
use chrono::{DateTime, Utc};
use croner::parser::{CronParser, Seconds};
use komodo_client::entities::{
    ResourceTarget, ScheduleFormat,
    action::{Action, ActionConfig},
    procedure::{Procedure, ProcedureConfig},
    schedule::{Schedule, ScheduleSortBy},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub const ACTION: &str = "000000000000000000000040";
pub const PROCEDURE: &str = "000000000000000000000030";
pub const PRIVATE: [&str; 4] = [
    "schedule-private-source",
    "schedule-private-defaults",
    "schedule-private-webhook",
    "schedule-private-step-args",
];
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
    pub parents: BTreeMap<(String, String), Value>,
    pub cache: BTreeMap<(String, String), Result<i64, String>>,
    pub requests: Vec<Request>,
    pub clock: DateTime<Utc>,
    pub core_timezone: String,
    pub last_run_at: Option<i64>,
    pub fault: Fault,
    pub deny_runtime: bool,
    pub deny_parent: bool,
    pub mismatch_runtime: bool,
    pub hide_runtime: bool,
    pub fillers: usize,
    pub duplicate: bool,
    pub conflicting_cache: bool,
    pub bad_credentials: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
fn key(kind: &str, id: &str) -> (String, String) {
    (kind.into(), id.into())
}
pub fn saved(kind: &str) -> Value {
    let mut value = if kind == "action" {
        serde_json::to_value(Action {
            id: ACTION.into(),
            name: "shared-owner".into(),
            config: ActionConfig {
                file_contents: format!("// {}\n", PRIVATE[0]),
                arguments: format!("PRIVATE={}\n", PRIVATE[1]),
                webhook_secret: PRIVATE[2].into(),
                run_at_startup: true,
                reload_deno_deps: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap()
    } else {
        let stages=serde_json::from_value(json!([{"name":"parallel","enabled":true,"executions":[{"enabled":false,"execution":{"type":"RunAction","params":{"action":ACTION,"args":{"private":PRIVATE[3]}}}},{"enabled":true,"execution":{"type":"Sleep","params":{"duration_ms":1}}}]}])).unwrap();
        serde_json::to_value(Procedure {
            id: PROCEDURE.into(),
            name: "shared-owner".into(),
            config: ProcedureConfig {
                stages,
                webhook_secret: PRIVATE[2].into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap()
    };
    value["config"]["schedule_format"] = json!("Cron");
    value["config"]["schedule"] = json!("0 0 9 * * *");
    value["config"]["schedule_enabled"] = json!(false);
    value["config"]["schedule_timezone"] = json!("Europe/London");
    value
}
pub fn occurrence(
    clock: DateTime<Utc>,
    expression: &str,
    format: &str,
    timezone: &str,
) -> Result<i64, String> {
    let expression = if format == "English" {
        english_to_cron::str_cron_syntax(expression)
            .map_err(|e| format!("Failed to parse English: {e:?}"))?
            .split(' ')
            .take(6)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        expression.into()
    };
    let cron = CronParser::builder()
        .seconds(Seconds::Required)
        .dom_and_dow(true)
        .build()
        .parse(&expression)
        .map_err(|e| format!("Invalid CRON schedule: {e}"))?;
    let tz: chrono_tz::Tz = timezone
        .parse()
        .map_err(|_| "Failed to parse timezone".to_owned())?;
    cron.find_next_occurrence(&clock.with_timezone(&tz), false)
        .map(|t| t.timestamp_millis())
        .map_err(|e| format!("Failed to find next run time: {e}"))
}
impl Data {
    pub fn refresh(&mut self, kind: &str, id: &str) {
        let k = key(kind, id);
        self.cache.remove(&k);
        let c = &self.parents[&k]["config"];
        if c["schedule_enabled"] != true || c["schedule"] == "" {
            return;
        }
        let timezone = c["schedule_timezone"].as_str().unwrap();
        let timezone = if timezone.is_empty() {
            self.core_timezone.as_str()
        } else {
            timezone
        };
        self.cache.insert(
            k,
            occurrence(
                self.clock,
                c["schedule"].as_str().unwrap(),
                c["schedule_format"].as_str().unwrap(),
                timezone,
            ),
        );
    }
}
impl Core {
    pub async fn start() -> Self {
        let data = Arc::new(Mutex::new(Data {
            parents: BTreeMap::from([
                (key("action", ACTION), saved("action")),
                (key("procedure", PROCEDURE), saved("procedure")),
            ]),
            cache: BTreeMap::new(),
            requests: vec![],
            clock: "2026-11-13T12:00:00Z".parse().unwrap(),
            core_timezone: "Asia/Tokyo".into(),
            last_run_at: Some(1794506400000),
            fault: Fault::None,
            deny_runtime: false,
            deny_parent: false,
            mismatch_runtime: false,
            hide_runtime: false,
            fillers: 0,
            duplicate: false,
            conflicting_cache: false,
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
    pub fn parent(&self, kind: &str) -> Value {
        self.data.lock().unwrap().parents
            [&key(kind, if kind == "action" { ACTION } else { PROCEDURE })]
            .clone()
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
fn rows(d: &Data) -> Vec<Schedule> {
    let mut values = d
        .parents
        .iter()
        .filter(|(_, p)| p["config"]["schedule"] != "")
        .map(|((kind, id), p)| {
            let c = &p["config"];
            let cached = d.cache.get(&key(kind, id));
            Schedule {
                target: if kind == "action" {
                    ResourceTarget::Action(id.clone())
                } else {
                    ResourceTarget::Procedure(id.clone())
                },
                name: p["name"].as_str().unwrap().into(),
                schedule: c["schedule"].as_str().unwrap().into(),
                schedule_format: if c["schedule_format"] == "Cron" {
                    ScheduleFormat::Cron
                } else {
                    ScheduleFormat::English
                },
                enabled: c["schedule_enabled"].as_bool().unwrap(),
                schedule_timezone: c["schedule_timezone"].as_str().unwrap().into(),
                next_scheduled_run: cached.and_then(|v| v.as_ref().ok()).copied(),
                schedule_error: cached.and_then(|v| v.as_ref().err()).cloned(),
                last_run_at: d.last_run_at,
                tags: vec![],
            }
        })
        .collect::<Vec<_>>();
    if d.hide_runtime {
        values.clear();
    }
    if d.mismatch_runtime {
        for s in &mut values {
            s.schedule = "0 0 10 * * *".into();
        }
    }
    if d.conflicting_cache {
        for s in &mut values {
            s.next_scheduled_run = Some(1);
            s.schedule_error = Some("conflicting cache".into());
        }
    }
    for n in 0..d.fillers {
        values.push(Schedule {
            target: ResourceTarget::Action(format!("{:024x}", 0x500 + n)),
            name: format!("a-shared-owner-filler-{n:05}"),
            schedule: "0 0 9 * * *".into(),
            schedule_format: ScheduleFormat::Cron,
            ..Default::default()
        });
    }
    values.sort_by(|a, b| a.name.cmp(&b.name));
    values
}
fn listed(kind: &str, id: &str, p: &Value, d: &Data) -> Value {
    let cached = d.cache.get(&key(kind, id));
    let mut info = json!({"state":"Unknown","last_run_at":d.last_run_at,"next_scheduled_run":cached.and_then(|v|v.as_ref().ok()),"schedule_error":cached.and_then(|v|v.as_ref().err())});
    if kind == "procedure" {
        info["stages"] = json!(p["config"]["stages"].as_array().unwrap().len());
    }
    json!({"type":if kind=="action"{"Action"}else{"Procedure"},"id":id,"name":p["name"],"template":p["template"].as_bool().unwrap_or(false),"tags":[],"info":info})
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &h, "read", &v);
    let p = &v["params"];
    match v["type"].as_str().unwrap() {
        "GetVersion" => Json(json!({"version":"2.3.2"})).into_response(),
        "ListActions" | "ListProcedures" => {
            let kind = if v["type"] == "ListActions" {
                "action"
            } else {
                "procedure"
            };
            let names = p["query"]["names"].as_array().unwrap();
            let terms = p["query"]["terms"].as_array().unwrap();
            let limit = p["limit"].as_u64().unwrap() as usize;
            let mut all = d
                .parents
                .iter()
                .filter(|((k, id), v)| {
                    k == kind
                        && (names.is_empty() || names.iter().any(|n| n == id || n == &v["name"]))
                        && terms
                            .iter()
                            .all(|t| v["name"].as_str().unwrap().contains(t.as_str().unwrap()))
                })
                .map(|((k, id), v)| listed(k, id, v, &d))
                .collect::<Vec<_>>();
            all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Json(json!(
                all.into_iter()
                    .skip(p["page"].as_u64().unwrap() as usize * limit)
                    .take(limit)
                    .collect::<Vec<_>>()
            ))
            .into_response()
        }
        "GetAction" | "GetProcedure" => {
            if d.deny_parent {
                return denied();
            }
            let kind = if v["type"] == "GetAction" {
                "action"
            } else {
                "procedure"
            };
            d.parents
                .get(&key(kind, p[kind].as_str().unwrap()))
                .map(|v| Json(v.clone()).into_response())
                .unwrap_or_else(|| (StatusCode::NOT_FOUND, "missing").into_response())
        }
        "ListSchedules" => {
            if d.deny_runtime {
                return denied();
            }
            assert_eq!(p["sort_by"], json!(ScheduleSortBy::Name));
            assert_eq!(p["sort_desc"], false);
            let limit = p["limit"].as_u64().unwrap();
            assert!((1..=100).contains(&limit));
            let mut all = rows(&d)
                .into_iter()
                .filter(|s| {
                    p["terms"].as_array().unwrap().iter().all(|t| {
                        s.name
                            .to_lowercase()
                            .contains(&t.as_str().unwrap().to_lowercase())
                    })
                })
                .skip((p["page"].as_u64().unwrap() * limit) as usize)
                .take(limit as usize)
                .collect::<Vec<_>>();
            if d.duplicate && !all.is_empty() {
                all.push(all[0].clone());
            }
            Json(json!(all)).into_response()
        }
        other => panic!("Unexpected read {other}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &h, "write", &v);
    let p = &v["params"];
    let kind = match v["type"].as_str().unwrap() {
        "UpdateAction" => "action",
        "UpdateProcedure" => "procedure",
        other => panic!("Unexpected write {other}"),
    };
    if matches!(d.fault, Fault::Denied) {
        return denied();
    }
    let id = p["id"].as_str().unwrap();
    let key = key(kind, id);
    let mut saved = d.parents[&key].clone();
    for (k, v) in p["config"].as_object().unwrap() {
        assert!(
            [
                "schedule",
                "schedule_format",
                "schedule_timezone",
                "schedule_enabled",
                "schedule_alert"
            ]
            .contains(&k.as_str())
        );
        saved["config"][k] = v.clone();
    }
    // Parse with the SDK as Core does; whole-parent comparison tests ensure that
    // no omitted source, stages, arguments or secrets were rewritten.
    saved = if kind == "action" {
        json!(serde_json::from_value::<Action>(saved).unwrap())
    } else {
        json!(serde_json::from_value::<Procedure>(saved).unwrap())
    };
    d.parents.insert(key.clone(), saved.clone());
    d.refresh(kind, id);
    match d.fault {
        Fault::Lost => return (StatusCode::SERVICE_UNAVAILABLE, "lost after save").into_response(),
        Fault::MissingEcho => {
            saved["config"]
                .as_object_mut()
                .unwrap()
                .remove(p["config"].as_object().unwrap().keys().next().unwrap());
        }
        Fault::ChangeUnrelated => {
            d.parents.get_mut(&key).unwrap()["config"]["webhook_secret"] =
                json!("concurrent private value");
            saved = d.parents[&key].clone();
        }
        Fault::ReadbackDenied => d.deny_parent = true,
        _ => {}
    }
    Json(saved).into_response()
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    record(&mut data.lock().unwrap(), &h, "execute", &v);
    (StatusCode::FORBIDDEN, "Schedule tools must not execute").into_response()
}
