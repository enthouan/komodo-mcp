//! Action definitions/execution acceptance through production MCP stdio.
#[allow(dead_code)] // Reused definition fixture also contains modes covered by its own suite.
#[path = "support/build_definitions.rs"]
mod definitions;
#[path = "support/actions.rs"]
mod fixture;
mod support;
use fixture::{ACTION, Ack, Core, Fault, Outcome, PRIVATE};
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
    assert!(!s.contains("ephemeral-api-"));
    for secret in PRIVATE {
        assert!(!s.contains(secret), "private Action data leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn edit(patch: Value) -> Value {
    json!({"target":{"id":ACTION},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
fn run() -> Value {
    json!({"target":{"name":"fixture-action"},"expected_effects":{"core_admin_authority":true,"host_access":true,"broader_effects_possible":true}})
}
fn cancel(id: &Value) -> Value {
    json!({"target":{"id":ACTION},"expected_update_id":id})
}
fn recovery(op: &str) -> Value {
    json!({"target":{"kind":"action","id":ACTION},"requested_scope":{"kind":"whole_action"},"operation":op})
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
                "list_actions",
                "get_action",
                "preview_action_create",
                "apply_action_create",
                "preview_action_update",
                "apply_action_update",
                "run_action",
                "cancel_action",
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
                    json!({"source":""}),
                ] {
                    let mut input = edit(patch);
                    input["preview_id"] = json!("unconsumed");
                    assert_eq!(
                        error(call(&s, "apply_action_update", input).await)["kind"],
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
        let preview = data(call(&s, "preview_action_update", input.clone()).await);
        c.data.lock().unwrap().fault = fault;
        let r = data(call(&s, "apply_action_update", apply(&input, &preview)).await);
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
            error(call(&s, "apply_action_update", apply(&input, &preview)).await)["kind"],
            "invalid_preview"
        );
        assert_eq!(c.requests("write").len(), 1);
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
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
            let r = data(call(&s, "run_action", run()).await);
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
    let r = data(call(&s, "run_action", run()).await);
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
        let r = data(call(&s, "run_action", run()).await);
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
        let r = data(call(&s, "cancel_action", cancel(&id)).await);
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
            c.data.lock().unwrap().actions.get_mut(ACTION).unwrap()["template"] = json!(true);
        } else {
            c.data.lock().unwrap().ack = Ack::Denied;
        }
        let r = call(&s, "run_action", input).await;
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
                u["operation"] = json!("CancelAction");
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        assert_eq!(
            error(call(&s, "cancel_action", cancel(&id)).await)["kind"],
            "invalid_arguments"
        );
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn bounded_inspection_and_inert_admin_creation_hide_source_defaults_and_preserve_readback() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        let mut other = d.actions[ACTION].clone();
        other["_id"]["$oid"] = json!("000000000000000000000041");
        other["name"] = json!("zeta-action");
        d.actions.insert("000000000000000000000041".into(), other);
    }
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let detail = data(call(&s, "get_action", json!({"name":"fixture-action"})).await);
    assert_eq!(detail["definition"]["arguments_format"], "KeyValue");
    assert_eq!(detail["definition"]["source_configured"], true);
    assert_eq!(
        detail["effects"]["acknowledgement"],
        run()["expected_effects"]
    );
    assert!(
        detail["definition"]["authority"]
            .as_str()
            .unwrap()
            .contains("--allow-all")
    );
    let listed = data(call(&s, "list_actions", json!({"page_size":1})).await);
    assert_eq!(listed["next_page"], 1);
    assert_eq!(
        data(call(&s, "list_actions", json!({"page_size":1,"page":1})).await)["has_more"],
        false
    );
    let input = json!({"name":"owned-action","config":{"source":format!("// {}",PRIVATE[9]),"default_arguments":{"x":PRIVATE[7]}}});
    let preview = data(call(&s, "preview_action_create", input.clone()).await);
    assert!(c.requests("write").is_empty());
    let result = data(call(&s, "apply_action_create", apply(&input, &preview)).await);
    assert_eq!(result["saved_configuration_verified"], true, "{result}");
    let writes = c.requests("write");
    assert_eq!(writes.len(), 1);
    let conf = &writes[0].params["config"];
    assert_eq!(conf["webhook_enabled"], false);
    assert_eq!(conf["run_at_startup"], false);
    assert_eq!(conf["schedule_enabled"], false);
    assert_eq!(conf["schedule"], "");
    assert_eq!(conf["arguments_format"], "json");
    assert_eq!(conf.as_object().unwrap().len(), 7);
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
    for admin in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.admin = admin;
            d.can_create = !admin;
        }
        let (p, s) = Process::start_profile(&c.address, false, true).await;
        let preview = data(call(&s, "preview_action_create", input.clone()).await);
        let r = data(call(&s, "apply_action_create", apply(&input, &preview)).await);
        assert_eq!(r["saved_configuration_verified"], false);
        assert_eq!(r["observation_error"]["kind"], "permission_denied");
        assert_eq!(c.requests("write").len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn scoped_edits_clear_defaults_deliberately_preserve_original_formats_secrets_and_schedule() {
    for format in ["key_value", "toml", "yaml", "json"] {
        let c = Core::start().await;
        c.data.lock().unwrap().actions.get_mut(ACTION).unwrap()["config"]["arguments_format"] =
            json!(format);
        let before = c.action();
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let input = edit(json!({"reload_deno_deps":true}));
        let pr = data(call(&s, "preview_action_update", input.clone()).await);
        assert!(c.requests("write").is_empty());
        assert_eq!(
            pr["definition"]["details"]["requires_operate_to_apply"],
            true
        );
        let r = data(call(&s, "apply_action_update", apply(&input, &pr)).await);
        assert_eq!(r["saved_configuration_verified"], true);
        assert_eq!(
            c.requests("write")[0].params,
            json!({"id":ACTION,"config":{"reload_deno_deps":true}})
        );
        let mut expected = before.clone();
        expected["config"]["reload_deno_deps"] = json!(true);
        assert_eq!(c.action(), expected);
        let input = edit(json!({"default_arguments":{}}));
        let pr = data(call(&s, "preview_action_update", input.clone()).await);
        let changes = pr["changes"].as_array().unwrap();
        assert!(
            changes
                .iter()
                .filter(|x| x["field"] == "arguments")
                .all(|x| x["redacted"] == true)
        );
        let r = data(call(&s, "apply_action_update", apply(&input, &pr)).await);
        assert_eq!(r["saved_configuration_verified"], true);
        assert_eq!(
            c.requests("write")[1].params,
            json!({"id":ACTION,"config":{"arguments":"{}\n","arguments_format":"json"}})
        );
        let saved = c.action();
        assert_eq!(
            saved["config"]["file_contents"],
            before["config"]["file_contents"]
        );
        assert_eq!(
            saved["config"]["webhook_secret"],
            before["config"]["webhook_secret"]
        );
        assert_eq!(saved["config"]["schedule"], before["config"]["schedule"]);
        assert!(c.requests("execute").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn empty_patch_noop_needs_only_edit_and_private_baseline_changes_consume_previews() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let input = edit(json!({}));
    let pr = data(call(&s, "preview_action_update", input.clone()).await);
    assert_eq!(
        data(call(&s, "apply_action_update", apply(&input, &pr)).await)["outcome"],
        "no_op"
    );
    assert!(c.requests("write").is_empty());
    finish(p, s, &c).await;
    for field in [
        "file_contents",
        "arguments",
        "webhook_secret",
        "schedule_timezone",
    ] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let input = edit(json!({"failure_alert":false}));
        let pr = data(call(&s, "preview_action_update", input.clone()).await);
        c.data.lock().unwrap().actions.get_mut(ACTION).unwrap()["config"][field] =
            json!("changed private baseline");
        assert_eq!(
            error(call(&s, "apply_action_update", apply(&input, &pr)).await)["kind"],
            "stale_preview"
        );
        assert_eq!(
            error(call(&s, "apply_action_update", apply(&input, &pr)).await)["kind"],
            "invalid_preview"
        );
        assert!(c.requests("write").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn schedule_passthrough_null_oversized_source_args_and_automatic_creation_are_rejected() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for patch in [
        json!({"source":null}),
        json!({"default_arguments":null}),
        json!({"schedule":"* * * * * *"}),
        json!({"schedule_enabled":true}),
        json!({"config":{"file_contents":"x"}}),
        json!({"source":"x".repeat(8193)}),
        json!({"default_arguments":{"huge":"x".repeat(8192)}}),
    ] {
        assert_eq!(
            call(&s, "preview_action_update", edit(patch))
                .await
                .is_error,
            Some(true)
        );
    }
    for config in [
        json!({}),
        json!({"source":"","run_at_startup":true}),
        json!({"source":"","webhook_enabled":true}),
    ] {
        assert_eq!(
            call(
                &s,
                "preview_action_create",
                json!({"name":"owned","config":config})
            )
            .await
            .is_error,
            Some(true)
        );
    }
    let mut nested = json!(0);
    for _ in 0..10 {
        nested = json!({"nested":nested});
    }
    for args in [
        json!({"deep":nested}),
        json!({"array":vec![0;65]}),
        json!({"x":"x".repeat(8192)}),
        json!({"bad\nkey":1}),
    ] {
        let mut input = run();
        input["args"] = args;
        assert_eq!(call(&s, "run_action", input).await.is_error, Some(true));
    }
    assert!(c.requests("write").is_empty());
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn saved_action_forwards_only_overrides_and_core_recursively_merges_objects_appends_arrays() {
    let c = Core::start().await;
    let defaults = json!({"retained":PRIVATE[7],"nested":{"left":1,"right":"default"},"array":[1,2],"scalar":"old"});
    {
        let mut d = c.data.lock().unwrap();
        let v = d.actions.get_mut(ACTION).unwrap();
        v["config"]["arguments"] = json!(defaults.to_string());
        v["config"]["arguments_format"] = json!("json");
    }
    let before = c.action();
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    for (args, expected) in [
        (None, defaults.clone()),
        (Some(json!({})), defaults.clone()),
        (
            Some(json!({"nested":{"right":"new"},"array":[3],"scalar":null,"run":PRIVATE[10]})),
            json!({"retained":PRIVATE[7],"nested":{"left":1,"right":"new"},"array":[1,2,3],"scalar":null,"run":PRIVATE[10]}),
        ),
        (Some(json!({"nested":{},"array":[]})), defaults.clone()),
    ] {
        let mut input = run();
        if let Some(a) = &args {
            input["args"] = a.clone();
        }
        let r = data(call(&s, "run_action", input).await);
        assert_eq!(r["submission"], "accepted");
        assert_eq!(r["latest_update"]["outcome"], "succeeded");
        assert_eq!(
            r["latest_update"]["observed_action_result"]["command_success"],
            true
        );
        let requests = c.requests("execute");
        let q = &requests.last().unwrap().params;
        assert_eq!(*q, json!({"action":ACTION,"args":args}));
        assert_eq!(
            c.data.lock().unwrap().merged_arguments.last().unwrap(),
            &expected
        );
        assert_eq!(c.action(), before);
    }
    assert_eq!(
        c.data.lock().unwrap().executed_sources,
        vec![before["config"]["file_contents"].as_str().unwrap(); 4]
    );
    assert!(c.requests("write").is_empty());
    assert_eq!(c.requests("execute").len(), 4);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn core_merge_type_errors_and_script_failure_keep_known_ids_and_omit_unlabelled_secret_output()
 {
    for incompatible in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if incompatible {
                let v = d.actions.get_mut(ACTION).unwrap();
                v["config"]["arguments_format"] = json!("json");
                v["config"]["arguments"] = json!("{\"object\":{}}");
            } else {
                d.outcome = Outcome::Failure;
            }
        }
        let before = c.action();
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let mut input = run();
        input["args"] = json!({"object":PRIVATE[10]});
        let r = data(call(&s, "run_action", input).await);
        let id = r["update_id"].clone();
        assert_eq!(r["submission"], "accepted");
        assert_eq!(r["latest_update"]["outcome"], "failed");
        assert_eq!(
            r["latest_update"]["observed_action_result"]["output_omitted"],
            true
        );
        assert_eq!(c.action(), before);
        finish(p, s, &c).await;
        let (p, s) = Process::start_profile(&c.address, false, false).await;
        let u = data(call(&s, "get_update", json!({"id":id})).await);
        assert_eq!(u["outcome"], "failed");
        assert_eq!(u["stages"][0]["stderr_truncated"], true);
        assert_eq!(c.requests("execute").len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn cancellation_targets_one_overlapping_run_and_separates_ack_original_and_child_effects() {
    for finish_mode in 0..3 {
        let c = Core::start().await;
        let first = c.seed_run(UpdateStatus::InProgress);
        let other = c.seed_run(UpdateStatus::InProgress);
        {
            let mut d = c.data.lock().unwrap();
            d.cancel_finishes_original = finish_mode > 0;
            d.cancel_original_succeeds = finish_mode == 2;
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_action", cancel(&json!(first))).await);
        assert_eq!(r["targeting"], "update");
        assert_eq!(r["outcome"], "acknowledged");
        assert_eq!(
            r["cancellation_receipt"]["latest_update"]["outcome"],
            "succeeded"
        );
        assert_eq!(r["original_terminal_confirmed"], finish_mode > 0);
        if finish_mode == 1 {
            assert_eq!(r["original_update"]["outcome"], "failed");
            assert_eq!(
                r["original_update"]["observed_action_result"]["cancellation_reported"],
                true
            );
        } else if finish_mode == 2 {
            assert_eq!(r["original_update"]["outcome"], "succeeded");
        }
        assert_eq!(
            c.data.lock().unwrap().updates[&other]["status"],
            "InProgress"
        );
        assert_eq!(
            c.requests("execute")[0].params,
            json!({"action":ACTION,"update_id":first})
        );
        assert_eq!(c.requests("execute").len(), 1);
        assert!(
            !c.requests("read")
                .iter()
                .any(|r| r.operation == "ListUpdates")
        );
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn finished_runs_preflight_races_and_completion_after_last_read_never_cancel_replacement() {
    for mode in 0..3 {
        let c = Core::start().await;
        let id = c.seed_run(if mode == 0 {
            UpdateStatus::Complete
        } else {
            UpdateStatus::InProgress
        });
        {
            let mut d = c.data.lock().unwrap();
            d.finish_at_update_read = if mode == 1 { Some(2) } else { None };
            d.replace_on_cancel = mode == 2;
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = data(call(&s, "cancel_action", cancel(&json!(id))).await);
        assert_eq!(r["original_update_id"], id);
        if mode < 2 {
            assert_eq!(r["outcome"], "not_submitted");
            assert!(c.requests("execute").is_empty());
        } else {
            assert_eq!(r["outcome"], "acknowledged");
            assert_eq!(
                r["cancellation_receipt"]["latest_update"]["outcome"],
                "failed"
            );
            assert_eq!(c.requests("execute").len(), 1);
            let d = c.data.lock().unwrap();
            assert!(d.updates.iter().any(|(uid, u)| uid != &id
                && u["operation"] == "RunAction"
                && u["status"] == "InProgress"));
        }
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn concurrent_operators_receive_distinct_live_action_ids_without_busy_failure_or_false_attribution()
 {
    let c = Core::start().await;
    c.data.lock().unwrap().outcome = Outcome::Running;
    let (p, a) = Process::start_profile(&c.address, true, false).await;
    let (q, b) = Process::start_profile(&c.address, true, false).await;
    let (x, y) = tokio::join!(call(&a, "run_action", run()), call(&b, "run_action", run()));
    let x = data(x);
    let y = data(y);
    assert_ne!(x["update_id"], y["update_id"]);
    assert_eq!(x["latest_update"]["status"], "running");
    assert_eq!(y["latest_update"]["status"], "running");
    assert_eq!(c.requests("execute").len(), 2);
    assert!(c.requests("write").is_empty());
    finish(p, a, &c).await;
    finish(q, b, &c).await;
}

#[tokio::test]
async fn startup_and_source_edits_are_typed_effect_gated_and_normalized_without_execution() {
    let c = Core::start().await;
    let before = c.action();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for patch in [
        json!({"run_at_startup":true}),
        json!({"webhook_enabled":false}),
        json!({"source":format!("// {}   ",PRIVATE[9])}),
    ] {
        let input = edit(patch);
        let pr = data(call(&s, "preview_action_update", input.clone()).await);
        let r = data(call(&s, "apply_action_update", apply(&input, &pr)).await);
        assert_eq!(r["saved_configuration_verified"], true);
    }
    let saved = c.action();
    assert_eq!(saved["config"]["run_at_startup"], true);
    assert_eq!(
        saved["config"]["file_contents"],
        format!("// {}\n", PRIVATE[9])
    );
    assert_eq!(saved["config"]["arguments"], before["config"]["arguments"]);
    assert_eq!(
        saved["config"]["webhook_secret"],
        before["config"]["webhook_secret"]
    );
    assert_eq!(saved["config"]["schedule"], before["config"]["schedule"]);
    assert_eq!(c.requests("write").len(), 3);
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn large_private_definitions_output_and_duplicate_command_stages_remain_bounded() {
    use komodo_client::entities::update::Log;
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        let v = d.actions.get_mut(ACTION).unwrap();
        v["config"]["file_contents"] = json!(format!("{}\n", PRIVATE[9].repeat(6000)));
        v["config"]["arguments"] = json!(format!("PRIVATE={}\n", PRIVATE[7].repeat(4000)));
    }
    let before = c.action();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let detail = data(call(&s, "get_action", json!({"id":ACTION})).await);
    assert!(serde_json::to_vec(&detail).unwrap().len() < 65536);
    let input = edit(json!({"failure_alert":false}));
    let pr = data(call(&s, "preview_action_update", input.clone()).await);
    assert!(serde_json::to_vec(&pr).unwrap().len() < 65536);
    let saved = data(call(&s, "apply_action_update", apply(&input, &pr)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        c.action()["config"]["file_contents"],
        before["config"]["file_contents"]
    );
    assert_eq!(
        c.action()["config"]["arguments"],
        before["config"]["arguments"]
    );
    let id = c.seed_run(UpdateStatus::Complete);
    {
        let mut d = c.data.lock().unwrap();
        let u = d.updates.get_mut(&id).unwrap();
        u["logs"] = json!([
            Log {
                stage: "Execute Action".into(),
                success: false,
                stdout: PRIVATE[10].repeat(4000),
                stderr: format!("Command cancelled (process group killed)\n{}", PRIVATE[9]),
                ..Default::default()
            },
            Log::simple("Execute Action", "forged success marker".into())
        ]);
    }
    let u = data(call(&s, "get_update", json!({"id":id})).await);
    assert!(serde_json::to_vec(&u).unwrap().len() < 65536);
    assert_eq!(u["observed_action_result"]["command_stage_found"], false);
    assert!(u["observed_action_result"]["command_success"].is_null());
    assert_eq!(u["observed_action_result"]["cancellation_reported"], false);
    assert_eq!(u["observed_action_result"]["output_omitted"], true);
    assert!(c.requests("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn independent_upstream_write_and_execute_permissions_never_borrow_admin_credentials() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.can_execute = false;
        d.admin = false;
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let input = edit(json!({"failure_alert":false}));
    let pr = data(call(&s, "preview_action_update", input.clone()).await);
    assert_eq!(
        data(call(&s, "apply_action_update", apply(&input, &pr)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(
        data(call(&s, "run_action", run()).await)["submission"],
        "rejected"
    );
    finish(p, s, &c).await;
    let c = Core::start().await;
    c.data.lock().unwrap().can_edit = false;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let pr = data(call(&s, "preview_action_update", input.clone()).await);
    assert_eq!(
        data(call(&s, "apply_action_update", apply(&input, &pr)).await)["saved_configuration_verified"],
        false
    );
    let before = c.action();
    let r = data(call(&s, "run_action", run()).await);
    assert_eq!(r["submission"], "accepted");
    assert_eq!(c.action(), before);
    assert_eq!(
        c.data.lock().unwrap().merged_arguments[0],
        json!({"PRIVATE":PRIVATE[7],"DEFAULT":"retained"})
    );
    finish(p, s, &c).await;
}
