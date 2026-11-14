//! ResourceSync definition acceptance through the production compiled stdio server.
#[path = "support/resource_syncs.rs"]
mod fixture;
mod support;
use fixture::{Core, Fault, PRIVATE, REPO, SYNC};
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
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let text = serde_json::to_string(&r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret));
    }
    r.structured_content.unwrap()
}
fn edit(patch: Value) -> Value {
    json!({"target":{"id":SYNC},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
async fn preview(s: &Session, input: &Value) -> Value {
    data(call(s, "preview_resource_sync_update", input.clone()).await)
}
async fn finish(p: Process, s: Session) {
    p.finish_excluding(s, &PRIVATE).await;
}
fn sources() -> Vec<Value> {
    vec![
        json!({"kind":"inline","contents":format!("[[variable]]\nname='owned'\nvalue='{}'",PRIVATE[2])}),
        json!({"kind":"git","repo":"fixture/desired","branch":"main","provider":"git.example:3000","https":false,"resource_paths":["desired.toml"]}),
        json!({"kind":"linked_repo","target":{"name":"source-repo"},"resource_paths":["linked/resources.toml"]}),
        json!({"kind":"host","resource_paths":["owned/resources.toml"]}),
    ]
}

#[tokio::test]
async fn all_sources_create_inspect_switch_and_preserve_unrelated_private_fields() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for (index, source) in sources().into_iter().enumerate() {
        let input = json!({"name":format!("new-sync-{index}"),"config":{"source":source,"managed":true,"delete":false,"include_variables":true,"include_user_groups":true,"match_tags":[]}});
        let proposed = data(call(&s, "preview_resource_sync_create", input.clone()).await);
        let details = &proposed["definition"]["details"];
        assert_eq!(details["after"]["effective_delete"], true);
        assert_eq!(details["after"]["resource_tag_filter_unrestricted"], true);
        assert_eq!(details["after"]["webhook_enabled"], false);
        assert_eq!(core.requests("write").len(), index * 2);
        let saved = data(call(&s, "apply_resource_sync_create", apply(&input, &proposed)).await);
        assert_eq!(saved["outcome"], "saved");
        assert_eq!(saved["saved_configuration_verified"], true);
        let id = saved["resource"]["id"].as_str().unwrap();
        let detail = data(call(&s, "get_resource_sync", json!({"id":id})).await);
        assert_eq!(detail["definition"]["source"]["kind"], source["kind"]);
        let created = core.saved(id);
        assert_eq!(created["config"]["webhook_enabled"], false);
        let update = edit(json!({"source":source}));
        let before = core.saved(SYNC);
        let proposed = preview(&s, &update).await;
        let saved = data(call(&s, "apply_resource_sync_update", apply(&update, &proposed)).await);
        assert_eq!(saved["saved_configuration_verified"], true);
        let after = core.saved(SYNC);
        for key in [
            "git_account",
            "webhook_secret",
            "managed",
            "delete",
            "include_variables",
            "include_user_groups",
            "pending_alert",
            "match_tags",
        ] {
            assert_eq!(
                before["config"][key], after["config"][key],
                "changed omitted {key}"
            );
        }
        let c = &after["config"];
        match source["kind"].as_str().unwrap() {
            "inline" => {
                assert!(!c["file_contents"].as_str().unwrap().is_empty());
                assert_eq!(c["repo"], "");
                assert_eq!(c["linked_repo"], "");
                assert_eq!(c["files_on_host"], false);
            }
            "git" => {
                assert_eq!(c["repo"], "fixture/desired");
                assert_eq!(c["linked_repo"], "");
                assert_eq!(c["file_contents"], "");
                assert_eq!(c["files_on_host"], false);
            }
            "linked_repo" => {
                assert_eq!(c["linked_repo"], REPO);
                assert_eq!(c["repo"], "");
                assert_eq!(c["file_contents"], "");
                assert_eq!(c["files_on_host"], false);
            }
            "host" => {
                assert_eq!(c["files_on_host"], true);
                assert_eq!(c["repo"], "");
                assert_eq!(c["linked_repo"], "");
                assert_eq!(c["file_contents"], "");
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(core.requests("write").len(), 8);
    assert!(core.requests("write").iter().all(|r| matches!(
        r.operation.as_str(),
        "CreateResourceSync" | "UpdateResourceSync"
    )));
    assert!(core.requests("execute").is_empty());
    assert!(!core.data.lock().unwrap().bad_credentials);
    finish(p, s).await;
}

#[tokio::test]
async fn partial_omission_noops_and_deliberate_clears_use_only_typed_fields() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.saved(SYNC);
    let input = edit(json!({"pending_alert":false}));
    let proposed = preview(&s, &input).await;
    assert_eq!(proposed["changes"].as_array().unwrap().len(), 1);
    assert_eq!(
        data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["outcome"],
        "saved"
    );
    assert_eq!(
        core.requests("write")[0].params,
        json!({"id":SYNC,"config":{"pending_alert":false}})
    );
    let after = core.saved(SYNC);
    for (key, value) in before["config"].as_object().unwrap() {
        if key != "pending_alert" {
            assert_eq!(&after["config"][key], value);
        }
    }
    for patch in [json!({}), json!({"pending_alert":false})] {
        let input = edit(patch);
        let proposed = preview(&s, &input).await;
        assert_eq!(proposed["outcome"], "no_op");
        assert_eq!(
            data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["writes_submitted"],
            0
        );
    }
    let input = edit(
        json!({"match_tags":[],"webhook_secret":"","source":{"kind":"host","resource_paths":[]}}),
    );
    let proposed = preview(&s, &input).await;
    assert_eq!(
        proposed["definition"]["details"]["after"]["resource_tag_filter_unrestricted"],
        true
    );
    assert_eq!(
        proposed["definition"]["details"]["after"]["source"]["resource_path_count"],
        0
    );
    let saved = data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    let last = &core.requests("write")[1].params["config"];
    assert_eq!(
        *last,
        json!({"match_tags":[],"webhook_secret":"","files_on_host":true,"file_contents":"","linked_repo":"","repo":"","resource_path":[]})
    );
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn definition_and_active_omitted_repo_secret_changes_reject_and_consume_previews() {
    for change_repo in [false, true] {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            let c = &mut d.syncs.get_mut(SYNC).unwrap()["config"];
            c["linked_repo"] = json!(REPO);
            c["file_contents"] = json!("");
        }
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit(json!({"pending_alert":false}));
        let proposed = preview(&s, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            if change_repo {
                d.repo["config"]["git_account"] = json!("changed-private-reference");
            } else {
                d.syncs.get_mut(SYNC).unwrap()["config"]["webhook_secret"] =
                    json!("secret-only-change");
            }
        }
        assert_eq!(
            error(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["kind"],
            "stale_preview"
        );
        assert_eq!(
            error(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["kind"],
            "invalid_preview"
        );
        assert!(core.requests("write").is_empty());
        finish(p, s).await;
    }
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = edit(json!({"managed":true}));
    let proposed = preview(&s, &input).await;
    let mut changed = apply(&input, &proposed);
    changed["patch"]["delete"] = json!(true);
    let reads = core.requests("read").len();
    assert_eq!(
        error(call(&s, "apply_resource_sync_update", changed).await)["kind"],
        "invalid_preview"
    );
    assert_eq!(core.requests("read").len(), reads);
    assert_eq!(
        error(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["kind"],
        "invalid_preview"
    );
    assert!(core.requests("write").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn saved_echo_and_independent_full_readback_preserve_uncertainty_without_retry() {
    for fault in [
        Fault::Denied,
        Fault::Lost,
        Fault::MissingRequested,
        Fault::ChangeUnrelated,
        Fault::ReadBackDenied,
        Fault::MissingRaw,
    ] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit(json!({"managed":true}));
        let proposed = preview(&s, &input).await;
        core.data.lock().unwrap().fault = fault;
        let r = data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await);
        let acknowledged = matches!(
            fault,
            Fault::ChangeUnrelated | Fault::ReadBackDenied | Fault::MissingRaw
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
        assert!(!r["observation_error"].is_null());
        assert_eq!(r["writes_submitted"], 1);
        assert_eq!(core.requests("write").len(), 1);
        assert_eq!(
            error(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["kind"],
            "invalid_preview"
        );
        assert_eq!(core.requests("write").len(), 1);
        assert!(core.requests("execute").is_empty());
        finish(p, s).await;
    }
}

#[tokio::test]
async fn capability_discovery_direct_dispatch_and_execution_effects_are_independent() {
    for operate in [false, true] {
        for edit_grant in [false, true] {
            let core = Core::start().await;
            let (p, s) = Process::start_profile(&core.address, operate, edit_grant).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            for name in ["commit_sync", "write_sync_file_contents"] {
                assert!(!tools.iter().any(|t| t.name == name));
                let reads = core.requests("read").len();
                let result = s
                    .call_tool(
                        CallToolRequestParams::new(name)
                            .with_arguments(json!({}).as_object().unwrap().clone()),
                    )
                    .await;
                assert!(result.is_err() || result.unwrap().is_error == Some(true));
                assert_eq!(core.requests("read").len(), reads);
            }
            for name in ["list_resource_syncs", "get_resource_sync"] {
                assert!(tools.iter().any(|t| t.name == name));
            }
            for name in [
                "preview_resource_sync_create",
                "apply_resource_sync_create",
                "preview_resource_sync_update",
                "apply_resource_sync_update",
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), edit_grant);
                if !edit_grant {
                    let before = core.requests("read").len();
                    let r = s
                        .call_tool(
                            CallToolRequestParams::new(name)
                                .with_arguments(json!({}).as_object().unwrap().clone()),
                        )
                        .await;
                    assert!(r.is_err() || r.unwrap().is_error == Some(true));
                    assert_eq!(core.requests("read").len(), before);
                }
            }
            data(call(&s, "get_resource_sync", json!({"id":SYNC})).await);
            if edit_grant {
                for patch in [
                    json!({"managed":true}),
                    json!({"delete":true}),
                    json!({"match_tags":[]}),
                    json!({"include_resources":false}),
                    json!({"include_variables":true}),
                    json!({"include_user_groups":true}),
                    json!({"webhook_enabled":true}),
                    json!({"webhook_secret":"replacement-private"}),
                    json!({"source":{"kind":"inline","contents":"[[server]]\nname='desired'"}}),
                ] {
                    let input = edit(patch);
                    let proposed = preview(&s, &input).await;
                    let before = core.requests("read").len();
                    let r = call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await;
                    if operate {
                        assert_eq!(data(r)["outcome"], "saved");
                    } else {
                        assert_eq!(error(r)["kind"], "capability_disabled");
                        assert_eq!(core.requests("read").len(), before);
                        assert!(core.requests("write").is_empty());
                    }
                }
            }
            assert!(core.requests("execute").is_empty());
            finish(p, s).await;
        }
    }
}

#[tokio::test]
async fn edit_only_safe_changes_and_inert_creation_keep_admin_and_attach_denials_authoritative() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, false, true).await;
    let automatic = json!({"name":"automatic-sync","config":{"source":{"kind":"inline","contents":""},"webhook_enabled":true}});
    let automatic_preview = data(call(&s, "preview_resource_sync_create", automatic.clone()).await);
    let reads = core.requests("read").len();
    assert_eq!(
        error(
            call(
                &s,
                "apply_resource_sync_create",
                apply(&automatic, &automatic_preview)
            )
            .await
        )["kind"],
        "capability_disabled"
    );
    assert_eq!(core.requests("read").len(), reads);
    assert!(core.requests("write").is_empty());
    for patch in [
        json!({}),
        json!({"pending_alert":false}),
        json!({"webhook_enabled":false}),
    ] {
        let input = edit(patch);
        let proposed = preview(&s, &input).await;
        assert_eq!(
            proposed["definition"]["details"]["requires_operate_to_apply"],
            false
        );
        assert_eq!(
            data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
            true
        );
    }
    let input = json!({"name":"inert-sync","config":{"source":{"kind":"inline","contents":""}}});
    let proposed = data(call(&s, "preview_resource_sync_create", input.clone()).await);
    let saved = data(call(&s, "apply_resource_sync_create", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        saved["definition"]["details"]["after"]["webhook_enabled"],
        false
    );
    let mut denied_input = input.clone();
    denied_input["name"] = json!("admin-required");
    let proposed = data(call(&s, "preview_resource_sync_create", denied_input.clone()).await);
    core.data.lock().unwrap().deny_create = true;
    let denied = data(
        call(
            &s,
            "apply_resource_sync_create",
            apply(&denied_input, &proposed),
        )
        .await,
    );
    assert_eq!(denied["observation_error"]["kind"], "permission_denied");
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = edit(json!({"source":{"kind":"linked_repo","target":{"id":REPO}}}));
    let proposed = preview(&s, &input).await;
    core.data.lock().unwrap().deny_attach = true;
    let denied = data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await);
    assert_eq!(denied["observation_error"]["kind"], "permission_denied");
    assert_eq!(core.requests("write").len(), 1);
    core.data.lock().unwrap().deny_read = true;
    assert_eq!(
        error(call(&s, "get_resource_sync", json!({"id":SYNC})).await)["kind"],
        "permission_denied"
    );
    assert!(!core.data.lock().unwrap().bad_credentials);
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn invalid_inputs_and_secret_bearing_toml_errors_are_bounded_before_reads() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for patch in [
        json!({"source":null}),
        json!({"managed":null}),
        json!({"unknown":"value"}),
        json!({"file_contents":"passthrough"}),
        json!({"source":{"kind":"git","repo":"https://user:private-url-password@git.invalid/repo"}}),
        json!({"source":{"kind":"git","repo":"fixture/repo","branch":"../escape"}}),
        json!({"source":{"kind":"host","resource_paths":["../outside.toml"]}}),
        json!({"source":{"kind":"host","resource_paths":["/etc/private.toml"]}}),
        json!({"source":{"kind":"host","resource_paths":vec!["x";33]}}),
        json!({"match_tags":vec!["x";33]}),
        json!({"webhook_secret":"[redacted]"}),
        json!({"source":{"kind":"inline","contents":"x".repeat(16385)}}),
        json!({"source":{"kind":"inline","contents":format!("[[server]]\nname='{}",PRIVATE[2])}}),
        json!({"source":{"kind":"inline","contents":format!("[[server]]\nname='ok'\nconfig.enabled='{}'",PRIVATE[2])}}),
        json!({"source":{"kind":"inline","contents":format!("[{}]\nvalue='secret'",PRIVATE[2])}}),
    ] {
        let before = core.requests("read").len();
        let e = error(call(&s, "preview_resource_sync_update", edit(patch)).await);
        assert_eq!(e["kind"], "invalid_arguments");
        assert_eq!(core.requests("read").len(), before);
        assert!(!e.to_string().contains("private-url-password"));
    }
    for config in [
        json!({}),
        json!({"source":{"kind":"git","repo":"fixture/source"}}),
        json!({"source":{"kind":"linked_repo","target":{"id":REPO}}}),
    ] {
        assert_eq!(
            error(
                call(
                    &s,
                    "preview_resource_sync_create",
                    json!({"name":"missing-source-paths","config":config})
                )
                .await
            )["kind"],
            "invalid_arguments"
        );
    }
    assert!(core.requests("write").is_empty());
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn pagination_exact_identity_and_bounded_inspection_never_mutate() {
    let core = Core::start().await;
    let original = core.saved(SYNC);
    {
        let mut d = core.data.lock().unwrap();
        d.syncs.clear();
        for i in 0..101 {
            let mut v = original.clone();
            let id = format!("{:024x}", 1000 + i);
            v["_id"]["$oid"] = json!(id);
            v["name"] = json!(format!("sync-{i:03}"));
            d.syncs.insert(id, v);
        }
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let page = data(call(&s, "list_resource_syncs", json!({"page_size":100})).await);
    assert_eq!(page["items"].as_array().unwrap().len(), 100);
    assert_eq!(page["next_page"], 1);
    let next = data(call(&s, "list_resource_syncs", json!({"page_size":100,"page":1})).await);
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["has_more"], false);
    for page_size in [0, 101] {
        assert_eq!(
            error(call(&s, "list_resource_syncs", json!({"page_size":page_size})).await)["kind"],
            "invalid_arguments"
        );
    }
    assert_eq!(
        error(call(&s, "get_resource_sync", json!({"name":"missing"})).await)["kind"],
        "not_found"
    );
    {
        let mut d = core.data.lock().unwrap();
        let mut v = original.clone();
        v["name"] = json!("00000000000000000000abcd");
        v["config"]["match_tags"] = json!(vec!["safe"; 40]);
        v["config"]["resource_path"] = json!(vec!["owned.toml"; 40]);
        d.syncs.insert(SYNC.into(), v);
    }
    let detail = data(
        call(
            &s,
            "get_resource_sync",
            json!({"name":"00000000000000000000abcd"}),
        )
        .await,
    );
    assert_eq!(detail["summary"]["id"], SYNC);
    assert_eq!(detail["definition"]["truncated"], true);
    assert_eq!(detail["definition"]["source"]["truncated"], true);
    assert_eq!(
        detail["definition"]["match_tags"].as_array().unwrap().len(),
        32
    );
    assert!(core.requests("write").is_empty());
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn private_references_for_supplied_linked_repo_and_git_option_omission_are_preserved() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = edit(json!({"source":{"kind":"linked_repo","target":{"id":REPO}}}));
    let proposed = preview(&s, &input).await;
    core.data.lock().unwrap().repo["config"]["webhook_secret"] = json!("linked-private-drift");
    assert_eq!(
        error(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await)["kind"],
        "stale_preview"
    );
    let input = edit(json!({"source":{"kind":"git","repo":"fixture/source"}}));
    let before = core.saved(SYNC);
    let proposed = preview(&s, &input).await;
    let saved = data(call(&s, "apply_resource_sync_update", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    let after = core.saved(SYNC);
    for key in [
        "git_account",
        "branch",
        "commit",
        "git_provider",
        "git_https",
        "resource_path",
        "webhook_secret",
    ] {
        assert_eq!(before["config"][key], after["config"][key]);
    }
    assert_eq!(core.requests("write").len(), 1);
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}

#[tokio::test]
async fn inline_validation_matches_core_triple_escaping_and_definition_names_use_core_normalization()
 {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    // Core 2.3.2 adapts backslashes inside triple double quotes before parsing.
    let contents = "[[server]]\nname='desired'\n[server.config]\naddress=\"\"\"https://example.invalid\\path\"\"\"\n  ";
    let input =
        json!({"name":"  Owned Sync  ","config":{"source":{"kind":"inline","contents":contents}}});
    let proposed = data(call(&s, "preview_resource_sync_create", input.clone()).await);
    assert_eq!(proposed["resource"]["name"], "Owned Sync");
    let saved = data(call(&s, "apply_resource_sync_create", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    let expected = format!("{}\n", contents.trim_end());
    assert_eq!(
        core.requests("write")[0].params["config"]["file_contents"],
        expected
    );
    let id = saved["resource"]["id"].as_str().unwrap();
    let update = json!({"target":{"id":id},"patch":{"source":{"kind":"inline","contents":format!("{expected}   ")}}});
    let proposed = preview(&s, &update).await;
    assert_eq!(proposed["outcome"], "no_op");
    assert_eq!(
        data(call(&s, "apply_resource_sync_update", apply(&update, &proposed)).await)["writes_submitted"],
        0
    );
    assert_eq!(core.requests("write").len(), 1);
    assert!(core.requests("execute").is_empty());
    finish(p, s).await;
}
