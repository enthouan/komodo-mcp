mod support;

use std::{
    future::{pending, ready},
    time::{Duration, Instant},
};

use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    history::UpdateOutcome,
    wait::{self, WaitEnd, WaitInput},
};
use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use support::{
    KEY, SECRET, assert_safe,
    incident::{IncidentCore, Options as IncidentOptions, fixture, wire},
    operations::{Core, Options, Process, UpdateRead, call, close},
};
use tokio::{
    net::TcpListener,
    sync::Notify,
    time::{sleep, timeout},
};

const ID: &str = "100000000000000000000001";
fn update(status: &str, success: bool) -> Value {
    let mut value = wire()["update"].clone();
    value["_id"] = json!({"$oid":ID});
    value["status"] = json!(status);
    value["success"] = json!(success);
    value["end_ts"] = if status == "Complete" {
        json!(1735689605000_i64)
    } else {
        Value::Null
    };
    value
}
fn input(seconds: u8) -> WaitInput {
    WaitInput {
        id: ID.into(),
        timeout_seconds: seconds,
    }
}
fn reads(core: &Core) -> usize {
    core.assert_credentials();
    core.requests()
        .iter()
        .filter(|r| r.operation == "GetUpdate")
        .count()
}
fn data(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_safe(&serde_json::to_string(&result).unwrap());
    let value = result.structured_content.unwrap();
    assert!(serde_json::to_vec(&value).unwrap().len() <= wait::MAX_OUTPUT_BYTES);
    value
}
async fn first_read(core: &Core) {
    timeout(Duration::from_secs(3), async {
        while reads(core) == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn queued_running_and_final_success_field_reuse_history_semantics() {
    for success in [false, true] {
        let core = Core::start(Options {
            update_reads: vec![
                UpdateRead::Value(update("Queued", false)),
                UpdateRead::Value(update("InProgress", false)),
                UpdateRead::Value(update("Complete", success)),
            ],
            ..Default::default()
        })
        .await;
        let result = wait::wait_for_update(
            &core.client(false, Duration::from_secs(3)),
            input(5),
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(result.end, WaitEnd::Complete);
        assert_eq!(result.poll_count, 3);
        assert_eq!(
            result.latest_update.unwrap().summary.outcome,
            if success {
                UpdateOutcome::Succeeded
            } else {
                UpdateOutcome::Failed
            }
        );
        assert_eq!(reads(&core), 3);
        assert_eq!(core.executions(), 0);
    }
    for status in ["Queued", "InProgress"] {
        let core = Core::start(Options::default()).await;
        core.insert_update(update(status, false));
        let start = Instant::now();
        let result = wait::wait_for_update(
            &core.client(false, Duration::from_secs(3)),
            input(1),
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(result.end, WaitEnd::Deadline);
        assert_eq!(result.update_id, ID);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            result.latest_update.unwrap().summary.outcome,
            if status == "Queued" {
                UpdateOutcome::Queued
            } else {
                UpdateOutcome::Running
            }
        );
        assert!(result.observation_error.is_none());
        assert!(!result.remote_cancellation_requested);
    }
}

#[tokio::test]
async fn immediate_completion_and_escaped_failed_evidence_are_bounded() {
    let core = Core::start(Options::default()).await;
    let mut value = update("Complete", false);
    value["logs"] = json!((0..20).map(|i| json!({"stage":format!("failure {i}"),"success":false,"start_ts":1,"end_ts":2,"stdout":"sensitive-stored-file-marker","stderr":"\u{0001}".repeat(1024),"command":"sensitive-compose-marker"})).collect::<Vec<_>>());
    core.insert_update(value);
    let (process, session) = Process::start(&core, false).await;
    let tools = session.list_tools(None).await.unwrap().tools;
    let tool = tools.iter().find(|t| t.name == "wait_for_update").unwrap();
    let a = tool.annotations.as_ref().unwrap();
    assert_eq!(a.read_only_hint, Some(true));
    assert_eq!(a.idempotent_hint, Some(true));
    assert_eq!(a.destructive_hint, Some(false));
    let r = data(call(&session, "wait_for_update", json!({"id":ID})).await);
    assert_eq!(r["end"], "complete");
    assert_eq!(r["poll_count"], 1);
    assert_eq!(r["latest_update"]["outcome"], "failed");
    assert_eq!(r["latest_update"]["stages_truncated"], true);
    assert!(!r["latest_update"]["stages"].as_array().unwrap().is_empty());
    assert_eq!(core.executions(), 0);
    process.finish(session).await;
}

#[tokio::test]
async fn request_cap_and_minimum_interval_bound_a_stalled_operation() {
    let core = Core::start(Options::default()).await;
    core.insert_update(update("InProgress", false));
    let started = Instant::now();
    let result = wait::wait_for_update(
        &core.client(false, Duration::from_secs(2)),
        input(30),
        pending(),
    )
    .await
    .unwrap();
    assert_eq!(result.end, WaitEnd::PollLimit);
    assert_eq!(result.poll_count, wait::MAX_POLLS);
    assert_eq!(reads(&core), wait::MAX_POLLS);
    assert!(started.elapsed() >= wait::POLL_INTERVAL * (wait::MAX_POLLS as u32 - 1));
    assert!(started.elapsed() < Duration::from_secs(30));
    sleep(Duration::from_millis(1100)).await;
    assert_eq!(reads(&core), wait::MAX_POLLS);
    assert_eq!(core.executions(), 0);
}

#[tokio::test]
async fn failed_or_stalled_later_reads_keep_last_observation_and_id() {
    for reply in [
        UpdateRead::Status(403),
        UpdateRead::Status(401),
        UpdateRead::Status(500),
        UpdateRead::Delay(Duration::from_secs(3)),
    ] {
        let core = Core::start(Options {
            update_reads: vec![UpdateRead::Value(update("InProgress", false)), reply],
            ..Default::default()
        })
        .await;
        core.insert_update(update("InProgress", false));
        let result = wait::wait_for_update(
            &core.client(false, Duration::from_secs(5)),
            input(2),
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(result.update_id, ID);
        assert_eq!(result.poll_count, 2);
        assert_eq!(
            result.latest_update.unwrap().summary.outcome,
            UpdateOutcome::Running
        );
        assert!(matches!(
            result.end,
            WaitEnd::ObservationFailed | WaitEnd::Deadline
        ));
        if result.end == WaitEnd::ObservationFailed {
            assert!(result.observation_error.is_some());
        }
        assert_eq!(reads(&core), 2);
        assert_eq!(core.executions(), 0);
    }
}

#[tokio::test]
async fn missing_inaccessible_slow_and_unreachable_history_are_observation_gaps() {
    for (options, kind) in [
        (Options::default(), "not_found"),
        (
            Options {
                observation_status: Some(403),
                ..Default::default()
            },
            "permission_denied",
        ),
        (
            Options {
                observation_delay: Duration::from_secs(3),
                ..Default::default()
            },
            "deadline",
        ),
    ] {
        let core = Core::start(options).await;
        let (session, task) = core.session(false).await;
        let r = data(
            call(
                &session,
                "wait_for_update",
                json!({"id":ID,"timeout_seconds":1}),
            )
            .await,
        );
        assert_eq!(r["update_id"], ID);
        assert_eq!(r["latest_update"], Value::Null);
        if kind == "deadline" {
            assert_eq!(r["end"], "deadline");
        } else {
            assert_eq!(r["end"], "observation_failed");
            assert_eq!(r["observation_error"]["kind"], kind);
        }
        assert_eq!(reads(&core), 1);
        assert_eq!(core.executions(), 0);
        close(session, task).await;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = CoreClient::new(
        Config::parse(&address, KEY, SECRET).unwrap(),
        RequestPolicy {
            connect_timeout: Duration::from_millis(100),
            operation_timeout: Duration::from_millis(100),
        },
    )
    .unwrap();
    let result = wait::wait_for_update(&client, input(1), pending())
        .await
        .unwrap();
    assert_eq!(result.end, WaitEnd::ObservationFailed);
    assert_eq!(
        result.observation_error.unwrap().kind,
        komodo_mcp::error::ErrorKind::Unreachable
    );
    assert_eq!(result.update_id, ID);
}

#[tokio::test]
async fn cancellation_before_read_during_read_and_between_reads_stops_polling() {
    let core = Core::start(Options::default()).await;
    let result = wait::wait_for_update(
        &core.client(false, Duration::from_secs(3)),
        input(30),
        ready(()),
    )
    .await
    .unwrap();
    assert_eq!(result.end, WaitEnd::Cancelled);
    assert_eq!(result.poll_count, 0);
    assert_eq!(reads(&core), 0);
    for delay in [Duration::ZERO, Duration::from_secs(3)] {
        let core = Core::start(Options {
            observation_delay: delay,
            ..Default::default()
        })
        .await;
        core.insert_update(update("InProgress", false));
        let notify = Notify::new();
        let client = core.client(false, Duration::from_secs(5));
        let (result, ()) = tokio::join!(
            wait::wait_for_update(&client, input(30), notify.notified()),
            async {
                first_read(&core).await;
                sleep(Duration::from_millis(100)).await;
                notify.notify_one();
            }
        );
        let result = result.unwrap();
        assert_eq!(result.end, WaitEnd::Cancelled);
        assert_eq!(result.update_id, ID);
        assert_eq!(result.poll_count, 1);
        assert_eq!(result.latest_update.is_some(), delay.is_zero());
        assert!(!result.remote_cancellation_requested);
        sleep(Duration::from_millis(1100)).await;
        assert_eq!(reads(&core), 1);
        assert_eq!(core.executions(), 0);
    }
}

#[tokio::test]
async fn production_timeout_cancel_and_process_restart_resume_with_reads_only() {
    let core = Core::start(Options::default()).await;
    core.insert_update(update("InProgress", false));
    let (process, session) = Process::start(&core, false).await;
    let r = data(
        call(
            &session,
            "wait_for_update",
            json!({"id":ID,"timeout_seconds":1}),
        )
        .await,
    );
    assert_eq!(r["end"], "deadline");
    assert_eq!(r["update_id"], ID);
    let handle = session
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("wait_for_update").with_arguments(
                    json!({"id":ID,"timeout_seconds":30})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();

    timeout(Duration::from_secs(3), async {
        while reads(&core) < 2 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    handle
        .cancel(Some("stop local observation".into()))
        .await
        .unwrap();
    sleep(Duration::from_millis(1200)).await;
    assert_eq!(reads(&core), 2);
    process.finish(session).await;
    // Lose a different process while its wait is active, then resume using only
    // the durable Core ID, without access to either previous client session.
    let (process, session) = Process::start(&core, false).await;
    let handle = session
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("wait_for_update").with_arguments(
                    json!({"id":ID,"timeout_seconds":30})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(3), async {
        while reads(&core) < 3 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    process.interrupt(session).await;
    drop(handle);
    let stopped = reads(&core);
    sleep(Duration::from_millis(1100)).await;
    assert_eq!(reads(&core), stopped);
    core.insert_update(update("Complete", true));
    let (process, session) = Process::start(&core, false).await;
    let r = data(call(&session, "wait_for_update", json!({"id":ID})).await);
    assert_eq!(r["end"], "complete");
    assert_eq!(r["latest_update"]["outcome"], "succeeded");
    assert_eq!(r["poll_count"], 1);
    let u = data(call(&session, "get_update", json!({"id":ID})).await);
    assert_eq!(u["id"], ID);
    assert_eq!(core.executions(), 0);
    process.finish(session).await;
}

#[tokio::test]
async fn invalid_wait_arguments_do_not_read_and_upstream_success_is_not_health() {
    let core = Core::start(Options::default()).await;
    let (session, task) = core.session(false).await;
    let before = core.requests().len();
    for args in [
        json!({"id":"bad"}),
        json!({"id":ID,"timeout_seconds":0}),
        json!({"id":ID,"timeout_seconds":31}),
        json!({"id":ID,"timeout_seconds":"1"}),
        json!({"id":ID,"scope":"all"}),
    ] {
        assert_eq!(
            call(&session, "wait_for_update", args).await.is_error,
            Some(true)
        );
    }
    assert_eq!(core.requests().len(), before);
    close(session, task).await;
    let f = fixture();
    let mut value = wire()["update"].clone();
    value["status"] = json!("Complete");
    value["success"] = json!(true);
    let core = IncidentCore::start(IncidentOptions {
        overrides: std::collections::BTreeMap::from([("GetUpdate".into(), value)]),
        ..Default::default()
    })
    .await;
    let (process, session) = Process::start_address(&core.address, false).await;
    let r = data(call(&session, "wait_for_update", json!({"id":f["update"]["id"]})).await);
    assert_eq!(r["end"], "complete");
    assert_eq!(r["latest_update"]["outcome"], "succeeded");
    let diagnosis = data(call(&session, "stack_diagnose", json!({"id":f["stack"]["id"]})).await);
    assert_eq!(diagnosis["outcome"], "unhealthy");
    assert!(
        core.requests()
            .iter()
            .all(|r| !r.operation.starts_with("Deploy") && !r.operation.starts_with("Start"))
    );
    process.finish(session).await;
}
