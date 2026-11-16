//! Production stdio boundary tests; provider behavior is deterministic fault injection.
#[path = "support/alerters.rs"]
mod fixture;
mod support;
use fixture::{Core, ID, PRIVATE, Reply, UPDATE, resource, update};
use komodo_client::entities::Operation;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
fn safe(r: &CallToolResult) {
    let s = serde_json::to_string(r).unwrap();
    support::assert_safe(&s);
    for private in PRIVATE {
        assert!(!s.contains(private), "private Alerter data escaped");
    }
    assert!(!s.contains("[[ENDPOINT_REF]]"));
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
fn target() -> Value {
    json!({"id":ID})
}
async fn preview(s: &Session, patch: &Value) -> Value {
    data(
        call(
            s,
            "preview_alerter_update",
            json!({"target":target(),"patch":patch}),
        )
        .await,
    )
}
async fn apply(s: &Session, patch: &Value, p: &Value) -> CallToolResult {
    call(
        s,
        "apply_alerter_update",
        json!({"target":target(),"patch":patch,"preview_id":p["preview_id"]}),
    )
    .await
}
async fn save(s: &Session, patch: &Value) -> Value {
    let p = preview(s, patch).await;
    data(apply(s, patch, &p).await)
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    assert!(
        c.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "unexpected")
    );
    p.finish_excluding(s, &PRIVATE).await;
}
async fn test(s: &Session) -> Value {
    data(
        call(
            s,
            "test_alerter",
            json!({"target":target(),"acknowledge_delivery":true}),
        )
        .await,
    )
}
#[tokio::test]
async fn profiles_and_direct_dispatch_preserve_independent_grants_before_io() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                64 + 20 * usize::from(operate)
                    + 55 * usize::from(edit)
                    + 4 * usize::from(operate && edit)
            );
            for (name, allowed) in [
                ("list_alerters", true),
                ("get_alerter", true),
                ("test_alerter", operate),
                ("preview_alerter_create", edit),
                ("apply_alerter_create", edit),
                ("preview_alerter_update", edit),
                ("apply_alerter_update", edit),
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), allowed);
                if !allowed {
                    let n = c.data.lock().unwrap().requests.len();
                    assert!(
                        s.call_tool(
                            CallToolRequestParams::new(name)
                                .with_arguments(json!({}).as_object().unwrap().clone())
                        )
                        .await
                        .is_err()
                    );
                    assert_eq!(c.data.lock().unwrap().requests.len(), n);
                }
            }
            if edit && !operate {
                let patch = json!({"enabled":false});
                let v = preview(&s, &patch).await;
                let n = c.data.lock().unwrap().requests.len();
                assert_eq!(
                    error(apply(&s, &patch, &v).await)["kind"],
                    "capability_disabled"
                );
                assert_eq!(c.data.lock().unwrap().requests.len(), n);
            }
            assert!(c.mutations("write").is_empty());
            assert!(c.mutations("execute").is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn safe_reads_omit_endpoint_values_but_explain_unrestricted_filters_and_maintenance() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let list = data(call(&s, "list_alerters", json!({})).await);
    assert_eq!(list["items"][0]["endpoint_type"], "Ntfy");
    assert_eq!(list["items"][0]["enabled"], true);
    let get = data(call(&s, "get_alerter", json!({"name":"owned-alerter"})).await);
    assert_eq!(get["definition"]["all_resources"], true);
    assert_eq!(get["definition"]["all_alert_types"], true);
    assert_eq!(get["definition"]["ntfy_email_configured"], true);
    assert!(c.mutations("write").is_empty());
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn all_five_creation_variants_default_disabled_without_test_sends() {
    for kind in ["custom", "slack", "discord", "ntfy", "pushover"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, false, true).await;
        let config =
            json!({"endpoint":{"kind":kind,"url":format!("https://example.test/{}",PRIVATE[1])}});
        let input = json!({"name":"new-alerter","config":config});
        let v = data(call(&s, "preview_alerter_create", input).await);
        assert_eq!(v["definition"]["details"]["after"]["enabled"], false);
        let r = data(
            call(
                &s,
                "apply_alerter_create",
                json!({"name":"new-alerter","config":config,"preview_id":v["preview_id"]}),
            )
            .await,
        );
        assert_eq!(r["outcome"], "saved");
        assert_eq!(r["saved_configuration_verified"], true);
        assert_eq!(r["writes_submitted"], 1);
        assert_eq!(c.mutations("write")[0].params["config"]["enabled"], false);
        assert_eq!(c.mutations("write")[0].operation, "CreateAlerter");
        assert!(c.mutations("execute").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn enabled_creation_requires_operate_and_creation_requires_actual_enabled_admin() {
    for (operate, admin, enabled) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().role["admin"] = json!(admin);
        c.data.lock().unwrap().role["enabled"] = json!(enabled);
        let (p, s) = Process::start_profile(&c.address, operate, true).await;
        let config = json!({"enabled":true,"endpoint":{"kind":"custom","url":"[[ALERTER_URL]]"}});
        let input = json!({"name":"new-alerter","config":config});
        if !admin || !enabled {
            assert_eq!(
                error(call(&s, "preview_alerter_create", input).await)["kind"],
                "permission_denied"
            );
        } else {
            let v = data(call(&s, "preview_alerter_create", input).await);
            let n = c.data.lock().unwrap().requests.len();
            assert_eq!(
                error(
                    call(
                        &s,
                        "apply_alerter_create",
                        json!({"name":"new-alerter","config":config,"preview_id":v["preview_id"]})
                    )
                    .await
                )["kind"],
                "capability_disabled"
            );
            assert_eq!(c.data.lock().unwrap().requests.len(), n);
        }
        assert!(c.mutations("write").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn patches_preserve_private_endpoint_references_unknown_fields_and_literal_ntfy_email() {
    let c = Core::start().await;
    let before = c.data.lock().unwrap().entries[ID].clone();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r=save(&s,&json!({"alert_types":["Test"],"resources":[{"kind":"system"}],"maintenance_windows":[{"name":"owned","schedule":{"kind":"daily"},"hour":10,"minute":30,"duration_minutes":5,"timezone":"Etc/UTC","enabled":true}]})).await;
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["saved_configuration_verified"], true);
    let after = c.data.lock().unwrap().entries[ID].clone();
    assert_eq!(before["config"]["endpoint"], after["config"]["endpoint"]);
    assert_eq!(
        before["config"]["private_extension"],
        after["config"]["private_extension"]
    );
    assert!(
        c.mutations("write")[0].params["config"]
            .get("endpoint")
            .is_none()
    );
    assert!(c.mutations("execute").is_empty());
    let r = save(
        &s,
        &json!({"alert_types":[],"resources":[],"except_resources":[],"maintenance_windows":[]}),
    )
    .await;
    assert_eq!(r["definition"]["details"]["after"]["all_resources"], true);
    assert_eq!(r["definition"]["details"]["after"]["all_alert_types"], true);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn endpoint_edits_use_original_values_explicit_email_clear_and_no_default_conversion() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let old = c.data.lock().unwrap().entries[ID]["config"]["endpoint"]["params"]["url"].clone();
    assert_eq!(
        save(
            &s,
            &json!({"endpoint":{"kind":"ntfy","email":{"kind":"clear"}}})
        )
        .await["saved_configuration_verified"],
        true
    );
    assert_eq!(
        c.data.lock().unwrap().entries[ID]["config"]["endpoint"]["params"]["url"],
        old
    );
    assert!(c.data.lock().unwrap().entries[ID]["config"]["endpoint"]["params"]["email"].is_null());
    let r = save(
        &s,
        &json!({"endpoint":{"kind":"ntfy","email":{"kind":"set","value":PRIVATE[2]}}}),
    )
    .await;
    assert_eq!(r["outcome"], "saved");
    assert_eq!(
        error(
            call(
                &s,
                "preview_alerter_update",
                json!({"target":target(),"patch":{"endpoint":{"kind":"slack"}}})
            )
            .await
        )["kind"],
        "invalid_arguments"
    );
    assert_eq!(
        save(
            &s,
            &json!({"endpoint":{"kind":"slack","url":"[[ENDPOINT_REF]]"}})
        )
        .await["saved_configuration_verified"],
        true
    );
    assert_eq!(
        c.data.lock().unwrap().entries[ID]["config"]["endpoint"],
        json!({"type":"Slack","params":{"url":"[[ENDPOINT_REF]]"}})
    );
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn unknown_endpoint_extensions_are_preserved_or_refused_instead_of_dropped() {
    let c = Core::start().await;
    c.data.lock().unwrap().entries.get_mut(ID).unwrap()["config"]["endpoint"]["params"]["unknown_private"] =
        json!(PRIVATE[1]);
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    assert_eq!(
        save(&s, &json!({"enabled":false})).await["saved_configuration_verified"],
        true
    );
    assert_eq!(
        c.data.lock().unwrap().entries[ID]["config"]["endpoint"]["params"]["unknown_private"],
        PRIVATE[1]
    );
    assert_eq!(error(call(&s,"preview_alerter_update",json!({"target":target(),"patch":{"endpoint":{"kind":"ntfy","email":{"kind":"clear"}}}})).await)["kind"],"invalid_arguments");
    assert_eq!(c.mutations("write").len(), 1);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn private_secret_reference_and_identity_drift_invalidates_preview_without_mutation() {
    for field in ["url", "extension", "name", "template"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let patch = json!({"enabled":false});
        let v = preview(&s, &patch).await;
        {
            let mut d = c.data.lock().unwrap();
            let a = d.entries.get_mut(ID).unwrap();
            match field {
                "url" => {
                    a["config"]["endpoint"]["params"]["url"] = json!("[[CHANGED_SECRET_REFERENCE]]")
                }
                "extension" => a["config"]["private_extension"] = json!("changed-private"),
                "name" => a["name"] = json!("renamed"),
                _ => a["template"] = json!(true),
            }
        }
        let e = error(apply(&s, &patch, &v).await);
        assert!(matches!(
            e["kind"].as_str(),
            Some("stale_preview" | "unsupported_target")
        ));
        assert!(c.mutations("write").is_empty());
        assert!(c.mutations("execute").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn noops_are_single_use_and_input_bound_and_never_send() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let patch = json!({"enabled":true});
    let v = preview(&s, &patch).await;
    let r = data(apply(&s, &patch, &v).await);
    assert_eq!(r["outcome"], "no_op");
    assert_eq!(r["writes_submitted"], 0);
    assert_eq!(
        error(apply(&s, &patch, &v).await)["kind"],
        "invalid_preview"
    );
    let v = preview(&s, &json!({"enabled":false})).await;
    assert_eq!(
        error(apply(&s, &patch, &v).await)["kind"],
        "invalid_preview"
    );
    assert!(c.mutations("write").is_empty());
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn raw_echo_missing_fields_and_lost_acknowledgements_never_invent_verified_saves_or_retry() {
    for kind in [
        Reply::Lost,
        Reply::Malformed,
        Reply::MissingEcho,
        Reply::WrongId,
        Reply::Oversized,
        Reply::Redirect,
        Reply::Denied,
        Reply::BeforeError,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().write_reply = kind;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = save(&s, &json!({"enabled":false})).await;
        assert_eq!(r["outcome"], "submission_unknown");
        assert_eq!(r["saved_configuration_verified"], false);
        assert_eq!(c.mutations("write").len(), 1);
        assert!(c.mutations("execute").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn independent_readback_detects_preservation_failure_and_retains_acknowledgement_on_denial() {
    for deny in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if deny {
                d.post_write_read_status = Some(403);
            } else {
                let mut changed = resource("owned-alerter", "Ntfy");
                changed["config"]["enabled"] = json!(false);
                changed["config"]["private_extension"] = json!("drift");
                d.post_write_drift = Some(changed);
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = save(&s, &json!({"enabled":false})).await;
        assert_eq!(r["outcome"], "saved");
        assert_eq!(r["saved_configuration_verified"], false);
        assert!(!r["observation_error"].is_null());
        assert_eq!(c.mutations("write").len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn explicit_test_preserves_known_id_and_separates_core_report_from_provider_and_receipt() {
    for success in [false, true] {
        let c = Core::start().await;
        c.data.lock().unwrap().update = update(Operation::TestAlerter, success);
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let n = c.data.lock().unwrap().requests.len();
        assert_eq!(
            error(
                call(
                    &s,
                    "test_alerter",
                    json!({"target":target(),"acknowledge_delivery":false})
                )
                .await
            )["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.data.lock().unwrap().requests.len(), n);
        let r = test(&s).await;
        assert_eq!(r["receipt"]["submission"], "accepted");
        assert_eq!(r["receipt"]["update_id"], UPDATE);
        assert_eq!(r["receipt"]["automatic_resubmission"], false);
        let o = &r["receipt"]["latest_update"]["observed_alerter_test"];
        assert_eq!(
            o["core_report"],
            if success {
                "success_reported"
            } else {
                "failure_reported"
            }
        );
        assert_eq!(o["provider_attempt"], "not_established");
        assert_eq!(o["independently_observed_receipt"], "not_verified");
        assert_eq!(c.mutations("execute").len(), 1);
        assert!(c.mutations("write").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn disabled_and_maintenance_tests_never_enable_or_claim_delivery() {
    for enabled in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.entries.get_mut(ID).unwrap()["config"]["enabled"] = json!(enabled);
            d.update = update(Operation::TestAlerter, enabled);
        }
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = test(&s).await;
        assert_eq!(
            r["receipt"]["latest_update"]["observed_alerter_test"]["independently_observed_receipt"],
            "not_verified"
        );
        assert_eq!(
            c.data.lock().unwrap().entries[ID]["config"]["enabled"],
            enabled
        );
        assert!(c.mutations("write").is_empty());
        assert_eq!(c.mutations("execute").len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn uncertain_tests_denials_and_bad_acknowledgements_are_not_replayed() {
    for kind in [
        Reply::Lost,
        Reply::Malformed,
        Reply::WrongTarget,
        Reply::WrongId,
        Reply::Oversized,
        Reply::Redirect,
        Reply::Denied,
        Reply::BeforeError,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().test_reply = kind;
        let (p, s) = Process::start_profile(&c.address, true, false).await;
        let r = test(&s).await;
        assert_eq!(
            r["receipt"]["submission"],
            if matches!(kind, Reply::Denied) {
                "rejected"
            } else {
                "unknown"
            }
        );
        assert!(r["receipt"]["update_id"].is_null());
        assert_eq!(c.mutations("execute").len(), 1);
        assert!(c.mutations("write").is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn known_test_id_survives_denied_history_and_readonly_wait_and_recovery_do_not_send() {
    let c = Core::start().await;
    c.data.lock().unwrap().history_status = Some(403);
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let r = test(&s).await;
    assert_eq!(r["receipt"]["update_id"], UPDATE);
    assert_eq!(
        r["receipt"]["observation_error"]["kind"],
        "permission_denied"
    );
    finish(p, s, &c).await;
    c.data.lock().unwrap().history_status = None;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let wait = data(
        call(
            &s,
            "wait_for_update",
            json!({"id":UPDATE,"timeout_seconds":1}),
        )
        .await,
    );
    assert_eq!(
        wait["latest_update"]["observed_alerter_test"]["core_report"],
        "success_reported"
    );
    let r=data(call(&s,"reconcile_operation",json!({"target":{"kind":"alerter","id":ID},"requested_scope":{"kind":"alerter_test"},"operation":"test"})).await);
    assert_eq!(r["core_operation"], "TestAlerter");
    assert_eq!(r["original_submission"], "unknown");
    assert_eq!(r["attribution"], "unconfirmed");
    assert_eq!(c.mutations("execute").len(), 1);
    assert!(c.mutations("write").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn history_after_restart_omits_all_alerter_configuration_and_provider_payloads() {
    for op in [
        Operation::CreateAlerter,
        Operation::UpdateAlerter,
        Operation::RenameAlerter,
        Operation::DeleteAlerter,
        Operation::TestAlerter,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().update = update(op, false);
        for _ in 0..2 {
            let (p, s) = Process::start_profile(&c.address, false, false).await;
            let r = data(call(&s, "get_update", json!({"id":UPDATE})).await);
            assert!(r["observed_revision"].is_null());
            assert_eq!(r["stages"][0]["stage"], "Alerter operation");
            assert_eq!(r["stages"][0]["stderr_truncated"], true);
            finish(p, s, &c).await;
        }
        assert!(c.mutations("execute").is_empty());
        assert!(c.mutations("write").is_empty());
    }
}
#[tokio::test]
async fn malformed_null_unknown_templates_ambiguity_and_read_denial_never_mutate() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for patch in [
        json!({"enabled":null}),
        json!({"endpoint":null}),
        json!({"endpoint":{"kind":"send_alert","url":"x"}}),
        json!({"alert_types":["Unknown"]}),
        json!({"resources":null}),
        json!({"maintenance_windows":[{"name":"x","schedule":{"kind":"daily"},"hour":24,"minute":0,"duration_minutes":1440,"enabled":true}]}),
        json!({"maintenance_windows":[{"name":"x","schedule":{"kind":"one_time","date":"2025-02-29"},"hour":1,"minute":0,"duration_minutes":1,"enabled":true}]}),
    ] {
        let r = call(
            &s,
            "preview_alerter_update",
            json!({"target":target(),"patch":patch}),
        )
        .await;
        assert_eq!(error(r)["kind"], "invalid_arguments");
    }
    c.data.lock().unwrap().read_status = Some(403);
    assert_eq!(
        error(call(&s, "get_alerter", target()).await)["kind"],
        "permission_denied"
    );
    assert!(c.mutations("write").is_empty());
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn ambiguous_names_templates_and_oversized_private_definitions_fail_closed() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    {
        let mut d = c.data.lock().unwrap();
        let mut other = resource("owned-alerter", "Custom");
        other["_id"]["$oid"] = json!("000000000000000000000082");
        d.entries.insert("000000000000000000000082".into(), other);
    }
    assert_eq!(
        error(
            call(
                &s,
                "test_alerter",
                json!({"target":{"name":"owned-alerter"},"acknowledge_delivery":true})
            )
            .await
        )["kind"],
        "ambiguous_target"
    );
    c.data.lock().unwrap().entries.get_mut(ID).unwrap()["template"] = json!(true);
    assert_eq!(
        error(
            call(
                &s,
                "test_alerter",
                json!({"target":target(),"acknowledge_delivery":true})
            )
            .await
        )["kind"],
        "unsupported_target"
    );
    assert_eq!(
        error(
            call(
                &s,
                "preview_alerter_update",
                json!({"target":target(),"patch":{"enabled":false}})
            )
            .await
        )["kind"],
        "unsupported_target"
    );
    {
        let mut d = c.data.lock().unwrap();
        let a = d.entries.get_mut(ID).unwrap();
        a["template"] = json!(false);
        a["config"]["private_extension"] = json!("x".repeat(65536));
    }
    assert_eq!(
        error(call(&s, "get_alerter", target()).await)["kind"],
        "response_too_large"
    );
    assert!(c.mutations("write").is_empty());
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn resource_filters_revalidate_exact_references_and_nonadmin_write_needs_no_admin_fallback() {
    let c = Core::start().await;
    c.data.lock().unwrap().role["admin"] = json!(false);
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let patch = json!({"resources":[{"kind":"resource","resource_kind":"server","target":{"id":"000000000000000000000001"}}],"except_resources":[{"kind":"system"}]});
    let review = preview(&s, &patch).await;
    c.data.lock().unwrap().server["name"] = json!("renamed-server");
    assert_eq!(
        error(apply(&s, &patch, &review).await)["kind"],
        "invalid_response"
    );
    assert!(c.mutations("write").is_empty());
    c.data.lock().unwrap().server = support::inventory::Family::Server.detail();
    let r = save(&s, &patch).await;
    assert_eq!(r["saved_configuration_verified"], true);
    assert_eq!(
        c.mutations("write")[0].params["config"]["resources"],
        json!([{"type":"Server","id":"000000000000000000000001"}])
    );
    c.data.lock().unwrap().server_read_status = Some(403);
    assert_eq!(
        error(
            call(
                &s,
                "preview_alerter_update",
                json!({"target":target(),"patch":patch})
            )
            .await
        )["kind"],
        "permission_denied"
    );
    assert_eq!(c.mutations("write").len(), 1);
    assert!(
        c.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "user")
    );
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn alerter_export_remains_inspection_only_and_omits_private_endpoint_fields() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r = data(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[{"kind":"alerter","target":target()}]}),
        )
        .await,
    );
    assert_eq!(r["authoritative_apply_input"], false);
    assert!(r["toml"].as_str().unwrap().contains("owned-alerter"));
    assert!(!r["toml"].as_str().unwrap().contains("url"));
    assert!(c.mutations("write").is_empty());
    assert!(c.mutations("execute").is_empty());
    finish(p, s, &c).await;
}
