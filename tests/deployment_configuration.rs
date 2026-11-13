#[path = "support/configuration.rs"]
mod fixture;
mod support;

use fixture::{Core, WriteReply};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{DEPLOYMENT, Process, STACK, Session, call},
};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn args(patch: Value) -> Value {
    json!({"target":{"id":DEPLOYMENT},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut a = input.clone();
    a["preview_id"] = p["preview_id"].clone();
    a
}
async fn preview(session: &Session, input: &Value) -> Value {
    data(call(session, "preview_deployment_update", input.clone()).await)
}
fn creation(name: &str, image: Value, swarm: bool) -> Value {
    json!({"name":name,"config":{"image":image,"attachment":{"kind":if swarm{"swarm"}else{"server"},"target":{"id":if swarm{fixture::SWARM}else{fixture::SERVER}}},"network":"bridge","ports":"19090:8080","environment":"TOKEN=[[DEPLOY_SECRET_REFERENCE]]\n","command":"sleep 600"}})
}
fn image(reference: &str) -> Value {
    json!({"kind":"image","reference":reference})
}
fn no_execute(core: &Core) {
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
async fn create_inspect_and_image_only_update_preserve_ports_environment_and_target() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = creation(" New Deployment.Name ", image("busybox:1.37.0"), false);
    let p = data(call(&session, "preview_deployment_create", input.clone()).await);
    assert_eq!(p["resource"]["name"], "New_Deployment.Name");
    assert!(p["resource"]["id"].is_null());
    assert!(core.writes().is_empty());
    assert!(!p.to_string().contains("DEPLOY_SECRET_REFERENCE"));
    let saved = data(call(&session, "apply_deployment_create", apply(&input, &p)).await);
    let id = saved["resource"]["id"].as_str().unwrap();
    assert_eq!(saved["outcome"], "saved");
    assert!(saved["definition"]["details"]["runtime_observation"].is_null());
    let inspected = data(call(&session, "get_deployment", json!({"id":id})).await);
    assert_eq!(inspected["state"], "not_deployed");
    let before = core.data.lock().unwrap().created_deployments[id]["config"].clone();
    let input = json!({"target":{"id":id},"patch":{"image":image("busybox:1.36.1")}});
    let p = preview(&session, &input).await;
    assert_eq!(
        p["definition"]["details"]["requires_operate_to_apply"],
        true
    );
    let result = data(call(&session, "apply_deployment_update", apply(&input, &p)).await);
    assert_eq!(result["outcome"], "saved");
    assert_eq!(
        result["definition"]["details"]["after"]["image"]["reference"],
        "busybox:1.36.1"
    );
    assert_eq!(
        result["definition"]["details"]["runtime_observation"]["state"],
        "not_deployed"
    );
    assert_eq!(
        result["definition"]["details"]["runtime_observation"]["running_configuration_verified"],
        false
    );
    assert_eq!(
        core.writes()[1].params,
        json!({"id":id,"config":{"image":{"type":"Image","params":{"image":"busybox:1.36.1"}}}})
    );
    let after = core.data.lock().unwrap().created_deployments[id]["config"].clone();
    for (k, v) in before.as_object().unwrap() {
        if k != "image" {
            assert_eq!(&after[k], v);
        }
    }
    let nop = preview(&session, &input).await;
    assert_eq!(nop["outcome"], "no_op");
    assert_eq!(
        nop["definition"]["details"]["core_registry_check_possible"],
        false
    );
    assert_eq!(
        data(call(&session, "apply_deployment_update", apply(&input, &nop)).await)["outcome"],
        "no_op"
    );
    assert_eq!(core.writes().len(), 2);
    no_execute(&core);
    process
        .finish_excluding(session, &["DEPLOY_SECRET_REFERENCE"])
        .await;
}

#[tokio::test]
async fn direct_and_build_images_cover_both_targets_with_explicit_latest_and_pinned_versions() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    for swarm in [false, true] {
        for (index, image) in [
            image("registry.example:5000/team/app:v2"),
            json!({"kind":"build","target":{"name":"fixture-build"},"version":"0.0.0"}),
            json!({"kind":"build","target":{"id":fixture::BUILD},"version":"1.2.3"}),
        ]
        .into_iter()
        .enumerate()
        {
            let input = creation(&format!("matrix_{swarm}_{index}"), image.clone(), swarm);
            let p = data(call(&session, "preview_deployment_create", input.clone()).await);
            let saved = data(call(&session, "apply_deployment_create", apply(&input, &p)).await);
            assert_eq!(saved["outcome"], "saved");
            assert_eq!(
                saved["definition"]["details"]["after"]["image"]["kind"],
                image["kind"]
            );
            assert_eq!(
                saved["definition"]["details"]["after"]["attachment"]["kind"],
                if swarm { "swarm" } else { "server" }
            );
            if image["kind"] == "build" {
                assert_eq!(
                    core.writes().last().unwrap().params["config"]["image"]["params"]["build_id"],
                    fixture::BUILD
                );
                assert_eq!(
                    saved["definition"]["details"]["after"]["image"]["latest_build_version_at_deploy"],
                    index == 1
                );
            }
            data(
                call(
                    &session,
                    "get_deployment",
                    json!({"id":saved["resource"]["id"]}),
                )
                .await,
            );
        }
    }
    no_execute(&core);
    process
        .finish_excluding(session, &["DEPLOY_SECRET_REFERENCE"])
        .await;
}

#[tokio::test]
async fn saved_image_and_target_change_do_not_claim_running_configuration_or_old_target_cleanup() {
    let core = Core::start().await;
    core.data.lock().unwrap().deployment["config"]["auto_update"] = json!(true);
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = args(
        json!({"image":image("busybox:1.35.0"),"attachment":{"kind":"swarm","target":{"id":fixture::SWARM}}}),
    );
    let p = preview(&session, &input).await;
    let d = &p["definition"]["details"];
    assert_eq!(d["core_auto_deploy_possible"], true);
    assert_eq!(d["target_changed"], true);
    assert_eq!(
        d["runtime_observation"]["core_reported_image"],
        "busybox:1.36.1"
    );
    assert_eq!(
        d["runtime_observation"]["image_may_be_desired_fallback"],
        true
    );
    assert_eq!(
        d["old_target_cleanup"],
        "not_performed_by_inspected_core_2_3_2_update_path"
    );
    let saved = data(call(&session, "apply_deployment_update", apply(&input, &p)).await);
    assert_eq!(
        saved["definition"]["details"]["after"]["image"]["reference"],
        "busybox:1.35.0"
    );
    assert_eq!(
        saved["definition"]["details"]["runtime_observation"],
        d["runtime_observation"]
    );
    assert_eq!(core.writes()[0].params["config"]["server_id"], "");
    assert_eq!(
        core.writes()[0].params["config"]["swarm_id"],
        fixture::SWARM
    );
    assert!(
        !core.writes()[0].params["config"]
            .as_object()
            .unwrap()
            .contains_key("custom_name")
    );
    no_execute(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn privileged_settings_are_redacted_and_supported_clears_remain_distinct_from_omission() {
    let core = Core::start().await;
    let original_ports = core.data.lock().unwrap().deployment["config"]["ports"].clone();
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(
        json!({"environment":"TOKEN=private-new-environment\n","command":"echo private-command","volumes":"/private-mount:/data","labels":"token=private-label","extra_args":["--env=TOKEN=private-extra-arg"],"image_registry_account":"private-new-registry-account"}),
    );
    let p = preview(&session, &input).await;
    for secret in [
        "private-new-environment",
        "private-command",
        "private-mount",
        "private-label",
        "private-extra-arg",
        "private-new-registry-account",
        "private-registry-account-reference",
        "deployment-private-environment",
    ] {
        assert!(!p.to_string().contains(secret));
    }
    assert!(
        p["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["redacted"] == true)
    );
    let saved = data(call(&session, "apply_deployment_update", apply(&input, &p)).await);
    assert_eq!(saved["outcome"], "saved");
    let input = args(
        json!({"environment":"","command":"","volumes":"","labels":"","extra_args":[],"image_registry_account":""}),
    );
    let p = preview(&session, &input).await;
    let saved = data(call(&session, "apply_deployment_update", apply(&input, &p)).await);
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(core.writes()[1].params["config"], input["patch"]);
    assert_eq!(
        core.data.lock().unwrap().deployment["config"]["ports"],
        original_ports
    );
    no_execute(&core);
    process
        .finish_excluding(
            session,
            &[
                "private-new-environment",
                "private-command",
                "private-mount",
                "private-label",
                "private-extra-arg",
                "private-new-registry-account",
                "private-registry-account-reference",
                "deployment-private-environment",
            ],
        )
        .await;
}

#[tokio::test]
async fn four_profiles_and_racing_auto_update_cannot_bypass_trusted_configuration_or_operate() {
    let core = Core::start().await;
    for operate in [false, true] {
        for edit in [false, true] {
            let (process, session) = Process::start_profile(&core.address, operate, edit).await;
            let tools = session.list_tools(None).await.unwrap().tools;
            for tool in [
                "preview_deployment_create",
                "apply_deployment_create",
                "preview_deployment_update",
                "apply_deployment_update",
            ] {
                assert_eq!(tools.iter().any(|t| t.name == tool), edit);
                let count = core.data.lock().unwrap().requests.len();
                let r = session
                    .call_tool(rmcp::model::CallToolRequestParams::new(tool))
                    .await;
                if edit {
                    assert_eq!(r.unwrap().is_error, Some(true));
                } else {
                    assert!(r.is_err());
                }
                assert_eq!(core.data.lock().unwrap().requests.len(), count);
            }
            if edit && !operate {
                for policy in [false, true] {
                    for source in [
                        image("busybox:1.36.1"),
                        json!({"kind":"build","target":{"id":fixture::BUILD},"version":"0.0.0"}),
                    ] {
                        core.data.lock().unwrap().deployment["config"]["auto_update"] =
                            json!(policy);
                        let input = args(json!({"image":source}));
                        let p = preview(&session, &input).await;
                        assert_eq!(
                            p["definition"]["details"]["requires_operate_to_apply"],
                            true
                        );
                        core.data.lock().unwrap().deployment["config"]["auto_update"] = json!(true);
                        let count = core.data.lock().unwrap().requests.len();
                        let r = call(&session, "apply_deployment_update", apply(&input, &p)).await;
                        assert_eq!(r.structured_content.unwrap()["kind"], "capability_disabled");
                        assert_eq!(core.data.lock().unwrap().requests.len(), count);
                    }
                }
                let input = creation("config_only_create", image("busybox:1.37.0"), false);
                let p = data(call(&session, "preview_deployment_create", input.clone()).await);
                assert_eq!(
                    data(call(&session, "apply_deployment_create", apply(&input, &p)).await)["outcome"],
                    "saved"
                );
            }
            process
                .finish_excluding(session, &["DEPLOY_SECRET_REFERENCE"])
                .await;
        }
    }
    assert_eq!(core.writes().len(), 1);
    no_execute(&core);
}

#[tokio::test]
async fn changed_raw_baselines_references_payloads_and_name_races_invalidate_before_submission() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = args(json!({"image":image("busybox:1.36.1")}));
    let p = preview(&session, &input).await;
    let changed = args(json!({"image":image("busybox:1.35.0")}));
    let count = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(&session, "apply_deployment_update", apply(&changed, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), count);
    let p = preview(&session, &input).await;
    core.data.lock().unwrap().deployment["config"]["environment"] =
        json!("TOKEN=changed-private-baseline\n");
    assert_eq!(
        call(&session, "apply_deployment_update", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    for (index, patch) in [
        json!({"attachment":{"kind":"server","target":{"id":fixture::SERVER}}}),
        json!({"image":{"kind":"build","target":{"id":fixture::BUILD},"version":"0.0.0"}}),
    ]
    .into_iter()
    .enumerate()
    {
        let input = args(patch);
        let p = preview(&session, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            d.server["config"]["address"] =
                json!(format!("https://changed-private-server-{index}.invalid"));
            d.build["config"]["build_args"] =
                json!(format!("PRIVATE=changed-private-build-{index}\n"));
        }
        assert_eq!(
            call(&session, "apply_deployment_update", apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "stale_preview"
        );
    }
    let input = creation("name_race", image("busybox:1.37.0"), false);
    let p = data(call(&session, "preview_deployment_create", input.clone()).await);
    core.data.lock().unwrap().deployment["name"] = json!("name_race");
    assert_eq!(
        call(&session, "apply_deployment_create", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    assert!(core.writes().is_empty());
    no_execute(&core);
    process
        .finish_excluding(
            session,
            &[
                "changed-private-baseline",
                "changed-private-server",
                "changed-private-build",
                "DEPLOY_SECRET_REFERENCE",
            ],
        )
        .await;
}

#[tokio::test]
async fn shared_tokens_cannot_cross_resource_types_even_with_identical_id_and_patch() {
    let core = Core::start().await;
    core.data.lock().unwrap().deployment["_id"] = json!({"$oid":STACK});
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"target":{"id":STACK},"patch":{"environment":""}});
    for (preview_tool, apply_tool) in [
        ("preview_stack_update", "apply_deployment_update"),
        ("preview_deployment_update", "apply_stack_update"),
    ] {
        let p = data(call(&session, preview_tool, input.clone()).await);
        let count = core.data.lock().unwrap().requests.len();
        assert_eq!(
            call(&session, apply_tool, apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "invalid_preview"
        );
        assert_eq!(core.data.lock().unwrap().requests.len(), count);
    }
    assert!(core.writes().is_empty());
    no_execute(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn mutation_failures_preserve_uncertainty_without_retry_or_replay_for_create_and_update() {
    for create in [false, true] {
        for reply in [
            WriteReply::Malformed,
            WriteReply::Oversized,
            WriteReply::Lost,
            WriteReply::Denied,
            WriteReply::WriteDenied,
            WriteReply::AttachDenied,
            WriteReply::Redirect,
            WriteReply::WrongName,
            WriteReply::Template,
            WriteReply::MissingConfig,
            WriteReply::MissingRequested,
            WriteReply::WrongTarget,
        ] {
            if create && matches!(reply, WriteReply::WrongTarget) {
                continue;
            }
            let core = Core::start().await;
            let (process, session) = Process::start_profile(&core.address, true, true).await;
            let (input, preview_tool, apply_tool) = if create {
                (
                    creation("failure_fixture", image("busybox:1.37.0"), false),
                    "preview_deployment_create",
                    "apply_deployment_create",
                )
            } else {
                (
                    args(json!({"image":image("busybox:1.36.1")})),
                    "preview_deployment_update",
                    "apply_deployment_update",
                )
            };
            let p = data(call(&session, preview_tool, input.clone()).await);
            core.data.lock().unwrap().reply = reply;
            let result = data(call(&session, apply_tool, apply(&input, &p)).await);
            assert_eq!(result["outcome"], "submission_unknown");
            assert_eq!(result["saved_configuration_verified"], false);
            assert_eq!(core.writes().len(), 1);
            assert_eq!(
                call(&session, apply_tool, apply(&input, &p))
                    .await
                    .structured_content
                    .unwrap()["kind"],
                "invalid_preview"
            );
            assert_eq!(core.writes().len(), 1);
            no_execute(&core);
            process
                .finish_excluding(session, &["DEPLOY_SECRET_REFERENCE"])
                .await;
        }
    }
}

#[tokio::test]
async fn legacy_secret_bearing_images_and_invalid_inputs_stay_sanitized_and_never_write() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    core.data.lock().unwrap().deployment["config"]["image"] = json!({"type":"Image","params":{"image":"https://private-user:private-image-token@registry.invalid/app"}});
    let p = preview(&session, &args(json!({"image":image("busybox:1.36.1")}))).await;
    assert!(!p.to_string().contains("private-image-token"));
    assert_eq!(
        p["definition"]["details"]["before"]["image"]["reference"],
        "[redacted]"
    );
    let count = core.data.lock().unwrap().requests.len();
    for patch in [
        json!({"image":null}),
        json!({"image":image("")}),
        json!({"image":image("@sha256:abc")}),
        json!({"image":image("busybox;touch-owned")}),
        json!({"image":image("https://secret:token@registry.invalid/app")}),
        json!({"image":{"kind":"build","target":{"id":fixture::BUILD},"version":"latest"}}),
        json!({"image":{"kind":"build","target":{"id":fixture::BUILD},"version":"-1.0.0"}}),
        json!({"network":""}),
        json!({"replicas":-1}),
        json!({"custom_name":"runtime-rename"}),
        json!({"environment":"[redacted]"}),
        json!({"extra_args":null}),
        json!({"extra_args":[""]}),
        json!({"environment":"x".repeat(17000)}),
    ] {
        assert_eq!(
            call(&session, "preview_deployment_update", args(patch))
                .await
                .is_error,
            Some(true)
        );
    }
    assert_eq!(core.data.lock().unwrap().requests.len(), count);
    assert!(core.writes().is_empty());
    no_execute(&core);
    process
        .finish_excluding(
            session,
            &[
                "private-image-token",
                "secret:token",
                "touch-owned",
                "runtime-rename",
            ],
        )
        .await;
}

#[tokio::test]
async fn visible_change_bounds_do_not_truncate_the_authoritative_partial_write() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let ports = (10000..10040)
        .map(|p| format!("{p}:{p}"))
        .collect::<Vec<_>>()
        .join("\n");
    let input = args(json!({"ports":ports}));
    let p = preview(&session, &input).await;
    let field = p["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["field"] == "ports")
        .unwrap();
    assert_eq!(field["after_truncated"], true);
    assert!(field["after"].as_str().unwrap().chars().count() <= 256);
    assert!(core.writes().is_empty());
    assert_eq!(
        data(call(&session, "apply_deployment_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    let writes = core.writes();
    let write = &writes[0].params["config"];
    assert_eq!(write.as_object().unwrap().len(), 1);
    assert!(write["ports"].as_str().unwrap().len() > 256);
    assert_eq!(
        core.data.lock().unwrap().deployment["config"]["ports"],
        write["ports"]
    );
    no_execute(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn a_lost_deadline_consumes_the_preview_and_preserves_unknown_submission() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        deployment_configuration as dc,
    };
    use std::time::Duration;
    let core = Core::start().await;
    let client = CoreClient::new(
        Config::parse(&core.address, support::KEY, support::SECRET)
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(true, true)),
        RequestPolicy {
            connect_timeout: Duration::from_millis(100),
            operation_timeout: Duration::from_millis(100),
        },
    )
    .unwrap();
    let input = args(json!({"image":image("busybox:1.36.1")}));
    let p = dc::preview_update(&client, serde_json::from_value(input.clone()).unwrap())
        .await
        .unwrap();
    let p = serde_json::to_value(p).unwrap();
    core.data.lock().unwrap().reply = WriteReply::Delayed;
    let saved = dc::apply_update(&client, serde_json::from_value(apply(&input, &p)).unwrap())
        .await
        .unwrap();
    let saved = serde_json::to_value(saved).unwrap();
    assert_eq!(saved["outcome"], "submission_unknown");
    assert_eq!(saved["observation_error"]["kind"], "timeout");
    assert_eq!(core.writes().len(), 1);
    let replay =
        dc::apply_update(&client, serde_json::from_value(apply(&input, &p)).unwrap()).await;
    assert!(matches!(replay,Err(e)if e.kind==komodo_mcp::error::ErrorKind::InvalidPreview));
    assert_eq!(core.writes().len(), 1);
    tokio::time::sleep(Duration::from_millis(350)).await;
}

#[tokio::test]
async fn build_redeploy_policy_and_pinned_digest_effects_are_disclosed_without_execution() {
    let core = Core::start().await;
    let (process, session) = Process::start_profile(&core.address, true, true).await;
    {
        let mut d = core.data.lock().unwrap();
        d.deployment["config"]["auto_update"] = json!(true);
        d.deployment["config"]["redeploy_on_build"] = json!(true);
    }
    let input =
        args(json!({"image":{"kind":"build","target":{"id":fixture::BUILD},"version":"0.0.0"}}));
    let p = preview(&session, &input).await;
    assert_eq!(
        p["definition"]["details"]["core_auto_deploy_possible"],
        true
    );
    assert_eq!(
        p["definition"]["details"]["core_registry_check_possible"],
        false
    );
    let input = args(json!({"image":image(&format!("busybox@sha256:{}","a".repeat(64)))}));
    let p = preview(&session, &input).await;
    assert_eq!(
        p["definition"]["details"]["core_auto_deploy_possible"],
        false
    );
    assert_eq!(
        p["definition"]["details"]["core_registry_check_possible"],
        false
    );
    assert_eq!(
        p["definition"]["details"]["requires_operate_to_apply"],
        true
    );
    assert!(core.writes().is_empty());
    no_execute(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn configured_credentials_are_removed_before_display_bounds_clip_metadata() {
    let core = Core::start().await;
    {
        let mut d = core.data.lock().unwrap();
        d.deployment["name"] = json!(format!("{}{}-tail", "n".repeat(250), support::SECRET));
        d.deployment["config"]["network"] =
            json!(format!("{}{}-tail", "n".repeat(250), support::KEY));
        d.deployment["config"]["ports"] = json!(format!("{}{}:80", "p".repeat(250), support::KEY));
        d.server["name"] = json!(format!("{}{}-tail", "s".repeat(250), support::SECRET));
    }
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(
        json!({"ports":"8000:80","attachment":{"kind":"server","target":{"id":fixture::SERVER}}}),
    );
    let p = preview(&session, &input).await;
    let encoded = p.to_string();
    assert!(!encoded.contains(&support::KEY[..13]));
    assert!(!encoded.contains(&support::SECRET[..16]));
    assert_eq!(p["resource"]["name"], "[redacted]");
    assert_eq!(
        p["definition"]["details"]["before"]["network"],
        "[redacted]"
    );
    assert_eq!(
        p["definition"]["details"]["resolved_references"][0]["name"],
        "[redacted]"
    );
    assert!(core.writes().is_empty());
    no_execute(&core);
    process.finish(session).await;
}

#[tokio::test]
async fn sdk_defaults_cannot_verify_a_field_missing_from_the_mutation_reply() {
    let core = Core::start().await;
    core.data.lock().unwrap().deployment["config"]["send_alerts"] = json!(false);
    let (process, session) = Process::start_profile(&core.address, false, true).await;
    let input = args(json!({"send_alerts":true}));
    let p = preview(&session, &input).await;
    core.data.lock().unwrap().reply = WriteReply::MissingRequested;
    let saved = data(call(&session, "apply_deployment_update", apply(&input, &p)).await);
    assert_eq!(saved["outcome"], "submission_unknown");
    assert_eq!(saved["saved_configuration_verified"], false);
    assert_eq!(saved["observation_error"]["kind"], "invalid_response");
    assert_eq!(core.writes().len(), 1);
    no_execute(&core);
    process.finish(session).await;
}
