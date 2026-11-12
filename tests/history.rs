use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    error::ErrorKind,
    history::{
        self, HistoryInput, HistoryTarget, ResourceKind, UpdateInput, UpdateOutcome, UpdateStatus,
    },
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

const KEY: &str = "history-api-key-marker";
const SECRET: &str = "history-api-secret-marker";

fn wire() -> Value {
    serde_json::from_str(include_str!("fixtures/core-2.3.2/incident-wire.json")).unwrap()
}
fn incident() -> Value {
    serde_json::from_str(include_str!("fixtures/core-2.3.2/incident.json")).unwrap()
}
fn stack() -> HistoryInput {
    HistoryInput {
        target: Some(HistoryTarget {
            kind: ResourceKind::Stack,
            id: incident()["stack"]["id"].as_str().unwrap().into(),
        }),
        page: 0,
    }
}
fn update_id() -> UpdateInput {
    UpdateInput {
        id: incident()["update"]["id"].as_str().unwrap().into(),
    }
}
fn query(page: u32) -> Value {
    json!({"query":{"target.type":"Stack","target.id":incident()["stack"]["id"]},"page":page})
}
fn step(operation: &str, params: Value, body: Value) -> (Value, StatusCode, Value) {
    (
        json!({"type":operation,"params":params}),
        StatusCode::OK,
        body,
    )
}

type Steps = VecDeque<(Value, StatusCode, Value)>;
struct Mock {
    client: CoreClient,
    state: Arc<Mutex<(Steps, usize)>>,
    task: JoinHandle<()>,
}
impl Mock {
    async fn start(steps: Vec<(Value, StatusCode, Value)>) -> Self {
        let state = Arc::new(Mutex::new((steps.into(), 0)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/komodo/read", post(reply))
            .with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = CoreClient::new(
            Config::parse(&address, KEY, SECRET).unwrap(),
            RequestPolicy {
                connect_timeout: Duration::from_millis(100),
                operation_timeout: Duration::from_millis(500),
            },
        )
        .unwrap();
        Self {
            client,
            state,
            task,
        }
    }
    fn done(&self, expected: usize) {
        let state = self.state.lock().unwrap();
        assert!(state.0.is_empty(), "unexpected missing read");
        assert_eq!(state.1, expected, "unexpected extra read");
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn reply(
    State(state): State<Arc<Mutex<(Steps, usize)>>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(headers["x-api-key"], KEY);
    assert_eq!(headers["x-api-secret"], SECRET);
    let mut state = state.lock().unwrap();
    state.1 += 1;
    let (expected, status, body) = state.0.pop_front().expect("unexpected extra request");
    assert_eq!(request, expected);
    (status, Json(body))
}

fn safe(value: &impl serde::Serialize) -> Value {
    let value = serde_json::to_value(value).unwrap();
    let text = value.to_string();
    for marker in [
        KEY,
        SECRET,
        "incident-private-toml-marker",
        "incident-private-other-data-marker",
        "incident-private-credential-marker",
    ] {
        assert!(
            !text.contains(marker),
            "unexpected private field in projection"
        );
    }
    value
}

#[tokio::test]
async fn shared_incident_links_alert_update_and_bounded_failed_stage() {
    let fixture = wire();
    let mock = Mock::start(vec![
        step(
            "ListAlerts",
            query(0),
            json!({"alerts":fixture["alerts"],"next_page":null}),
        ),
        step(
            "ListUpdates",
            query(0),
            json!({"updates":fixture["updates"],"next_page":null}),
        ),
        step(
            "GetUpdate",
            json!({"id":incident()["update"]["id"]}),
            fixture["update"].clone(),
        ),
    ])
    .await;
    let alerts = history::list_alerts(&mock.client, stack()).await.unwrap();
    assert_eq!(alerts.items.len(), 1);
    assert_eq!(alerts.items[0].id, incident()["alert"]["id"]);
    assert_eq!(alerts.items[0].target, stack().target.unwrap());
    assert_eq!(alerts.items[0].variant, "StackStateChange");
    assert_eq!(alerts.items[0].level.to_ascii_uppercase(), "CRITICAL");
    assert_eq!(alerts.items[0].ts, incident()["alert"]["ts"]);
    let alert = safe(&alerts);
    assert!(alert["items"][0].get("data").is_none());
    let updates = history::list_updates(&mock.client, stack()).await.unwrap();
    assert_eq!(updates.items[0].id, incident()["update"]["id"]);
    assert_eq!(updates.items[0].operation, "DeployStack");
    assert_eq!(updates.items[0].outcome, UpdateOutcome::Failed);
    assert_eq!(updates.items[0].status, UpdateStatus::Complete);
    safe(&updates);
    let detail = history::get_update(&mock.client, update_id())
        .await
        .unwrap();
    assert_eq!(detail.summary.id, updates.items[0].id);
    assert_eq!(detail.summary.target, alerts.items[0].target);
    assert_eq!(detail.stages[0].stage, incident()["update"]["failed_stage"]);
    assert!(
        detail.stages[0]
            .stderr
            .contains(incident()["update"]["stage_stderr"].as_str().unwrap())
    );
    assert!(!detail.stages[0].success);
    let projected = safe(&detail);
    for field in [
        "current_toml",
        "prev_toml",
        "other_data",
        "operator",
        "logs",
    ] {
        assert!(projected.get(field).is_none());
    }
    for field in ["command", "stdout"] {
        assert!(projected["stages"][0].get(field).is_none());
    }
    mock.done(3);
}

#[tokio::test]
async fn queued_and_running_do_not_claim_a_final_outcome() {
    let fixture = wire();
    let mut records = Vec::new();
    for (index, (status, success)) in [
        ("Queued", true),
        ("InProgress", false),
        ("Complete", true),
        ("Complete", false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = fixture["updates"][0].clone();
        record["id"] = json!(format!("{:024x}", index + 16));
        record["status"] = json!(status);
        record["success"] = json!(success);
        records.push(record);
    }
    let mock = Mock::start(vec![step(
        "ListUpdates",
        query(0),
        json!({"updates":records,"next_page":null}),
    )])
    .await;
    let result = history::list_updates(&mock.client, stack()).await.unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.outcome)
            .collect::<Vec<_>>(),
        [
            UpdateOutcome::Queued,
            UpdateOutcome::Running,
            UpdateOutcome::Succeeded,
            UpdateOutcome::Failed
        ]
    );
    safe(&result);
    mock.done(1);
}

#[tokio::test]
async fn fixed_size_history_pages_continue_without_dropping_records_and_end_empty() {
    let fixture = wire();
    let updates = (0..100)
        .map(|index| {
            let mut record = fixture["updates"][0].clone();
            record["id"] = json!(format!("{:024x}", index + 32));
            record
        })
        .collect::<Vec<_>>();
    let alerts = (0..100)
        .map(|index| {
            let mut record = fixture["alerts"][0].clone();
            record["_id"] = json!({"$oid":format!("{:024x}", index + 32)});
            record
        })
        .collect::<Vec<_>>();
    let mock = Mock::start(vec![
        step(
            "ListUpdates",
            query(0),
            json!({"updates":updates,"next_page":1}),
        ),
        step(
            "ListUpdates",
            query(1),
            json!({"updates":[],"next_page":null}),
        ),
        step(
            "ListAlerts",
            query(0),
            json!({"alerts":alerts,"next_page":1}),
        ),
        step(
            "ListAlerts",
            query(1),
            json!({"alerts":[],"next_page":null}),
        ),
    ])
    .await;
    let first = history::list_updates(&mock.client, stack()).await.unwrap();
    assert_eq!(first.items.len(), 100);
    assert_eq!(first.next_page, Some(1));
    let next = HistoryInput {
        page: first.next_page.unwrap(),
        ..stack()
    };
    let last = history::list_updates(&mock.client, next).await.unwrap();
    assert!(last.items.is_empty());
    assert_eq!(last.next_page, None);
    let first = history::list_alerts(&mock.client, stack()).await.unwrap();
    assert_eq!(first.items.len(), 100);
    let next = HistoryInput {
        page: first.next_page.unwrap(),
        ..stack()
    };
    let last = history::list_alerts(&mock.client, next).await.unwrap();
    assert!(last.items.is_empty());
    assert_eq!(last.next_page, None);
    mock.done(4);
}

#[tokio::test]
async fn detail_prioritizes_late_failed_stages_and_redacts_before_utf8_clipping() {
    let mut detail = wire()["update"].clone();
    let mut stages = Vec::new();
    for index in 0..25 {
        let mut log = detail["logs"][0].clone();
        log["success"] = json!(true);
        log["stage"] = json!(format!("prepare {index}"));
        stages.push(log);
    }
    let mut failure = detail["logs"][0].clone();
    failure["stage"] = json!(format!("{}{}", SECRET, "é".repeat(200)));
    failure["stderr"] = json!(format!(
        "TOKEN=incident-private-credential-marker\n{KEY}\n{}",
        "é".repeat(2000)
    ));
    stages.push(failure);
    detail["logs"] = json!(stages);
    let mock = Mock::start(vec![step(
        "GetUpdate",
        json!({"id":incident()["update"]["id"]}),
        detail,
    )])
    .await;
    let detail = history::get_update(&mock.client, update_id())
        .await
        .unwrap();
    assert_eq!(detail.stages.len(), history::MAX_UPDATE_STAGES);
    assert!(detail.stages_truncated);
    assert_eq!(detail.stages[0].index, 25);
    assert!(!detail.stages[0].success);
    assert!(detail.stages[0].stage_truncated);
    assert!(detail.stages[0].stderr_truncated);
    for stage in &detail.stages {
        assert!(stage.stage.len() <= history::MAX_STAGE_NAME_BYTES);
        assert!(stage.stderr.len() <= history::MAX_STAGE_STDERR_BYTES);
    }
    assert!(serde_json::to_vec(&safe(&detail)).unwrap().len() < 40_000);
    mock.done(1);
}

#[tokio::test]
async fn invalid_arguments_make_no_core_requests() {
    let mock = Mock::start(vec![]).await;
    let mut target = stack();
    target.target.as_mut().unwrap().id = "bad-id".into();
    for input in [
        HistoryInput {
            page: u32::MAX,
            ..stack()
        },
        target,
    ] {
        assert_eq!(
            history::list_alerts(&mock.client, input.clone())
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArguments
        );
        assert_eq!(
            history::list_updates(&mock.client, input)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArguments
        );
    }
    assert_eq!(
        history::get_update(
            &mock.client,
            UpdateInput {
                id: "bad-id".into()
            }
        )
        .await
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArguments
    );
    assert!(serde_json::from_value::<HistoryInput>(json!({"query":{"$where":"bad"}})).is_err());
    mock.done(0);
}

#[tokio::test]
async fn incompatible_pages_targets_and_lookup_identity_are_rejected() {
    let fixture = wire();
    let mut wrong = fixture["updates"][0].clone();
    wrong["target"]["id"] = json!("000000000000000000000004");
    let oversized = vec![fixture["updates"][0].clone(); 101];
    let mut detail = fixture["update"].clone();
    detail["_id"]["$oid"] = json!("100000000000000000000002");
    let mock = Mock::start(vec![
        step(
            "ListUpdates",
            query(0),
            json!({"updates":oversized,"next_page":null}),
        ),
        step(
            "ListUpdates",
            query(0),
            json!({"updates":fixture["updates"],"next_page":0}),
        ),
        step(
            "ListUpdates",
            query(0),
            json!({"updates":[wrong],"next_page":null}),
        ),
        step(
            "ListAlerts",
            query(0),
            json!({"alerts":fixture["alerts"],"next_page":-1}),
        ),
        step(
            "GetUpdate",
            json!({"id":incident()["update"]["id"]}),
            detail,
        ),
    ])
    .await;
    for _ in 0..3 {
        assert_eq!(
            history::list_updates(&mock.client, stack())
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidResponse
        );
    }
    assert_eq!(
        history::list_alerts(&mock.client, stack())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidResponse
    );
    assert_eq!(
        history::get_update(&mock.client, update_id())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidResponse
    );
    mock.done(5);
}

#[tokio::test]
async fn failed_read_returns_safe_error_without_fallback_or_retry() {
    let expected = json!({"type":"ListUpdates","params":query(0)});
    let mock = Mock::start(vec![(expected, StatusCode::INTERNAL_SERVER_ERROR, json!({"error":format!("private {KEY} {SECRET}"),"trace":["incident-private-other-data-marker"]}))]).await;
    let error = history::list_updates(&mock.client, stack())
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Upstream);
    safe(&error);
    mock.done(1);
}
