#[path = "support/configuration.rs"]
mod fixture;
mod support;

use fixture::{Core, WriteReply};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{Process, STACK, call},
};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn args(patch: Value) -> Value {
    json!({"target":{"id":STACK},"patch":patch})
}
async fn preview(session: &support::operations::Session, input: &Value) -> Value {
    data(call(session, "preview_stack_update", input.clone()).await)
}
fn apply(input: &Value, preview: &Value) -> Value {
    let mut a = input.clone();
    a["preview_id"] = preview["preview_id"].clone();
    a
}

#[tokio::test]
async fn production_preview_apply_sends_only_one_field_and_preserves_secret_references() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let before = core.data.lock().unwrap().stack.clone();
    let input = args(json!({"auto_pull":true}));
    let p = preview(&session, &input).await;
    assert_eq!(p["resource"]["id"], STACK);
    assert_eq!(
        p["changes"],
        json!([{"field":"auto_pull","before":false,"after":true,"redacted":false,"before_truncated":false}])
    );
    assert_eq!(p["writes_submitted"], 0);
    assert!(core.writes().is_empty());
    let applied = data(call(&session, "apply_stack_update", apply(&input, &p)).await);
    assert_eq!(applied["outcome"], "saved");
    assert_eq!(applied["saved_configuration_verified"], true);
    assert_eq!(
        core.writes()[0].params,
        json!({"id":STACK,"config":{"auto_pull":true}})
    );
    let saved = core.data.lock().unwrap().stack.clone();
    for field in [
        "environment",
        "file_contents",
        "git_account",
        "server_id",
        "project_name",
        "ignore_services",
    ] {
        assert_eq!(saved["config"][field], before["config"][field]);
    }
    data(call(&session, "get_stack", json!({"id":STACK})).await);
    assert_eq!(
        call(&session, "apply_stack_update", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.writes().len(), 1);
    assert!(!core.data.lock().unwrap().bad_credentials);
    process.finish(session).await;
}

#[tokio::test]
async fn explicit_clears_are_distinct_from_omission_and_secrets_never_leave_preview() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    for patch in [
        json!({"environment":"PASSWORD=new-secret-input\n"}),
        json!({"environment":""}),
        json!({"ignore_services":[]}),
    ] {
        let input = args(patch.clone());
        let p = preview(&session, &input).await;
        assert!(!p.to_string().contains("new-secret-input"));
        assert!(
            p["changes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["field"] != "environment" || c["redacted"] == true)
        );
        assert_eq!(
            data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
            "saved"
        );
        assert_eq!(core.writes().last().unwrap().params["config"], patch);
    }
    for patch in [
        json!({"auto_pull":null}),
        json!({"environment":null}),
        json!({"ignore_services":null}),
        json!({"environment":"[redacted]"}),
        json!({"environment":"x".repeat(17000)}),
        json!({"file_contents":"later issue"}),
    ] {
        assert_eq!(
            call(&session, "preview_stack_update", args(patch))
                .await
                .is_error,
            Some(true)
        );
    }
    assert_eq!(core.writes().len(), 3);
    process
        .finish_excluding(session, &["new-secret-input", "later issue"])
        .await;
}

#[tokio::test]
async fn altered_payload_target_raw_secret_baseline_and_noops_never_write() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(json!({"auto_pull":true}));
    for altered in [
        args(json!({"auto_pull":false})),
        json!({"target":{"id":"000000000000000000000099"},"patch":{"auto_pull":true}}),
    ] {
        let p = preview(&session, &input).await;
        let reads = core.data.lock().unwrap().requests.len();
        assert_eq!(
            call(&session, "apply_stack_update", apply(&altered, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "invalid_preview"
        );
        assert_eq!(core.data.lock().unwrap().requests.len(), reads);
    }
    let p = preview(&session, &input).await;
    core.data.lock().unwrap().stack["config"]["environment"] =
        json!("PASSWORD=changed-secret-baseline");
    let stale = call(&session, "apply_stack_update", apply(&input, &p)).await;
    assert_safe(&serde_json::to_string(&stale).unwrap());
    assert_eq!(stale.structured_content.unwrap()["kind"], "stale_preview");
    for patch in [json!({}), json!({"auto_pull":false})] {
        let input = args(patch);
        let p = preview(&session, &input).await;
        assert_eq!(p["outcome"], "no_op");
        assert_eq!(
            data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
            "no_op"
        );
    }
    for target in [
        json!({}),
        json!({"id":"bad"}),
        json!({"id":STACK,"name":"fixture-stack"}),
        json!({"name":"missing"}),
    ] {
        assert_eq!(
            call(
                &session,
                "preview_stack_update",
                json!({"target":target,"patch":{}})
            )
            .await
            .is_error,
            Some(true)
        );
    }
    assert!(core.writes().is_empty());
    process.finish(session).await;
}

#[tokio::test]
async fn all_four_profiles_enforce_discovery_and_direct_dispatch_before_any_core_reads() {
    let core = Core::start().await;
    for operate in [false, true] {
        for edit in [false, true] {
            let (process, session) = Process::start_profile(&core.address, operate, edit).await;
            let tools = session.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                16 + 8 * usize::from(operate) + 2 * usize::from(edit)
            );
            for name in ["preview_stack_update", "apply_stack_update"] {
                assert_eq!(tools.iter().any(|t| t.name == name), edit);
                let before = core.data.lock().unwrap().requests.len();
                let r = session
                    .call_tool(
                        CallToolRequestParams::new(name)
                            .with_arguments(json!({}).as_object().unwrap().clone()),
                    )
                    .await;
                if edit {
                    assert_eq!(r.unwrap().is_error, Some(true));
                } else {
                    assert!(r.is_err());
                }
                assert_eq!(core.data.lock().unwrap().requests.len(), before);
            }
            if !operate {
                assert!(
                    session
                        .call_tool(CallToolRequestParams::new("restart_stack"))
                        .await
                        .is_err()
                );
            }
            process.finish(session).await;
        }
    }
    assert!(core.writes().is_empty());
}

#[tokio::test]
async fn uncertain_and_denied_responses_are_sanitized_never_retried_and_tokens_stay_consumed() {
    for reply in [
        WriteReply::Malformed,
        WriteReply::Oversized,
        WriteReply::WrongTarget,
        WriteReply::Lost,
        WriteReply::Denied,
        WriteReply::WriteDenied,
        WriteReply::Redirect,
    ] {
        let core = Core::start().await;
        let (process, session) = Process::start_profile(&core.address, false, true).await;
        let input = args(json!({"auto_pull":true}));
        let p = preview(&session, &input).await;
        core.data.lock().unwrap().reply = reply;
        let r = data(call(&session, "apply_stack_update", apply(&input, &p)).await);
        assert_eq!(r["outcome"], "submission_unknown");
        assert_eq!(r["saved_configuration_verified"], false);
        assert!(r["observation_error"].is_object());
        assert_eq!(core.writes().len(), 1);
        assert_eq!(
            call(&session, "apply_stack_update", apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "invalid_preview"
        );
        assert_eq!(core.writes().len(), 1);
        assert!(!core.data.lock().unwrap().bad_credentials);
        process.finish(session).await;
    }
}

#[tokio::test]
async fn permission_loss_during_revalidation_and_new_process_tokens_fail_without_writes() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(json!({"auto_pull":true}));
    let p = preview(&session, &input).await;
    core.data.lock().unwrap().read_status = Some(403);
    let r = call(&session, "apply_stack_update", apply(&input, &p)).await;
    assert_eq!(r.structured_content.unwrap()["kind"], "permission_denied");
    core.data.lock().unwrap().read_status = None;
    let p = preview(&session, &input).await;
    process.finish(session).await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let new = preview(&session, &input).await;
    assert_ne!(p["preview_id"], new["preview_id"]);
    assert_eq!(
        call(&session, "apply_stack_update", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert!(core.writes().is_empty());
    process.finish(session).await;
}

#[tokio::test]
async fn concurrent_apply_of_one_preview_submits_once() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(json!({"auto_pull":true}));
    let p = preview(&session, &input).await;
    let (first, second) = tokio::join!(
        call(&session, "apply_stack_update", apply(&input, &p)),
        call(&session, "apply_stack_update", apply(&input, &p))
    );
    let results = [
        first.structured_content.unwrap(),
        second.structured_content.unwrap(),
    ];
    assert_eq!(
        results.iter().filter(|v| v["outcome"] == "saved").count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|v| v["kind"] == "invalid_preview")
            .count(),
        1
    );
    assert_eq!(core.writes().len(), 1);
    process.finish(session).await;
}

#[tokio::test]
async fn write_deadline_retains_unknown_acceptance_and_direct_apis_enforce_policy() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        configuration::{self, StackApplyInput, StackPreviewInput},
    };
    use std::time::Duration;
    let core = Core::start().await;
    let client = |edit| {
        CoreClient::new(
            Config::parse(&core.address, support::KEY, support::SECRET)
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(true, edit)),
            RequestPolicy {
                connect_timeout: Duration::from_millis(100),
                operation_timeout: Duration::from_millis(100),
            },
        )
        .unwrap()
    };
    let denied = client(false);
    let input = args(json!({"auto_pull":true}));
    let result = configuration::preview_stack_update(
        &denied,
        serde_json::from_value::<StackPreviewInput>(input.clone()).unwrap(),
    )
    .await;
    assert!(matches!(result,Err(e) if e.kind==komodo_mcp::error::ErrorKind::CapabilityDisabled));
    assert!(core.data.lock().unwrap().requests.is_empty());
    let allowed = client(true);
    let p = configuration::preview_stack_update(
        &allowed,
        serde_json::from_value::<StackPreviewInput>(input.clone()).unwrap(),
    )
    .await
    .unwrap();
    core.data.lock().unwrap().reply = WriteReply::Delayed;
    let result = configuration::apply_stack_update(
        &allowed,
        serde_json::from_value::<StackApplyInput>(apply(&input, &serde_json::to_value(p).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap();
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(result["outcome"], "submission_unknown");
    assert_eq!(result["observation_error"]["kind"], "timeout");
    assert_eq!(core.writes().len(), 1);
    // The owned connection completes before the fixture server is dropped.
    tokio::time::sleep(Duration::from_millis(350)).await;
}
