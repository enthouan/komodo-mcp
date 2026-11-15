//! Production-stdio acceptance for Core-owned schedules, without a live Core.
#[path = "support/schedules.rs"]
mod fixture;
mod support;
use fixture::{ACTION, Core, Fault, PRIVATE, PROCEDURE};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    let text = serde_json::to_string(&r).unwrap();
    support::assert_safe(&text);
    for s in PRIVATE {
        assert!(!text.contains(s), "private parent leaked");
    }
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    support::assert_safe(&serde_json::to_string(&r).unwrap());
    r.structured_content.unwrap()
}
fn target(kind: &str) -> Value {
    json!({"owner_kind":kind,"target":{"id":if kind=="action"{ACTION}else{PROCEDURE}}})
}
fn input(kind: &str, patch: Value) -> Value {
    let mut v = target(kind);
    v["patch"] = patch;
    v
}
fn apply(v: &Value, p: &Value) -> Value {
    let mut v = v.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    assert!(c.requests("execute").is_empty());
    p.finish_excluding(s, &PRIVATE).await;
}
async fn save(s: &Session, kind: &str, patch: Value) -> Value {
    let input = input(kind, patch);
    let p = data(call(s, "preview_schedule_update", input.clone()).await);
    data(call(s, "apply_schedule_update", apply(&input, &p)).await)
}
fn ts(s: &str) -> i64 {
    s.parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_millis()
}

#[tokio::test]
async fn all_profiles_direct_calls_and_generic_definition_edits_keep_schedule_effect_gates_before_io()
 {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let reads = c.requests("read").len();
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                34 + 17 * usize::from(operate)
                    + 46 * usize::from(edit)
                    + 2 * usize::from(operate && edit)
            );
            for name in [
                "list_schedules",
                "get_schedule",
                "preview_schedule_update",
                "apply_schedule_update",
            ] {
                let allowed = !name.contains("update") || edit;
                assert_eq!(tools.iter().any(|t| t.name == name), allowed);
                let r = s
                    .call_tool(
                        rmcp::model::CallToolRequestParams::new(name).with_arguments(
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
            if edit {
                for kind in ["action", "procedure"] {
                    for patch in [
                        json!({"schedule":"0 0 9 * * *"}),
                        json!({"schedule_enabled":true}),
                        json!({"schedule_timezone":"Asia/Tokyo"}),
                    ] {
                        assert_eq!(call(&s,&format!("preview_{kind}_update"),json!({"target":{"id":if kind=="action"{ACTION}else{PROCEDURE}},"patch":patch})).await.is_error,Some(true));
                    }
                    if !operate {
                        let mut v = input(kind, json!({"enabled":false}));
                        v["preview_id"] = json!("not-consumed");
                        assert_eq!(
                            error(call(&s, "apply_schedule_update", v).await)["kind"],
                            "capability_disabled"
                        );
                    }
                }
                assert_eq!(c.requests("read").len(), reads);
            }
            assert!(c.requests("write").is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn list_uses_positive_pages_and_lookahead_and_includes_disabled_and_template_owners() {
    let c = Core::start().await;
    c.data
        .lock()
        .unwrap()
        .parents
        .get_mut(&("action".into(), ACTION.into()))
        .unwrap()["template"] = json!(true);
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let a = data(call(&s, "list_schedules", json!({"page_size":1})).await);
    assert_eq!(a["next_page"], 1);
    assert_eq!(a["items"][0]["fields"]["enabled"], false);
    assert_eq!(a["items"][0]["owner"]["kind"], "action");
    assert!(a["items"][0]["next_scheduled_run"].is_null());
    assert!(a["evidence_limit"].as_str().unwrap().contains("templates"));
    let b = data(call(&s, "list_schedules", json!({"page_size":1,"page":1})).await);
    assert_eq!(b["has_more"], false);
    assert_eq!(b["items"][0]["owner"]["kind"], "procedure");
    assert_eq!(b["items"][0]["last_run_at"], 1794506400000i64);
    let detail = data(call(&s, "get_schedule", target("action")).await);
    assert_eq!(detail["template"], true);
    assert_eq!(detail["saved"]["alert"], true);
    assert!(a["items"][0]["fields"]["alert"].is_null());
    let q = c.requests("read");
    let schedules = q
        .iter()
        .filter(|q| q.operation == "ListSchedules")
        .collect::<Vec<_>>();
    assert!(
        schedules
            .iter()
            .all(|r| r.params["limit"].as_u64().unwrap() > 0 && r.params["sort_desc"] == false)
    );
    assert!(
        schedules
            .iter()
            .any(|r| r.params["page"] == 1 && r.params["limit"] == 1)
    );
    let n = c.requests("read").len();
    for v in [
        json!({"page_size":0}),
        json!({"page_size":101}),
        json!({"page":u64::MAX}),
        json!({"page":u64::MAX/2,"page_size":100}),
    ] {
        assert_eq!(call(&s, "list_schedules", v).await.is_error, Some(true));
    }
    assert_eq!(c.requests("read").len(), n);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn enable_update_disable_and_clear_preserve_full_private_parent_and_send_only_schedule_fields()
 {
    for kind in ["action", "procedure"] {
        let c = Core::start().await;
        let mut expected = c.parent(kind);
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        for (patch, wire) in [
            (json!({"enabled":true}), json!({"schedule_enabled":true})),
            (
                json!({"expression":"0 30 8 * * *","format":"cron","timezone":"America/New_York","alert":false}),
                json!({"schedule":"0 30 8 * * *","schedule_format":"Cron","schedule_timezone":"America/New_York","schedule_alert":false}),
            ),
            (json!({"enabled":false}), json!({"schedule_enabled":false})),
            (json!({"expression":""}), json!({"schedule":""})),
        ] {
            let v = input(kind, patch);
            let pr = data(call(&s, "preview_schedule_update", v.clone()).await);
            let writes = c.requests("write").len();
            assert_eq!(pr["writes_submitted"], 0);
            assert_eq!(
                pr["definition"]["details"]["requires_operate_to_apply"],
                true
            );
            let r = data(call(&s, "apply_schedule_update", apply(&v, &pr)).await);
            assert_eq!(r["saved_configuration_verified"], true);
            assert_eq!(r["writes_submitted"], 1);
            assert_eq!(c.requests("write").len(), writes + 1);
            for (k, x) in wire.as_object().unwrap() {
                expected["config"][k] = x.clone();
            }
            assert_eq!(c.parent(kind), expected);
            assert_eq!(
                c.requests("write").last().unwrap().params,
                json!({"id":if kind=="action"{ACTION}else{PROCEDURE},"config":wire})
            );
            if expected["config"]["schedule"] == "" {
                assert_eq!(
                    r["definition"]["details"]["observation"]["status"],
                    "not_listed_empty_expression"
                );
            } else {
                assert_eq!(
                    r["definition"]["details"]["observation"]["matches_expected_fields"],
                    true
                );
            }
        }
        let detail = data(call(&s, "get_schedule", target(kind)).await);
        assert_eq!(detail["saved"]["expression"], "");
        assert_eq!(
            detail["observation"]["status"],
            "not_listed_empty_expression"
        );
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn cron_and_english_use_core_clock_timezone_and_report_dst_offset_changes_for_both_owners() {
    for kind in ["action", "procedure"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        for (expr, expected) in [
            ("0 0 9 13 3 *", "2027-03-13T14:00:00Z"),
            ("0 0 9 15 3 *", "2027-03-15T13:00:00Z"),
            ("0 0 9 6 11 *", "2027-11-06T13:00:00Z"),
            ("0 0 9 8 11 *", "2027-11-08T14:00:00Z"),
        ] {
            let r=save(&s,kind,json!({"enabled":true,"expression":expr,"format":"cron","timezone":"America/New_York"})).await;
            assert_eq!(r["saved_configuration_verified"], true);
            let o = &r["definition"]["details"]["observation"];
            assert_eq!(o["matches_expected_fields"], true);
            assert_eq!(o["reported"]["next_scheduled_run"], ts(expected));
            assert!(o["reported"]["schedule_error"].is_null());
        }
        let r = save(
            &s,
            kind,
            json!({"expression":"every day at 9:00 am","format":"english","timezone":""}),
        )
        .await;
        let f = &r["definition"]["details"]["after"];
        assert_eq!(f["timezone"], "");
        assert_eq!(
            f["timezone_source"],
            "core_configured_timezone_or_core_host_local"
        );
        assert_eq!(
            r["definition"]["details"]["observation"]["reported"]["next_scheduled_run"],
            ts("2026-11-14T00:00:00Z")
        );
        // Core fixture clock and default zone are independent of the MCP clock/zone.
        let r = data(call(&s, "get_schedule", target(kind)).await);
        assert_eq!(
            r["observation"]["reported"]["next_scheduled_run"],
            ts("2026-11-14T00:00:00Z")
        );
        assert_eq!(r["saved"]["format"], "english");
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn invalid_expression_or_timezone_can_save_with_core_cache_error_and_disabled_is_not_validated()
 {
    for kind in ["action", "procedure"] {
        for patch in [
            json!({"enabled":true,"expression":"invalid cron","format":"cron"}),
            json!({"enabled":true,"expression":"not a supported English expression","format":"english"}),
            json!({"enabled":true,"timezone":"Not/AZone"}),
        ] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let r = save(&s, kind, patch).await;
            assert_eq!(r["outcome"], "saved");
            assert_eq!(r["saved_configuration_verified"], true);
            assert!(r["observation_error"].is_null());
            let report = &r["definition"]["details"]["observation"]["reported"];
            assert!(
                report["schedule_error"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            );
            assert!(report["next_scheduled_run"].is_null());
            let r = save(&s, kind, json!({"enabled":false})).await;
            let report = &r["definition"]["details"]["observation"]["reported"];
            assert!(report["schedule_error"].is_null());
            assert!(report["next_scheduled_run"].is_null());
            assert!(report["fields"]["expression"] != "");
            assert_eq!(report["fields"]["enabled"], false);
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn empty_patch_edit_only_and_identical_schedule_noops_never_write_or_execute() {
    for operate in [false, true] {
        for kind in ["action", "procedure"] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, true).await;
            let r = save(&s, kind, json!({})).await;
            assert_eq!(r["outcome"], "no_op");
            assert_eq!(r["saved_configuration_verified"], true);
            if operate {
                let r = save(&s, kind, json!({"enabled":false,"format":"cron"})).await;
                assert_eq!(r["outcome"], "no_op");
            }
            assert!(c.requests("write").is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn private_source_args_steps_and_secrets_invalidate_shared_consumed_previews() {
    for kind in ["action", "procedure"] {
        for field in if kind == "action" {
            vec!["file_contents", "arguments", "webhook_secret"]
        } else {
            vec!["stages", "webhook_secret"]
        } {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let v = input(kind, json!({"enabled":true}));
            let pr = data(call(&s, "preview_schedule_update", v.clone()).await);
            {
                let mut d = c.data.lock().unwrap();
                let key = (
                    kind.into(),
                    if kind == "action" {
                        ACTION.into()
                    } else {
                        PROCEDURE.into()
                    },
                );
                d.parents.get_mut(&key).unwrap()["config"][field] = if field == "stages" {
                    json!([])
                } else {
                    json!("changed private value\n")
                };
            }
            assert_eq!(
                error(call(&s, "apply_schedule_update", apply(&v, &pr)).await)["kind"],
                "stale_preview"
            );
            assert_eq!(
                error(call(&s, "apply_schedule_update", apply(&v, &pr)).await)["kind"],
                "invalid_preview"
            );
            assert!(c.requests("write").is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn owner_kind_input_binding_rejects_cross_family_replay_despite_same_name() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let v =
        json!({"owner_kind":"action","target":{"name":"shared-owner"},"patch":{"enabled":true}});
    let pr = data(call(&s, "preview_schedule_update", v.clone()).await);
    let mut wrong = apply(&v, &pr);
    wrong["owner_kind"] = json!("procedure");
    assert_eq!(
        call(&s, "apply_schedule_update", wrong).await.is_error,
        Some(true)
    );
    assert!(c.requests("write").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn unknown_null_oversized_and_executable_parent_fields_cannot_enter_schedule_writes() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let reads = c.requests("read").len();
    for kind in ["action", "procedure"] {
        for patch in [
            json!({"enabled":null}),
            json!({"expression":null}),
            json!({"format":"future"}),
            json!({"source":"code"}),
            json!({"stages":[]}),
            json!({"arguments":"private"}),
            json!({"run_at_startup":true}),
            json!({"expression":"x".repeat(1025)}),
            json!({"timezone":"x".repeat(129)}),
            json!({"expression":"0 0 9\n* * *"}),
        ] {
            assert_eq!(
                call(&s, "preview_schedule_update", input(kind, patch))
                    .await
                    .is_error,
                Some(true)
            );
        }
    }
    assert_eq!(c.requests("read").len(), reads);
    assert!(c.requests("write").is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn save_faults_and_private_readback_failures_never_retry_and_keep_cache_evidence_separate() {
    for kind in ["action", "procedure"] {
        for fault in [
            Fault::Denied,
            Fault::Lost,
            Fault::MissingEcho,
            Fault::ChangeUnrelated,
            Fault::ReadbackDenied,
        ] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let v = input(kind, json!({"enabled":true}));
            let pr = data(call(&s, "preview_schedule_update", v.clone()).await);
            c.data.lock().unwrap().fault = fault;
            let r = data(call(&s, "apply_schedule_update", apply(&v, &pr)).await);
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
            assert_eq!(c.requests("write").len(), 1);
            assert_eq!(
                error(call(&s, "apply_schedule_update", apply(&v, &pr)).await)["kind"],
                "invalid_preview"
            );
            assert_eq!(c.requests("write").len(), 1);
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn runtime_denial_missing_mismatch_and_scan_limits_are_explicit_without_undoing_verified_saves()
 {
    for kind in ["action", "procedure"] {
        for mode in 0..4 {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let v = input(kind, json!({"enabled":true}));
            let pr = data(call(&s, "preview_schedule_update", v.clone()).await);
            {
                let mut d = c.data.lock().unwrap();
                match mode {
                    0 => d.deny_runtime = true,
                    1 => d.hide_runtime = true,
                    2 => d.mismatch_runtime = true,
                    _ => d.fillers = 400,
                }
            }
            let r = data(call(&s, "apply_schedule_update", apply(&v, &pr)).await);
            assert_eq!(r["outcome"], "saved");
            assert_eq!(r["saved_configuration_verified"], true);
            let o = &r["definition"]["details"]["observation"];
            assert_eq!(
                o["status"],
                match mode {
                    0 => "unavailable",
                    1 => "not_found",
                    2 => "configuration_mismatch",
                    _ => "search_limit",
                }
            );
            if mode == 0 {
                assert_eq!(o["error"]["kind"], "permission_denied");
            }
            if mode == 2 {
                assert_eq!(o["matches_expected_fields"], false);
            }
            if mode == 3 {
                assert_eq!(o["pages_read"], 4);
            }
            assert_eq!(c.requests("write").len(), 1);
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn disabled_owner_can_still_report_stale_cache_without_being_claimed_stopped_or_valid() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.cache.insert(
            ("action".into(), ACTION.into()),
            Ok(ts("2026-11-13T11:00:00Z")),
        );
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r = data(call(&s, "get_schedule", target("action")).await);
    assert_eq!(r["saved"]["enabled"], false);
    assert_eq!(
        r["observation"]["reported"]["next_scheduled_run"],
        ts("2026-11-13T11:00:00Z")
    );
    assert!(
        r["observation"]["evidence_limit"]
            .as_str()
            .unwrap()
            .contains("cache")
    );
    assert!(
        r["effects"]
            .as_str()
            .unwrap()
            .contains("previously captured")
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn template_mutation_and_upstream_parent_denial_are_not_bypassed() {
    for kind in ["action", "procedure"] {
        for template in [false, true] {
            let c = Core::start().await;
            {
                let mut d = c.data.lock().unwrap();
                if template {
                    d.parents
                        .get_mut(&(
                            kind.into(),
                            if kind == "action" {
                                ACTION.into()
                            } else {
                                PROCEDURE.into()
                            },
                        ))
                        .unwrap()["template"] = json!(true);
                } else {
                    d.deny_parent = true;
                }
            }
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            assert_eq!(
                error(
                    call(
                        &s,
                        "preview_schedule_update",
                        input(kind, json!({"enabled":true}))
                    )
                    .await
                )["kind"],
                if template {
                    "unsupported_target"
                } else {
                    "permission_denied"
                }
            );
            assert!(c.requests("write").is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn large_parent_configs_and_schedule_metadata_are_bounded_and_invalid_listing_is_rejected() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        let p = d
            .parents
            .get_mut(&("action".into(), ACTION.into()))
            .unwrap();
        p["config"]["file_contents"] = json!(format!("{}\n", PRIVATE[0].repeat(5000)));
        p["config"]["arguments"] = json!(format!("{}\n", PRIVATE[1].repeat(5000)));
        p["config"]["schedule"] = json!("long-expression".repeat(1000));
        d.cache.insert(
            ("action".into(), ACTION.into()),
            Err(format!("{} key={}", "bad".repeat(2000), support::KEY)),
        );
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = data(call(&s, "get_schedule", target("action")).await);
    assert!(serde_json::to_vec(&r).unwrap().len() < 65536);
    assert_eq!(r["saved"]["expression_truncated"], true);
    assert_eq!(
        r["observation"]["reported"]["schedule_error_truncated"],
        true
    );
    let before = c.parent("action");
    let r = save(&s, "action", json!({"alert":false})).await;
    assert_eq!(r["saved_configuration_verified"], true);
    assert_eq!(
        c.parent("action")["config"]["arguments"],
        before["config"]["arguments"]
    );
    assert!(serde_json::to_vec(&r).unwrap().len() < 65536);
    for duplicate in [false, true] {
        {
            let mut d = c.data.lock().unwrap();
            d.duplicate = duplicate;
            d.conflicting_cache = !duplicate;
        }
        assert_eq!(
            error(call(&s, "list_schedules", json!({})).await)["kind"],
            "invalid_response"
        );
    }
    finish(p, s, &c).await;
}
