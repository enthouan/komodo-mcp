//! Repo acceptance through the compiled production MCP stdio server.
#[path = "support/repositories.rs"]
mod fixture;
mod support;

use fixture::{Ack, Core, PRIVATE, REPO, SERVER, WriteFault};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    let text = serde_json::to_string(&r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret), "private configuration leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true));
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn edit(patch: Value) -> Value {
    json!({"target":{"id":REPO},"patch":patch})
}
fn apply(input: &Value, preview: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = preview["preview_id"].clone();
    v
}
fn execution() -> Value {
    json!({"target":{"id":REPO},"expected_hooks":{"on_clone":true,"on_pull":true}})
}
async fn finish(process: Process, session: Session) {
    process.finish_excluding(session, &PRIVATE).await;
}
async fn preview(s: &Session, input: &Value) -> Value {
    data(call(s, "preview_repo_update", input.clone()).await)
}

#[tokio::test]
async fn create_inspect_and_one_field_edits_preserve_private_and_unrelated_configuration() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"name":"new-repo","config":{"target":{"kind":"server","target":{"id":SERVER}},"repo":"fixture/new-workload","branch":"main","git_account":"new-private-account"}});
    let proposed = data(call(&s, "preview_repo_create", input.clone()).await);
    assert!(core.requests("write").is_empty());
    assert!(!proposed.to_string().contains("new-private-account"));
    let saved = data(call(&s, "apply_repo_create", apply(&input, &proposed)).await);
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(saved["saved_configuration_verified"], true);
    let id = saved["resource"]["id"].as_str().unwrap();
    assert_eq!(
        core.requests("write")[0].params,
        json!({"name":"new-repo","config":{"server_id":SERVER,"repo":"fixture/new-workload","branch":"main","git_account":"new-private-account","webhook_enabled":false}})
    );
    let inspected = data(call(&s, "get_repo", json!({"id":id})).await);
    assert_eq!(inspected["definition"]["webhook_enabled"], false);
    let before = core.saved(REPO);
    let input = edit(json!({"branch":"release/v05"}));
    let preview = preview(&s, &input).await;
    let saved = data(call(&s, "apply_repo_update", apply(&input, &preview)).await);
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        core.requests("write")[1].params,
        json!({"id":REPO,"config":{"branch":"release/v05"}})
    );
    let after = core.saved(REPO);
    for (key, value) in before["config"].as_object().unwrap() {
        if key != "branch" {
            assert_eq!(&after["config"][key], value);
        }
    }
    assert!(core.requests("execute").is_empty());
    let reads = core.requests("read");
    assert_eq!(reads.last().unwrap().operation, "GetRepo");
    assert!(!core.data.lock().unwrap().bad_credentials);
    finish(p, s).await;
}

#[tokio::test]
async fn noop_and_explicit_clears_are_deliberate_partial_writes() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for patch in [json!({}), json!({"branch":"main"})] {
        let input = edit(patch);
        let preview = preview(&s, &input).await;
        assert_eq!(preview["outcome"], "no_op");
        let r = data(call(&s, "apply_repo_update", apply(&input, &preview)).await);
        assert_eq!(r["outcome"], "no_op");
        assert!(core.requests("write").is_empty());
    }
    let input = edit(
        json!({"git_account":"","commit":"","path":"","environment":"","on_pull":{"path":"","command":"","shell_mode":false},"target":{"kind":"unassigned"}}),
    );
    let preview = preview(&s, &input).await;
    let r = data(call(&s, "apply_repo_update", apply(&input, &preview)).await);
    assert_eq!(r["saved_configuration_verified"], true);
    let w = core.requests("write")[0].params["config"].clone();
    assert_eq!(
        w,
        json!({"git_account":"","commit":"","path":"","environment":"","on_pull":{"path":"","command":"","shell_mode":false},"server_id":""})
    );
    assert_eq!(
        core.saved(REPO)["config"]["on_clone"]["command"],
        format!("printf {}\n", PRIVATE[1])
    );
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn private_baseline_and_reference_changes_invalidate_consumed_previews() {
    for reference in [false, true] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit(if reference {
            json!({"target":{"kind":"server","target":{"id":SERVER}}})
        } else {
            json!({"branch":"next"})
        });
        let preview = preview(&s, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            if reference {
                d.server["info"]["public_key"] = json!("different-private-trust");
            } else {
                d.repos.get_mut(REPO).unwrap()["config"]["webhook_secret"] =
                    json!("secret-only-change");
            }
        }
        assert_eq!(
            error(call(&s, "apply_repo_update", apply(&input, &preview)).await)["kind"],
            "stale_preview"
        );
        assert_eq!(
            error(call(&s, "apply_repo_update", apply(&input, &preview)).await)["kind"],
            "invalid_preview"
        );
        assert!(core.requests("write").is_empty());
        finish(p, s).await;
    }
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = edit(json!({"branch":"next"}));
    let preview = preview(&s, &input).await;
    let mut changed = apply(&input, &preview);
    changed["patch"]["branch"] = json!("another");
    let count = core.requests("read").len();
    assert_eq!(
        error(call(&s, "apply_repo_update", changed).await)["kind"],
        "invalid_preview"
    );
    assert_eq!(core.requests("read").len(), count);
    assert!(core.requests("write").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn save_errors_and_authoritative_readback_failures_never_retry() {
    for fault in [
        WriteFault::Denied,
        WriteFault::Lost,
        WriteFault::MissingRequested,
        WriteFault::ChangeUnrelated,
        WriteFault::ReadBackDenied,
    ] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit(json!({"branch":"next"}));
        let preview = preview(&s, &input).await;
        core.data.lock().unwrap().write_fault = fault;
        let r = data(call(&s, "apply_repo_update", apply(&input, &preview)).await);
        let acknowledged = matches!(
            fault,
            WriteFault::ChangeUnrelated | WriteFault::ReadBackDenied
        );
        assert_eq!(
            r["outcome"],
            if acknowledged {
                "saved"
            } else {
                "submission_unknown"
            }
        );
        assert_eq!(r["saved_configuration_verified"], false);
        assert!(r["observation_error"].is_object());
        assert_eq!(core.requests("write").len(), 1);
        assert_eq!(
            error(call(&s, "apply_repo_update", apply(&input, &preview)).await)["kind"],
            "invalid_preview"
        );
        assert_eq!(core.requests("write").len(), 1);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn independent_grants_guard_discovery_direct_dispatch_and_webhook_enable_before_io() {
    for operate in [false, true] {
        for edit_grant in [false, true] {
            let core = Core::start().await;
            let (p, s) = Process::start_profile(&core.address, operate, edit_grant).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                25 + 14 * usize::from(operate)
                    + 36 * usize::from(edit_grant)
                    + 2 * usize::from(operate && edit_grant)
            );
            for (name, enabled) in [
                ("list_repos", true),
                ("get_repo", true),
                ("clone_repo", operate),
                ("pull_repo", operate),
                ("preview_repo_create", edit_grant),
                ("apply_repo_create", edit_grant),
                ("preview_repo_update", edit_grant),
                ("apply_repo_update", edit_grant),
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), enabled);
                let before = core.requests("read").len();
                let result =
                    s.call_tool(CallToolRequestParams::new(name).with_arguments(
                        json!({"unexpected":"value"}).as_object().unwrap().clone(),
                    ))
                    .await;
                if enabled {
                    assert_eq!(result.unwrap().is_error, Some(true));
                } else {
                    assert!(result.is_err());
                }
                assert_eq!(core.requests("read").len(), before);
                assert!(core.requests("write").is_empty());
                assert!(core.requests("execute").is_empty());
            }
            if edit_grant {
                let input = edit(json!({"webhook_enabled":true}));
                let preview = preview(&s, &input).await;
                assert_eq!(
                    preview["definition"]["details"]["requires_operate_to_apply"],
                    true
                );
                let before = core.requests("read").len();
                let result = call(&s, "apply_repo_update", apply(&input, &preview)).await;
                if operate {
                    assert_eq!(data(result)["outcome"], "saved");
                    assert_eq!(core.requests("write").len(), 1);
                } else {
                    assert_eq!(error(result)["kind"], "capability_disabled");
                    assert_eq!(core.requests("read").len(), before);
                    assert!(core.requests("write").is_empty());
                }
            }
            finish(p, s).await;
        }
    }
}

#[tokio::test]
async fn edit_only_cannot_change_execution_fields_under_active_or_racing_webhooks() {
    for active in [false, true] {
        let core = Core::start().await;
        core.data.lock().unwrap().repos.get_mut(REPO).unwrap()["config"]["webhook_enabled"] =
            json!(active);
        let (p, s) = Process::start_profile(&core.address, false, true).await;
        for patch in [
            json!({"repo":"fixture/changed"}),
            json!({"branch":"next"}),
            json!({"git_provider":"source.invalid"}),
            json!({"git_https":false}),
            json!({"git_account":"new-reference"}),
            json!({"commit":"abcd1234"}),
            json!({"path":"owned/path"}),
            json!({"target":{"kind":"server","target":{"id":SERVER}}}),
            json!({"environment":"TOKEN=[[REFERENCE]]"}),
            json!({"on_clone":{"path":"","command":"printf changed","shell_mode":true}}),
            json!({"on_pull":{"path":"","command":"printf changed","shell_mode":true}}),
            json!({"webhook_enabled":false,"branch":"next"}),
        ] {
            let input = edit(patch);
            let preview = preview(&s, &input).await;
            assert_eq!(
                preview["definition"]["details"]["requires_operate_to_apply"],
                true
            );
            let before = core.requests("read").len();
            assert_eq!(
                error(call(&s, "apply_repo_update", apply(&input, &preview)).await)["kind"],
                "capability_disabled"
            );
            assert_eq!(core.requests("read").len(), before);
            assert!(core.requests("write").is_empty());
        }
        // A genuine empty no-op and removing the automatic-execution capability
        // remain possible with edit alone; neither grants permission to execute.
        let empty = edit(json!({}));
        let empty_preview = preview(&s, &empty).await;
        assert_eq!(
            data(call(&s, "apply_repo_update", apply(&empty, &empty_preview)).await)["outcome"],
            "no_op"
        );
        let disable = edit(json!({"webhook_enabled":false}));
        let disable_preview = preview(&s, &disable).await;
        let saved = data(call(&s, "apply_repo_update", apply(&disable, &disable_preview)).await);
        assert_eq!(saved["saved_configuration_verified"], true);
        assert_eq!(core.requests("write").len(), usize::from(active));
        assert!(core.requests("execute").is_empty());
        finish(p, s).await;
    }
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, false, true).await;
    let input = json!({"name":"inert-repo","config":{"target":{"kind":"server","target":{"id":SERVER}},"repo":"fixture/inert","branch":"main","on_pull":{"path":"","command":"printf trusted","shell_mode":true}}});
    let preview = data(call(&s, "preview_repo_create", input.clone()).await);
    let saved = data(call(&s, "apply_repo_create", apply(&input, &preview)).await);
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(
        saved["definition"]["details"]["after"]["webhook_enabled"],
        false
    );
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn invalid_fields_nulls_source_credentials_and_sdk_passthrough_are_rejected_before_reads() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for patch in [
        json!({"branch":null}),
        json!({"repo":""}),
        json!({"repo":"https://user:private-url-secret@source.invalid/path"}),
        json!({"branch":"../escape"}),
        json!({"git_provider":"user:private@source.invalid"}),
        json!({"commit":"not-a-commit"}),
        json!({"on_clone":{"command":"incomplete"}}),
        json!({"builder_id":"aws"}),
        json!({"schedule":"every second"}),
        json!({"webhook_secret":"private"}),
        json!({"git_account":"[redacted]"}),
    ] {
        let before = core.requests("read").len();
        let r = error(call(&s, "preview_repo_update", edit(patch)).await);
        assert_eq!(r["kind"], "invalid_arguments");
        assert_eq!(core.requests("read").len(), before);
        assert!(!r.to_string().contains("private-url-secret"));
    }
    assert!(core.requests("write").is_empty());
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn clone_and_pull_run_only_selected_saved_definitions_and_report_observed_revision() {
    let core = Core::start().await;
    let before = core.saved(REPO);
    let (p, s) = Process::start_profile(&core.address, true, false).await;
    let detail = data(call(&s, "get_repo", json!({"name":"fixture-repo"})).await);
    assert_eq!(detail["observed_cloned_revision"], "765abcd");
    assert_eq!(detail["definition"]["on_clone_configured"], true);
    for (name, op, revision) in [
        ("clone_repo", "CloneRepo", "a1b2c3d"),
        ("pull_repo", "PullRepo", "e4f5a6b"),
    ] {
        let r = data(call(&s, name, execution()).await);
        assert_eq!(r["submission"], "accepted");
        assert_eq!(r["core_operation"], op);
        assert_eq!(r["resource"]["kind"], "repo");
        assert_eq!(r["resource"]["id"], REPO);
        assert_eq!(r["requested_scope"]["kind"], "whole_repo");
        assert_eq!(
            r["configured_effects"]["repository"]["runs_on_clone"],
            name == "clone_repo"
        );
        assert_eq!(r["configured_effects"]["repository"]["runs_on_pull"], true);
        assert_eq!(
            r["configured_effects"]["repository"]["can_replace_working_directory"],
            name == "clone_repo"
        );
        assert_eq!(
            r["configured_effects"]["repository"]["environment_file_write_possible"],
            true
        );
        assert_eq!(
            r["configured_effects"]["repository"]["definition"]["source"]["branch"],
            "main"
        );
        assert_eq!(
            r["latest_update"]["observed_revision"]["revision"],
            revision
        );
        assert_eq!(r["latest_update"]["outcome"], "succeeded");
        assert_eq!(
            data(call(&s, "wait_for_update", json!({"id":r["update_id"]})).await)["end"],
            "complete"
        );
    }
    let requests = core.requests("execute");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.params == json!({"repo":REPO})));
    assert_eq!(core.saved(REPO), before);
    assert!(core.requests("write").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn hook_failures_and_upstream_denials_retain_stage_evidence_without_private_credentials() {
    for mode in 0..3 {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            d.hook_failure = mode == 0;
            d.run_denied = mode == 1;
            if mode == 2 {
                d.ack = Ack::Denied;
            }
        }
        let (p, s) = Process::start_profile(&core.address, true, false).await;
        let r = data(call(&s, "pull_repo", execution()).await);
        if mode == 2 {
            assert_eq!(r["submission"], "rejected");
            assert_eq!(r["submission_error"]["kind"], "permission_denied");
        } else {
            assert_eq!(r["submission"], "accepted");
            assert_eq!(r["latest_update"]["outcome"], "failed");
            assert_eq!(
                r["latest_update"]["stages"][0]["stage"],
                if mode == 0 { "On Pull" } else { "Task Error" }
            );
            assert!(
                r["latest_update"]["stages"][0]["stderr"]
                    .as_str()
                    .unwrap()
                    .contains(if mode == 0 {
                        "hook failed"
                    } else {
                        "permission denied"
                    })
            );
            for secret in ["stage-private-token", "git-private-password"] {
                assert!(!r.to_string().contains(secret));
            }
        }
        assert_eq!(core.requests("execute").len(), 1);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn changed_hook_presence_templates_and_unassigned_targets_never_execute() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, false).await;
    let mut input = execution();
    input["expected_hooks"]["on_pull"] = json!(false);
    assert_eq!(
        error(call(&s, "clone_repo", input).await)["kind"],
        "invalid_arguments"
    );
    core.data.lock().unwrap().repos.get_mut(REPO).unwrap()["template"] = json!(true);
    assert_eq!(
        error(call(&s, "clone_repo", execution()).await)["kind"],
        "unsupported_target"
    );
    {
        let mut d = core.data.lock().unwrap();
        d.repos.get_mut(REPO).unwrap()["template"] = json!(false);
        d.repos.get_mut(REPO).unwrap()["config"]["server_id"] = json!("");
    }
    assert_eq!(
        error(call(&s, "pull_repo", execution()).await)["kind"],
        "unsupported_target"
    );
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn lost_or_invalid_acknowledgements_restart_and_reconcile_without_replay_or_false_attribution()
 {
    for ack in [Ack::Lost, Ack::WrongTarget] {
        let core = Core::start().await;
        core.data.lock().unwrap().ack = ack;
        for expected in 1..=2 {
            let (p, s) = Process::start_profile(&core.address, true, false).await;
            let r = data(call(&s, "clone_repo", execution()).await);
            assert_eq!(r["submission"], "unknown");
            assert_eq!(r["update_id"], Value::Null);
            assert_eq!(r["automatic_resubmission"], false);
            assert_eq!(core.requests("execute").len(), expected);
            p.interrupt(s).await;
            let (p, s) = Process::start_profile(&core.address, false, false).await;
            let recovered=data(call(&s,"reconcile_operation",json!({"target":{"kind":"repo","id":REPO},"requested_scope":{"kind":"whole_repo"},"operation":"clone"})).await);
            assert_eq!(recovered["original_submission"], "unknown");
            assert_eq!(recovered["attribution"], "unconfirmed");
            assert_eq!(recovered["candidates"].as_array().unwrap().len(), expected);
            assert_eq!(core.requests("execute").len(), expected);
            assert_eq!(recovered["automatic_resubmission"], false);
            finish(p, s).await;
        }
    }
}

#[tokio::test]
async fn pending_wait_resumes_after_restart_and_observation_failure_preserves_known_id() {
    let core = Core::start().await;
    core.data.lock().unwrap().running = true;
    let (p, s) = Process::start_profile(&core.address, true, false).await;
    let r = data(call(&s, "pull_repo", execution()).await);
    let id = r["update_id"].clone();
    let pending = data(call(&s, "wait_for_update", json!({"id":id,"timeout_seconds":1})).await);
    assert_eq!(pending["end"], "deadline");
    assert_eq!(pending["update_id"], id);
    assert_eq!(pending["remote_cancellation_requested"], false);
    finish(p, s).await;
    {
        let mut d = core.data.lock().unwrap();
        for u in d.updates.values_mut() {
            u["status"] = json!("Complete");
            u["success"] = json!(true);
        }
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let r = data(call(&s, "wait_for_update", json!({"id":id})).await);
    assert_eq!(r["end"], "complete");
    assert_eq!(
        r["latest_update"]["observed_revision"]["revision"],
        "e4f5a6b"
    );
    assert_eq!(core.requests("execute").len(), 1);
    finish(p, s).await;
    for delay in [false, true] {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            d.fail_updates = !delay;
            if delay {
                d.update_delay = std::time::Duration::from_secs(3);
            }
        }
        let (p, s) = Process::start_profile(&core.address, true, false).await;
        let r = data(call(&s, "clone_repo", execution()).await);
        assert_eq!(r["submission"], "accepted");
        assert!(r["update_id"].is_string());
        assert_eq!(r["latest_update"]["status"], "queued");
        assert_eq!(
            r["observation_error"]["kind"],
            if delay { "timeout" } else { "upstream" }
        );
        assert_eq!(core.requests("execute").len(), 1);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn listing_is_bounded_and_private_source_locators_are_hidden() {
    let core = Core::start().await;
    {
        let mut d = core.data.lock().unwrap();
        let mut second = d.repos[REPO].clone();
        second["_id"]["$oid"] = json!("00000000000000000000000c");
        second["name"] = json!("second-repo");
        d.repos.insert("00000000000000000000000c".into(), second);
        d.repos.get_mut(REPO).unwrap()["config"]["repo"] =
            json!("https://user:source-private-password@source.invalid/private");
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let first = data(call(&s, "list_repos", json!({"page_size":1})).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["next_page"], 1);
    let next = data(call(&s, "list_repos", json!({"page_size":1,"page":1})).await);
    assert_eq!(next["has_more"], false);
    assert_eq!(next["items"][0]["name"], "second-repo");
    let detail = data(call(&s, "get_repo", json!({"id":REPO})).await);
    assert!(!detail.to_string().contains("source-private-password"));
    assert_eq!(detail["configured_branch"], "main");
    assert_eq!(detail["observed_cloned_revision"], "765abcd");
    assert!(core.requests("execute").is_empty());
    assert!(core.requests("write").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn concurrent_callers_keep_distinct_acknowledged_ids_and_recovery_never_assigns_ownership() {
    let core = Core::start().await;
    let (p1, s1) = Process::start_profile(&core.address, true, false).await;
    let (p2, s2) = Process::start_profile(&core.address, true, false).await;
    let (r1, r2) = tokio::join!(
        call(&s1, "pull_repo", execution()),
        call(&s2, "pull_repo", execution())
    );
    let (r1, r2) = (data(r1), data(r2));
    assert_ne!(r1["update_id"], r2["update_id"]);
    assert_eq!(r1["submission"], "accepted");
    assert_eq!(r2["submission"], "accepted");
    assert_eq!(core.requests("execute").len(), 2);
    finish(p1, s1).await;
    finish(p2, s2).await;
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let r=data(call(&s,"reconcile_operation",json!({"target":{"kind":"repo","id":REPO},"requested_scope":{"kind":"whole_repo"},"operation":"pull"})).await);
    assert_eq!(r["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(r["attribution"], "unconfirmed");
    assert_eq!(core.requests("execute").len(), 2);
    finish(p, s).await;
}

#[tokio::test]
async fn missing_or_ambiguous_revision_evidence_stays_unknown_and_stage_output_is_bounded() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, false).await;
    let r = data(call(&s, "clone_repo", execution()).await);
    let id = r["update_id"].as_str().unwrap();
    {
        let mut d = core.data.lock().unwrap();
        let u = d.updates.get_mut(id).unwrap();
        let duplicate = u["logs"][0].clone();
        u["logs"].as_array_mut().unwrap().push(duplicate);
        u["logs"][1]["success"] = json!(false);
        u["logs"][1]["stderr"] = json!("bounded failure ".repeat(1000));
    }
    let observed = data(call(&s, "get_update", json!({"id":id})).await);
    assert!(observed.get("observed_revision").is_none());
    assert_eq!(observed["stages"][0]["stderr_truncated"], true);
    assert!(observed["stages"][0]["stderr"].as_str().unwrap().len() <= 1024);
    assert_eq!(core.requests("execute").len(), 1);
    finish(p, s).await;
}
