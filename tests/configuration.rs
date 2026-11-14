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
                30 + 16 * usize::from(operate)
                    + 42 * usize::from(edit)
                    + 2 * usize::from(operate && edit)
            );
            for name in [
                "preview_stack_update",
                "apply_stack_update",
                "preview_stack_create",
                "apply_stack_create",
            ] {
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
        WriteReply::WrongName,
        WriteReply::Template,
        WriteReply::MissingConfig,
        WriteReply::MissingRequested,
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
                connect_timeout: Duration::from_secs(1),
                operation_timeout: Duration::from_secs(1),
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
    tokio::time::sleep(Duration::from_secs(3)).await;
}

fn creation(name: &str, source: Value, swarm: bool) -> Value {
    json!({"name":name,"config":{"source":source,"attachment":{"kind":if swarm {"swarm"}else{"server"},"target":{"id":if swarm {fixture::SWARM}else{fixture::SERVER}}},"auto_pull":false}})
}
async fn create_stack(session: &support::operations::Session, input: &Value) -> Value {
    let p = data(call(session, "preview_stack_create", input.clone()).await);
    assert_eq!(p["writes_submitted"], 0);
    data(call(session, "apply_stack_create", apply(input, &p)).await)
}
fn no_executions(core: &Core) {
    assert!(
        core.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "execute")
    );
}

#[tokio::test]
async fn source_updates_require_both_grants_before_reads_even_with_disabled_or_racing_auto_update()
{
    for auto_update in [false, true] {
        let core = Core::start().await;
        let (process, session) = Process::start_profile(&core.address, false, true).await;
        let tools = session.list_tools(None).await.unwrap().tools;
        let apply_tool = tools
            .iter()
            .find(|t| t.name == "apply_stack_update")
            .unwrap();
        assert!(
            apply_tool
                .description
                .as_ref()
                .unwrap()
                .contains("BOTH operate and configuration-edit")
        );
        for source in [
            json!({"kind":"inline","contents":"services: {}"}),
            json!({"kind":"git","repo":"fixtures/compose"}),
            json!({"kind":"linked_repo","target":{"id":fixture::REPO}}),
            json!({"kind":"host","run_directory":"/owned/compose","file_paths":[]}),
        ] {
            core.data.lock().unwrap().stack["config"]["auto_update"] = json!(auto_update);
            let input = args(json!({"source":source}));
            let p = preview(&session, &input).await;
            assert_eq!(
                p["definition"]["details"]["requires_operate_to_apply"],
                true
            );
            // Another writer can enable auto-update after a false preview.
            core.data.lock().unwrap().stack["config"]["auto_update"] = json!(true);
            let before = core.data.lock().unwrap().requests.len();
            let denied = call(&session, "apply_stack_update", apply(&input, &p)).await;
            assert_eq!(
                denied.structured_content.unwrap()["kind"],
                "capability_disabled"
            );
            assert_eq!(core.data.lock().unwrap().requests.len(), before);
            assert!(core.writes().is_empty());
        }
        no_executions(&core);
        process.finish(session).await;
    }
}

#[tokio::test]
async fn create_inspect_patch_one_field_preserves_compose_source_and_runtime_observation() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let contents = "services:\n  web:\n    image: busybox:1.37.0\n# new-compose-private-marker\n";
    let input = creation(
        "New Stack",
        json!({"kind":"inline","contents":contents}),
        false,
    );
    let p = data(call(&session, "preview_stack_create", input.clone()).await);
    assert_eq!(p["resource"]["name"], "new_stack");
    assert!(p["resource"]["id"].is_null());
    assert_eq!(
        p["definition"]["details"]["after"]["source"]["kind"],
        "inline"
    );
    assert!(!p.to_string().contains("new-compose-private-marker"));
    assert!(core.writes().is_empty());
    let saved = data(call(&session, "apply_stack_create", apply(&input, &p)).await);
    let id = saved["resource"]["id"].as_str().unwrap();
    assert_eq!(saved["outcome"], "saved");
    assert!(saved["definition"]["details"]["observed_runtime_state"].is_null());
    assert_eq!(
        core.writes()[0].params,
        json!({"name":"new_stack","config":{"server_id":fixture::SERVER,"file_contents":contents,"auto_pull":false}})
    );
    let inspected = data(call(&session, "get_stack", json!({"id":id})).await);
    assert_eq!(inspected["state"], "down");
    let before = core.data.lock().unwrap().created[id]["config"].clone();
    let update = json!({"target":{"id":id},"patch":{"auto_pull":true}});
    let preview = preview(&session, &update).await;
    let patched = data(call(&session, "apply_stack_update", apply(&update, &preview)).await);
    assert_eq!(patched["outcome"], "saved");
    assert_eq!(
        patched["definition"]["details"]["observed_runtime_state"],
        "down"
    );
    assert_eq!(
        core.writes()[1].params,
        json!({"id":id,"config":{"auto_pull":true}})
    );
    let after = core.data.lock().unwrap().created[id]["config"].clone();
    for (field, value) in before.as_object().unwrap() {
        if field != "auto_pull" {
            assert_eq!(&after[field], value);
        }
    }
    assert!(!patched.to_string().contains("new-compose-private-marker"));
    no_executions(&core);
    process
        .finish_excluding(session, &["new-compose-private-marker"])
        .await;
}

#[tokio::test]
async fn every_declared_source_and_target_is_typed_and_settings_patch_preserves_source() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let sources = [
        json!({"kind":"inline","contents":"services: {}"}),
        json!({"kind":"git","repo":"fixtures/compose","branch":"release","provider":"git.example:3000","https":false,"account":"[[GIT_ACCOUNT_REF]]","commit":""}),
        json!({"kind":"linked_repo","target":{"name":"fixture-repo"},"run_directory":"deploy","file_paths":["compose.yaml"]}),
        json!({"kind":"host","run_directory":"/owned/compose","file_paths":[]}),
    ];
    for swarm in [false, true] {
        for (n, source) in sources.iter().enumerate() {
            let input = creation(
                &format!("matrix_{}_{}", usize::from(swarm), n),
                source.clone(),
                swarm,
            );
            let saved = create_stack(&session, &input).await;
            assert_eq!(saved["outcome"], "saved");
            assert_eq!(
                saved["definition"]["details"]["after"]["source"]["kind"],
                source["kind"]
            );
            assert_eq!(
                saved["definition"]["details"]["after"]["attachment"]["kind"],
                if swarm { "swarm" } else { "server" }
            );
            let id = saved["resource"]["id"].as_str().unwrap();
            data(call(&session, "get_stack", json!({"id":id})).await);
            let before = core.data.lock().unwrap().created[id]["config"].clone();
            let update = json!({"target":{"id":id},"patch":{"auto_pull":true}});
            let p = preview(&session, &update).await;
            assert_eq!(
                data(call(&session, "apply_stack_update", apply(&update, &p)).await)["outcome"],
                "saved"
            );
            assert_eq!(
                core.writes().last().unwrap().params["config"],
                json!({"auto_pull":true})
            );
            let after = core.data.lock().unwrap().created[id]["config"].clone();
            for (field, value) in before.as_object().unwrap() {
                if field != "auto_pull" {
                    assert_eq!(&after[field], value);
                }
            }
        }
    }
    no_executions(&core);
    assert!(!core.data.lock().unwrap().bad_credentials);
    process
        .finish_excluding(session, &["[[GIT_ACCOUNT_REF]]"])
        .await;
}

#[tokio::test]
async fn source_switches_disclose_effects_preserve_omitted_credentials_and_never_write_files() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let before = core.data.lock().unwrap().stack.clone();
    let source = json!({"kind":"git","repo":"fixtures/compose","branch":"release"});
    let input = args(json!({"source":source}));
    let p = preview(&session, &input).await;
    assert_eq!(
        p["definition"]["details"]["before"]["source"]["kind"],
        "inline"
    );
    assert_eq!(p["definition"]["details"]["after"]["source"]["kind"], "git");
    assert_eq!(
        p["definition"]["details"]["core_auto_deploy_possible"],
        false
    );
    assert_eq!(
        data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    assert_eq!(
        core.writes()[0].params["config"],
        json!({"repo":"fixtures/compose","branch":"release","file_contents":""})
    );
    assert_eq!(
        core.data.lock().unwrap().stack["config"]["git_account"],
        before["config"]["git_account"]
    );
    for source in [
        json!({"kind":"host","run_directory":"/owned/compose","file_paths":["a.yaml"]}),
        json!({"kind":"inline","contents":"services: {}","run_directory":"","file_paths":[]}),
    ] {
        let input = args(json!({"source":source.clone()}));
        let p = preview(&session, &input).await;
        assert_eq!(
            p["definition"]["details"]["after"]["source"]["kind"],
            source["kind"]
        );
        assert_eq!(
            data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
            "saved"
        );
    }
    let stored = core.data.lock().unwrap().stack.clone();
    assert_eq!(
        stored["config"]["git_account"],
        before["config"]["git_account"]
    );
    assert_eq!(
        stored["config"]["environment"],
        before["config"]["environment"]
    );
    assert_eq!(stored["config"]["files_on_host"], false);
    assert_eq!(stored["config"]["repo"], "");
    assert_eq!(stored["config"]["run_directory"], "");
    assert!(core.writes().iter().all(|r| r.operation == "UpdateStack"));
    no_executions(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn source_auto_update_and_target_reassignment_effects_are_separate_from_mcp_execution() {
    let core = Core::start().await;
    core.data.lock().unwrap().stack["config"]["auto_update"] = json!(true);
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = args(
        json!({"source":{"kind":"inline","contents":"services: {}"},"attachment":{"kind":"swarm","target":{"name":"fixture-swarm"}}}),
    );
    let p = preview(&session, &input).await;
    assert_eq!(
        p["definition"]["details"]["core_auto_deploy_possible"],
        true
    );
    assert_eq!(p["definition"]["details"]["target_changed"], true);
    assert_eq!(
        p["definition"]["details"]["old_target_cleanup"],
        "not_performed_by_inspected_core_2_3_2_update_path"
    );
    assert!(p["effects"].as_array().unwrap().iter().any(|e| {
        e.as_str()
            .unwrap()
            .contains("Core itself can automatically deploy")
    }));
    assert!(core.writes().is_empty());
    assert_eq!(
        data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    assert_eq!(core.writes()[0].params["config"]["server_id"], "");
    assert_eq!(
        core.writes()[0].params["config"]["swarm_id"],
        fixture::SWARM
    );
    no_executions(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn create_name_races_reference_changes_and_altered_creation_payloads_do_not_submit() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = creation(
        "new_stack",
        json!({"kind":"inline","contents":"services: {}"}),
        false,
    );
    let p = data(call(&session, "preview_stack_create", input.clone()).await);
    let mut changed = input.clone();
    changed["config"]["auto_pull"] = json!(true);
    assert_eq!(
        call(&session, "apply_stack_create", apply(&changed, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    let p = data(call(&session, "preview_stack_create", input.clone()).await);
    core.data.lock().unwrap().server["config"]["address"] =
        json!("https://changed-secret-endpoint.invalid");
    assert_eq!(
        call(&session, "apply_stack_create", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    let p = data(call(&session, "preview_stack_create", input.clone()).await);
    core.data.lock().unwrap().stack["name"] = json!("new_stack");
    assert_eq!(
        call(&session, "apply_stack_create", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    assert!(core.writes().is_empty());
    no_executions(&core);
    process
        .finish_excluding(session, &["changed-secret-endpoint"])
        .await;
}

#[tokio::test]
async fn attached_reference_raw_changes_invalidate_update_and_source_secrets_stay_redacted() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    for (index, patch) in [
        json!({"attachment":{"kind":"server","target":{"id":fixture::SERVER}}}),
        json!({"source":{"kind":"linked_repo","target":{"id":fixture::REPO}}}),
    ]
    .into_iter()
    .enumerate()
    {
        let input = args(patch);
        let p = preview(&session, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            d.server["config"]["address"] =
                json!(format!("https://new-private-server-{index}.invalid"));
            d.repo["config"]["git_account"] = json!(format!("new-private-repo-reference-{index}"));
        }
        assert_eq!(
            call(&session, "apply_stack_update", apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "stale_preview"
        );
    }
    core.data.lock().unwrap().stack["config"]["git_provider"] =
        json!("https://secret-user:private-upstream-token@git.invalid");
    let input = args(
        json!({"source":{"kind":"git","repo":"fixture/new","provider":"git.example","account":"new-private-account-reference"}}),
    );
    let p = preview(&session, &input).await;
    assert!(!p.to_string().contains("private-upstream-token"));
    assert!(!p.to_string().contains("new-private-account-reference"));
    let result = data(call(&session, "apply_stack_update", apply(&input, &p)).await);
    assert!(!result.to_string().contains("private-upstream-token"));
    assert!(!result.to_string().contains("new-private-account-reference"));
    assert_eq!(core.writes().len(), 1);
    no_executions(&core);
    process
        .finish_excluding(
            session,
            &[
                "private-upstream-token",
                "new-private-account-reference",
                "new-private-repo-reference",
            ],
        )
        .await;
}

#[tokio::test]
async fn create_failures_retain_submission_uncertainty_without_retries_or_replayed_tokens() {
    for reply in [
        WriteReply::Malformed,
        WriteReply::Oversized,
        WriteReply::Lost,
        WriteReply::Denied,
        WriteReply::AttachDenied,
        WriteReply::Redirect,
    ] {
        let core = Core::start().await;
        let (process, session) = Process::start_profile(&core.address, false, true).await;
        let input = creation(
            "new_stack",
            json!({"kind":"inline","contents":"services: {}"}),
            false,
        );
        let p = data(call(&session, "preview_stack_create", input.clone()).await);
        core.data.lock().unwrap().reply = reply;
        let result = data(call(&session, "apply_stack_create", apply(&input, &p)).await);
        assert_eq!(result["outcome"], "submission_unknown");
        assert_eq!(result["saved_configuration_verified"], false);
        if matches!(reply, WriteReply::AttachDenied) {
            assert_eq!(result["observation_error"]["kind"], "permission_denied");
            assert!(
                result["observation_error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("Read/Attach")
            );
        }
        assert_eq!(core.writes().len(), 1);
        assert_eq!(
            call(&session, "apply_stack_create", apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "invalid_preview"
        );
        assert_eq!(core.writes().len(), 1);
        no_executions(&core);
        process.finish(session).await;
    }
}

#[tokio::test]
async fn malformed_sources_unsupported_clears_and_missing_creation_fields_never_write() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let startup_reads = core.data.lock().unwrap().requests.len();
    for source in [
        json!({"kind":"inline","contents":""}),
        json!({"kind":"inline","contents":"services: {}","file_paths":null}),
        json!({"kind":"git","repo":"https://secret:token@git.invalid/repo"}),
        json!({"kind":"git","repo":"../repo"}),
        json!({"kind":"git","repo":"namespace/repo","branch":"main;touch owned"}),
        json!({"kind":"git","repo":"namespace/repo","provider":"git.invalid/path"}),
        json!({"kind":"git","repo":"namespace/repo","branch":""}),
        json!({"kind":"git","repo":"namespace/repo","commit":"HEAD"}),
        json!({"kind":"host","run_directory":"/owned","file_paths":[""]}),
        json!({"kind":"host","run_directory":"/owned\nnew","file_paths":[]}),
        json!({"kind":"linked_repo","target":{"id":fixture::REPO},"write_contents":"not supported"}),
    ] {
        assert_eq!(
            call(
                &session,
                "preview_stack_create",
                creation("invalid", source, false)
            )
            .await
            .is_error,
            Some(true)
        );
    }
    for input in [
        json!({"name":"missing","config":{"auto_pull":false}}),
        json!({"name":"","config":{"source":{"kind":"inline","contents":"services: {}"}}}),
        creation(
            "000000000000000000000001",
            json!({"kind":"inline","contents":"services: {}"}),
            false,
        ),
    ] {
        assert_eq!(
            call(&session, "preview_stack_create", input).await.is_error,
            Some(true)
        );
    }
    assert_eq!(core.data.lock().unwrap().requests.len(), startup_reads);
    assert!(core.writes().is_empty());
    no_executions(&core);
    process
        .finish_excluding(session, &["secret:token", "touch owned", "not supported"])
        .await;
}

#[tokio::test]
async fn unchanged_source_with_enabled_auto_update_reports_no_write_or_trigger() {
    let core = Core::start().await;
    core.data.lock().unwrap().stack["config"]["auto_update"] = json!(true);
    let contents = core.data.lock().unwrap().stack["config"]["file_contents"].clone();
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = args(json!({"source":{"kind":"inline","contents":contents}}));
    let p = preview(&session, &input).await;
    assert_eq!(p["outcome"], "no_op");
    assert_eq!(p["definition"]["details"]["before"]["auto_update"], true);
    assert_eq!(
        p["definition"]["details"]["core_auto_deploy_possible"],
        false
    );
    assert!(p["effects"].as_array().unwrap().iter().any(|e| {
        e.as_str()
            .unwrap()
            .contains("triggers no Core source-update check")
    }));
    assert_eq!(
        data(call(&session, "apply_stack_update", apply(&input, &p)).await)["outcome"],
        "no_op"
    );
    assert!(core.writes().is_empty());
    no_executions(&core);
    process.finish(session).await;
}
