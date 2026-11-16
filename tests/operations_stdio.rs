//! Integrated operator transcripts against the compiled production stdio binary.
mod support;

use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use support::{
    assert_safe,
    incident::wire,
    operations::{Core, DEPLOYMENT, Options, Outcome, Process, STACK, call},
    recovery::{FaultProxy, LostReply},
};

fn data(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_safe(&serde_json::to_string(&result).unwrap());
    result.structured_content.unwrap()
}
fn stack(service: Option<&str>) -> Value {
    json!({"target":{"id":STACK},"scope":service.map_or(json!({"kind":"whole_stack"}), |s|json!({"kind":"service","service":s}))})
}

#[tokio::test]
async fn discover_inspect_act_follow_and_observe_health_for_every_action_and_scope() {
    let core = Core::start(Options {
        outcome: Outcome::Succeeded,
        ..Default::default()
    })
    .await;
    let (process, session) = Process::start(&core, true).await;
    assert_eq!(session.list_tools(None).await.unwrap().tools.len(), 64 + 20);
    assert_eq!(
        data(call(&session, "get_stack", json!({"id":STACK})).await)["id"],
        STACK
    );
    assert_eq!(
        data(call(&session, "get_deployment", json!({"id":DEPLOYMENT})).await)["id"],
        DEPLOYMENT
    );
    let before = data(call(&session, "stack_diagnose", json!({"id":STACK})).await);
    assert_eq!(before["outcome"], "unhealthy");
    assert!(before["gaps"].as_array().unwrap().is_empty());
    for action in ["deploy", "start", "stop", "restart"] {
        for service in [None, Some("web")] {
            let receipt = data(call(&session, &format!("{action}_stack"), stack(service)).await);
            assert_eq!(receipt["submission"], "accepted");
            assert_eq!(receipt["resource"]["id"], STACK);
            assert_eq!(receipt["operation"], action);
            assert_eq!(receipt["requested_scope"], stack(service)["scope"]);
            let effects = &receipt["configured_effects"];
            assert_eq!(effects["pre_deploy_hook"], action == "deploy");
            assert_eq!(effects["post_deploy_hook"], action == "deploy");
            assert_eq!(effects["broader_effects_possible"], true);
            assert_eq!(
                effects["compose_dependencies"],
                match action {
                    "deploy" => "deploy_saved_definition",
                    "start" => "start_existing_with_dependencies",
                    "stop" => "stop_selected_only",
                    _ => "restart_with_configured_dependents",
                }
            );
            let observed = data(
                call(
                    &session,
                    "wait_for_update",
                    json!({"id":receipt["update_id"]}),
                )
                .await,
            );
            assert_eq!(observed["end"], "complete");
            assert_eq!(observed["latest_update"]["outcome"], "succeeded");
            let stages = observed["latest_update"]["stages"].as_array().unwrap();
            assert_eq!(
                stages.iter().any(|s| s["stage"] == "Pre Deploy"),
                action == "deploy"
            );
            assert_eq!(
                stages.iter().any(|s| s["stage"] == "Post Deploy"),
                action == "deploy"
            );
        }
        let receipt = data(
            call(
                &session,
                &format!("{action}_deployment"),
                json!({"id":DEPLOYMENT}),
            )
            .await,
        );
        assert_eq!(receipt["submission"], "accepted");
        assert_eq!(receipt["resource"]["id"], DEPLOYMENT);
        assert_eq!(receipt["requested_scope"]["kind"], "whole_deployment");
        let observed = data(call(&session, "get_update", json!({"id":receipt["update_id"]})).await);
        assert_eq!(observed["outcome"], "succeeded");
    }
    // The same Core serves inspection, execution, history and health. A
    // successful command never silently changes independent health evidence.
    let after = data(call(&session, "stack_diagnose", json!({"id":STACK})).await);
    assert_eq!(after["outcome"], "unhealthy");
    assert!(after["gaps"].as_array().unwrap().is_empty());
    let health = data(call(&session, "fleet_health", json!({})).await);
    assert_eq!(health["stacks"][0]["health"], "unhealthy");
    assert_eq!(health["deployments"][0]["id"], DEPLOYMENT);
    assert_eq!(core.executions(), 12);
    core.assert_credentials();
    process.finish(session).await;
}

#[tokio::test]
async fn readonly_defaults_and_core_denied_busy_rollout_failures_remain_observable() {
    for outcome in [Outcome::Denied, Outcome::Busy, Outcome::RolloutFailed] {
        let core = Core::start(Options {
            outcome,
            ..Default::default()
        })
        .await;
        let (process, session) = Process::start(&core, false).await;
        assert_eq!(session.list_tools(None).await.unwrap().tools.len(), 64);
        let reads = core.requests().len();
        let disabled = tokio::time::timeout(
            support::operations::DEADLINE,
            session.call_tool(
                CallToolRequestParams::new("deploy_stack")
                    .with_arguments(stack(Some("web")).as_object().unwrap().clone()),
            ),
        )
        .await
        .unwrap();
        assert!(disabled.is_err());
        assert_eq!(core.requests().len(), reads);
        data(call(&session, "stack_diagnose", json!({"id":STACK})).await);
        assert_eq!(core.executions(), 0);
        process.finish(session).await;
        let (process, session) = Process::start(&core, true).await;
        let receipt = data(call(&session, "deploy_stack", stack(Some("web"))).await);
        assert_eq!(receipt["submission"], "accepted");
        assert_eq!(receipt["latest_update"]["outcome"], "failed");
        let error = receipt["latest_update"]["stages"].to_string();
        assert!(error.contains(match outcome {
            Outcome::Denied => "permission",
            Outcome::Busy => "busy",
            _ => "rollout",
        }));
        process.finish(session).await;
        let (process, session) = Process::start(&core, false).await;
        let resumed = data(
            call(
                &session,
                "wait_for_update",
                json!({"id":receipt["update_id"]}),
            )
            .await,
        );
        assert_eq!(resumed["end"], "complete");
        assert_eq!(resumed["latest_update"]["outcome"], "failed");
        data(call(&session, "fleet_health", json!({})).await);
        assert_eq!(core.executions(), 1);
        core.assert_credentials();
        process.finish(session).await;
    }
}

#[tokio::test]
async fn invalid_selection_and_swarm_filters_never_expand_scope_in_stdio() {
    for swarm in [false, true] {
        let core = Core::start(Options {
            swarm,
            ..Default::default()
        })
        .await;
        let (process, session) = Process::start(&core, true).await;
        for service in ["", " ", "*", "missing", "WEB"] {
            assert_eq!(
                call(&session, "deploy_stack", stack(Some(service)))
                    .await
                    .is_error,
                Some(true)
            );
        }
        if swarm {
            assert_eq!(
                call(&session, "deploy_stack", stack(Some("web")))
                    .await
                    .is_error,
                Some(true)
            );
            assert_eq!(
                call(&session, "restart_stack", stack(None)).await.is_error,
                Some(true)
            );
            assert_eq!(
                call(&session, "restart_deployment", json!({"id":DEPLOYMENT}))
                    .await
                    .is_error,
                Some(true)
            );
        }
        assert_eq!(core.executions(), 0);
        core.assert_credentials();
        process.finish(session).await;
    }
}

#[tokio::test]
async fn independent_call_and_lost_ack_process_restart_reconcile_without_attribution() {
    let core = Core::start(Options {
        outcome: Outcome::SecondBusy,
        ..Default::default()
    })
    .await;
    let proxy = FaultProxy::start(&core, LostReply::Timeout).await;
    let (first_process, first) = Process::start_address(&proxy.address, true).await;
    let handle = first
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("restart_stack")
                    .with_arguments(stack(Some("web")).as_object().unwrap().clone()),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    proxy.accepted(1).await;
    let (second_process, second) = Process::start(&core, true).await;
    let receipt = data(call(&second, "restart_stack", stack(Some("web"))).await);
    assert_eq!(receipt["submission"], "accepted");
    assert_eq!(receipt["latest_update"]["outcome"], "failed");
    assert!(
        receipt["latest_update"]["stages"]
            .to_string()
            .contains("busy")
    );
    assert_eq!(core.executions(), 2);
    first_process.interrupt(first).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.await_response())
            .await
            .unwrap()
            .is_err()
    );
    second_process.finish(second).await;
    drop(proxy);
    let (process, observer) = Process::start(&core, false).await;
    let recovered = data(call(&observer, "reconcile_operation", json!({
        "target":{"kind":"stack","id":STACK},"requested_scope":{"kind":"service","service":"web"},"operation":"restart"
    })).await);
    assert_eq!(recovered["original_submission"], "unknown");
    assert_eq!(recovered["attribution"], "unconfirmed");
    assert!(recovered.get("selected_update_id").is_none());
    assert_eq!(recovered["candidates"].as_array().unwrap().len(), 2);
    let observed = data(
        call(
            &observer,
            "wait_for_update",
            json!({"id":receipt["update_id"]}),
        )
        .await,
    );
    assert_eq!(observed["end"], "complete");
    assert_eq!(observed["latest_update"]["outcome"], "failed");
    data(call(&observer, "stack_diagnose", json!({"id":STACK})).await);
    assert_eq!(core.executions(), 2);
    core.assert_credentials();
    process.finish(observer).await;
}

#[tokio::test]
async fn accepted_running_operation_times_out_locally_then_resumes_readonly_and_checks_health() {
    let core = Core::start(Options::default()).await;
    let (process, session) = Process::start(&core, true).await;
    let receipt = data(call(&session, "deploy_deployment", json!({"id":DEPLOYMENT})).await);
    let observed = data(
        call(
            &session,
            "wait_for_update",
            json!({"id":receipt["update_id"],"timeout_seconds":1}),
        )
        .await,
    );
    assert_eq!(observed["end"], "deadline");
    assert_eq!(observed["update_id"], receipt["update_id"]);
    assert_eq!(observed["latest_update"]["outcome"], "running");
    process.finish(session).await;
    let mut completed = wire()["update"].clone();
    completed["_id"] = json!({"$oid":receipt["update_id"]});
    completed["target"] = json!({"type":"Deployment","id":DEPLOYMENT});
    completed["operation"] = json!("Deploy");
    completed["status"] = json!("Complete");
    completed["success"] = json!(true);
    core.insert_update(completed);
    let (process, session) = Process::start(&core, false).await;
    let resumed = data(
        call(
            &session,
            "wait_for_update",
            json!({"id":receipt["update_id"]}),
        )
        .await,
    );
    assert_eq!(resumed["end"], "complete");
    assert_eq!(resumed["latest_update"]["outcome"], "succeeded");
    data(call(&session, "fleet_health", json!({})).await);
    assert_eq!(core.executions(), 1);
    process.finish(session).await;
}
