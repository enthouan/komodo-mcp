use std::{
    collections::VecDeque,
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
type MockState = Arc<Mutex<(Steps, usize, bool)>>;
struct Mock {
    client: CoreClient,
    address: String,
    state: MockState,
    task: JoinHandle<()>,
}
impl Mock {
    async fn start(steps: Vec<(Value, StatusCode, Value)>) -> Self {
        Self::start_with_chunking(steps, false).await
    }
    async fn start_with_chunking(steps: Vec<(Value, StatusCode, Value)>, chunked: bool) -> Self {
        let state = Arc::new(Mutex::new((steps.into(), 0, chunked)));
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
            address,
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
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    assert_eq!(headers["x-api-key"], KEY);
    assert_eq!(headers["x-api-secret"], SECRET);
    let mut state = state.lock().unwrap();
    state.1 += 1;
    let (expected, status, body) = state.0.pop_front().expect("unexpected extra request");
    assert_eq!(request, expected);
    let mut response = (status, Json(body)).into_response();
    if state.2 {
        let body = std::mem::take(response.body_mut());
        *response.body_mut() = axum::body::Body::from_stream(body.into_data_stream());
        response.headers_mut().remove("content-length");
    }
    response
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

#[tokio::test]
async fn missing_lookup_matches_only_the_tagged_core_failure() {
    let params = json!({"id":incident()["update"]["id"]});
    let mock = Mock::start(vec![
        (
            json!({"type":"GetUpdate","params":params}),
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"no update exists with given id","trace":[KEY,SECRET]}),
        ),
        (
            json!({"type":"GetUpdate","params":params}),
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"proxy: no update exists with given id","trace":[KEY,SECRET]}),
        ),
    ])
    .await;
    for expected in [ErrorKind::NotFound, ErrorKind::Upstream] {
        let error = history::get_update(&mock.client, update_id())
            .await
            .unwrap_err();
        assert_eq!(error.kind, expected);
        safe(&error);
    }
    mock.done(2);
}

#[tokio::test]
async fn history_body_limits_apply_before_decode_to_fixed_and_chunked_responses() {
    let large = "x".repeat(komodo_mcp::client::MAX_EVIDENCE_RESPONSE_BYTES);
    for chunked in [false, true] {
        let fixture = wire();
        let mut alert = fixture["alerts"][0].clone();
        alert["data"]["data"]["name"] = json!(large);
        let mut update = fixture["updates"][0].clone();
        update["other_data"] = json!(large);
        let mut detail = fixture["update"].clone();
        detail["current_toml"] = json!(large);
        let mock = Mock::start_with_chunking(
            vec![
                step(
                    "ListAlerts",
                    query(0),
                    json!({"alerts":[alert],"next_page":null}),
                ),
                step(
                    "ListUpdates",
                    query(0),
                    json!({"updates":[update],"next_page":null}),
                ),
                step(
                    "GetUpdate",
                    json!({"id":incident()["update"]["id"]}),
                    detail,
                ),
            ],
            chunked,
        )
        .await;
        let errors = [
            history::list_alerts(&mock.client, stack())
                .await
                .unwrap_err(),
            history::list_updates(&mock.client, stack())
                .await
                .unwrap_err(),
            history::get_update(&mock.client, update_id())
                .await
                .unwrap_err(),
        ];
        for (error, operation) in errors
            .into_iter()
            .zip(["ListAlerts", "ListUpdates", "GetUpdate"])
        {
            assert_eq!(
                error.kind,
                ErrorKind::ResponseTooLarge,
                "{operation}, chunked={chunked}"
            );
            assert_eq!(error.operation, operation);
            safe(&error);
        }
        mock.done(3);
    }
}

#[tokio::test]
async fn unusable_overflow_continuation_is_rejected() {
    let page = u32::MAX - 1;
    let updates = vec![wire()["updates"][0].clone(); history::HISTORY_PAGE_SIZE];
    let mock = Mock::start(vec![step(
        "ListUpdates",
        query(page),
        json!({"updates":updates,"next_page":u32::MAX}),
    )])
    .await;
    let error = history::list_updates(&mock.client, HistoryInput { page, ..stack() })
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidResponse);
    mock.done(1);
}

#[tokio::test]
async fn oversized_stage_fields_are_hidden_and_explicitly_marked_truncated() {
    let mut detail = wire()["update"].clone();
    detail["logs"][0]["stderr"] = json!("x".repeat(komodo_mcp::logs::MAX_INPUT_BYTES + 1));
    let mock = Mock::start(vec![step(
        "GetUpdate",
        json!({"id":incident()["update"]["id"]}),
        detail,
    )])
    .await;
    let detail = history::get_update(&mock.client, update_id())
        .await
        .unwrap();
    assert_eq!(detail.stages[0].stderr, "[redacted]");
    assert!(detail.stages[0].stderr_truncated);
    safe(&detail);
    mock.done(1);
}

#[tokio::test]
async fn literal_system_targets_are_preserved_without_object_id_normalization() {
    let target = HistoryTarget {
        kind: ResourceKind::System,
        id: "SystemName".into(),
    };
    let mut update = wire()["updates"][0].clone();
    update["target"] = json!({"type":"System","id":target.id});
    let mock = Mock::start(vec![step(
        "ListUpdates",
        json!({"query":{"target.type":"System","target.id":target.id},"page":0}),
        json!({"updates":[update],"next_page":null}),
    )])
    .await;
    let result = history::list_updates(
        &mock.client,
        HistoryInput {
            target: Some(target.clone()),
            page: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.items[0].target, target);
    let empty = HistoryInput {
        target: Some(HistoryTarget {
            kind: ResourceKind::System,
            id: String::new(),
        }),
        page: 0,
    };
    assert_eq!(
        history::list_updates(&mock.client, empty)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArguments
    );
    mock.done(1);
}

#[tokio::test]
async fn stdio_links_incident_history_and_redacts_stage_names_and_stderr() {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    timeout(Duration::from_secs(15), async {
        let fixture = wire();
        let mut detail = fixture["update"].clone();
        let cases = [
            "dbPassword=incident-private-credential-marker\n",
            "Authorization:\n  Basic incident-private-credential-marker\n",
            r#"{"pass\u0077ord":"incident-private-credential-marker","neighbor":"keep"}"#,
            "clientSecret: |-\n  incident-private-credential-marker\n",
        ];
        detail["logs"] = json!(
            cases
                .iter()
                .map(|source| {
                    let mut log = detail["logs"][0].clone();
                    log["stage"] = json!(source);
                    log["stderr"] = json!(source);
                    log
                })
                .collect::<Vec<_>>()
        );
        let mock = Mock::start(vec![
            step("GetVersion", json!({}), json!({"version":"2.3.2"})),
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
                detail,
            ),
        ])
        .await;
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &mock.address)
            .env("KOMODO_API_KEY", KEY)
            .env("KOMODO_API_SECRET", SECRET)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut session = ClientConfig::new(
            Default::default(),
            Implementation::new("history-regression", "1"),
        )
        .serve_with_lifecycle(
            (child.stdout.take().unwrap(), child.stdin.take().unwrap()),
            ClientLifecycleMode::Initialize,
        )
        .await
        .unwrap();
        let mut reports = Vec::new();
        for (tool, arguments) in [
            (
                "list_alerts",
                json!({"target":{"kind":"stack","id":incident()["stack"]["id"]},"page":0}),
            ),
            (
                "list_updates",
                json!({"target":{"kind":"stack","id":incident()["stack"]["id"]},"page":0}),
            ),
            ("get_update", json!({"id":incident()["update"]["id"]})),
        ] {
            let result = session
                .call_tool(
                    CallToolRequestParams::new(tool)
                        .with_arguments(arguments.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false));
            let result = safe(&result);
            let text: Value =
                serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(result["structuredContent"], text);
            reports.push(text);
        }
        assert_eq!(reports[0]["items"][0]["id"], incident()["alert"]["id"]);
        assert_eq!(reports[1]["items"][0]["id"], reports[2]["id"]);
        assert_eq!(reports[2]["outcome"], "failed");
        for stage in reports[2]["stages"].as_array().unwrap() {
            for (field, limit) in [
                ("stage", history::MAX_STAGE_NAME_BYTES),
                ("stderr", history::MAX_STAGE_STDERR_BYTES),
            ] {
                let value = stage[field].as_str().unwrap();
                assert!(value.contains("[redacted]"));
                assert!(value.len() <= limit);
            }
        }
        session.close().await.unwrap();
        assert!(child.wait().await.unwrap().success());
        mock.done(4);
    })
    .await
    .unwrap();
}
