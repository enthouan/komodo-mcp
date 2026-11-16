//! Acceptance of Server Builder and Build definitions through production stdio.
#[path = "support/build_definitions.rs"]
mod fixture;
mod support;
use fixture::{BUILD, BUILDER, Core, DEPLOYMENT, Fault, PRIVATE, REPO, SERVER, SERVER2};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    let text = serde_json::to_string(&r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret), "private definition leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true));
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn edit(kind: &str, patch: Value) -> Value {
    json!({"target":{"id":if kind=="builder" {BUILDER} else {BUILD}},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut a = input.clone();
    a["preview_id"] = p["preview_id"].clone();
    a
}
async fn preview(s: &Session, kind: &str, input: &Value) -> Value {
    data(call(s, &format!("preview_{kind}_update"), input.clone()).await)
}
async fn finish(p: Process, s: Session, core: &Core) {
    assert!(core.requests("execute").is_empty());
    assert!(!core.data.lock().unwrap().bad_credentials);
    p.finish_excluding(s, &PRIVATE).await;
}
fn create_build(source: Value) -> Value {
    json!({"name":"new-build","config":{"builder":{"id":BUILDER},"source":source,"image_name":"owned/workload","image_registry":[]}})
}

#[tokio::test]
async fn discovery_and_direct_calls_enforce_every_local_profile_without_core_io() {
    for operate in [false, true] {
        for edit_grant in [false, true] {
            let core = Core::start().await;
            let (p, s) = Process::start_profile(&core.address, operate, edit_grant).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                58 + 20 * usize::from(operate)
                    + 54 * usize::from(edit_grant)
                    + 3 * usize::from(operate && edit_grant)
            );
            for name in [
                "list_builders",
                "get_builder",
                "list_builds",
                "get_build",
                "preview_builder_create",
                "apply_builder_create",
                "preview_builder_update",
                "apply_builder_update",
                "preview_build_create",
                "apply_build_create",
                "preview_build_update",
                "apply_build_update",
            ] {
                let enabled =
                    !name.starts_with("preview_") && !name.starts_with("apply_") || edit_grant;
                assert_eq!(tools.iter().any(|t| t.name == name), enabled);
                let before = core.requests("read").len();
                let r =
                    s.call_tool(CallToolRequestParams::new(name).with_arguments(
                        json!({"unexpected":"value"}).as_object().unwrap().clone(),
                    ))
                    .await;
                if enabled {
                    assert_eq!(r.unwrap().is_error, Some(true));
                } else {
                    assert!(r.is_err());
                }
                assert_eq!(core.requests("read").len(), before);
                assert!(core.requests("write").is_empty());
            }
            finish(p, s, &core).await;
        }
    }
}

#[tokio::test]
async fn builder_creation_and_edits_preserve_explicit_variant_and_ordered_fallback() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"name":"new-builder","config":{"kind":"server","servers":[{"id":SERVER2},{"id":SERVER}]}});
    let proposed = data(call(&s, "preview_builder_create", input.clone()).await);
    assert!(core.requests("write").is_empty());
    let saved = data(call(&s, "apply_builder_create", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        core.requests("write")[0].params,
        json!({"name":"new-builder","config":{"type":"Server","params":{"server_ids":[SERVER2,SERVER]}}})
    );
    let inspected = data(call(&s, "get_builder", json!({"id":saved["resource"]["id"]})).await);
    assert_eq!(
        inspected["definition"]["ordered_server_ids"],
        json!([SERVER2, SERVER])
    );
    let input = edit(
        "builder",
        json!({"kind":"server","servers":[{"name":"second-server"},{"id":SERVER}]}),
    );
    let proposed = preview(&s, "builder", &input).await;
    let saved = data(call(&s, "apply_builder_update", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        core.requests("write")[1].params,
        json!({"id":BUILDER,"config":{"type":"Server","params":{"server_ids":[SERVER2,SERVER]}}})
    );
    assert_eq!(
        core.builder()["config"]["params"]["server_ids"],
        json!([SERVER2, SERVER])
    );
    for patch in [
        json!({"kind":"server"}),
        json!({"kind":"server","servers":[{"id":SERVER2},{"id":SERVER}]}),
    ] {
        let input = edit("builder", patch);
        let proposed = preview(&s, "builder", &input).await;
        assert_eq!(proposed["outcome"], "no_op");
        assert_eq!(
            data(call(&s, "apply_builder_update", apply(&input, &proposed)).await)["outcome"],
            "no_op"
        );
        assert_eq!(core.requests("write").len(), 2);
    }
    let input = edit("builder", json!({"kind":"server","servers":[]}));
    let proposed = preview(&s, "builder", &input).await;
    assert_eq!(
        data(call(&s, "apply_builder_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(core.builder()["config"]["params"]["server_ids"], json!([]));
    finish(p, s, &core).await;
}

#[tokio::test]
async fn unsupported_builders_are_inspectable_without_secrets_and_never_converted() {
    for kind in ["Aws", "Url"] {
        let core = Core::start().await;
        core.data.lock().unwrap().builders.get_mut(BUILDER).unwrap()["config"] = if kind == "Aws" {
            json!({"type":"Aws","params":{"user_data":PRIVATE[4]}})
        } else {
            json!({"type":"Url","params":{"address":format!("https://private:{}@agent.invalid",PRIVATE[6]),"passkey":PRIVATE[6]}})
        };
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let listed = data(call(&s, "list_builders", json!({"page_size":1})).await);
        assert_eq!(listed["items"][0]["supported"], false);
        let detail = data(call(&s, "get_builder", json!({"id":BUILDER})).await);
        assert_eq!(detail["definition"]["supported"], false);
        assert!(
            detail["definition"]["support_limit"]
                .as_str()
                .unwrap()
                .contains("existing-Server")
        );
        let build = data(call(&s, "get_build", json!({"id":BUILD})).await);
        assert_eq!(build["builder"]["supported"], false);
        for (family, patch) in [
            (
                "builder",
                json!({"kind":"server","servers":[{"id":SERVER}]}),
            ),
            ("build", json!({"image_tag":"next"})),
        ] {
            assert_eq!(
                error(call(&s, &format!("preview_{family}_update"), edit(family, patch)).await)["kind"],
                "unsupported_target"
            );
        }
        assert!(core.requests("write").is_empty());
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn builder_creation_retains_core_admin_rule_and_never_borrows_credentials() {
    let core = Core::start().await;
    core.data.lock().unwrap().admin = false;
    let (p, s) = Process::start_profile(&core.address, false, true).await;
    let input =
        json!({"name":"denied-builder","config":{"kind":"server","servers":[{"id":SERVER}]}});
    let proposed = data(call(&s, "preview_builder_create", input.clone()).await);
    let saved = data(call(&s, "apply_builder_create", apply(&input, &proposed)).await);
    assert_eq!(saved["outcome"], "submission_unknown");
    assert_eq!(saved["observation_error"]["kind"], "permission_denied");
    assert_eq!(core.requests("write").len(), 1);
    assert_eq!(core.data.lock().unwrap().builders.len(), 1);
    assert_eq!(
        error(call(&s, "apply_builder_create", apply(&input, &proposed)).await)["kind"],
        "invalid_preview"
    );
    finish(p, s, &core).await;
}

#[tokio::test]
async fn build_creation_supports_explicit_sources_without_running_builds() {
    for source in [
        json!({"kind":"inline","dockerfile":"FROM scratch"}),
        json!({"kind":"git","repo":"fixture/source","branch":"release/v05","git_https":false,"git_provider":"source.invalid:8000","commit":"abcd1234"}),
        json!({"kind":"linked_repo","target":{"id":REPO}}),
        json!({"kind":"host"}),
    ] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = create_build(source.clone());
        let proposed = data(call(&s, "preview_build_create", input.clone()).await);
        assert!(core.requests("write").is_empty());
        assert_eq!(
            proposed["definition"]["details"]["after"]["webhook_enabled"],
            false
        );
        let saved = data(call(&s, "apply_build_create", apply(&input, &proposed)).await);
        assert_eq!(saved["saved_configuration_verified"], true);
        let req = &core.requests("write")[0].params;
        assert_eq!(req["config"]["builder_id"], BUILDER);
        assert_eq!(req["config"]["webhook_enabled"], false);
        assert_eq!(req["config"]["image_registry"], json!([]));
        assert!(req["config"].get("secret_args").is_none());
        assert!(req["config"].get("version").is_none());
        let inspected = data(call(&s, "get_build", json!({"id":saved["resource"]["id"]})).await);
        assert_eq!(inspected["definition"]["source"]["kind"], source["kind"]);
        assert_eq!(
            inspected["builder"]["ordered_server_ids"],
            json!([SERVER, SERVER2])
        );
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn one_field_build_edits_preserve_secrets_source_and_other_settings_and_read_back() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.build();
    let input = edit("build", json!({"image_tag":"next"}));
    let proposed = preview(&s, "build", &input).await;
    let definition = &proposed["definition"]["details"];
    assert_eq!(
        definition["dependent_deployments"]["visible"][0]["id"],
        DEPLOYMENT
    );
    assert_eq!(
        definition["dependent_deployments"]["visible"][0]["redeploy_on_build"],
        true
    );
    assert!(
        definition["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["visible_reference_found"] == true)
    );
    let saved = data(call(&s, "apply_build_update", apply(&input, &proposed)).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    assert_eq!(
        core.requests("write")[0].params,
        json!({"id":BUILD,"config":{"image_tag":"next"}})
    );
    let after = core.build();
    for (key, value) in before["config"].as_object().unwrap() {
        if key != "image_tag" {
            assert_eq!(&after["config"][key], value);
        }
    }
    assert_eq!(core.requests("read").last().unwrap().operation, "GetBuild");
    let get = data(call(&s, "get_build", json!({"id":BUILD})).await);
    assert_eq!(get["cached_built_revision"], "abc1234");
    assert_eq!(get["definition"]["source"]["configured_branch"], "main");
    assert_eq!(get["definition"]["pre_build_configured"], true);
    finish(p, s, &core).await;
}

#[tokio::test]
async fn build_noops_clears_and_source_switches_have_exact_partial_payloads() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for patch in [json!({}), json!({"image_tag":""})] {
        let input = edit("build", patch);
        let proposed = preview(&s, "build", &input).await;
        assert_eq!(proposed["outcome"], "no_op");
        assert_eq!(
            data(call(&s, "apply_build_update", apply(&input, &proposed)).await)["outcome"],
            "no_op"
        );
    }
    assert!(core.requests("write").is_empty());
    let input = edit(
        "build",
        json!({"git_account":"","secret_args":"","build_args":"","image_registry":[],"extra_args":[],"pre_build":{"path":"","command":"","shell_mode":false}}),
    );
    let proposed = preview(&s, "build", &input).await;
    assert_eq!(
        data(call(&s, "apply_build_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(core.requests("write")[0].params["config"], input["patch"]);
    let input = edit(
        "build",
        json!({"source":{"kind":"inline","dockerfile":"FROM scratch"}}),
    );
    let proposed = preview(&s, "build", &input).await;
    assert_eq!(
        data(call(&s, "apply_build_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(
        core.requests("write")[1].params["config"],
        json!({"files_on_host":false,"repo":"","linked_repo":"","dockerfile":"FROM scratch"})
    );
    assert_eq!(core.build()["config"]["webhook_secret"], PRIVATE[3]);
    finish(p, s, &core).await;
}

#[tokio::test]
async fn stale_private_baselines_and_effective_references_consume_previews_without_writes() {
    for mode in 0..7 {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = if mode == 6 {
            edit(
                "builder",
                json!({"kind":"server","servers":[{"id":SERVER2}]}),
            )
        } else if mode == 3 {
            edit(
                "build",
                json!({"source":{"kind":"linked_repo","target":{"id":REPO}}}),
            )
        } else {
            edit("build", json!({"image_tag":"new"}))
        };
        let family = if mode == 6 { "builder" } else { "build" };
        let proposed = preview(&s, family, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            match mode {
                0 => {
                    d.builds.get_mut(BUILD).unwrap()["config"]["webhook_secret"] =
                        json!("changed-private")
                }
                1 => {
                    d.builders.get_mut(BUILDER).unwrap()["config"]["params"]["server_ids"] =
                        json!([SERVER2, SERVER])
                }
                2 | 6 => {
                    d.servers.get_mut(SERVER2).unwrap()["info"]["public_key"] = json!("changed-key")
                }
                3 => {
                    d.repos.get_mut(REPO).unwrap()["config"]["git_account"] =
                        json!("changed-account")
                }
                4 => d.git_accounts[0]["token"] = json!("changed-private-token"),
                5 => {
                    d.deployments.get_mut(DEPLOYMENT).unwrap()["config"]["redeploy_on_build"] =
                        json!(false)
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(
            error(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed)
                )
                .await
            )["kind"],
            "stale_preview"
        );
        assert_eq!(
            error(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed)
                )
                .await
            )["kind"],
            "invalid_preview"
        );
        assert!(core.requests("write").is_empty());
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn edit_only_cannot_redirect_builders_or_trigger_build_cache_or_automatic_execution() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, false, true).await;
    for (family, patch) in [
        (
            "builder",
            json!({"kind":"server","servers":[{"id":SERVER2}]}),
        ),
        ("build", json!({"image_tag":"new"})),
        ("build", json!({"webhook_enabled":false})),
        (
            "build",
            json!({"source":{"kind":"inline","dockerfile":"FROM scratch"}}),
        ),
        (
            "build",
            json!({"pre_build":{"path":"","command":"echo effect","shell_mode":true}}),
        ),
    ] {
        let input = edit(family, patch);
        let proposed = preview(&s, family, &input).await;
        let reads = core.requests("read").len();
        assert_eq!(
            error(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed)
                )
                .await
            )["kind"],
            "capability_disabled"
        );
        assert_eq!(core.requests("read").len(), reads);
        assert!(core.requests("write").is_empty());
    }
    for (family, patch) in [("builder", json!({"kind":"server"})), ("build", json!({}))] {
        let input = edit(family, patch);
        let proposed = preview(&s, family, &input).await;
        assert_eq!(
            data(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed)
                )
                .await
            )["outcome"],
            "no_op"
        );
    }
    let input = create_build(json!({"kind":"inline","dockerfile":"FROM scratch"}));
    let proposed = data(call(&s, "preview_build_create", input.clone()).await);
    assert_eq!(
        data(call(&s, "apply_build_create", apply(&input, &proposed)).await)["saved_configuration_verified"],
        true
    );
    for source in [
        json!({"kind":"host"}),
        json!({"kind":"git","repo":"fixture/workload","branch":"main"}),
        json!({"kind":"linked_repo","target":{"id":REPO}}),
    ] {
        let mut input = create_build(source);
        input["name"] = json!("other-new-build");
        let proposed = data(call(&s, "preview_build_create", input.clone()).await);
        let reads = core.requests("read").len();
        assert_eq!(
            error(call(&s, "apply_build_create", apply(&input, &proposed)).await)["kind"],
            "capability_disabled"
        );
        assert_eq!(core.requests("read").len(), reads);
    }
    assert_eq!(core.requests("write").len(), 1);
    finish(p, s, &core).await;
}

#[tokio::test]
async fn credentials_visible_absent_or_hidden_have_explicit_evidence_without_elevation() {
    for mode in 0..3 {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            match mode {
                0 => {}
                1 => {
                    d.deny_account_reads = true;
                    d.deny_config_reads = true;
                }
                2 => {
                    d.git_accounts.clear();
                    d.config_git.clear();
                }
                _ => unreachable!(),
            }
        }
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit("build", json!({"image_tag":"next"}));
        if mode == 2 {
            assert_eq!(
                error(call(&s, "preview_build_update", input).await)["kind"],
                "invalid_arguments"
            );
            assert!(core.requests("write").is_empty());
        } else {
            let proposed = preview(&s, "build", &input).await;
            let creds = proposed["definition"]["details"]["credentials"]
                .as_array()
                .unwrap();
            assert_eq!(creds.len(), 2);
            assert!(
                creds
                    .iter()
                    .all(|c| c["visible_reference_found"] == (mode == 0)
                        && c["sources_complete"] == (mode == 0))
            );
            assert_eq!(
                data(call(&s, "apply_build_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
                true
            );
            let server_ids = core
                .requests("read")
                .iter()
                .filter(|r| r.operation == "ListGitProvidersFromConfig")
                .map(|r| r.params["target"]["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert!(server_ids.contains(&SERVER.into()) && server_ids.contains(&SERVER2.into()));
        }
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn denied_reference_or_write_paths_stop_without_fallback_or_replay() {
    for deny in ["GetServer", "GetBuilder", "GetRepo", "write"] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        if deny != "write" {
            core.data.lock().unwrap().deny_read = Some(deny.into());
        }
        let input = edit(
            "build",
            json!({"source":{"kind":"linked_repo","target":{"id":REPO}}}),
        );
        if deny == "write" {
            let proposed = preview(&s, "build", &input).await;
            core.data.lock().unwrap().fault = Fault::Denied;
            let saved = data(call(&s, "apply_build_update", apply(&input, &proposed)).await);
            assert_eq!(saved["outcome"], "submission_unknown");
            assert_eq!(saved["observation_error"]["kind"], "permission_denied");
            assert_eq!(core.requests("write").len(), 1);
        } else {
            assert_eq!(
                error(call(&s, "preview_build_update", input).await)["kind"],
                "permission_denied"
            );
            assert!(core.requests("write").is_empty());
        }
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn ambiguous_ack_and_complete_readback_failures_never_retry_or_manufacture_success() {
    for family in ["builder", "build"] {
        for fault in [
            Fault::Lost,
            Fault::MissingEcho,
            Fault::ChangeUnrelated,
            Fault::ReadBackDenied,
        ] {
            let core = Core::start().await;
            let (p, s) = Process::start_profile(&core.address, true, true).await;
            let input = edit(
                family,
                if family == "builder" {
                    json!({"kind":"server","servers":[{"id":SERVER}]})
                } else {
                    json!({"image_tag":"next"})
                },
            );
            let proposed = preview(&s, family, &input).await;
            core.data.lock().unwrap().fault = fault;
            let saved = data(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed),
                )
                .await,
            );
            assert_eq!(saved["saved_configuration_verified"], false);
            assert_eq!(
                saved["outcome"],
                if matches!(fault, Fault::Lost | Fault::MissingEcho) {
                    "submission_unknown"
                } else {
                    "saved"
                }
            );
            assert_eq!(core.requests("write").len(), 1);
            assert_eq!(
                error(
                    call(
                        &s,
                        &format!("apply_{family}_update"),
                        apply(&input, &proposed)
                    )
                    .await
                )["kind"],
                "invalid_preview"
            );
            finish(p, s, &core).await;
            let (p, s) = Process::start_profile(&core.address, true, true).await;
            assert_eq!(
                error(
                    call(
                        &s,
                        &format!("apply_{family}_update"),
                        apply(&input, &proposed)
                    )
                    .await
                )["kind"],
                "invalid_preview"
            );
            assert_eq!(core.requests("write").len(), 1);
            finish(p, s, &core).await;
        }
    }
}

#[tokio::test]
async fn invalid_variants_nulls_passthrough_and_credential_urls_are_rejected_before_reads() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for (family, patch) in [
        ("builder", json!({"kind":"aws","servers":[]})),
        (
            "builder",
            json!({"type":"Server","params":{"server_ids":[SERVER]}}),
        ),
        (
            "builder",
            json!({"kind":"url","address":"https://private.invalid"}),
        ),
        ("builder", json!({"kind":"server","servers":null})),
        ("build", json!({"builder_id":BUILDER})),
        ("build", json!({"source":null})),
        (
            "build",
            json!({"image_registry":[{"domain":"https://user:private@registry.invalid","account":"account","organization":""}]}),
        ),
        (
            "build",
            json!({"source":{"kind":"git","repo":"https://user:private@repo.invalid/x","branch":"main"}}),
        ),
        ("build", json!({"image_tag":"bad;command"})),
        ("build", json!({"version":"01.0.0"})),
        ("build", json!({"schedule":"* * * * * *"})),
    ] {
        let reads = core.requests("read").len();
        assert_eq!(
            error(call(&s, &format!("preview_{family}_update"), edit(family, patch)).await)["kind"],
            "invalid_arguments"
        );
        assert_eq!(core.requests("read").len(), reads);
    }
    assert!(core.requests("write").is_empty());
    finish(p, s, &core).await;
}

#[tokio::test]
async fn bounded_listing_identity_ambiguity_and_large_unsupported_detail_remain_safe() {
    let core = Core::start().await;
    {
        let mut d = core.data.lock().unwrap();
        for n in 1..4 {
            let mut b = d.builders[BUILDER].clone();
            let id = format!("{:024x}", 200 + n);
            b["_id"]["$oid"] = json!(id);
            b["name"] = json!(format!("fixture-builder-{n}"));
            d.builders.insert(id, b);
        }
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let first = data(call(&s, "list_builders", json!({"page_size":2})).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["next_page"], 1);
    let last = data(call(&s, "list_builders", json!({"page":1,"page_size":2})).await);
    assert_eq!(last["has_more"], false);
    let reads = core.requests("read").len();
    assert_eq!(
        error(call(&s, "list_builds", json!({"page_size":0})).await)["kind"],
        "invalid_arguments"
    );
    assert_eq!(core.requests("read").len(), reads);
    {
        let mut d = core.data.lock().unwrap();
        d.builders.get_mut(BUILDER).unwrap()["config"] =
            json!({"type":"Aws","params":{"user_data":"X".repeat(2*1024*1024)}});
    }
    assert_eq!(
        error(call(&s, "get_builder", json!({"id":BUILDER})).await)["kind"],
        "response_too_large"
    );
    finish(p, s, &core).await;
}

#[tokio::test]
async fn write_deadline_and_direct_library_policy_preserve_unknown_save_without_retry() {
    use komodo_mcp::{
        build_configuration::{self, ApplyInput, PreviewInput},
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
    };
    use std::time::Duration;
    let core = Core::start().await;
    let client = |edit_grant| {
        CoreClient::new(
            Config::parse(&core.address, support::KEY, support::SECRET)
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(true, edit_grant)),
            RequestPolicy {
                connect_timeout: Duration::from_secs(1),
                operation_timeout: Duration::from_secs(1),
            },
        )
        .unwrap()
    };
    let input = edit("build", json!({"image_tag":"next"}));
    let denied = build_configuration::preview_update(
        &client(false),
        serde_json::from_value::<PreviewInput>(input.clone()).unwrap(),
    )
    .await;
    assert!(matches!(denied,Err(e) if e.kind==komodo_mcp::error::ErrorKind::CapabilityDisabled));
    assert!(core.requests("read").is_empty());
    let allowed = client(true);
    let proposed = build_configuration::preview_update(
        &allowed,
        serde_json::from_value::<PreviewInput>(input.clone()).unwrap(),
    )
    .await
    .unwrap();
    core.data.lock().unwrap().fault = Fault::Delayed;
    let saved = build_configuration::apply_update(
        &allowed,
        serde_json::from_value::<ApplyInput>(apply(
            &input,
            &serde_json::to_value(proposed).unwrap(),
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    let saved = serde_json::to_value(saved).unwrap();
    assert_eq!(saved["outcome"], "submission_unknown");
    assert_eq!(saved["observation_error"]["kind"], "timeout");
    assert_eq!(core.requests("write").len(), 1);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(core.requests("write").len(), 1);
    assert_eq!(core.build()["config"]["image_tag"], "next");
    assert!(core.requests("execute").is_empty());
}

#[tokio::test]
async fn shared_transport_rechecks_variant_and_reports_preflight_rejection_without_a_write() {
    for family in ["builder", "build"] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let input = edit(
            family,
            if family == "builder" {
                json!({"kind":"server","servers":[{"id":SERVER2}]})
            } else {
                json!({"image_tag":"next"})
            },
        );
        let proposed = preview(&s, family, &input).await;
        let count = core
            .requests("read")
            .iter()
            .filter(|r| r.operation == "GetBuilder")
            .count();
        core.data.lock().unwrap().switch_builder_on_read = Some(count + 2);
        let result = error(
            call(
                &s,
                &format!("apply_{family}_update"),
                apply(&input, &proposed),
            )
            .await,
        );
        assert_eq!(result["kind"], "unsupported_target");
        assert!(core.requests("write").is_empty());
        assert_eq!(
            error(
                call(
                    &s,
                    &format!("apply_{family}_update"),
                    apply(&input, &proposed)
                )
                .await
            )["kind"],
            "invalid_preview"
        );
        finish(p, s, &core).await;
    }
}

#[tokio::test]
async fn inherited_unsafe_git_source_is_not_fed_into_core_cache_refresh_by_an_ordinary_edit() {
    let core = Core::start().await;
    core.data.lock().unwrap().builds.get_mut(BUILD).unwrap()["config"]["repo"] =
        json!("fixture/workload;echo private");
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    assert_eq!(
        error(
            call(
                &s,
                "preview_build_update",
                edit("build", json!({"image_tag":"next"}))
            )
            .await
        )["kind"],
        "invalid_arguments"
    );
    assert!(core.requests("write").is_empty());
    finish(p, s, &core).await;
}

#[tokio::test]
async fn explicit_reassociation_registry_order_and_version_are_saved_as_separate_partial_edits() {
    let core = Core::start().await;
    let original_secret_args = core.build()["config"]["secret_args"].clone();
    let other = "000000000000000000000050";
    {
        let mut d = core.data.lock().unwrap();
        let mut b = d.builders[BUILDER].clone();
        b["_id"]["$oid"] = json!(other);
        b["name"] = json!("other-server-builder");
        b["config"]["params"]["server_ids"] = json!([SERVER2, SERVER]);
        d.builders.insert(other.into(), b);
    }
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let registry = json!([
        {"domain":"registry.invalid:5000","account":PRIVATE[1],"organization":"second-destination"},
        {"domain":"registry.invalid:5000","account":PRIVATE[1],"organization":"first-destination"}
    ]);
    for (patch, expected) in [
        (
            json!({"builder":{"name":"other-server-builder"}}),
            json!({"builder_id":other}),
        ),
        (
            json!({"image_registry":registry}),
            json!({"image_registry":registry}),
        ),
        (
            json!({"version":"1.2.3"}),
            json!({"version":{"major":1,"minor":2,"patch":3}}),
        ),
    ] {
        let input = edit("build", patch);
        let proposed = preview(&s, "build", &input).await;
        let saved = data(call(&s, "apply_build_update", apply(&input, &proposed)).await);
        assert_eq!(saved["saved_configuration_verified"], true);
        assert_eq!(
            core.requests("write").last().unwrap().params["config"],
            expected
        );
    }
    let inspected = data(call(&s, "get_build", json!({"id":BUILD})).await);
    assert_eq!(inspected["builder_id"], other);
    assert_eq!(
        inspected["builder"]["ordered_server_ids"],
        json!([SERVER2, SERVER])
    );
    assert_eq!(inspected["definition"]["configured_version"], "1.2.3");
    assert_eq!(
        inspected["definition"]["image_registries"][0]["organization"],
        "second-destination"
    );
    assert_eq!(core.build()["config"]["secret_args"], original_secret_args);
    finish(p, s, &core).await;
}

#[tokio::test]
async fn git_cache_paths_reject_absolute_and_parent_traversal_while_host_paths_remain_supported() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    for patch in [
        json!({"build_path":"../outside"}),
        json!({"dockerfile_path":"/private/CoreConfig"}),
        json!({"build_path":"owned;command"}),
    ] {
        assert_eq!(
            error(call(&s, "preview_build_update", edit("build", patch)).await)["kind"],
            "invalid_arguments"
        );
        assert!(core.requests("write").is_empty());
    }
    // A poisoned inherited path must also block an unrelated image edit.
    core.data.lock().unwrap().builds.get_mut(BUILD).unwrap()["config"]["build_path"] =
        json!("../outside");
    assert_eq!(
        error(
            call(
                &s,
                "preview_build_update",
                edit("build", json!({"image_tag":"next"}))
            )
            .await
        )["kind"],
        "invalid_arguments"
    );
    let input = edit(
        "build",
        json!({"source":{"kind":"host"},"build_path":"/tmp/owned-workload","dockerfile_path":"Dockerfile"}),
    );
    let proposed = preview(&s, "build", &input).await;
    assert_eq!(
        data(call(&s, "apply_build_update", apply(&input, &proposed)).await)["saved_configuration_verified"],
        true
    );
    assert_eq!(core.build()["config"]["build_path"], "/tmp/owned-workload");
    finish(p, s, &core).await;
}
