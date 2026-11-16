//! Production stdio boundaries. Actual owned-engine removals are accepted in #112.
#[path = "support/pruning.rs"]
mod fixture;
mod support;
use fixture::{Core, PRIVATE, Reply, SERVER, UPDATE};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
fn safe(r: &CallToolResult) {
    let s = serde_json::to_string(r).unwrap();
    support::assert_safe(&s);
    for secret in PRIVATE {
        assert!(!s.contains(secret), "private prune evidence escaped");
    }
}
fn data(r: CallToolResult) -> Value {
    safe(&r);
    assert_eq!(r.is_error, Some(false), "{r:?}");
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    safe(&r);
    assert_eq!(r.is_error, Some(true), "{r:?}");
    r.structured_content.unwrap()
}
fn input(category: &str) -> Value {
    json!({"target":{"id":SERVER},"category":category})
}
async fn preview(s: &Session, category: &str) -> Value {
    data(call(s, "preview_docker_prune", input(category)).await)
}
async fn apply(s: &Session, category: &str, p: &Value) -> CallToolResult {
    let mut v = input(category);
    v["preview_id"] = p["preview_id"].clone();
    v["acknowledge_destructive_effects"] = json!(true);
    call(s, "apply_docker_prune", v).await
}
async fn finish(p: Process, s: Session, c: &Core) {
    {
        let d = c.data.lock().unwrap();
        assert!(!d.bad_credentials);
        assert!(d.requests.iter().all(|r| r.path != "unexpected"));
    }
    p.finish_excluding(s, &PRIVATE).await;
}
#[tokio::test]
async fn every_profile_requires_pruning_plus_operate_without_configuration_edit() {
    for operate in [false, true] {
        for edit in [false, true] {
            for pruning in [false, true] {
                let c = Core::start().await;
                let (p, s) =
                    Process::start_profile_pruning(&c.address, operate, edit, pruning).await;
                let tools = s.list_tools(None).await.unwrap().tools;
                assert_eq!(
                    tools.len(),
                    64 + 20 * usize::from(operate)
                        + 55 * usize::from(edit)
                        + 4 * usize::from(operate && edit)
                        + 2 * usize::from(operate && pruning)
                );
                for name in ["preview_docker_prune", "apply_docker_prune"] {
                    assert_eq!(tools.iter().any(|t| t.name == name), operate && pruning);
                    if !operate || !pruning {
                        let before = c.data.lock().unwrap().requests.len();
                        assert!(
                            s.call_tool(
                                rmcp::model::CallToolRequestParams::new(name)
                                    .with_arguments(json!({}).as_object().unwrap().clone())
                            )
                            .await
                            .is_err()
                        );
                        assert_eq!(c.data.lock().unwrap().requests.len(), before);
                    }
                }
                if operate && pruning {
                    let v = preview(&s, "image").await;
                    assert_eq!(v["prune_submissions"], 0);
                    assert_eq!(v["exact_dry_run_available"], false);
                    assert_eq!(v["server"]["id"], SERVER);
                }
                assert!(c.executions().is_empty());
                finish(p, s, &c).await;
            }
        }
    }
}
#[tokio::test]
async fn exact_five_categories_submit_only_typed_server_and_keep_known_update() {
    for (category, operation) in [
        ("image", "PruneImages"),
        ("container", "PruneContainers"),
        ("network", "PruneNetworks"),
        ("volume", "PruneVolumes"),
        ("system", "PruneSystem"),
    ] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        let v = preview(&s, category).await;
        assert!(c.executions().is_empty());
        if category == "system" {
            assert!(
                v["destructive_effect"]
                    .as_str()
                    .unwrap()
                    .contains("anonymous volumes")
            );
            assert!(
                v["destructive_effect"]
                    .as_str()
                    .unwrap()
                    .contains("build cache")
            );
            assert_eq!(v["supporting_inventory"].as_array().unwrap().len(), 4);
        }
        if category == "volume" {
            assert!(v["destructive_effect"].as_str().unwrap().contains("named"));
        }
        if category == "image" {
            assert!(v["destructive_effect"].as_str().unwrap().contains("tagged"));
        }
        let r = data(apply(&s, category, &v).await);
        assert_eq!(r["prune_submissions"], 1);
        assert_eq!(r["receipt"]["submission"], "accepted");
        assert_eq!(r["receipt"]["core_operation"], operation);
        assert_eq!(r["receipt"]["update_id"], UPDATE);
        assert_eq!(r["receipt"]["automatic_resubmission"], false);
        assert_eq!(c.executions().len(), 1);
        assert_eq!(c.executions()[0].operation, operation);
        assert_eq!(c.executions()[0].params, json!({"server":SERVER}));
        data(
            call(
                &s,
                "wait_for_update",
                json!({"id":UPDATE,"timeout_seconds":1}),
            )
            .await,
        );
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn explicit_ack_is_required_and_cache_failure_never_changes_the_category() {
    let c = Core::start().await;
    c.data.lock().unwrap().inventory_status = Some(403);
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let v = preview(&s, "image").await;
    assert_eq!(v["supporting_inventory"][0]["scan_incomplete"], true);
    assert!(v["supporting_inventory"][0]["cached_observed_count"].is_null());
    let mut a = input("image");
    a["preview_id"] = v["preview_id"].clone();
    a["acknowledge_destructive_effects"] = json!(false);
    let before = c.data.lock().unwrap().requests.len();
    assert_eq!(
        error(call(&s, "apply_docker_prune", a).await)["kind"],
        "invalid_arguments"
    );
    assert_eq!(c.data.lock().unwrap().requests.len(), before);
    data(apply(&s, "image", &v).await);
    assert_eq!(c.executions().len(), 1);
    assert_eq!(c.executions()[0].operation, "PruneImages");
    finish(p, s, &c).await;
}
#[tokio::test]
async fn unknown_categories_options_and_missing_targets_fail_before_core_io() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, true, true).await;
    for v in [
        json!({"category":"system"}),
        json!({"target":{},"category":"image"}),
        json!({"target":{"id":SERVER},"category":"builder"}),
        json!({"target":{"id":SERVER},"category":"buildx"}),
        json!({"target":{"id":SERVER},"category":"everything"}),
        json!({"target":{"id":SERVER},"category":"volume","name":"one-volume"}),
        json!({"target":{"id":SERVER},"category":"image","flags":["--all"]}),
        json!({"target":{"id":SERVER,"name":"x"},"category":"network"}),
    ] {
        let n = c.data.lock().unwrap().requests.len();
        assert_eq!(
            error(call(&s, "preview_docker_prune", v).await)["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.data.lock().unwrap().requests.len(), n);
    }
    assert!(c.executions().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn failed_or_ambiguous_server_resolution_never_prunes_another_target() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let r = error(
        call(
            &s,
            "preview_docker_prune",
            json!({"target":{"id":"000000000000000000000099"},"category":"system"}),
        )
        .await,
    );
    assert!(matches!(
        r["kind"].as_str(),
        Some("not_found" | "upstream" | "upstream_server")
    ));
    {
        let mut d = c.data.lock().unwrap();
        let mut other = d.summaries[0].clone();
        other["id"] = json!("000000000000000000000099");
        d.summaries.push(other);
    }
    let name = c.data.lock().unwrap().server["name"].clone();
    assert_eq!(
        error(
            call(
                &s,
                "preview_docker_prune",
                json!({"target":{"name":name},"category":"volume"})
            )
            .await
        )["kind"],
        "ambiguous_target"
    );
    assert!(c.executions().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn read_only_core_or_disabled_caller_denies_pruning_even_with_both_local_flags() {
    for disabled in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if disabled {
                d.caller["enabled"] = json!(false);
            } else {
                d.permission = json!("Read");
            }
        }
        let (p, s) = Process::start_profile_pruning(&c.address, true, true, true).await;
        assert_eq!(
            error(call(&s, "preview_docker_prune", input("system")).await)["kind"],
            "permission_denied"
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn disabled_or_template_servers_are_not_substituted_or_executed() {
    for field in ["template", "enabled"] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if field == "template" {
                d.server["template"] = json!(true);
                d.summaries[0]["template"] = json!(true);
            } else {
                d.server["config"]["enabled"] = json!(false);
            }
        }
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        assert_eq!(
            error(call(&s, "preview_docker_prune", input("container")).await)["kind"],
            "unsupported_target"
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn private_connection_authority_and_category_drift_consume_preview_without_pruning() {
    for field in ["connection", "permission", "caller", "category"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        let v = preview(&s, "network").await;
        {
            let mut d = c.data.lock().unwrap();
            match field {
                "connection" => {
                    d.server["config"]["private_extension"] = json!("secret-only-change")
                }
                "permission" => d.permission = json!("Write"),
                "caller" => d.caller["super_admin"] = json!(true),
                _ => (),
            }
        }
        let r = error(
            apply(
                &s,
                if field == "category" {
                    "system"
                } else {
                    "network"
                },
                &v,
            )
            .await,
        );
        assert_eq!(
            r["kind"],
            if field == "category" {
                "invalid_preview"
            } else {
                "stale_preview"
            }
        );
        assert!(c.executions().is_empty());
        assert_eq!(
            error(apply(&s, "network", &v).await)["kind"],
            "invalid_preview"
        );
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn final_transport_rechecks_secret_connection_after_handler_preflight() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let v = preview(&s, "image").await;
    {
        let mut d = c.data.lock().unwrap();
        let mut next = d.server.clone();
        next["config"]["private_extension"] = json!("late-private-rotation");
        let at = d.server_reads + 4;
        d.drift_server = Some((at, next));
    }
    assert_eq!(error(apply(&s, "image", &v).await)["kind"], "stale_preview");
    assert!(c.executions().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn concurrent_apply_of_one_token_submits_only_one_prune() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let v = preview(&s, "image").await;
    let (a, b) = tokio::join!(apply(&s, "image", &v), apply(&s, "image", &v));
    safe(&a);
    safe(&b);
    assert_ne!(a.is_error, b.is_error);
    assert_eq!(c.executions().len(), 1);
    assert_eq!(c.data.lock().unwrap().accepted_prunes, 1);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn uncertain_or_mismatched_acknowledgements_never_replay_and_remain_unconfirmed() {
    for reply in [
        Reply::Lost,
        Reply::Busy,
        Reply::Stalled,
        Reply::Malformed,
        Reply::Oversized,
        Reply::Redirect,
        Reply::WrongTarget,
        Reply::WrongOperation,
        Reply::WrongId,
        Reply::BeforeError,
        Reply::Denied,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().reply = reply;
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        let v = preview(&s, "volume").await;
        let r = data(apply(&s, "volume", &v).await);
        assert_eq!(
            r["receipt"]["submission"],
            if matches!(reply, Reply::Denied | Reply::Busy) {
                "rejected"
            } else {
                "unknown"
            }
        );
        assert!(r["receipt"]["update_id"].is_null());
        assert_eq!(c.executions().len(), 1);
        assert_eq!(
            error(apply(&s, "volume", &v).await)["kind"],
            "invalid_preview"
        );
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn failed_core_update_and_denied_followup_keep_known_receipt_without_retry() {
    for denied in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.update_success = false;
            if denied {
                d.history_status = Some(403);
            }
        }
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        let v = preview(&s, "system").await;
        let r = data(apply(&s, "system", &v).await);
        assert_eq!(r["receipt"]["submission"], "accepted");
        assert_eq!(r["receipt"]["update_id"], UPDATE);
        if denied {
            assert!(!r["receipt"]["observation_error"].is_null());
        } else {
            assert_eq!(r["receipt"]["latest_update"]["outcome"], "failed");
        }
        assert_eq!(c.executions().len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn restart_invalidates_tokens_but_read_only_recovery_needs_no_prune_optin() {
    let c = Core::start().await;
    c.data.lock().unwrap().reply = Reply::Lost;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let old = preview(&s, "image").await;
    let v = preview(&s, "image").await;
    data(apply(&s, "image", &v).await);
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r=data(call(&s,"reconcile_operation",json!({"target":{"kind":"server","id":SERVER},"requested_scope":{"kind":"docker_prune","category":"image"},"operation":"prune"})).await);
    assert_eq!(r["core_operation"], "PruneImages");
    assert_eq!(r["original_submission"], "unknown");
    assert_eq!(r["attribution"], "unconfirmed");
    data(
        call(
            &s,
            "wait_for_update",
            json!({"id":UPDATE,"timeout_seconds":1}),
        )
        .await,
    );
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    assert_eq!(
        error(apply(&s, "image", &old).await)["kind"],
        "invalid_preview"
    );
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn restarted_history_omits_prune_object_names_commands_and_revision_payloads() {
    for op in [
        "PruneImages",
        "PruneContainers",
        "PruneNetworks",
        "PruneVolumes",
        "PruneSystem",
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().update = fixture::update(op, false);
        for _ in 0..2 {
            let (p, s) = Process::start_profile(&c.address, false, false).await;
            let r = data(call(&s, "get_update", json!({"id":UPDATE})).await);
            assert!(r["observed_revision"].is_null());
            assert_eq!(r["stages"][0]["stage"], "Docker prune operation");
            assert_eq!(r["stages"][0]["stderr_truncated"], true);
            assert!(c.executions().is_empty());
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn final_transport_denies_execute_permission_or_enabled_state_lost_after_handler_check() {
    for permission in [false, true] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
        let v = preview(&s, "network").await;
        {
            let mut d = c.data.lock().unwrap();
            if permission {
                d.drift_permission = Some((3, json!("Read")));
            } else {
                let mut caller = d.caller.clone();
                caller["enabled"] = json!(false);
                d.drift_caller = Some((3, caller));
            }
        }
        assert_eq!(
            error(apply(&s, "network", &v).await)["kind"],
            "permission_denied"
        );
        assert!(c.executions().is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn changing_cached_objects_never_becomes_a_frozen_set_or_skips_an_explicit_prune() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let v = preview(&s, "volume").await;
    assert_eq!(v["supporting_inventory"][0]["cached_observed_count"], 0);
    c.data.lock().unwrap().reads.insert("ListVolumes".into(),json!([{"name":"new-unused-owned-data","driver":"local","mountpoint":PRIVATE[1],"created":"2026-11-15T00:00:00Z","scope":"local","size":1,"in_use":false}]));
    let r = data(apply(&s, "volume", &v).await);
    assert_eq!(r["prune_submissions"], 1);
    assert_eq!(r["supporting_inventory"][0]["cached_observed_count"], 0);
    assert_eq!(r["exact_dry_run_available"], false);
    assert_eq!(c.executions().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn independent_core_auto_prune_policy_is_visible_bound_and_never_changed() {
    let c = Core::start().await;
    c.data.lock().unwrap().server["config"]["auto_prune"] = json!(true);
    let (p, s) = Process::start_profile_pruning(&c.address, true, false, true).await;
    let v = preview(&s, "image").await;
    assert_eq!(v["server"]["automatic_image_pruning_enabled"], true);
    assert!(c.executions().is_empty());
    c.data.lock().unwrap().server["config"]["auto_prune"] = json!(false);
    assert_eq!(error(apply(&s, "image", &v).await)["kind"], "stale_preview");
    assert!(c.executions().is_empty());
    let next = preview(&s, "image").await;
    assert_eq!(next["server"]["automatic_image_pruning_enabled"], false);
    let applied = data(apply(&s, "image", &next).await);
    assert_eq!(applied["server"]["automatic_image_pruning_enabled"], false);
    assert_eq!(c.executions().len(), 1);
    assert_eq!(c.data.lock().unwrap().server["config"]["auto_prune"], false);
    finish(p, s, &c).await;
}
