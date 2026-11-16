//! Actual production stdio against a bounded, authenticated pinned-wire fixture.
#[path = "support/variables.rs"]
mod fixture;
mod support;
use fixture::{Core, PRIVATE, Reply, UPDATE, history, variable};
use komodo_client::entities::Operation;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

fn safe(r: &CallToolResult) {
    let text = serde_json::to_string(r).unwrap();
    support::assert_safe(&text);
    for value in PRIVATE {
        assert!(
            !text.contains(value),
            "private Core/variable payload escaped"
        );
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
fn value_change(value: &str) -> Value {
    json!({"kind":"set_ordinary_value","name":"APP_MODE","value":value})
}
fn description(name: &str, text: &str) -> Value {
    json!({"kind":"set_description","name":name,"description":text})
}
async fn preview(s: &Session, change: &Value) -> Value {
    data(call(s, "preview_variable_change", json!({"change":change})).await)
}
async fn apply(s: &Session, change: &Value, p: &Value) -> Value {
    data(
        call(
            s,
            "apply_variable_change",
            json!({"change":change,"preview_id":p["preview_id"]}),
        )
        .await,
    )
}
async fn save(s: &Session, change: &Value) -> Value {
    let p = preview(s, change).await;
    apply(s, change, &p).await
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn profiles_discovery_and_direct_calls_enforce_independent_grants_before_io() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                58 + 20 * usize::from(operate)
                    + 54 * usize::from(edit)
                    + 3 * usize::from(operate && edit)
            );
            for name in ["list_variables", "get_variable", "list_core_secret_names"] {
                assert!(tools.iter().any(|t| t.name == name));
            }
            for name in ["preview_variable_change", "apply_variable_change"] {
                assert_eq!(tools.iter().any(|t| t.name == name), edit);
                if !edit {
                    let n = c.data.lock().unwrap().requests.len();
                    assert!(
                        s.call_tool(
                            CallToolRequestParams::new(name).with_arguments(
                                json!({"change":value_change("green")})
                                    .as_object()
                                    .unwrap()
                                    .clone()
                            )
                        )
                        .await
                        .is_err()
                    );
                    assert_eq!(c.data.lock().unwrap().requests.len(), n);
                }
            }
            if edit {
                let change = value_change("green");
                let review = preview(&s, &change).await;
                if !operate {
                    let n = c.data.lock().unwrap().requests.len();
                    assert_eq!(
                        error(
                            call(
                                &s,
                                "apply_variable_change",
                                json!({"change":change,"preview_id":review["preview_id"]})
                            )
                            .await
                        )["kind"],
                        "capability_disabled"
                    );
                    assert_eq!(c.data.lock().unwrap().requests.len(), n);
                } else {
                    assert_eq!(apply(&s, &change, &review).await["outcome"], "saved");
                }
                assert_eq!(
                    save(&s, &description("API_CREDENTIAL", "edited metadata")).await["outcome"],
                    "saved"
                );
            }
            assert!(
                c.data
                    .lock()
                    .unwrap()
                    .requests
                    .iter()
                    .all(|r| r.path != "execute")
            );
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn ordinary_lifecycle_preserves_other_fields_with_one_write_per_explicit_apply() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let create = json!({"kind":"create_ordinary","name":"NEW_VALUE","value":"first","description":"created"});
    let result = save(&s, &create).await;
    assert_eq!(result["outcome"], "saved");
    assert_eq!(result["writes_submitted"], 1);
    assert_eq!(c.writes()[0].operation, "CreateVariable");
    assert_eq!(c.writes()[0].params["is_secret"], false);
    let before = c.data.lock().unwrap().entries["APP_MODE"].clone();
    let result = save(&s, &value_change("green")).await;
    assert_eq!(result["saved_state_matches"], true);
    let after = c.data.lock().unwrap().entries["APP_MODE"].clone();
    for (k, v) in before.as_object().unwrap() {
        if k != "value" {
            assert_eq!(&after[k], v);
        }
    }
    assert_eq!(
        save(&s, &description("APP_MODE", "")).await["outcome"],
        "saved"
    );
    let result = save(&s, &json!({"kind":"delete_ordinary","name":"NEW_VALUE"})).await;
    assert_eq!(result["outcome"], "deleted");
    assert_eq!(result["saved_state_matches"], true);
    assert!(!c.data.lock().unwrap().entries.contains_key("NEW_VALUE"));
    assert_eq!(c.writes().len(), 4);
    assert_eq!(c.writes()[1].params.as_object().unwrap().len(), 2);
    assert_eq!(c.writes()[2].params.as_object().unwrap().len(), 2);
    assert!(
        result["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().contains("automatic"))
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn admin_secret_reads_and_description_acknowledgements_omit_values_and_auth() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let page = data(call(&s, "list_variables", json!({})).await);
    assert_eq!(page["items"][0]["is_secret"], true);
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("value").is_none())
    );
    let secret = data(call(&s, "get_variable", json!({"name":"API_CREDENTIAL"})).await);
    assert_eq!(secret["value_status"], "secret_omitted");
    assert!(secret.get("value").is_none());
    c.data
        .lock()
        .unwrap()
        .entries
        .get_mut("API_CREDENTIAL")
        .unwrap()["description"] = json!(format!("accidental {} metadata", PRIVATE[0]));
    let hidden_description = data(call(&s, "get_variable", json!({"name":"API_CREDENTIAL"})).await);
    assert_eq!(hidden_description["description"], "[omitted]");
    let result = save(&s, &description("API_CREDENTIAL", "metadata only")).await;
    assert_eq!(result["outcome"], "saved");
    assert!(result["before"].get("value").is_none());
    assert!(result["after"].get("value").is_none());
    assert_eq!(
        c.data.lock().unwrap().entries["API_CREDENTIAL"]["value"],
        PRIVATE[0]
    );
    assert_eq!(c.writes()[0].operation, "UpdateVariableDescription");
    assert!(c.writes()[0].params.get("value").is_none());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn secret_value_provisioning_deletion_reclassification_and_unknown_fields_are_rejected() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for change in [
        json!({"kind":"create_ordinary","name":"NEW_SECRET","value":PRIVATE[0],"is_secret":true}),
        json!({"kind":"set_secret","name":"APP_MODE","is_secret":false}),
        json!({"kind":"set_description","name":"API_CREDENTIAL","description":"metadata","value":PRIVATE[0]}),
    ] {
        let n = c.data.lock().unwrap().requests.len();
        error(call(&s, "preview_variable_change", json!({"change":change})).await);
        assert_eq!(c.data.lock().unwrap().requests.len(), n);
    }
    for change in [
        json!({"kind":"set_ordinary_value","name":"API_CREDENTIAL","value":"replacement"}),
        json!({"kind":"delete_ordinary","name":"API_CREDENTIAL"}),
    ] {
        assert_eq!(
            error(call(&s, "preview_variable_change", json!({"change":change})).await)["kind"],
            "invalid_arguments"
        );
    }
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn ordinary_value_classification_description_and_unknown_field_drift_stale_the_review() {
    for field in ["value", "is_secret", "description", "extra_private"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let change = value_change("green");
        let reviewed = preview(&s, &change).await;
        {
            let mut d = c.data.lock().unwrap();
            d.entries.get_mut("APP_MODE").unwrap()[field] = if field == "is_secret" {
                json!(true)
            } else {
                json!("changed privately")
            };
        }
        assert_eq!(
            error(
                call(
                    &s,
                    "apply_variable_change",
                    json!({"change":change,"preview_id":reviewed["preview_id"]})
                )
                .await
            )["kind"],
            "stale_preview"
        );
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn secret_only_changes_and_current_caller_identity_invalidate_description_reviews() {
    for caller_change in [false, true] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, false, true).await;
        let change = description("API_CREDENTIAL", "new metadata");
        let reviewed = preview(&s, &change).await;
        {
            let mut d = c.data.lock().unwrap();
            if caller_change {
                d.role["_id"]["$oid"] = json!("000000000000000000000078");
            } else {
                d.entries.get_mut("API_CREDENTIAL").unwrap()["value"] =
                    json!(format!("{}-changed", PRIVATE[0]));
            }
        }
        assert_eq!(
            error(
                call(
                    &s,
                    "apply_variable_change",
                    json!({"change":change,"preview_id":reviewed["preview_id"]})
                )
                .await
            )["kind"],
            "stale_preview"
        );
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn shared_transport_final_baseline_check_catches_drift_after_apply_preflight() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let change = value_change("green");
    let reviewed = preview(&s, &change).await;
    {
        let mut d = c.data.lock().unwrap();
        let mut v = d.entries["APP_MODE"].clone();
        v["is_secret"] = json!(true);
        v["value"] = json!(PRIVATE[0]);
        d.drift_on_list = Some((3, v));
    }
    assert_eq!(
        error(
            call(
                &s,
                "apply_variable_change",
                json!({"change":change,"preview_id":reviewed["preview_id"]})
            )
            .await
        )["kind"],
        "stale_preview"
    );
    assert_eq!(c.data.lock().unwrap().list_reads, 3);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn noops_and_missing_or_existing_names_never_submit_unnecessary_writes() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for change in [
        value_change("blue"),
        description("API_CREDENTIAL", "metadata"),
    ] {
        let result = save(&s, &change).await;
        assert_eq!(result["outcome"], "no_op");
        assert_eq!(result["writes_submitted"], 0);
        assert_eq!(result["saved_state_matches"], true);
    }
    assert_eq!(
        error(call(&s, "get_variable", json!({"name":"MISSING"})).await)["kind"],
        "not_found"
    );
    error(
        call(
            &s,
            "preview_variable_change",
            json!({"change":{"kind":"create_ordinary","name":"APP_MODE","value":"x"}}),
        )
        .await,
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn disabled_nonadmin_missing_authority_fields_and_upstream_denial_never_fallback() {
    for case in [
        "nonadmin",
        "disabled",
        "missing_admin",
        "role401",
        "role403",
        "role500",
        "list403",
    ] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            match case {
                "nonadmin" => d.role["admin"] = json!(false),
                "disabled" => d.role["enabled"] = json!(false),
                "missing_admin" => {
                    d.role.as_object_mut().unwrap().remove("admin");
                }
                "role401" => d.role_status = Some(401),
                "role403" => d.role_status = Some(403),
                "role500" => d.role_status = Some(500),
                "list403" => d.read_status = Some(403),
                _ => unreachable!(),
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        error(
            call(
                &s,
                "preview_variable_change",
                json!({"change":value_change("green")}),
            )
            .await,
        );
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let change = value_change("green");
    let reviewed = preview(&s, &change).await;
    c.data.lock().unwrap().reply = Reply::Denied;
    let result = apply(&s, &change, &reviewed).await;
    assert_eq!(result["outcome"], "rejected");
    assert_eq!(result["submission_error"]["kind"], "permission_denied");
    assert_eq!(c.writes().len(), 1);
    assert_eq!(c.data.lock().unwrap().entries["APP_MODE"]["value"], "blue");
    finish(p, s, &c).await;
}

#[tokio::test]
async fn lost_malformed_wrong_and_oversized_acknowledgements_remain_uncertain_without_replay() {
    for reply in [
        Reply::Lost,
        Reply::Malformed,
        Reply::MissingClassification,
        Reply::WrongEntry,
        Reply::Oversized,
        Reply::Redirect,
        Reply::BeforeError,
    ] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let change = value_change("green");
        let reviewed = preview(&s, &change).await;
        c.data.lock().unwrap().reply = reply;
        let result = apply(&s, &change, &reviewed).await;
        assert_eq!(result["outcome"], "submission_unknown");
        assert_eq!(result["acknowledgement_valid"], false);
        assert_eq!(
            result["saved_state_matches"],
            !matches!(reply, Reply::BeforeError)
        );
        assert_eq!(c.writes().len(), 1);
        assert_eq!(
            error(
                call(
                    &s,
                    "apply_variable_change",
                    json!({"change":change,"preview_id":reviewed["preview_id"]})
                )
                .await
            )["kind"],
            "invalid_preview"
        );
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        data(call(&s, "get_variable", json!({"name":"APP_MODE"})).await);
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn readback_denial_and_concurrent_secret_reclassification_do_not_claim_a_verified_save() {
    for denied in [false, true] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let change = value_change("green");
        let reviewed = preview(&s, &change).await;
        {
            let mut d = c.data.lock().unwrap();
            if denied {
                d.post_write_read_status = Some(403);
            } else {
                d.replace_after_write = Some(variable("APP_MODE", PRIVATE[0], true));
            }
        }
        let result = apply(&s, &change, &reviewed).await;
        assert_eq!(result["outcome"], "verification_failed");
        assert_eq!(result["acknowledgement_valid"], true);
        assert_eq!(result["saved_state_matches"], false);
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn list_pages_and_incomplete_oversized_missing_classification_or_value_fail_safely() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        for n in 0..520 {
            let name = format!("VAR_{n:04}");
            d.entries
                .insert(name.clone(), variable(&name, "ordinary", false));
        }
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let first = data(call(&s, "list_variables", json!({"limit":50})).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 50);
    assert_eq!(first["scan_truncated"], true);
    let next = data(call(&s, "list_variables", json!({"page":2,"limit":50})).await);
    assert_ne!(first["items"][0]["name"], next["items"][0]["name"]);
    assert_eq!(
        error(call(&s, "get_variable", json!({"name":"APP_MODE"})).await)["kind"],
        "incomplete_resolution"
    );
    error(
        call(
            &s,
            "preview_variable_change",
            json!({"change":value_change("green")}),
        )
        .await,
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
    for case in ["classification", "value", "duplicate", "oversized"] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            let mut v = variable("APP_MODE", "blue", false);
            match case {
                "classification" => {
                    v.as_object_mut().unwrap().remove("is_secret");
                }
                "value" => {
                    v.as_object_mut().unwrap().remove("value");
                }
                "oversized" => v["value"] = json!("x".repeat(2 * 1024 * 1024)),
                _ => {}
            };
            d.read_override = Some(if case == "duplicate" {
                json!([v, v])
            } else {
                json!([v])
            });
        }
        let (p, s) = Process::start_profile(&c.address, false, false).await;
        let result = error(call(&s, "list_variables", json!({})).await);
        assert_eq!(
            result["kind"],
            if case == "oversized" {
                "response_too_large"
            } else {
                "invalid_response"
            }
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn private_previews_are_bound_to_the_change_single_use_capacity_and_process() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let change = value_change("green");
    let reviewed = preview(&s, &change).await;
    assert_eq!(error(call(&s,"apply_variable_change",json!({"change":description("APP_MODE","other"),"preview_id":reviewed["preview_id"]})).await)["kind"],"invalid_preview");
    assert_eq!(
        error(
            call(
                &s,
                "apply_variable_change",
                json!({"change":change,"preview_id":reviewed["preview_id"]})
            )
            .await
        )["kind"],
        "invalid_preview"
    );
    let mut last = Value::Null;
    for _ in 0..16 {
        last = preview(&s, &change).await;
    }
    error(call(&s, "preview_variable_change", json!({"change":change})).await);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    assert_eq!(
        error(
            call(
                &s,
                "apply_variable_change",
                json!({"change":change,"preview_id":last["preview_id"]})
            )
            .await
        )["kind"],
        "invalid_preview"
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn external_variable_history_is_sanitized_after_restart_in_get_wait_and_export() {
    let c = Core::start().await;
    for operation in [
        Operation::CreateVariable,
        Operation::UpdateVariableValue,
        Operation::DeleteVariable,
    ] {
        c.data.lock().unwrap().update = history(operation);
        let (p, s) = Process::start_profile(&c.address, false, false).await;
        let update = data(call(&s, "get_update", json!({"id":UPDATE})).await);
        assert_eq!(update["stages"][0]["stage"], "Variable operation");
        assert!(update.get("observed_revision").is_none());
        assert!(
            update["stages"][0]["stderr"]
                .as_str()
                .unwrap()
                .contains("omitted")
        );
        data(
            call(
                &s,
                "wait_for_update",
                json!({"id":UPDATE,"timeout_seconds":1}),
            )
            .await,
        );
        data(call(&s, "list_updates", json!({})).await);
        let exported = data(
            call(
                &s,
                "export_resources_toml",
                json!({"targets":[],"user_groups":[],"include_variables":true}),
            )
            .await,
        );
        assert!(!exported.to_string().contains("ordinary-export-value"));
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn ordinary_values_use_the_documented_whole_value_projection_before_clipping() {
    for (value, status) in [
        ("blue".into(), "available"),
        ("".into(), "available"),
        ("x".repeat(1025), "too_long_omitted"),
        (
            "https://example.invalid/private?token=value".into(),
            "unsafe_omitted",
        ),
        ("password=credential".into(), "unsafe_omitted"),
        (format!("prefix{}suffix", support::SECRET), "unsafe_omitted"),
        ("line\nvalue".into(), "unsafe_omitted"),
        ("[[CORE_KEY]]".into(), "available"),
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().entries.get_mut("APP_MODE").unwrap()["value"] = json!(value);
        let (p, s) = Process::start_profile(&c.address, false, false).await;
        let result = data(call(&s, "get_variable", json!({"name":"APP_MODE"})).await);
        assert_eq!(result["value_status"], status);
        assert_eq!(result.get("value").is_some(), status == "available");
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn configured_core_key_names_are_distinct_authorized_names_only_without_host_expansion() {
    let c = Core::start().await;
    c.data.lock().unwrap().role["admin"] = json!(false);
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let names = data(call(&s, "list_core_secret_names", json!({})).await);
    assert_eq!(names["scope"], "configured_core_secret_key_names");
    assert_eq!(names["names"], json!(["CORE_KEY", "SECOND.KEY"]));
    assert!(
        c.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.operation == "ListSecrets")
            .all(|r| r.params["target"].is_null())
    );
    let n = c.data.lock().unwrap().requests.len();
    error(
        call(
            &s,
            "list_core_secret_names",
            json!({"target":{"type":"Server","id":"000000000000000000000001"}}),
        )
        .await,
    );
    assert_eq!(c.data.lock().unwrap().requests.len(), n);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
