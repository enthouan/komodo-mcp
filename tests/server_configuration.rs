#[path = "support/configuration.rs"]
mod fixture;
mod support;

use fixture::{Core, SERVER, WriteReply};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{Process, Session, call},
};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn args(patch: Value) -> Value {
    json!({"target":{"id":SERVER},"patch":patch})
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
async fn preview(s: &Session, input: &Value) -> Value {
    data(call(s, "preview_server_update", input.clone()).await)
}
fn creation(name: &str, enabled: bool) -> Value {
    let mut v = json!({"name":name,"config":{"address":"wss://unreachable.invalid:8120","enabled":enabled,"insecure_tls":false,"region":"owned-region"}});
    if enabled {
        v["initial_public_key"] = json!("fixture-explicit-initial-trust");
    }
    v
}
fn record_only(core: &Core) {
    let d = core.data.lock().unwrap();
    assert!(!d.bad_credentials);
    assert!(d.requests.iter().all(|r| {
        match r.path {
            "read" => [
                "GetVersion",
                "ListServers",
                "GetServer",
                "ListStacks",
                "GetStack",
            ]
            .contains(&r.operation.as_str()),
            "write" => ["CreateServer", "UpdateServer"].contains(&r.operation.as_str()),
            _ => false,
        }
    }));
}

#[tokio::test]
async fn create_inspect_and_alert_only_save_preserve_connection_credentials_and_other_settings() {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let input = creation(" New Server.Name ", false);
    let p = data(call(&s, "preview_server_create", input.clone()).await);
    assert_eq!(p["resource"]["name"], "New Server.Name");
    assert!(core.writes().is_empty());
    let r = data(call(&s, "apply_server_create", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["saved_configuration_verified"], true);
    let id = r["resource"]["id"].as_str().unwrap();
    let def = &r["definition"]["details"];
    assert_eq!(def["after"]["auto_prune"], false);
    assert_eq!(def["after"]["auto_rotate_keys"], false);
    assert_eq!(def["connectivity"]["core_reported_state"], "Disabled");
    assert_eq!(def["connectivity"]["connection_verified"], false);
    assert_eq!(
        data(call(&s, "get_server", json!({"id":id})).await)["enabled"],
        false
    );
    let before = core.data.lock().unwrap().created_servers[id]["config"].clone();
    let input = json!({"target":{"id":id},"patch":{"send_unreachable_alerts":false}});
    let p = preview(&s, &input).await;
    let r = data(call(&s, "apply_server_update", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        core.writes()[1].params,
        json!({"id":id,"config":{"send_unreachable_alerts":false}})
    );
    let after = core.data.lock().unwrap().created_servers[id]["config"].clone();
    for (k, v) in before.as_object().unwrap() {
        if k != "send_unreachable_alerts" {
            assert_eq!(&after[k], v);
        }
    }
    let p = preview(&s, &input).await;
    assert_eq!(p["outcome"], "no_op");
    assert_eq!(
        data(call(&s, "apply_server_update", apply(&input, &p)).await)["outcome"],
        "no_op"
    );
    assert_eq!(core.writes().len(), 2);
    record_only(&core);
    process.finish(s).await;
}

#[tokio::test]
async fn enabled_creation_uses_only_explicit_initial_trust_and_unreachable_is_separate_from_saved()
{
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let input = creation("unreachable-server", true);
    let p = data(call(&s, "preview_server_create", input.clone()).await);
    assert!(!p.to_string().contains("fixture-explicit-initial-trust"));
    assert_eq!(
        p["definition"]["details"]["requires_operate_to_apply"],
        true
    );
    let r = data(call(&s, "apply_server_create", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["saved_configuration_verified"], true);
    let d = &r["definition"]["details"];
    assert_eq!(d["after"]["enabled"], true);
    assert_eq!(d["after"]["public_key_configured"], true);
    assert_eq!(d["connectivity"]["core_reported_state"], "NotOk");
    assert_eq!(
        d["connectivity"]["error"]["operation"],
        "ServerConnectivity"
    );
    assert_eq!(d["connectivity"]["connection_verified"], false);
    assert!(
        d["connectivity"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("public key")
    );
    let w = &core.writes()[0];
    assert_eq!(w.params["public_key"], input["initial_public_key"]);
    assert_eq!(w.params["config"]["auto_prune"], false);
    assert_eq!(w.params["config"]["auto_rotate_keys"], false);
    record_only(&core);
    process
        .finish_excluding(s, &["fixture-explicit-initial-trust"])
        .await;
}

#[tokio::test]
async fn four_profiles_hide_and_deny_all_server_tools_and_connection_apply_checks_both_grants_before_reads()
 {
    let core = Core::start().await;
    for operate in [false, true] {
        for edit in [false, true] {
            let (process, s) = Process::start_profile(&core.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                56 + 19 * usize::from(operate)
                    + 50 * usize::from(edit)
                    + 3 * usize::from(operate && edit)
            );
            for name in [
                "preview_server_create",
                "apply_server_create",
                "preview_server_update",
                "apply_server_update",
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), edit);
                let before = core.data.lock().unwrap().requests.len();
                let r = s
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
            if edit && !operate {
                for patch in [
                    json!({"address":"ws://periphery:8120"}),
                    json!({"enabled":false}),
                    json!({"insecure_tls":false}),
                ] {
                    let input = args(patch);
                    let p = preview(&s, &input).await;
                    let before = core.data.lock().unwrap().requests.len();
                    let r = call(&s, "apply_server_update", apply(&input, &p)).await;
                    assert_eq!(r.structured_content.unwrap()["kind"], "capability_disabled");
                    assert_eq!(core.data.lock().unwrap().requests.len(), before);
                }
                let input = creation("enabled-needs-operate", true);
                let p = data(call(&s, "preview_server_create", input.clone()).await);
                let before = core.data.lock().unwrap().requests.len();
                assert_eq!(
                    call(&s, "apply_server_create", apply(&input, &p))
                        .await
                        .structured_content
                        .unwrap()["kind"],
                    "capability_disabled"
                );
                assert_eq!(core.data.lock().unwrap().requests.len(), before);
            }
            process
                .finish_excluding(s, &["fixture-explicit-initial-trust"])
                .await;
        }
    }
    assert!(core.writes().is_empty());
    record_only(&core);
}

#[tokio::test]
async fn connection_patch_preserves_pinned_trust_and_legacy_credentials_while_reporting_cached_failure()
 {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.data.lock().unwrap().server.clone();
    let input = args(json!({"address":"https://periphery.internal:8120"}));
    let p = preview(&s, &input).await;
    assert_eq!(p["changes"][0]["redacted"], true);
    assert!(!p.to_string().contains("periphery.internal"));
    let r = data(call(&s, "apply_server_update", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        r["definition"]["details"]["connectivity"]["observed_at"],
        "after_save"
    );
    assert_eq!(
        r["definition"]["details"]["connectivity"]["core_reported_state"],
        "NotOk"
    );
    assert_eq!(r["definition"]["details"]["existing_trust_preserved"], true);
    let after = core.data.lock().unwrap().server.clone();
    assert_eq!(before["info"], after["info"]);
    for (k, v) in before["config"].as_object().unwrap() {
        if k != "address" {
            assert_eq!(&after["config"][k], v);
        }
    }
    assert_eq!(
        core.writes()[0].params,
        json!({"id":SERVER,"config":{"address":"https://periphery.internal:8120"}})
    );
    record_only(&core);
    process.finish_excluding(s, &["periphery.internal"]).await;
}

#[tokio::test]
async fn omission_empty_values_false_and_raw_private_staleness_have_distinct_zero_write_paths() {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    core.data.lock().unwrap().server["config"]["ignore_mounts"] = json!(["/private"]);
    for patch in [
        json!({"region":""}),
        json!({"ignore_mounts":[]}),
        json!({"stats_monitoring":false}),
        json!({"address":""}),
    ] {
        let input = args(patch.clone());
        let p = preview(&s, &input).await;
        let expected = if p["changes"].as_array().unwrap().is_empty() {
            "no_op"
        } else {
            "saved"
        };
        assert_eq!(
            data(call(&s, "apply_server_update", apply(&input, &p)).await)["outcome"],
            expected
        );
    }
    for field in ["passkey", "public_key"] {
        let input = args(json!({"region":"changed"}));
        let p = preview(&s, &input).await;
        let count = core.writes().len();
        if field == "passkey" {
            core.data.lock().unwrap().server["config"][field] = json!("changed-hidden-passkey");
        } else {
            core.data.lock().unwrap().server["info"][field] = json!("changed-pinned-trust");
        }
        assert_eq!(
            call(&s, "apply_server_update", apply(&input, &p))
                .await
                .structured_content
                .unwrap()["kind"],
            "stale_preview"
        );
        assert_eq!(core.writes().len(), count);
    }
    // An attempted key is an observation, not accepted trust. Do not freeze it.
    let input = args(json!({"send_cpu_alerts":false}));
    let p = preview(&s, &input).await;
    core.data.lock().unwrap().server["info"]["attempted_public_key"] =
        json!("unaccepted-attempted-key");
    assert_eq!(
        data(call(&s, "apply_server_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    record_only(&core);
    process
        .finish_excluding(
            s,
            &[
                "changed-hidden-passkey",
                "changed-pinned-trust",
                "unaccepted-attempted-key",
            ],
        )
        .await;
}

#[tokio::test]
async fn invalid_inputs_and_unpinned_enabled_records_never_submit_or_invent_trust() {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.data.lock().unwrap().requests.len();
    for patch in [
        json!({"address":null}),
        json!({"address":"ftp://host"}),
        json!({"address":"https://user:private-password@host"}),
        json!({"address":"wss://host?token=private-query"}),
        json!({"address":"https://host/#fragment"}),
        json!({"address":"https://host\n"}),
        json!({"address":"https://host:0"}),
        json!({"region":"x".repeat(257)}),
        json!({"ignore_mounts":[""]}),
        json!({"enabled":null}),
        json!({"public_key":"silently-accept"}),
        json!({"passkey":"fallback"}),
        json!({"auto_prune":true}),
        json!({"auto_rotate_keys":true}),
        json!({"region":"[redacted]"}),
    ] {
        assert_eq!(
            call(&s, "preview_server_update", args(patch))
                .await
                .is_error,
            Some(true)
        );
    }
    for input in [
        json!({"name":"missing-connection-policy","config":{}}),
        creation("", false),
        creation("000000000000000000000099", false),
        json!({"name":"bad-key","config":{"address":"","enabled":false,"insecure_tls":false},"initial_public_key":""}),
    ] {
        assert_eq!(
            call(&s, "preview_server_create", input).await.is_error,
            Some(true)
        );
    }
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let mut input = creation("unpinned", true);
    input.as_object_mut().unwrap().remove("initial_public_key");
    assert_eq!(
        call(&s, "preview_server_create", input)
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_arguments"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    core.data.lock().unwrap().server["info"]["public_key"] = json!("");
    assert_eq!(
        call(
            &s,
            "preview_server_update",
            args(json!({"region":"changed"}))
        )
        .await
        .structured_content
        .unwrap()["kind"],
        "invalid_arguments"
    );
    let input = args(json!({"enabled":false}));
    let p = preview(&s, &input).await;
    assert_eq!(
        data(call(&s, "apply_server_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    assert_eq!(core.data.lock().unwrap().server["info"]["public_key"], "");
    record_only(&core);
    process
        .finish_excluding(s, &["private-password", "private-query", "silently-accept"])
        .await;
}

#[tokio::test]
async fn changed_target_input_kind_and_name_collision_consume_no_writes() {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let input = args(json!({"region":"changed"}));
    let p = preview(&s, &input).await;
    let before = core.data.lock().unwrap().requests.len();
    let mut changed = apply(&input, &p);
    changed["patch"]["region"] = json!("other");
    assert_eq!(
        call(&s, "apply_server_update", changed)
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    // Identical stable ID and JSON patch cannot reuse a Stack token for a Server.
    core.data.lock().unwrap().stack["_id"] = json!({"$oid":SERVER});
    let input = args(json!({}));
    let p = data(call(&s, "preview_stack_update", input.clone()).await);
    let before = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(&s, "apply_server_update", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let input = creation("collision", false);
    let p = data(call(&s, "preview_server_create", input.clone()).await);
    core.data.lock().unwrap().server["name"] = json!("collision");
    assert_eq!(
        call(&s, "apply_server_create", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    assert!(core.writes().is_empty());
    record_only(&core);
    process.finish(s).await;
}

#[tokio::test]
async fn a_post_save_permission_failure_retains_saved_config_and_reports_observation_gap() {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let input = args(json!({"region":"changed"}));
    let p = preview(&s, &input).await;
    core.data.lock().unwrap().post_write_read_status = Some(403);
    let r = data(call(&s, "apply_server_update", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["saved_configuration_verified"], true);
    assert!(r["observation_error"].is_null());
    assert_eq!(
        r["definition"]["details"]["connectivity"]["error"]["kind"],
        "permission_denied"
    );
    assert!(r["definition"]["details"]["connectivity"]["core_reported_state"].is_null());
    assert_eq!(core.writes().len(), 1);
    record_only(&core);
    process.finish(s).await;
}

#[tokio::test]
async fn create_and_update_failures_keep_submission_uncertain_and_never_retry_or_replay() {
    for create in [false, true] {
        for reply in [
            WriteReply::Malformed,
            WriteReply::Oversized,
            WriteReply::Denied,
            WriteReply::WriteDenied,
            WriteReply::AttachDenied,
            WriteReply::WrongTarget,
            WriteReply::WrongName,
            WriteReply::Template,
            WriteReply::MissingConfig,
            WriteReply::MissingRequested,
            WriteReply::WrongTrust,
            WriteReply::MissingTrust,
            WriteReply::Redirect,
            WriteReply::Lost,
        ] {
            if create && matches!(reply, WriteReply::WrongTarget) {
                continue;
            }
            let core = Core::start().await;
            let (process, s) = Process::start_profile(&core.address, true, true).await;
            let (input, pt, at) = if create {
                (
                    creation("uncertain", true),
                    "preview_server_create",
                    "apply_server_create",
                )
            } else {
                (
                    args(json!({"region":"changed"})),
                    "preview_server_update",
                    "apply_server_update",
                )
            };
            let p = data(call(&s, pt, input.clone()).await);
            core.data.lock().unwrap().reply = reply;
            let r = data(call(&s, at, apply(&input, &p)).await);
            assert_eq!(r["outcome"], "submission_unknown");
            assert_eq!(r["saved_configuration_verified"], false);
            assert_eq!(core.writes().len(), 1);
            if matches!(reply, WriteReply::WriteDenied) {
                assert!(
                    r["observation_error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains(if create { "creation" } else { "Write" })
                );
            }
            assert_eq!(
                call(&s, at, apply(&input, &p))
                    .await
                    .structured_content
                    .unwrap()["kind"],
                "invalid_preview"
            );
            assert_eq!(core.writes().len(), 1);
            record_only(&core);
            process
                .finish_excluding(
                    s,
                    &[
                        "fixture-explicit-initial-trust",
                        "unexpected-accepted-public-key",
                    ],
                )
                .await;
        }
    }
}

#[tokio::test]
async fn configured_credentials_are_redacted_before_metadata_bounds_and_private_partial_values_remain_full()
 {
    let core = Core::start().await;
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let region = format!("{}{}", "x".repeat(250), support::SECRET);
    core.data.lock().unwrap().server["config"]["region"] = json!(region);
    let input = args(
        json!({"region":"new-region","ignore_mounts":["/mount/".to_string()+&"x".repeat(245)]}),
    );
    let p = preview(&s, &input).await;
    assert_eq!(p["definition"]["details"]["before"]["region"], "[redacted]");
    assert_eq!(
        data(call(&s, "apply_server_update", apply(&input, &p)).await)["outcome"],
        "saved"
    );
    assert_eq!(core.writes()[0].params["config"], input["patch"]);
    record_only(&core);
    process.finish(s).await;
}

#[tokio::test]
async fn a_lost_write_deadline_is_unknown_and_consumes_the_preview() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        server_configuration as sc,
    };
    use std::time::Duration;
    let core = Core::start().await;
    let client = CoreClient::new(
        Config::parse(&core.address, support::KEY, support::SECRET)
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(false, true)),
        RequestPolicy {
            connect_timeout: Duration::from_secs(1),
            operation_timeout: Duration::from_secs(1),
        },
    )
    .unwrap();
    let input = args(json!({"region":"changed"}));
    let p = serde_json::to_value(
        sc::preview_update(&client, serde_json::from_value(input.clone()).unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    core.data.lock().unwrap().reply = WriteReply::Delayed;
    let r = serde_json::to_value(
        sc::apply_update(&client, serde_json::from_value(apply(&input, &p)).unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(r["outcome"], "submission_unknown");
    assert_eq!(r["observation_error"]["kind"], "timeout");
    assert_eq!(core.writes().len(), 1);
    assert!(
        matches!(sc::apply_update(&client,serde_json::from_value(apply(&input,&p)).unwrap()).await,Err(e) if e.kind==komodo_mcp::error::ErrorKind::InvalidPreview)
    );
    assert_eq!(core.writes().len(), 1);
    tokio::time::sleep(Duration::from_secs(3)).await;
}
