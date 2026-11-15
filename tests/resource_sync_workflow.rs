//! Integrated production-stdio ResourceSync contracts on deterministic Core wire.
#[path = "support/export.rs"]
#[allow(dead_code)]
mod exports;
mod support;
#[path = "support/resource_syncs.rs"]
mod syncs;
#[path = "support/resource_sync_workflow.rs"]
mod workflow;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{self, Process, call};
use workflow::{Core, REVISION, SERVER};

fn source() -> String {
    "[[server]]\nname='workflow-server'\n[server.config]\nenabled=false\n".into()
}
async fn saved(s: &operations::Session, input: Value, create: bool) -> Value {
    let operation = if create {
        "resource_sync_create"
    } else {
        "resource_sync_update"
    };
    let preview = data(call(s, &format!("preview_{operation}"), input.clone()).await);
    let mut input = input;
    input["preview_id"] = preview["preview_id"].clone();
    let result = data(call(s, &format!("apply_{operation}"), input).await);
    assert_eq!(result["saved_configuration_verified"], true);
    result
}

#[tokio::test]
async fn four_source_export_define_refresh_review_apply_inspect_and_restart_share_one_core() {
    for kind in ["inline", "git", "linked_repo", "host"] {
        let core = Core::start(source()).await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let exported=data(call(&s,"export_resources_toml",json!({"targets":[{"kind":"server","target":{"id":SERVER}}],"include_variables":true})).await);
        let parsed: toml::Value = toml::from_str(exported["toml"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["server"].as_array().unwrap().len(), 1);
        assert!(parsed["variable"][0].get("value").is_none());
        assert_eq!(exported["authoritative_apply_input"], false);
        assert!(exported["omitted_fields"].as_u64().unwrap() > 0);
        assert_eq!(core.count("write"), 0);
        assert_eq!(core.count("execute"), 0);
        let variant = match kind {
            "inline" => json!({"kind":kind,"contents":source()}),
            "git" => {
                json!({"kind":kind,"repo":"fixture/source","branch":"main","resource_paths":["resources.toml"]})
            }
            "linked_repo" => {
                json!({"kind":kind,"target":{"id":syncs::REPO},"resource_paths":["resources.toml"]})
            }
            _ => json!({"kind":kind,"resource_paths":["resources.toml"]}),
        };
        let result=saved(&s,json!({"name":format!("workflow-{kind}"),"config":{"source":variant,"include_variables":false,"include_user_groups":false,"match_tags":[]}}),true).await;
        let id = result["resource"]["id"].as_str().unwrap();
        let before = core.syncs.saved(id)["config"].clone();
        saved(
            &s,
            json!({"target":{"id":id},"patch":{"pending_alert":false}}),
            false,
        )
        .await;
        let after = core.syncs.saved(id)["config"].clone();
        for (k, v) in before.as_object().unwrap() {
            if k != "pending_alert" {
                assert_eq!(&after[k], v, "omitted patch field {k} changed");
            }
        }
        assert_eq!(core.count("execute"), 0);
        let writes = core.count("write");
        let cached = data(call(&s, "get_resource_sync_pending", json!({"id":id})).await);
        assert_eq!(cached["cached_status"], "pending");
        assert_eq!(core.count("write"), writes);
        let refreshed = data(call(&s, "refresh_resource_sync_pending", json!({"id":id})).await);
        assert_eq!(refreshed["refresh_requests_submitted"], 1);
        assert_eq!(core.count("write"), writes + 1);
        assert_eq!(core.count("execute"), 0);
        let reviewed=data(call(&s,"review_resource_sync_pending",json!({"target":{"id":id},"selection":{"kind":"resources","resource_type":"server","targets":[{"id":SERVER}]}})).await);
        assert!(reviewed["review_reference"].is_string(), "{reviewed}");
        assert_eq!(
            reviewed["plan"]["source"]["pending_git_revision"].is_null(),
            matches!(kind, "inline" | "host")
        );
        assert_eq!(reviewed["plan"]["variables_in_review_scope"], false);
        let applied=data(call(&s,"apply_resource_sync",json!({"review_reference":reviewed["review_reference"],"binding":reviewed["apply_binding"],"wait_timeout_seconds":1})).await);
        assert_eq!(applied["outcome"], "accepted");
        assert_eq!(applied["preflight"]["refresh_requests_submitted"], 1);
        assert_eq!(applied["run_sync_requests_submitted"], 1);
        assert_eq!(core.count("write"), writes + 2);
        assert_eq!(core.count("execute"), 1);
        assert_eq!(
            core.syncs.data.lock().unwrap().server["config"]["auto_prune"],
            true,
            "desired omission must default-reset the old false value"
        );
        let inspected = data(call(&s, "get_server", json!({"id":SERVER})).await);
        assert_eq!(inspected["name"], "workflow-server");
        assert_eq!(*core.source.lock().unwrap(), (source(), REVISION.into()));
        p.finish_excluding(s, &syncs::PRIVATE).await;
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let resumed=data(call(&s,"wait_for_resource_sync_update",json!({"binding":reviewed["apply_binding"],"update_id":applied["receipt"]["update_id"],"timeout_seconds":1})).await);
        assert_eq!(resumed["wait"]["end"], "complete");
        assert_eq!(resumed["selection_verified_by_core_history"], false);
        assert_eq!(core.count("execute"), 1);
        assert_eq!(core.count("write"), writes + 2);
        p.finish_excluding(s, &syncs::PRIVATE).await;
    }
}

fn data(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let text = serde_json::to_string(&result).unwrap();
    support::assert_safe(&text);
    for secret in syncs::PRIVATE.into_iter().chain(exports::PRIVATE) {
        assert!(!text.contains(secret), "private workflow value leaked");
    }
    result.structured_content.unwrap()
}

#[tokio::test]
async fn integrated_denial_source_drift_and_lost_ack_remain_single_submit_and_unconfirmed_after_restart()
 {
    let core = Core::start(source()).await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let created=saved(&s,json!({"name":"workflow-recovery","config":{"source":{"kind":"git","repo":"fixture/source","resource_paths":["resources.toml"]},"include_variables":false,"include_user_groups":false,"match_tags":[]}}),true).await;
    let id = created["resource"]["id"].as_str().unwrap();
    let input = json!({"target":{"id":id},"selection":{"kind":"whole_sync"}});
    let denied_review = data(call(&s, "review_resource_sync_pending", input.clone()).await);
    core.syncs.data.lock().unwrap().deny_refresh = true;
    let denied=data(call(&s,"apply_resource_sync",json!({"review_reference":denied_review["review_reference"],"binding":denied_review["apply_binding"]})).await);
    assert_eq!(denied["outcome"], "not_submitted");
    assert_eq!(denied["preflight"]["error"]["kind"], "permission_denied");
    assert_eq!(core.count("execute"), 0);
    core.syncs.data.lock().unwrap().deny_refresh = false;
    let stale = data(call(&s, "review_resource_sync_pending", input.clone()).await);
    *core.source.lock().unwrap() = (
        source() + "auto_prune=false\n",
        "abcdef0123456789abcdef0123456789abcdef01".into(),
    );
    let rejected = data(
        call(
            &s,
            "apply_resource_sync",
            json!({"review_reference":stale["review_reference"],"binding":stale["apply_binding"]}),
        )
        .await,
    );
    assert_eq!(rejected["outcome"], "not_submitted");
    assert_eq!(rejected["preflight"]["refresh_requests_submitted"], 1);
    assert_eq!(rejected["run_sync_requests_submitted"], 0);
    assert_eq!(core.count("execute"), 0);
    let reviewed = data(call(&s, "review_resource_sync_pending", input).await);
    assert!(reviewed["review_reference"].is_string());
    core.syncs.data.lock().unwrap().execute_fault = syncs::ExecuteFault::Lost;
    let apply = json!({"review_reference":reviewed["review_reference"],"binding":reviewed["apply_binding"],"wait_timeout_seconds":1});
    let unknown = data(call(&s, "apply_resource_sync", apply.clone()).await);
    assert_eq!(unknown["outcome"], "submission_unknown");
    assert!(unknown["receipt"]["update_id"].is_null());
    assert!(!unknown["receipt"]["submission_window"].is_null());
    assert_eq!(core.count("execute"), 1);
    assert_eq!(
        call(&s, "apply_resource_sync", apply)
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    p.finish_excluding(s, &syncs::PRIVATE).await;
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let before = (core.count("write"), core.count("execute"));
    // The historical wire fixture uses a separate fixed clock; unknown clock
    // alignment is represented by omitted window bounds, not false nonacceptance.
    let recovered=data(call(&s,"reconcile_operation",json!({"target":{"kind":"resource_sync","id":id},"requested_scope":{"kind":"resource_sync","selection":reviewed["apply_binding"]["selection"]},"operation":"run"})).await);
    assert_eq!(recovered["original_submission"], "unknown");
    assert_eq!(recovered["attribution"], "unconfirmed");
    assert_eq!(recovered["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(recovered["candidates"][0]["scope_evidence"], "unavailable");
    assert_eq!(recovered["automatic_resubmission"], false);
    assert_eq!((core.count("write"), core.count("execute")), before);
    p.finish_excluding(s, &syncs::PRIVATE).await;
}

#[tokio::test]
async fn shared_new_dependency_checks_every_creation_edge_before_deduplication() {
    let core = syncs::Core::start().await;
    let repo = "[[repo]]\nname='new-repo'\n[repo.config]\nbuilder_id='new-builder'\nrepo='fixture/source'\n";
    let builder = "[[builder]]\nname='new-builder'\n[builder.config]\ntype='Server'\n[builder.config.params]\nserver_ids=[]\n";
    let alerter = "[[alerter]]\nname='new-alerter'\n[alerter.config]\nresources=[{type='Builder',id='new-builder'}]\n";
    {
        let mut d = core.data.lock().unwrap();
        let s = d.syncs.get_mut(syncs::SYNC).unwrap();
        let source = format!("{repo}{builder}{alerter}");
        s["config"]["file_contents"] = json!(source);
        s["config"]["match_tags"] = json!([]);
        s["info"]["pending_error"] = Value::Null;
        s["info"]["remote_contents"] = json!([{"resource_path":"","path":"","contents":source}]);
        s["info"]["resource_updates"] = json!([
            {"target":{"type":"Repo","id":""},"data":{"type":"Create","data":{"name":"new-repo","proposed":repo}}},
            {"target":{"type":"Builder","id":""},"data":{"type":"Create","data":{"name":"new-builder","proposed":builder}}},
            {"target":{"type":"Alerter","id":""},"data":{"type":"Create","data":{"name":"new-alerter","proposed":alerter}}}
        ]);
    }
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let review = data(
        call(
            &s,
            "review_resource_sync_pending",
            json!({"target":{"id":syncs::SYNC},"selection":{"kind":"whole_sync"}}),
        )
        .await,
    );
    assert!(
        review["review_reference"].is_null(),
        "a shared dependency must not hide a later unsupported creation edge"
    );
    assert!(
        review["plan"]["coverage_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "desired_references_not_authoritatively_observed")
    );
    assert!(core.requests("write").is_empty() && core.requests("execute").is_empty());
    p.finish_excluding(s, &syncs::PRIVATE).await;
}
