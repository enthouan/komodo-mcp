//! Mutable Core 2.3.2 Repo wire fixture, shared across restarted stdio processes.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_client::entities::{
    Operation, ResourceTarget,
    repo::{Repo, RepoConfig, RepoListItemInfo},
    update::{Log, Update, UpdateStatus},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::support::{
    KEY, SECRET,
    inventory::Family,
    operations::{Request, Task},
};

pub const REPO: &str = "00000000000000000000000a";
pub const SERVER: &str = "000000000000000000000001";
pub const PRIVATE: [&str; 4] = [
    "private-repo-account",
    "repo-hook-private",
    "repo-env-private",
    "repo-webhook-private",
];

#[derive(Clone, Copy, Default)]
pub enum WriteFault {
    #[default]
    None,
    Denied,
    Lost,
    MissingRequested,
    ChangeUnrelated,
    ReadBackDenied,
}
#[derive(Clone, Copy, Default)]
pub enum Ack {
    #[default]
    Valid,
    Lost,
    Denied,
    WrongTarget,
}
pub struct Data {
    pub repos: BTreeMap<String, Value>,
    pub server: Value,
    pub requests: Vec<Request>,
    pub updates: BTreeMap<String, Value>,
    pub write_fault: WriteFault,
    pub ack: Ack,
    pub hook_failure: bool,
    pub run_denied: bool,
    pub running: bool,
    pub fail_updates: bool,
    pub update_delay: Duration,
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
        let mut config = RepoConfig {
            server_id: SERVER.into(),
            repo: "fixture/workload".into(),
            branch: "main".into(),
            git_account: PRIVATE[0].into(),
            environment: format!("PRIVATE={}", PRIVATE[2]),
            webhook_secret: PRIVATE[3].into(),
            webhook_enabled: false,
            links: vec!["https://internal.example.invalid/private-link".into()],
            ..Default::default()
        };
        config.on_clone.command = format!("printf {}", PRIVATE[1]);
        config.on_pull.command = format!("printf {}", PRIVATE[1]);
        let repo = Repo {
            id: REPO.into(),
            name: "fixture-repo".into(),
            description: "private description".into(),
            config,
            ..Default::default()
        };
        let repo: Repo = serde_json::from_value(serde_json::to_value(repo).unwrap()).unwrap();
        let data = Arc::new(Mutex::new(Data {
            repos: BTreeMap::from([(REPO.into(), serde_json::to_value(repo).unwrap())]),
            server: Family::Server.detail(),
            requests: vec![],
            updates: BTreeMap::new(),
            write_fault: WriteFault::None,
            ack: Ack::Valid,
            hook_failure: false,
            run_denied: false,
            running: false,
            fail_updates: false,
            update_delay: Duration::ZERO,
            bad_credentials: false,
        }));
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
    pub fn saved(&self, id: &str) -> Value {
        self.data.lock().unwrap().repos[id].clone()
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
        Json(json!({"error":format!("permission denied {KEY} {SECRET}")})),
    )
        .into_response()
}
fn filtered(items: Vec<Value>, params: &Value) -> Vec<Value> {
    let names = params["query"]["names"].as_array().unwrap();
    let terms = params["query"]["terms"].as_array().unwrap();
    let mut items = items
        .into_iter()
        .filter(|v| {
            (names.is_empty() || names.iter().any(|n| n == &v["id"] || n == &v["name"]))
                && terms
                    .iter()
                    .all(|t| v["name"].as_str().unwrap().contains(t.as_str().unwrap()))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let limit = params["limit"].as_u64().unwrap() as usize;
    assert!((1..=100).contains(&limit));
    items
        .into_iter()
        .skip(params["page"].as_u64().unwrap() as usize * limit)
        .take(limit)
        .collect()
}
fn repo_item(v: &Value) -> Value {
    let c = &v["config"];
    let info = RepoListItemInfo {
        server_id: c["server_id"].as_str().unwrap().into(),
        repo: c["repo"].as_str().unwrap().into(),
        branch: c["branch"].as_str().unwrap().into(),
        git_provider: c["git_provider"].as_str().unwrap().into(),
        cloned_hash: Some("765abcd".into()),
        ..Default::default()
    };
    json!({"id":v["_id"]["$oid"],"name":v["name"],"type":"Repo","template":v["template"],"tags":[],"info":info})
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let op = v["type"].as_str().unwrap();
    let delay = {
        let mut d = data.lock().unwrap();
        record(&mut d, &headers, "read", &v);
        if op == "GetUpdate" {
            d.update_delay
        } else {
            Duration::ZERO
        }
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let d = data.lock().unwrap();
    let p = &v["params"];
    match op {
        "GetVersion" => Json(json!({"version":"2.3.2"})).into_response(),
        "ListRepos" => Json(filtered(d.repos.values().map(repo_item).collect(), p)).into_response(),
        "GetRepo" => {
            if matches!(d.write_fault, WriteFault::ReadBackDenied)
                && d.requests.iter().any(|r| r.path == "write")
            {
                return denied();
            }
            Json(d.repos[p["repo"].as_str().unwrap()].clone()).into_response()
        }
        "ListServers" => {
            let mut s = Family::Server.summary();
            s["name"] = d.server["name"].clone();
            s["id"] = d.server["_id"]["$oid"].clone();
            Json(filtered(vec![s], p)).into_response()
        }
        "GetServer" => Json(d.server.clone()).into_response(),
        "GetUpdate" => {
            if d.fail_updates {
                return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
            }
            Json(d.updates[p["id"].as_str().unwrap()].clone()).into_response()
        }
        "ListUpdates" => {
            assert_eq!(p["query"]["target.type"], "Repo");
            let mut updates = d
                .updates
                .values()
                .filter(|u| u["target"]["id"] == p["query"]["target.id"])
                .cloned()
                .collect::<Vec<_>>();
            updates.sort_by(|a, b| b["start_ts"].as_i64().cmp(&a["start_ts"].as_i64()));
            let page = p["page"].as_u64().unwrap() as usize;
            let updates=updates.into_iter().skip(page*100).take(100).map(|u|json!({"id":u["_id"]["$oid"],"operation":u["operation"],"start_ts":u["start_ts"],"target":u["target"],"status":u["status"],"success":u["success"],"username":"fixture-operator","operator":u["operator"]})).collect::<Vec<_>>();
            Json(json!({"next_page":if updates.len()==100 {Some(page+1)}else{None},"updates":updates})).into_response()
        }
        _ => panic!("unexpected read {op}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "write", &v);
    if matches!(d.write_fault, WriteFault::Denied) {
        return denied();
    }
    let p = &v["params"];
    let config = p["config"].as_object().unwrap();
    assert!(config.values().all(|v| !v.is_null()));
    let mut saved = if v["type"] == "CreateRepo" {
        let id = format!("{:024x}", 32 + d.repos.len());
        serde_json::to_value(Repo {
            id,
            name: p["name"].as_str().unwrap().into(),
            ..Default::default()
        })
        .unwrap()
    } else {
        assert_eq!(v["type"], "UpdateRepo");
        d.repos[p["id"].as_str().unwrap()].clone()
    };
    saved["config"]
        .as_object_mut()
        .unwrap()
        .extend(config.clone());
    if matches!(d.write_fault, WriteFault::ChangeUnrelated) {
        saved["config"]["webhook_secret"] = json!("changed-secret-in-upstream");
    }
    let id = saved["_id"]["$oid"].as_str().unwrap().to_string();
    d.repos.insert(id, saved.clone());
    match d.write_fault {
        WriteFault::Lost => (StatusCode::OK, "lost invalid acknowledgement").into_response(),
        WriteFault::MissingRequested => {
            saved["config"]
                .as_object_mut()
                .unwrap()
                .remove(config.keys().next().unwrap());
            Json(saved).into_response()
        }
        _ => Json(saved).into_response(),
    }
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "execute", &v);
    if matches!(d.ack, Ack::Denied) {
        return denied();
    }
    let id = format!("1{:023x}", d.updates.len() + 1);
    let op = match v["type"].as_str().unwrap() {
        "CloneRepo" => Operation::CloneRepo,
        "PullRepo" => Operation::PullRepo,
        other => panic!("unexpected {other}"),
    };
    let mut update = Update {
        id: id.clone(),
        target: ResourceTarget::Repo(v["params"]["repo"].as_str().unwrap().into()),
        operation: op,
        start_ts: 1000 + d.updates.len() as i64,
        status: UpdateStatus::Queued,
        success: false,
        operator: "same-test-operator".into(),
        ..Default::default()
    };
    let ack = serde_json::to_value(&update).unwrap();
    update.status = if d.running {
        UpdateStatus::InProgress
    } else {
        UpdateStatus::Complete
    };
    update.success = !d.hook_failure && !d.run_denied;
    update.end_ts = (!d.running).then_some(2000);
    update.logs.push(Log {
        stage: "Latest Commit".into(),
        stdout:
            "\u{1b}[2mhash:\u{1b}[0m \u{1b}[1ma1b2c3d\u{1b}[0m\nmessage: private commit message"
                .into(),
        success: true,
        ..Default::default()
    });
    if op == Operation::PullRepo {
        update.commit_hash = "e4f5a6b".into();
    }
    update.logs.push(Log{stage:if d.run_denied{"Task Error"}else{"On Pull"}.into(),success:!d.hook_failure && !d.run_denied,
        stderr:if d.hook_failure || d.run_denied {format!("{} token=stage-private-token key={KEY} secret={SECRET} https://user:git-private-password@source.invalid/fixture",if d.run_denied{"permission denied"}else{"hook failed"})}else{String::new()},
        ..Default::default()});
    d.updates.insert(id, serde_json::to_value(update).unwrap());
    match d.ack {
        Ack::Lost => Json(json!({})).into_response(),
        Ack::WrongTarget => {
            let mut a = ack;
            a["target"]["id"] = json!(SERVER);
            Json(a).into_response()
        }
        _ => Json(ack).into_response(),
    }
}
