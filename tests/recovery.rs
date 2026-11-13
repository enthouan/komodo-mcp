mod support;

use std::{
    future::{pending, ready},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use komodo_mcp::{
    capabilities::CapabilityPolicy,
    client::{CoreClient, RequestPolicy},
    config::Config,
    recovery::{self, RecoveryInput, SearchEnd},
};
use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use support::{
    KEY, SECRET, assert_safe,
    incident::wire,
    operations::{
        Ack, Core, DEPLOYMENT, Options, Outcome, Process, STACK, UpdateRead, call, close,
    },
    recovery::{FaultProxy, LostReply},
};

#[derive(Clone)]
struct Case {
    tool: String,
    arguments: Value,
    target: Value,
    scope: Value,
    action: &'static str,
}
impl Case {
    fn context(&self, window: Value) -> Value {
        json!({"target":self.target,"requested_scope":self.scope,"operation":self.action,"submission_window":window})
    }
}
fn cases() -> Vec<Case> {
    let mut result = vec![];
    for action in ["deploy", "start", "stop", "restart"] {
        for scope in [
            json!({"kind":"whole_stack"}),
            json!({"kind":"service","service":"web"}),
        ] {
            result.push(Case {
                tool: format!("{action}_stack"),
                arguments: json!({"target":{"id":STACK},"scope":scope}),
                target: json!({"kind":"stack","id":STACK}),
                scope,
                action,
            });
        }
        result.push(Case {
            tool: format!("{action}_deployment"),
            arguments: json!({"id":DEPLOYMENT}),
            target: json!({"kind":"deployment","id":DEPLOYMENT}),
            scope: json!({"kind":"whole_deployment"}),
            action,
        });
    }
    result
}
fn value(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_safe(&serde_json::to_string(&result).unwrap());
    let result = result.structured_content.unwrap();
    assert!(serde_json::to_vec(&result).unwrap().len() <= recovery::MAX_OUTPUT_BYTES);
    result
}
fn unconfirmed(result: &Value) {
    assert_eq!(result["original_submission"], "unknown");
    assert_eq!(result["attribution"], "unconfirmed");
    assert_eq!(result["automatic_resubmission"], false);
    assert!(result.get("update_id").is_none());
    assert!(result.get("selected_update_id").is_none());
}
fn from_receipt(r: &Value) -> Value {
    json!({"target":{"kind":r["resource"]["kind"],"id":r["resource"]["id"]},"requested_scope":r["requested_scope"],"operation":r["operation"],"submission_window":r["submission_window"]})
}
fn client(address: &str, operate: bool) -> CoreClient {
    CoreClient::new(
        Config::parse(address, KEY, SECRET)
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(operate, false)),
        RequestPolicy {
            connect_timeout: Duration::from_secs(1),
            operation_timeout: Duration::from_secs(1),
        },
    )
    .unwrap()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn update(n: u128, operation: &str, service: Option<&str>, ts: i64) -> Value {
    let mut u = wire()["update"].clone();
    u["_id"] = json!({"$oid":format!("{n:024x}")});
    u["operation"] = json!(operation);
    u["start_ts"] = json!(ts);
    u["target"] = json!({"type":"Stack","id":STACK});
    u["status"] = json!("Complete");
    u["success"] = json!(true);
    u["operator"] = json!(format!("other-private-operator-{n}"));
    u["other_data"] = json!("sensitive-stored-file-marker");
    u["logs"] = service.map_or(json!([]), |s| json!([{"stage":"Service/s","success":true,"start_ts":1,"end_ts":2,"stdout":format!("Execution requested for Stack service/s {s}"),"stderr":"","command":"sensitive-compose-marker"}]));
    u
}

#[tokio::test]
async fn all_action_scopes_keep_unknown_after_accepted_disconnect_timeout_and_bad_ack() {
    for fault in 0..4 {
        for case in cases() {
            let core = Core::start(Options {
                outcome: Outcome::Succeeded,
                ack: match fault {
                    2 => Ack::Malformed,
                    3 => Ack::Oversized,
                    _ => Ack::Valid,
                },
                ..Default::default()
            })
            .await;
            let proxy = if fault < 2 {
                Some(
                    FaultProxy::start(
                        &core,
                        if fault == 0 {
                            LostReply::Disconnect
                        } else {
                            LostReply::Timeout
                        },
                    )
                    .await,
                )
            } else {
                None
            };
            let address = proxy
                .as_ref()
                .map_or(core.address.as_str(), |p| p.address.as_str());
            let (session, task) = Core::session_with_client(client(address, true)).await;
            let r = value(call(&session, &case.tool, case.arguments.clone()).await);
            assert_eq!(r["submission"], "unknown", "{} fault={fault}", case.tool);
            assert_eq!(r["update_id"], Value::Null);
            assert_eq!(r["resource"]["id"], case.target["id"]);
            assert_eq!(r["requested_scope"], case.scope);
            assert_eq!(r["operation"], case.action);
            assert!(r["submission_window"]["started_ts"].is_i64());
            assert!(r["submission_window"]["ended_ts"].is_i64());
            assert!(
                r["submission_error"]["correction"]
                    .as_str()
                    .unwrap()
                    .contains("reconcile_operation")
            );
            if let Some(proxy) = &proxy {
                proxy.accepted(1).await;
            }
            assert_eq!(core.executions(), 1);
            close(session, task).await;
            drop(proxy);
            let (fresh, task) = core.session(false).await;
            let recovered = value(call(&fresh, "reconcile_operation", from_receipt(&r)).await);
            unconfirmed(&recovered);
            assert_eq!(recovered["candidates"].as_array().unwrap().len(), 1);
            assert_eq!(
                recovered["candidates"][0]["update"]["id"],
                "100000000000000000000001"
            );
            assert_eq!(
                recovered["candidates"][0]["scope_evidence"],
                if case.scope["kind"] == "service" {
                    "named_service_matches"
                } else {
                    "whole_workload_variant"
                }
            );
            assert_eq!(core.executions(), 1);
            core.assert_credentials();
            close(fresh, task).await;
        }
    }
}

#[tokio::test]
async fn zero_one_multiple_other_operator_and_delayed_history_never_establish_attribution() {
    let case = cases().remove(1);
    let core = Core::start(Options {
        history_hidden: true,
        ..Default::default()
    })
    .await;
    core.insert_update(update(1, "DeployStackService", Some("web"), 1000));
    let (session, task) = core.session(false).await;
    let args = case.context(Value::Null);
    let r = value(call(&session, "reconcile_operation", args.clone()).await);
    unconfirmed(&r);
    assert!(r["candidates"].as_array().unwrap().is_empty());
    core.set_history_hidden(false);
    let r = value(call(&session, "reconcile_operation", args.clone()).await);
    unconfirmed(&r);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
    assert!(
        !serde_json::to_string(&r)
            .unwrap()
            .contains("other-private-operator")
    );
    core.insert_update(update(2, "DeployStackService", Some("web"), 2000));
    core.insert_update(update(3, "DeployStackService", Some("db"), 2000));
    core.insert_update(update(4, "RestartStackService", Some("web"), 2000));
    let r = value(call(&session, "reconcile_operation", args).await);
    unconfirmed(&r);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(r["excluded_by_scope"], 1);
    let r = value(
        call(
            &session,
            "reconcile_operation",
            case.context(json!({"started_ts":1500,"ended_ts":2500})),
        )
        .await,
    );
    unconfirmed(&r);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(r["candidates"][0]["update"]["id"], format!("{:024x}", 2));
    let r = value(
        call(
            &session,
            "reconcile_operation",
            case.context(json!({"started_ts":3000})),
        )
        .await,
    );
    unconfirmed(&r);
    assert!(r["candidates"].as_array().unwrap().is_empty());
    assert_eq!(core.executions(), 0);
    core.assert_credentials();
    close(session, task).await;
}

#[tokio::test]
async fn inaccessible_history_and_missing_or_ambiguous_service_evidence_remain_gaps() {
    let case = cases().remove(1);
    for status in [401, 403, 500] {
        let core = Core::start(Options {
            history_status: Some(status),
            ..Default::default()
        })
        .await;
        let (session, task) = core.session(false).await;
        let r = value(call(&session, "reconcile_operation", case.context(Value::Null)).await);
        unconfirmed(&r);
        assert_eq!(r["end"], "read_failed");
        assert_eq!(r["evidence_incomplete"], true);
        assert!(r["candidates"].as_array().unwrap().is_empty());
        assert!(r["history_error"].is_object());
        assert_eq!(core.executions(), 0);
        close(session, task).await;
    }
    for mode in 0..5 {
        let core = Core::start(Options {
            observation_status: if mode == 0 { Some(403) } else { None },
            ..Default::default()
        })
        .await;
        let mut u = update(1, "DeployStackService", Some("web"), 1000);
        if mode == 1 {
            u["logs"] = json!([]);
        }
        if mode == 2 {
            let log = u["logs"][0].clone();
            u["logs"].as_array_mut().unwrap().push(log);
        }
        if mode == 3 {
            u["logs"][0]["stdout"] = json!("some unrelated output mentions web");
        }
        if mode == 4 {
            u["logs"][0]["success"] = json!(false);
        }
        core.insert_update(u);
        let (session, task) = core.session(false).await;
        let r = value(call(&session, "reconcile_operation", case.context(Value::Null)).await);
        unconfirmed(&r);
        assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(r["candidates"][0]["scope_evidence"], "unavailable");
        assert_eq!(r["evidence_incomplete"], true);
        if mode == 0 {
            assert_eq!(
                r["candidates"][0]["lookup_error"]["kind"],
                "permission_denied"
            );
        }
        assert_eq!(core.executions(), 0);
        close(session, task).await;
    }
    let mut wrong = update(1, "DeployStackService", Some("web"), 1000);
    wrong["target"]["id"] = json!("000000000000000000000099");
    let core = Core::start(Options {
        update_reads: vec![UpdateRead::Value(wrong)],
        ..Default::default()
    })
    .await;
    core.insert_update(update(1, "DeployStackService", Some("web"), 1000));
    let (session, task) = core.session(false).await;
    let r = value(call(&session, "reconcile_operation", case.context(Value::Null)).await);
    unconfirmed(&r);
    assert!(r["candidates"].as_array().unwrap().is_empty());
    assert_eq!(r["history_error"]["kind"], "invalid_response");
    assert_eq!(r["evidence_incomplete"], true);
    close(session, task).await;
}

#[tokio::test]
async fn page_candidate_detail_and_total_time_budgets_bound_reconciliation() {
    let whole = cases().remove(0);
    let service = cases().remove(1);

    for repeat in [false, true] {
        let core = Core::start(Options {
            repeat_history_page: repeat,
            history_status_after: if repeat { None } else { Some((1, 403)) },
            ..Default::default()
        })
        .await;
        for n in 1..=100 {
            core.insert_update(update(
                n,
                if n == 100 {
                    "DeployStackService"
                } else {
                    "StartStack"
                },
                if n == 100 { Some("web") } else { None },
                1000 + n as i64,
            ));
        }
        let (session, task) = core.session(false).await;
        let r = value(
            call(
                &session,
                "reconcile_operation",
                service.context(Value::Null),
            )
            .await,
        );
        unconfirmed(&r);
        assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(r["detail_requests"], 1);
        assert_eq!(r["evidence_incomplete"], true);
        assert_eq!(r["end"], if repeat { "page_limit" } else { "read_failed" });
        assert_eq!(r["history_requests"], if repeat { 3 } else { 2 });
        close(session, task).await;
    }
    let core = Core::start(Options::default()).await;
    for n in 1..=301 {
        core.insert_update(update(n, "StartStack", None, 1000 + n as i64));
    }
    let (session, task) = core.session(false).await;
    let r = value(call(&session, "reconcile_operation", whole.context(Value::Null)).await);
    unconfirmed(&r);
    assert_eq!(r["end"], "page_limit");
    assert_eq!(r["history_requests"], 3);
    assert_eq!(r["detail_requests"], 0);
    assert_eq!(
        core.requests()
            .iter()
            .filter(|r| r.operation == "ListUpdates")
            .count(),
        3
    );
    close(session, task).await;
    let core = Core::start(Options::default()).await;
    // Newest five have a different proven scope; once their lookup budget is
    // spent, later matches can only be presented with unverified service scope.
    for n in 1..=10 {
        core.insert_update(update(
            n,
            "DeployStackService",
            Some(if n > 5 { "db" } else { "web" }),
            1000 + n as i64,
        ));
    }
    let (session, task) = core.session(false).await;
    let r = value(
        call(
            &session,
            "reconcile_operation",
            service.context(Value::Null),
        )
        .await,
    );
    unconfirmed(&r);
    assert_eq!(r["end"], "candidate_limit");
    assert_eq!(r["detail_requests"], 5);
    assert_eq!(r["excluded_by_scope"], 5);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 5);
    assert_eq!(r["candidates_truncated"], true);
    assert!(
        r["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["scope_evidence"] == "unavailable")
    );
    close(session, task).await;
    let core = Core::start(Options {
        observation_delay: Duration::from_secs(3),
        ..Default::default()
    })
    .await;
    for n in 1..=5 {
        core.insert_update(update(
            n,
            "DeployStackService",
            Some("web"),
            1000 + n as i64,
        ));
    }
    let started = Instant::now();
    let r = recovery::reconcile_operation(
        &core.client(false, Duration::from_secs(15)),
        serde_json::from_value(service.context(Value::Null)).unwrap(),
        pending(),
    )
    .await
    .unwrap();
    assert_eq!(r.end, SearchEnd::Deadline);
    assert!(started.elapsed() < Duration::from_secs(13));
    assert_eq!(r.detail_requests, 5);
    assert!(!r.candidates.is_empty());
    assert_eq!(core.executions(), 0);
}

#[tokio::test]
async fn accepted_busy_or_denied_updates_are_not_rejected_or_safely_repeatable() {
    for outcome in [Outcome::Busy, Outcome::Denied] {
        for case in [cases().remove(1), cases().remove(2)] {
            let core = Core::start(Options {
                outcome,
                ..Default::default()
            })
            .await;
            let (session, task) = core.session(true).await;
            let r = value(call(&session, &case.tool, case.arguments.clone()).await);
            assert_eq!(r["submission"], "accepted");
            assert!(r["update_id"].is_string());
            assert_eq!(r["latest_update"]["outcome"], "failed");
            close(session, task).await;
            let (fresh, task) = core.session(false).await;
            let observed =
                value(call(&fresh, "wait_for_update", json!({"id":r["update_id"]})).await);
            assert_eq!(observed["end"], "complete");
            assert_eq!(observed["latest_update"]["outcome"], "failed");
            assert_eq!(core.executions(), 1);
            close(fresh, task).await;
        }
    }
    let case = cases().remove(1);
    let core = Core::start(Options {
        outcome: Outcome::SecondBusy,
        ack: Ack::Malformed,
        ..Default::default()
    })
    .await;
    let (one, t1) = core.session(true).await;
    let (two, t2) = core.session(true).await;
    let (a, b) = tokio::join!(
        call(&one, &case.tool, case.arguments.clone()),
        call(&two, &case.tool, case.arguments.clone())
    );
    assert_eq!(value(a)["submission"], "unknown");
    assert_eq!(value(b)["submission"], "unknown");
    assert_eq!(core.executions(), 2);
    close(one, t1).await;
    close(two, t2).await;
    let (fresh, task) = core.session(false).await;
    let r = value(call(&fresh, "reconcile_operation", case.context(Value::Null)).await);
    unconfirmed(&r);
    let candidates = r["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0]["update"]["id"], candidates[1]["update"]["id"]);
    let mut outcomes = candidates
        .iter()
        .map(|c| c["update"]["outcome"].as_str().unwrap())
        .collect::<Vec<_>>();
    outcomes.sort();
    assert_eq!(outcomes, ["failed", "succeeded"]);
    assert_eq!(core.executions(), 2);
    close(fresh, task).await;
}

#[tokio::test]
async fn production_shutdown_after_acceptance_recovers_from_fresh_readonly_process_for_all_scopes()
{
    for case in cases() {
        let core = Core::start(Options {
            outcome: Outcome::Succeeded,
            ..Default::default()
        })
        .await;
        let proxy = FaultProxy::start(&core, LostReply::Timeout).await;
        let (process, session) = Process::start_address(&proxy.address, true).await;
        let start = now();
        let handle = session
            .send_request_with_option(
                ClientRequest::CallToolRequest(CallToolRequest::new(
                    CallToolRequestParams::new(case.tool.clone())
                        .with_arguments(case.arguments.as_object().unwrap().clone()),
                )),
                PeerRequestOptions::no_options(),
            )
            .await
            .unwrap();
        proxy.accepted(1).await;
        assert_eq!(core.executions(), 1);
        process.interrupt(session).await;
        assert!(
            tokio::time::timeout(Duration::from_secs(5), handle.await_response())
                .await
                .unwrap()
                .is_err()
        );
        drop(proxy);
        let (process, fresh) = Process::start(&core, false).await;
        let r = value(
            call(
                &fresh,
                "reconcile_operation",
                case.context(json!({"started_ts":start,"ended_ts":now()})),
            )
            .await,
        );
        unconfirmed(&r);
        assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(
            r["candidates"][0]["update"]["id"],
            "100000000000000000000001"
        );
        assert_eq!(core.executions(), 1);
        core.assert_credentials();
        process.finish(fresh).await;
    }
}

#[tokio::test]
async fn production_submission_deadline_retains_unknown_without_second_execution() {
    let case = cases()
        .into_iter()
        .find(|c| c.tool == "start_deployment")
        .unwrap();
    let core = Core::start(Options {
        outcome: Outcome::Succeeded,
        ..Default::default()
    })
    .await;
    let proxy = FaultProxy::start(&core, LostReply::Timeout).await;
    let (process, session) = Process::start_address(&proxy.address, true).await;
    let started = Instant::now();
    let r = value(call(&session, &case.tool, case.arguments).await);
    assert!(started.elapsed() >= Duration::from_secs(14));
    assert!(started.elapsed() < Duration::from_secs(22));
    assert_eq!(r["submission"], "unknown");
    assert_eq!(r["submission_error"]["kind"], "timeout");
    proxy.accepted(1).await;
    assert_eq!(core.executions(), 1);
    process.finish(session).await;
    drop(proxy);
    let (process, fresh) = Process::start(&core, false).await;
    let r = value(call(&fresh, "reconcile_operation", from_receipt(&r)).await);
    unconfirmed(&r);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(core.executions(), 1);
    process.finish(fresh).await;
}

#[tokio::test]
async fn invalid_context_and_cancelled_reconciliation_make_no_history_or_execute_requests() {
    let core = Core::start(Options::default()).await;
    let case = cases().remove(1);
    let (session, task) = core.session(false).await;
    let before = core.requests().len();
    let mut invalid = vec![];
    let mut v = case.context(Value::Null);
    v["target"]["id"] = json!("display-name");
    invalid.push(v);
    let mut v = case.context(Value::Null);
    v["requested_scope"]["service"] = json!("*");
    invalid.push(v);
    let mut v = case.context(Value::Null);
    v["target"]["kind"] = json!("deployment");
    invalid.push(v);
    invalid.push(case.context(json!({"started_ts":-1})));
    invalid.push(case.context(json!({"started_ts":20,"ended_ts":10})));
    for args in invalid {
        assert_eq!(
            call(&session, "reconcile_operation", args).await.is_error,
            Some(true)
        );
    }
    assert_eq!(core.requests().len(), before);
    close(session, task).await;
    let input: RecoveryInput = serde_json::from_value(case.context(Value::Null)).unwrap();
    let r = recovery::reconcile_operation(
        &core.client(false, Duration::from_secs(3)),
        input,
        ready(()),
    )
    .await
    .unwrap();
    assert_eq!(r.end, SearchEnd::Cancelled);
    assert_eq!(r.history_requests, 0);
    assert_eq!(core.requests().len(), before);
    assert_eq!(core.executions(), 0);
}
