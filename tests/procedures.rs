//! Procedure definitions/execution acceptance through production MCP stdio.
#[allow(dead_code)] // Reused definition fixture also contains modes covered by its own suite.
#[path = "support/build_definitions.rs"]
mod definitions;
#[path = "support/procedures.rs"]
mod fixture;
mod support;
use fixture::{ACTION, Ack, BUILD, CHILD, Core, Fault, Outcome, PRIVATE, PROCEDURE};
use komodo_client::entities::update::UpdateStatus;
use rmcp::{
    model::{CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    let s = serde_json::to_string(&r).unwrap();
    support::assert_safe(&s);
    for secret in PRIVATE {
        assert!(!s.contains(secret), "private Procedure data leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn edit(patch: Value) -> Value {
    json!({"target":{"id":PROCEDURE},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
fn run() -> Value {
    json!({"target":{"name":"fixture-procedure"},"expected_effects":{"core_admin_authority":true,"broader_effects_possible":true}})
}
fn cancel(id: &Value) -> Value {
    json!({"target":{"id":PROCEDURE},"expected_update_id":id})
}
fn recovery(op: &str) -> Value {
    json!({"target":{"kind":"procedure","id":PROCEDURE},"requested_scope":{"kind":"whole_procedure"},"operation":op})
}
fn stages() -> Value {
    json!([
        {"name":"first","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"sleep","duration_ms":1}},{"enabled":false,"execution":{"kind":"run_action","resource_id":ACTION,"args":{"private":PRIVATE[7]}}}]},
        {"name":"parallel","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"run_build","resource_id":BUILD}},{"enabled":true,"execution":{"kind":"run_action","resource_id":ACTION,"args":{"private":PRIVATE[7]}}}]},
        {"name":"disabled","enabled":false,"executions":[{"enabled":true,"execution":{"kind":"none"}}]}
    ])
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    assert!(!c.definitions.data.lock().unwrap().bad_credentials);
    assert!(c.definitions.requests("write").is_empty());
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn all_profiles_guard_discovery_direct_dispatch_and_existing_execution_effects_before_io() {
    for operate in [false, true] {
        for edit_grant in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit_grant).await;
            let reads = c.requests("read").len();
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                27 + 16 * usize::from(operate)
                    + 40 * usize::from(edit_grant)
                    + 2 * usize::from(operate && edit_grant)
            );
            for name in [
                "list_procedures",
                "get_procedure",
                "preview_procedure_create",
                "apply_procedure_create",
                "preview_procedure_update",
                "apply_procedure_update",
                "run_procedure",
                "cancel_procedure",
            ] {
                let allowed = if name.starts_with("run_") || name.starts_with("cancel_") {
                    operate
                } else if name.starts_with("preview_") || name.starts_with("apply_") {
                    edit_grant
                } else {
                    true
                };
                assert_eq!(tools.iter().any(|t| t.name == name), allowed);
                let r = s
                    .call_tool(
                        CallToolRequestParams::new(name).with_arguments(
                            json!({"unexpected":true}).as_object().unwrap().clone(),
                        ),
                    )
                    .await;
                if allowed {
                    assert_eq!(r.unwrap().is_error, Some(true));
                } else {
                    assert!(r.is_err());
                }
            }
            assert_eq!(c.requests("read").len(), reads);
            assert!(c.requests("write").is_empty());
            assert!(c.requests("execute").is_empty());
            if edit_grant && !operate {
                for patch in [
                    json!({"failure_alert":false}),
                    json!({"webhook_enabled":false}),
                    json!({"webhook_enabled":true}),
                    json!({"stages":[]}),
                ] {
                    let mut input = edit(patch);
                    input["preview_id"] = json!("unconsumed");
                    assert_eq!(
                        error(call(&s, "apply_procedure_update", input).await)["kind"],
                        "capability_disabled"
                    );
                }
                assert_eq!(c.requests("read").len(), reads);
            }
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn inspect_and_paginate_preserve_stage_order_parallel_groups_and_disabled_entries_without_arguments()
 {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        let mut other = d.procedures[PROCEDURE].clone();
        other["_id"]["$oid"] = json!("000000000000000000000031");
        other["name"] = json!("zeta-procedure");
        d.procedures
            .insert("000000000000000000000031".into(), other);
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let listed = data(call(&s, "list_procedures", json!({"page_size":1})).await);
    assert_eq!(listed["next_page"], 1);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        data(call(&s, "list_procedures", json!({"page_size":1,"page":1})).await)["has_more"],
        false
    );
    let detail = data(call(&s, "get_procedure", json!({"name":"fixture-procedure"})).await);
    let d = &detail["definition"];
    assert_eq!(d["stage_count"], 4);
    assert_eq!(d["stages"][0]["name"], "prepare");
    assert_eq!(d["stages"][0]["executions"][1]["enabled"], false);
    assert_eq!(d["stages"][1]["execution_count"], 2);
    assert_eq!(d["stages"][2]["enabled"], false);
    assert_eq!(
        d["stages"][1]["executions"][1]["arguments_configured"],
        true
    );
    assert!(d["authority"].as_str().unwrap().contains("administrator"));
    assert_eq!(
        detail["effects"]["acknowledgement"],
        run()["expected_effects"]
    );
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn inert_creation_and_one_field_edit_have_scoped_payloads_and_complete_private_readback() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let input = json!({"name":"New Procedure","config":{"stages":stages()}});
    let preview = data(call(&s, "preview_procedure_create", input.clone()).await);
    assert!(c.requests("write").is_empty());
    let saved = data(call(&s, "apply_procedure_create", apply(&input, &preview)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(saved["resource"]["name"], "New Procedure");
    let wire = c.requests("write")[0].params.clone();
    assert_eq!(wire["config"]["schedule"], "");
    assert_eq!(wire["config"]["schedule_enabled"], false);
    assert_eq!(wire["config"]["webhook_enabled"], false);
    assert_eq!(wire["config"].as_object().unwrap().len(), 4);
    assert_eq!(
        wire["config"]["stages"][0]["executions"][1]["enabled"],
        false
    );
    assert_eq!(
        wire["config"]["stages"][1]["executions"][1]["execution"]["params"]["args"]["private"],
        PRIVATE[7]
    );
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
    let before = c.procedure();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let input = edit(json!({"failure_alert":false}));
    let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
    let saved = data(call(&s, "apply_procedure_update", apply(&input, &preview)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        c.requests("write")[1].params,
        json!({"id":PROCEDURE,"config":{"failure_alert":false}})
    );
    let after = c.procedure();
    for (k, v) in before["config"].as_object().unwrap() {
        if k != "failure_alert" {
            assert_eq!(&after["config"][k], v);
        }
    }
    finish(p, s, &c).await;
}

#[tokio::test]
async fn typed_stage_replacement_and_deliberate_clear_never_flatten_parallel_or_disabled_steps() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let mut cleared = stages();
    cleared[0]["executions"][1]["execution"]["args"] = json!({});
    cleared[1]["executions"][1]["execution"]["args"] = json!({});
    for replacement in [stages(), cleared, json!([])] {
        let input = edit(json!({"stages":replacement}));
        let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
        assert_eq!(preview["changes"][0]["redacted"], true);
        let r = data(call(&s, "apply_procedure_update", apply(&input, &preview)).await);
        assert_eq!(r["saved_configuration_verified"], true);
        let wire = c.requests("write").last().unwrap().params.clone();
        assert_eq!(wire["config"].as_object().unwrap().len(), 1);
        if !replacement.as_array().unwrap().is_empty() {
            let v = &wire["config"]["stages"];
            assert_eq!(v[0]["name"], "first");
            assert_eq!(v[0]["executions"][1]["enabled"], false);
            assert_eq!(v[1]["executions"].as_array().unwrap().len(), 2);
            assert_eq!(v[2]["enabled"], false);
            assert_eq!(
                v[1]["executions"][1]["execution"]["params"]["args"],
                replacement[1]["executions"][1]["execution"]["args"]
            );
        } else {
            assert_eq!(c.procedure()["config"]["stages"], json!([]));
        }
    }
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn edit_only_noop_is_write_free_and_unknown_existing_step_types_are_preserved_on_unrelated_edits()
 {
    let c = Core::start().await;
    c.data
        .lock()
        .unwrap()
        .procedures
        .get_mut(PROCEDURE)
        .unwrap()["config"]["stages"][0]["executions"][0]["execution"] =
        json!({"type":"BuildRepo","params":{"repo":fixture::REPO}});
    let before = c.procedure();
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let input = edit(json!({}));
    let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
    assert_eq!(preview["outcome"], "no_op");
    assert_eq!(
        data(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["outcome"],
        "no_op"
    );
    assert!(c.requests("write").is_empty());
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let detail = data(call(&s, "get_procedure", json!({"id":PROCEDURE})).await);
    assert_eq!(
        detail["definition"]["stages"][0]["executions"][0]["writable"],
        false
    );
    let input = edit(json!({"failure_alert":false}));
    let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
    assert_eq!(
        data(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(
        c.procedure()["config"]["stages"],
        before["config"]["stages"]
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn private_baseline_and_reference_changes_invalidate_consumed_previews() {
    for reference in [false, true] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let input = edit(json!({"failure_alert":false}));
        let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
        if reference {
            c.definitions
                .data
                .lock()
                .unwrap()
                .builds
                .get_mut(BUILD)
                .unwrap()["config"]["webhook_secret"] = json!("changed-private-reference");
        } else {
            c.data
                .lock()
                .unwrap()
                .procedures
                .get_mut(PROCEDURE)
                .unwrap()["config"]["webhook_secret"] = json!("changed-private-baseline");
        }
        assert_eq!(
            error(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["kind"],
            "stale_preview"
        );
        assert_eq!(
            error(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["kind"],
            "invalid_preview"
        );
        assert!(c.requests("write").is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn core_checks_editor_execute_admin_even_disabled_steps_and_creation_policy_without_fallback()
{
    for mode in 0..3 {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.admin = mode != 0;
            d.editor_execute = mode != 1;
            d.can_create = mode != 2;
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let input = if mode == 2 {
            json!({"name":"new","config":{"stages":[]}})
        } else {
            edit(
                json!({"stages":[{"name":"disabled","enabled":false,"executions":[{"enabled":false,"execution":if mode==0{json!({"kind":"batch_run_build","pattern":"owned-*"})}else{json!({"kind":"run_build","resource_id":BUILD})}}]}]}),
            )
        };
        let (create, apply_name) = if mode == 2 {
            ("preview_procedure_create", "apply_procedure_create")
        } else {
            ("preview_procedure_update", "apply_procedure_update")
        };
        let preview = data(call(&s, create, input.clone()).await);
        let r = data(call(&s, apply_name, apply(&input, &preview)).await);
        assert_eq!(r["saved_configuration_verified"], false);
        assert_eq!(r["observation_error"]["kind"], "permission_denied");
        assert_eq!(c.requests("write").len(), 1);
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn schedules_nulls_generic_execution_payloads_and_self_reference_cannot_bypass_definition_rules()
 {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let reads = c.requests("read").len();
    for patch in [
        json!({"schedule":"* * * * * *"}),
        json!({"schedule_enabled":true}),
        json!({"schedule_timezone":"UTC"}),
        json!({"stages":null}),
        json!({"stages":[{"name":"keep-private","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"run_action","resource_id":ACTION}}]}]}),
        json!({"stages":[{"name":"keep-private","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"run_action","resource_id":ACTION,"args":null}}]}]}),
        json!({"stages":[{"name":"bad","enabled":true,"executions":[{"enabled":true,"execution":{"type":"RunBuild","params":{"build":BUILD}}}]}]}),
    ] {
        assert_eq!(
            error(call(&s, "preview_procedure_update", edit(patch)).await)["kind"],
            "invalid_arguments"
        );
    }
    assert_eq!(c.requests("read").len(), reads);
    for execution in [
        json!({"kind":"run_procedure","resource_id":PROCEDURE}),
        json!({"kind":"sleep","duration_ms":86400001}),
        json!({"kind":"run_build","resource_id":"not-an-id"}),
    ] {
        assert_eq!(error(call(&s,"preview_procedure_update",edit(json!({"stages":[{"name":"one","enabled":false,"executions":[{"enabled":false,"execution":execution}]}]}))).await)["kind"],"invalid_arguments");
    }
    assert!(c.requests("write").is_empty());
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn ambiguous_writes_and_independent_readback_failures_never_retry() {
    for fault in [
        Fault::Denied,
        Fault::Lost,
        Fault::MissingEcho,
        Fault::ChangeUnrelated,
        Fault::ReadbackDenied,
    ] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let input = edit(json!({"failure_alert":false}));
        let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
        c.data.lock().unwrap().fault = fault;
        let r = data(call(&s, "apply_procedure_update", apply(&input, &preview)).await);
        assert_eq!(r["saved_configuration_verified"], false);
        assert!(r["observation_error"].is_object());
        assert_eq!(
            r["outcome"],
            if matches!(fault, Fault::ChangeUnrelated | Fault::ReadbackDenied) {
                "saved"
            } else {
                "submission_unknown"
            }
        );
        assert_eq!(
            error(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["kind"],
            "invalid_preview"
        );
        assert_eq!(c.requests("write").len(), 1);
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn saved_run_has_one_stable_id_request_sequential_parallel_reports_and_no_definition_writes()
{
    let c = Core::start().await;
    let before = c.procedure();
    {
        let mut d = c.data.lock().unwrap();
        d.admin = false;
        d.editor_execute = false;
    }
    c.definitions.data.lock().unwrap().deny_read = Some("GetBuild".into());
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let inspected = data(call(&s, "get_procedure", json!({"id":PROCEDURE})).await);
    let mut input = run();
    input["expected_effects"] = inspected["effects"]["acknowledgement"].clone();
    let r = data(call(&s, "run_procedure", input).await);
    assert_eq!(r["submission"], "accepted");
    assert_eq!(r["resource"]["kind"], "procedure");
    assert_eq!(r["requested_scope"]["kind"], "whole_procedure");
    assert_eq!(r["latest_update"]["outcome"], "succeeded");
    let o = &r["latest_update"]["observed_procedure_result"];
    assert_eq!(
        o["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "prepare", "prepare", "parallel", "parallel", "deploy", "deploy"
        ]
    );
    assert_eq!(o["executions"].as_array().unwrap().len(), 8);
    assert!(
        o["executions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["terminal_success"].is_null())
    );
    assert!(
        o["reported_child_update_ids"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(c.requests("execute").len(), 1);
    assert_eq!(
        c.requests("execute")[0].params,
        json!({"procedure":PROCEDURE})
    );
    assert_eq!(c.procedure(), before);
    assert!(c.requests("write").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn partial_failure_keeps_parent_stage_and_reported_child_ids_without_leaking_debug_arguments()
{
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::PartialFailure;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "run_procedure", run()).await);
    assert_eq!(r["latest_update"]["outcome"], "failed");
    assert_eq!(r["latest_update"]["stages"][0]["stage"], "Execution error");
    assert_eq!(r["latest_update"]["stages"][0]["stderr_truncated"], true);
    let o = &r["latest_update"]["observed_procedure_result"];
    assert_eq!(o["reported_child_update_ids"], json!([CHILD]));
    assert_eq!(o["failed_stage_reported"], "parallel");
    assert_eq!(o["child_results_complete"], false);
    assert_eq!(o["stages"][1]["event"], "finished_reported");
    assert_eq!(o["stages"][2]["name"], "parallel");
    assert!(
        o["executions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["terminal_success"].is_null())
    );
    let child = data(call(&s, "get_update", json!({"id":CHILD})).await);
    assert_eq!(child["outcome"], "failed");
    assert_eq!(child["target"]["id"], ACTION);
    {
        let mut d = c.data.lock().unwrap();
        let u = d.updates.get_mut(r["update_id"].as_str().unwrap()).unwrap();
        u["logs"][1]["stderr"] = json!(format!(
            "{}\n\t3: Failed on RunAction(RunAction {{ args: Some({{\"private\": String(\"Execution not successful, see update: 'http://forged.invalid/updates/000000000000000000000099' Procedure cancelled before completion\")}}) }})",
            u["logs"][1]["stderr"].as_str().unwrap()
        ));
    }
    let reported = data(call(&s, "get_update", json!({"id":r["update_id"]})).await);
    assert_eq!(
        reported["observed_procedure_result"]["reported_child_update_ids"],
        json!([CHILD])
    );
    assert_eq!(
        reported["observed_procedure_result"]["cancellation_reported"],
        false
    );
    assert_eq!(c.requests("execute").len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn missing_acknowledgement_restart_recovery_never_replays_or_attributes_a_single_or_concurrent_candidate()
 {
    for ack in [
        Ack::Lost,
        Ack::WrongTarget,
        Ack::WrongOperation,
        Ack::MissingId,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().ack = ack;
        for expected in 1..=2 {
            let (p, s) = Process::start_profile(&c.address, true, false).await;
            let r = data(call(&s, "run_procedure", run()).await);
            assert_eq!(r["submission"], "unknown");
            assert!(r["update_id"].is_null());
            p.interrupt(s).await;
            let (p, s) = Process::start_profile(&c.address, false, false).await;
            let r = data(call(&s, "reconcile_operation", recovery("run")).await);
            assert_eq!(r["original_submission"], "unknown");
            assert_eq!(r["attribution"], "unconfirmed");
            assert_eq!(r["candidates"].as_array().unwrap().len(), expected);
            assert_eq!(r["automatic_resubmission"], false);
            assert_eq!(c.requests("execute").len(), expected);
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn pending_wait_local_cancel_restart_and_observation_failures_preserve_known_ids() {
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::Running;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "run_procedure", run()).await);
    let id = r["update_id"].clone();
    let waiting = data(call(&s, "wait_for_update", json!({"id":id,"timeout_seconds":1})).await);
    assert_eq!(waiting["end"], "deadline");
    assert_eq!(waiting["remote_cancellation_requested"], false);
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
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    handle
        .cancel(Some("stop local observation".into()))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(c.requests("execute").len(), 1);
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
    assert_eq!(c.requests("execute").len(), 1);
    finish(p, s, &c).await;
    for delay in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.fail_updates = !delay;
            if delay {
                d.update_delay = std::time::Duration::from_secs(3);
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "run_procedure", run()).await);
        assert_eq!(r["submission"], "accepted");
        assert!(r["update_id"].is_string());
        assert_eq!(
            r["observation_error"]["kind"],
            if delay { "timeout" } else { "upstream" }
        );
        assert_eq!(c.requests("execute").len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn concurrent_callers_keep_distinct_ids_and_later_busy_failure_is_not_the_active_run() {
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::Running;
    let (p1, s1) = Process::start_profile(&c.address, true, false).await;
    let (p2, s2) = Process::start_profile(&c.address, true, false).await;
    let (a, b) = tokio::join!(
        call(&s1, "run_procedure", run()),
        call(&s2, "run_procedure", run())
    );
    let (a, b) = (data(a), data(b));
    assert_ne!(a["update_id"], b["update_id"]);
    let active = if a["latest_update"]["outcome"] == "running" {
        &a
    } else {
        &b
    };
    let cancelled = data(call(&s1, "cancel_procedure", cancel(&active["update_id"])).await);
    assert_eq!(cancelled["outcome"], "acknowledged");
    assert_eq!(cancelled["replacement_run_observed"], false);
    assert_eq!(c.requests("execute").len(), 3);
    finish(p1, s1, &c).await;
    finish(p2, s2, &c).await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r = data(call(&s, "reconcile_operation", recovery("run")).await);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(r["attribution"], "unconfirmed");
    assert_eq!(c.requests("execute").len(), 3);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn cancellation_acknowledges_token_separately_from_completion_after_stage_or_last_stage_success()
 {
    for mode in 0..3 {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        {
            let mut d = c.data.lock().unwrap();
            d.cancel_finishes_original = mode > 0;
            d.cancel_original_succeeds = mode == 2;
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_procedure", cancel(&id)).await);
        assert_eq!(r["outcome"], "acknowledged");
        assert_eq!(r["targeting"], "resource");
        assert_eq!(r["original_terminal_confirmed"], mode > 0);
        assert_eq!(
            r["original_update"]["outcome"],
            match mode {
                0 => "running",
                1 => "failed",
                _ => "succeeded",
            }
        );
        assert_eq!(
            r["cancellation_receipt"]["latest_update"]["outcome"],
            "succeeded"
        );
        assert!(r["targeting_limit"].as_str().unwrap().contains("ignores"));
        if mode == 1 {
            assert_eq!(
                r["original_update"]["observed_procedure_result"]["cancellation_reported"],
                true
            );
        }
        assert_eq!(c.requests("execute").len(), 1);
        assert_eq!(
            c.requests("execute")[0].params,
            json!({"procedure":PROCEDURE,"update_id":null})
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn finished_replaced_ambiguous_or_truncated_runs_never_receive_resource_cancellation() {
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
        let r = data(call(&s, "cancel_procedure", cancel(&id)).await);
        assert_eq!(r["outcome"], "not_submitted");
        assert_eq!(
            r["not_submitted_reason"],
            match mode {
                0 => "original_complete",
                1 | 2 => "replacement_run",
                3 => "ambiguous_active_runs",
                _ => "incomplete_active_evidence",
            }
        );
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    let id = json!(c.seed_run(UpdateStatus::InProgress));
    c.data.lock().unwrap().replace_on_cancel = true;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = data(call(&s, "cancel_procedure", cancel(&id)).await);
    assert_eq!(r["replacement_run_observed"], true);
    assert_ne!(r["active_update_id"], id);
    assert_eq!(r["original_update"]["outcome"], "succeeded");
    assert_eq!(c.requests("execute").len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn cancellation_lost_reply_denial_and_post_ack_read_failure_preserve_uncertainty_and_ids() {
    for mode in 0..3 {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        {
            let mut d = c.data.lock().unwrap();
            d.ack = match mode {
                0 => Ack::Lost,
                1 => Ack::Denied,
                _ => Ack::Valid,
            };
            d.fail_observation_after_cancel = mode == 2;
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_procedure", cancel(&id)).await);
        assert_eq!(
            r["outcome"],
            match mode {
                0 => "unknown",
                1 => "rejected",
                _ => "acknowledged",
            }
        );
        assert_eq!(r["original_update_id"], id);
        assert_eq!(r["original_terminal_confirmed"], false);
        if mode == 2 {
            assert!(r["cancellation_receipt"]["update_id"].is_string());
            assert_eq!(r["original_observation_error"]["kind"], "upstream");
        }
        finish(p, s, &c).await;
        if mode < 2 {
            let (p, s) = Process::start_profile(&c.address, false, false).await;
            let r = data(call(&s, "reconcile_operation", recovery("cancel")).await);
            assert_eq!(r["attribution"], "unconfirmed");
            assert_eq!(r["original_submission"], "unknown");
            assert_eq!(c.requests("execute").len(), 1);
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn unacknowledged_authority_templates_wrong_original_targets_and_upstream_run_denial_are_guarded()
 {
    for mode in 0..3 {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let mut input = run();
        if mode == 0 {
            input["expected_effects"]["core_admin_authority"] = json!(false);
        } else if mode == 1 {
            c.data
                .lock()
                .unwrap()
                .procedures
                .get_mut(PROCEDURE)
                .unwrap()["template"] = json!(true);
        } else {
            c.data.lock().unwrap().ack = Ack::Denied;
        }
        let r = call(&s, "run_procedure", input).await;
        if mode < 2 {
            assert_eq!(
                error(r)["kind"],
                if mode == 0 {
                    "invalid_arguments"
                } else {
                    "unsupported_target"
                }
            );
            assert!(c.requests("execute").is_empty());
        } else {
            let r = data(r);
            assert_eq!(r["submission"], "rejected");
            assert_eq!(r["submission_error"]["kind"], "permission_denied");
            assert_eq!(c.requests("execute").len(), 1);
        }
        finish(p, s, &c).await;
    }
    for wrong_target in [false, true] {
        let c = Core::start().await;
        let id = json!(c.seed_run(UpdateStatus::InProgress));
        {
            let mut d = c.data.lock().unwrap();
            let u = d.updates.get_mut(id.as_str().unwrap()).unwrap();
            if wrong_target {
                u["target"]["id"] = json!("000000000000000000000099");
            } else {
                u["operation"] = json!("CancelProcedure");
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        assert_eq!(
            error(call(&s, "cancel_procedure", cancel(&id)).await)["kind"],
            "invalid_arguments"
        );
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn large_definitions_and_parent_output_stay_bounded_and_ambiguous_logs_do_not_assign_steps() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        let p = d.procedures.get_mut(PROCEDURE).unwrap();
        let step = p["config"]["stages"][1]["executions"][1].clone();
        p["config"]["stages"]=json!((0..16).map(|n|json!({"name":format!("stage{n}{}","\\\"".repeat(45)),"enabled":true,"executions":vec![step.clone();32]})).collect::<Vec<_>>());
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let detail = data(call(&s, "get_procedure", json!({"id":PROCEDURE})).await);
    assert_eq!(detail["definition"]["execution_count"], 512);
    assert!(
        detail["definition"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["executions_complete"] == false)
    );
    assert!(serde_json::to_vec(&detail).unwrap().len() <= 65536);
    let input = edit(json!({"failure_alert":false}));
    let preview = data(call(&s, "preview_procedure_update", input.clone()).await);
    assert!(serde_json::to_vec(&preview).unwrap().len() <= 65536);
    assert_eq!(
        data(call(&s, "apply_procedure_update", apply(&input, &preview)).await)["saved_configuration_verified"],
        true
    );
    let id = c.seed_run(UpdateStatus::Complete);
    {
        let mut d = c.data.lock().unwrap();
        let u = d.updates.get_mut(&id).unwrap();
        u["logs"]=json!([komodo_client::entities::update::Log::simple("Execute procedure",format!("INFO: Executing stage: 'bounded'\nINFO: Executing: RunAction(RunAction {{ args: Some({{\"private\": String(\"{}\")}}) }})\n",PRIVATE[7]).repeat(1200))]);
    }
    let r = data(call(&s, "get_update", json!({"id":id})).await);
    assert_eq!(r["observed_procedure_result"]["evidence_incomplete"], true);
    assert!(
        r["observed_procedure_result"]["stages"]
            .as_array()
            .unwrap()
            .len()
            <= 24
    );
    assert!(
        r["observed_procedure_result"]["executions"]
            .as_array()
            .unwrap()
            .len()
            <= 32
    );
    assert!(serde_json::to_vec(&r).unwrap().len() <= 65536);
    {
        let mut d = c.data.lock().unwrap();
        let u = d.updates.get_mut(&id).unwrap();
        let log = u["logs"][0].clone();
        u["logs"].as_array_mut().unwrap().push(log);
    }
    let r = data(call(&s, "get_update", json!({"id":id})).await);
    assert_eq!(r["observed_procedure_result"]["evidence_ambiguous"], true);
    assert!(
        r["observed_procedure_result"]["executions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    finish(p, s, &c).await;
}
