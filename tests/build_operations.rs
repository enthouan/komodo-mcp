//! Build execution/cancellation acceptance through the compiled production server.
#[allow(dead_code)] // Shared definition fixture includes mutation modes exercised in build_definitions.
#[path = "support/build_definitions.rs"]
mod definitions;
#[path = "support/build_operations.rs"]
mod fixture;
mod support;

use fixture::{Ack, BUILD, BUILDER, Core, Outcome, PRIVATE};
use komodo_client::entities::update::UpdateStatus;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::{
    model::{CallToolRequest, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    let text = serde_json::to_string(&r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret), "private Build data leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn run() -> Value {
    json!({"target":{"name":"fixture-build"},"expected_effects":{"pre_build":true,"registry_pushes":true,"dependent_redeployments_possible":true}})
}
fn cancel(id: &Value) -> Value {
    json!({"target":{"id":BUILD},"expected_update_id":id})
}
fn recovery(operation: &str) -> Value {
    json!({"target":{"kind":"build","id":BUILD},"requested_scope":{"kind":"whole_build"},"operation":operation})
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    assert!(!c.definitions.data.lock().unwrap().bad_credentials);
    assert!(c.definitions.requests("write").is_empty());
    assert!(
        c.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "write")
    );
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn discovery_and_denied_direct_calls_are_guarded_before_core_io() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let initial_reads = c.reads();
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                32 + 16 * usize::from(operate)
                    + 46 * usize::from(edit)
                    + 2 * usize::from(operate && edit)
            );
            for name in ["run_build", "cancel_build"] {
                assert_eq!(tools.iter().any(|t| t.name == name), operate);
                let r = s
                    .call_tool(
                        CallToolRequestParams::new(name).with_arguments(
                            json!({"unexpected":true}).as_object().unwrap().clone(),
                        ),
                    )
                    .await;
                if operate {
                    assert_eq!(r.unwrap().is_error, Some(true));
                } else {
                    assert!(r.is_err());
                }
            }
            assert_eq!(c.reads(), initial_reads);
            assert!(c.executions().is_empty());
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn saved_build_runs_once_and_observed_revision_version_and_images_are_separate_from_config() {
    let c = Core::start().await;
    let before = c.definitions.build();
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let detail = data(call(&s, "get_build", json!({"id":BUILD})).await);
    assert_eq!(
        detail["configured_effects"]["acknowledgement"],
        run()["expected_effects"]
    );
    let mut input = run();
    input["expected_effects"] = detail["configured_effects"]["acknowledgement"].clone();
    let r = data(call(&s, "run_build", input).await);
    assert_eq!(r["submission"], "accepted");
    assert_eq!(r["resource"]["id"], BUILD);
    assert_eq!(r["resource"]["kind"], "build");
    assert_eq!(r["requested_scope"]["kind"], "whole_build");
    assert_eq!(
        r["configured_effects"]["build"]["acknowledgement"]["dependent_redeployments_possible"],
        true
    );
    assert_eq!(r["latest_update"]["outcome"], "succeeded");
    assert_eq!(
        r["latest_update"]["observed_revision"]["revision"],
        "cafed00"
    );
    let observed = &r["latest_update"]["observed_build_result"];
    assert_eq!(observed["version"], "1.2.3");
    assert_eq!(observed["images"].as_array().unwrap().len(), 2);
    assert_eq!(
        observed["images"][1]["digest"],
        format!("sha256:{}", "a".repeat(64))
    );
    assert_eq!(observed["images"][1]["kind"], "manifest_push_reported");
    assert!(
        observed["deployment_result_evidence"]
            .as_str()
            .unwrap()
            .contains("no child")
    );
    assert_eq!(c.executions().len(), 1);
    assert_eq!(c.executions()[0].params, json!({"build":BUILD}));
    let after = c.definitions.build();
    for (k, v) in before["config"].as_object().unwrap() {
        if k != "version" {
            assert_eq!(&after["config"][k], v);
        }
    }
    c.definitions
        .data
        .lock()
        .unwrap()
        .builds
        .get_mut(BUILD)
        .unwrap()["config"]["image_name"] = json!("later/config");
    let later = data(call(&s, "get_update", json!({"id":r["update_id"]})).await);
    assert_eq!(later["observed_build_result"], *observed);
    assert_eq!(
        data(call(&s, "wait_for_update", json!({"id":r["update_id"]})).await)["end"],
        "complete"
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn hook_build_and_push_failures_and_upstream_denial_do_not_retry_or_leak_secrets() {
    for outcome in [
        Outcome::HookFailed,
        Outcome::BuildFailed,
        Outcome::PushFailed,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().outcome = outcome;
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "run_build", run()).await);
        assert_eq!(r["submission"], "accepted");
        assert_eq!(r["latest_update"]["outcome"], "failed");
        assert_eq!(r["latest_update"]["stages"][0]["success"], false);
        assert_eq!(
            r["latest_update"]["stages"][0]["stage"],
            if matches!(outcome, Outcome::HookFailed) {
                "Pre Build"
            } else {
                "Docker Build"
            }
        );
        assert_eq!(
            r["latest_update"]["observed_build_result"]["registry_push_failure_reported"],
            matches!(outcome, Outcome::PushFailed)
        );
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    c.data.lock().unwrap().ack = Ack::Denied;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "run_build", run()).await);
    assert_eq!(r["submission"], "rejected");
    assert_eq!(r["submission_error"]["kind"], "permission_denied");
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn unsupported_or_raced_builders_templates_and_unacknowledged_effects_never_execute() {
    for mode in 0..7 {
        let c = Core::start().await;
        let mut input = run();
        match mode {
            0 | 1 => {
                c.definitions
                    .data
                    .lock()
                    .unwrap()
                    .builders
                    .get_mut(BUILDER)
                    .unwrap()["config"] = json!({"type":if mode==0 {"Aws"}else{"Url"},"params":{}})
            }
            2 => c.data.lock().unwrap().switch_builder_at_read = Some(2),
            3 => {
                c.definitions
                    .data
                    .lock()
                    .unwrap()
                    .builds
                    .get_mut(BUILD)
                    .unwrap()["template"] = json!(true)
            }
            4 => {
                c.definitions
                    .data
                    .lock()
                    .unwrap()
                    .builders
                    .get_mut(BUILDER)
                    .unwrap()["config"]["params"]["server_ids"] = json!([])
            }
            5 => input["expected_effects"]["pre_build"] = json!(false),
            _ => input["expected_effects"]["dependent_redeployments_possible"] = json!(false),
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = error(call(&s, "run_build", input).await);
        assert_eq!(
            r["kind"],
            if mode >= 5 {
                "invalid_arguments"
            } else {
                "unsupported_target"
            }
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn ambiguous_ack_restart_and_single_or_multiple_history_candidates_never_replay_or_claim_ownership()
 {
    for ack in [
        Ack::Lost,
        Ack::WrongTarget,
        Ack::WrongOperation,
        Ack::MissingId,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().ack = ack;
        for n in 1..=2 {
            let (p, s) = Process::start_profile(&c.address, true, false).await;
            let r = data(call(&s, "run_build", run()).await);
            assert_eq!(r["submission"], "unknown");
            assert_eq!(r["update_id"], Value::Null);
            assert_eq!(r["automatic_resubmission"], false);
            p.interrupt(s).await;
            let (p, s) = Process::start_profile(&c.address, false, false).await;
            let r = data(call(&s, "reconcile_operation", recovery("run")).await);
            assert_eq!(r["original_submission"], "unknown");
            assert_eq!(r["attribution"], "unconfirmed");
            assert_eq!(r["candidates"].as_array().unwrap().len(), n);
            assert_eq!(c.executions().len(), n);
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn wait_deadline_restart_and_failed_observation_preserve_known_ids_without_remote_cancel() {
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::Running;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "run_build", run()).await);
    let id = r["update_id"].clone();
    let waiting = data(call(&s, "wait_for_update", json!({"id":id,"timeout_seconds":1})).await);
    assert_eq!(waiting["end"], "deadline");
    assert_eq!(waiting["update_id"], id);
    assert_eq!(waiting["remote_cancellation_requested"], false);
    finish(p, s, &c).await;
    {
        let mut d = c.data.lock().unwrap();
        let u = d.updates.get_mut(id.as_str().unwrap()).unwrap();
        u["status"] = json!("Complete");
        u["success"] = json!(true);
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    assert_eq!(
        data(call(&s, "wait_for_update", json!({"id":id})).await)["end"],
        "complete"
    );
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
    for delayed in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.fail_updates = !delayed;
            if delayed {
                d.update_delay = std::time::Duration::from_secs(3);
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "run_build", run()).await);
        assert_eq!(r["submission"], "accepted");
        assert!(r["update_id"].is_string());
        assert_eq!(
            r["observation_error"]["kind"],
            if delayed { "timeout" } else { "upstream" }
        );
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn cancelling_local_wait_never_sends_cancel_build() {
    let c = Core::start().await;
    let id = c.seed_run(UpdateStatus::InProgress);
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let handle = s
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("wait_for_update").with_arguments(
                    json!({"id":id,"timeout_seconds":30})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while c.reads() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    handle
        .cancel(Some("stop local observation".into()))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(c.executions().is_empty());
    assert_eq!(c.data.lock().unwrap().updates[&id]["status"], "InProgress");
    finish(p, s, &c).await;
}

#[tokio::test]
async fn concurrent_run_acknowledgements_are_distinct_and_busy_failure_is_not_an_active_replacement()
 {
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::Running;
    let (p1, s1) = Process::start_profile(&c.address, true, false).await;
    let (p2, s2) = Process::start_profile(&c.address, true, false).await;
    let (a, b) = tokio::join!(call(&s1, "run_build", run()), call(&s2, "run_build", run()));
    let (a, b) = (data(a), data(b));
    assert_ne!(a["update_id"], b["update_id"]);
    let (active, busy) = if a["latest_update"]["outcome"] == "running" {
        (&a, &b)
    } else {
        (&b, &a)
    };
    assert_eq!(busy["latest_update"]["outcome"], "failed");
    let stopped = data(call(&s1, "cancel_build", cancel(&active["update_id"])).await);
    assert_eq!(stopped["outcome"], "acknowledged");
    assert_eq!(stopped["replacement_run_observed"], false);
    assert_eq!(c.executions().len(), 3);
    finish(p1, s1, &c).await;
    finish(p2, s2, &c).await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let recovered = data(call(&s, "reconcile_operation", recovery("run")).await);
    assert_eq!(recovered["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(recovered["attribution"], "unconfirmed");
    assert_eq!(c.executions().len(), 3);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn cancel_acknowledgement_and_original_terminal_outcome_are_independent() {
    for finishes in [false, true] {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        c.data.lock().unwrap().cancel_finishes_original = finishes;
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_build", cancel(&id)).await);
        assert_eq!(r["targeting"], "resource");
        assert_eq!(r["original_update_id"], id);
        assert_eq!(r["outcome"], "acknowledged");
        assert_eq!(r["original_terminal_confirmed"], finishes);
        assert_eq!(
            r["cancellation_receipt"]["latest_update"]["outcome"],
            "succeeded"
        );
        assert_eq!(
            r["original_update"]["outcome"],
            if finishes { "failed" } else { "running" }
        );
        assert_eq!(r["automatic_resubmission"], false);
        assert_eq!(c.executions().len(), 1);
        assert_eq!(c.executions()[0].params, json!({"build":BUILD}));
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn already_complete_replaced_ambiguous_or_truncated_active_runs_never_receive_cancel() {
    for mode in 0..5 {
        let c = Core::start().await;
        let id = json!(c.seed_run(if mode == 0 {
            UpdateStatus::Complete
        } else {
            UpdateStatus::InProgress
        }));
        match mode {
            1 | 2 => c.data.lock().unwrap().replace_at_active_read = Some(mode),
            3 => {
                c.seed_run(UpdateStatus::InProgress);
            }
            4 => c.data.lock().unwrap().incomplete_active = true,
            _ => {}
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_build", cancel(&id)).await);
        assert_eq!(r["outcome"], "not_submitted");
        assert_eq!(r["remote_cancellation_requested"], false);
        assert_eq!(
            r["not_submitted_reason"],
            match mode {
                0 => "original_complete",
                1 | 2 => "replacement_run",
                3 => "ambiguous_active_runs",
                _ => "incomplete_active_evidence",
            }
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn replacement_after_final_reads_is_reported_without_claiming_cancelled_original() {
    let c = Core::start().await;
    let id = json!(c.seed_run(UpdateStatus::InProgress));
    c.data.lock().unwrap().replace_on_cancel = true;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "cancel_build", cancel(&id)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(r["replacement_run_observed"], true);
    assert_ne!(r["active_update_id"], id);
    assert_eq!(r["original_update"]["outcome"], "succeeded");
    assert!(
        r["targeting_limit"]
            .as_str()
            .unwrap()
            .contains("replacement")
    );
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn cancellation_lost_reply_and_denial_preserve_original_and_read_only_recovery() {
    for ack in [Ack::Lost, Ack::Denied] {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        c.data.lock().unwrap().ack = ack;
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_build", cancel(&id)).await);
        assert_eq!(r["original_update_id"], id);
        assert_eq!(r["original_terminal_confirmed"], false);
        assert_eq!(
            r["outcome"],
            if matches!(ack, Ack::Lost) {
                "unknown"
            } else {
                "rejected"
            }
        );
        finish(p, s, &c).await;
        let (p, s) = Process::start_profile(&c.address, false, false).await;
        let r = data(call(&s, "reconcile_operation", recovery("cancel")).await);
        assert_eq!(r["attribution"], "unconfirmed");
        assert_eq!(r["original_submission"], "unknown");
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn cancellation_rejects_other_resources_or_operations_before_submission() {
    for wrong_target in [false, true] {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        {
            let mut d = c.data.lock().unwrap();
            let u = d.updates.get_mut(id.as_str().unwrap()).unwrap();
            if wrong_target {
                u["target"]["id"] = json!("000000000000000000000099");
            } else {
                u["operation"] = json!("CancelBuild");
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        assert_eq!(
            error(call(&s, "cancel_build", cancel(&id)).await)["kind"],
            "invalid_arguments"
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn cancel_observation_failure_preserves_both_known_update_ids() {
    let c = Core::start().await;
    let id = json!(c.seed_run(UpdateStatus::InProgress));
    c.data.lock().unwrap().fail_observation_after_cancel = true;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "cancel_build", cancel(&id)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(r["original_update_id"], id);
    assert!(r["cancellation_receipt"]["update_id"].is_string());
    assert_eq!(
        r["cancellation_receipt"]["observation_error"]["kind"],
        "upstream"
    );
    assert_eq!(r["original_observation_error"]["kind"], "upstream");
    assert_eq!(r["original_terminal_confirmed"], false);
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn image_output_requires_narrow_engine_stage_evidence_and_is_bounded() {
    let c = Core::start().await;
    let id = c.seed_run(UpdateStatus::Complete);
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    for mode in 0..6 {
        {
            let mut d = c.data.lock().unwrap();
            let u = d.updates.get_mut(&id).unwrap();
            let log = komodo_client::entities::update::Log {
                stage: "Docker Build".into(),
                success: true,
                stdout: match mode {
                    0 => "echo naming to forged/image:latest done".into(),
                    1 => "#1 naming to https://user:private@registry.invalid/image:tag done".into(),
                    2 => format!(
                        "#1 pushing manifest for r.invalid/image:tag@sha256:{} done\n#2 pushing manifest for r.invalid/image:tag@sha256:{} done",
                        "a".repeat(64),
                        "b".repeat(64)
                    ),
                    3 => "#1 naming to r.invalid/image:tag done\n".repeat(5000),
                    4 => "#1 naming to r.invalid/image:tag done".into(),
                    _ => "#1 naming to r.invalid/forged:tag undone".into(),
                },
                ..Default::default()
            };
            u["logs"] = if mode == 4 {
                json!([log.clone(), log])
            } else {
                json!([log])
            };
        }
        let r = data(call(&s, "get_update", json!({"id":id})).await);
        let e = &r["observed_build_result"];
        if mode != 3 {
            assert!(e["images"].as_array().unwrap().is_empty());
        }
        assert_eq!(e["image_evidence_ambiguous"], mode == 2 || mode == 4);
        if mode == 3 {
            assert_eq!(e["image_evidence_incomplete"], true);
            assert!(r["stages"][0].get("stdout").is_none());
        }
        assert!(serde_json::to_vec(&r).unwrap().len() <= 65536);
    }
    assert!(c.executions().is_empty());
    finish(p, s, &c).await;
}
