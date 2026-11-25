//! Production stdio contracts and deterministic faults; actual Core roles are exercised in #112.
#[path = "support/access.rs"]
mod fixture;
mod support;
use fixture::{ACTOR, Core, GROUP, OTHER, PRIVATE, Reply, SERVER, USER, group, permission, user};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
fn safe(r: &CallToolResult) {
    let s = serde_json::to_string(r).unwrap();
    support::assert_safe(&s);
    for v in PRIVATE {
        assert!(!s.contains(v), "private administration payload escaped");
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
fn target(id: &str) -> Value {
    json!({"id":id})
}
fn subject(id: &str) -> Value {
    json!({"kind":"user","target":target(id)})
}
fn group_subject() -> Value {
    json!({"kind":"group","target":target(GROUP)})
}
fn base(id: &str, enabled: bool) -> Value {
    json!({"kind":"set_user_base","target":target(id),"enabled":enabled})
}
fn member(id: &str, present: bool) -> Value {
    json!({"kind":"set_group_member","group":target(GROUP),"user":target(id),"present":present})
}
fn grant(s: Value, p: Value) -> Value {
    json!({"kind":"change_permission","subject":s,"scope":{"kind":"resource","resource_kind":"server","target":target(SERVER)},"patch":p})
}
async fn preview(s: &Session, c: &Value) -> Value {
    data(call(s, "preview_access_change", json!({"change":c})).await)
}
async fn apply(s: &Session, c: &Value, p: &Value) -> CallToolResult {
    call(
        s,
        "apply_access_change",
        json!({"change":c,"preview_id":p["preview_id"]}),
    )
    .await
}
async fn save(s: &Session, c: &Value) -> Value {
    let p = preview(s, c).await;
    data(apply(s, c, &p).await)
}
async fn finish(p: Process, s: Session, c: &Core) {
    {
        let d = c.data.lock().unwrap();
        assert!(!d.bad_credentials);
        assert!(d.requests.iter().all(|r| r.path != "unexpected"));
    }
    p.finish_excluding(s, &PRIVATE).await;
}
#[tokio::test]
async fn profiles_discovery_and_direct_dispatch_guard_both_grants_before_io() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            assert_eq!(
                tools.len(),
                66 + 20 * usize::from(operate)
                    + 55 * usize::from(edit)
                    + 4 * usize::from(operate && edit)
            );
            for (name, allowed) in [
                ("list_users", true),
                ("get_user", true),
                ("list_user_groups", true),
                ("get_user_group", true),
                ("list_direct_permissions", true),
                ("get_effective_permission", true),
                ("preview_access_change", edit),
                ("apply_access_change", edit && operate),
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
            if edit {
                let r = preview(&s, &base(USER, false)).await;
                assert_eq!(r["writes_submitted"], 0);
            }
            assert!(c.writes().is_empty());
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn admin_inspection_omits_authentication_and_labels_direct_records_separately() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let a = data(call(&s, "list_users", json!({"kind":"service"})).await);
    assert_eq!(a["items"].as_array().unwrap().len(), 1);
    assert_eq!(a["items"][0]["service_user"], true);
    assert_eq!(a["items"][0]["authentication_details"], "omitted");
    let u = data(call(&s, "get_user", target(OTHER)).await);
    assert_eq!(u["username"], "ordinary-user");
    assert!(u.get("config").is_none());
    let g = data(call(&s, "get_user_group", target(GROUP)).await);
    assert_eq!(g["member_count"], 2);
    assert_eq!(g["everyone"], false);
    let d = data(
        call(
            &s,
            "list_direct_permissions",
            json!({"subject":{"kind":"selected","subject":subject(USER)}}),
        )
        .await,
    );
    assert_eq!(d["records"][0]["grant"]["level"], "read");
    assert!(d["evidence"].as_str().unwrap().contains("Not an effective"));
    let own = data(
        call(
            &s,
            "list_direct_permissions",
            json!({"subject":{"kind":"current_caller"}}),
        )
        .await,
    );
    assert!(own["direct_type_grants"].as_array().unwrap().is_empty());
    assert_eq!(own["subject"]["id"], ACTOR);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn service_user_create_description_and_clear_issue_no_credentials_or_human_accounts() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(
        &s,
        &json!({"kind":"create_service_user","username":"new-owned-service","description":"owned"}),
    )
    .await;
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["acknowledgement_valid"], true);
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(r["after"]["details"]["admin"], false);
    assert_eq!(r["after"]["details"]["enabled"], true);
    let r = save(
        &s,
        &json!({"kind":"set_service_user_description","target":target(USER),"description":""}),
    )
    .await;
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(
        c.data.lock().unwrap().users[USER]["config"]["data"]["description"],
        ""
    );
    assert_eq!(c.writes().len(), 2);
    assert_eq!(c.writes()[1].params["username"], "owned-service");
    assert!(
        c.writes()
            .iter()
            .all(|r| !r.operation.contains("ApiKey") && !r.operation.contains("LocalUser"))
    );
    assert_eq!(error(call(&s,"preview_access_change",json!({"change":{"kind":"set_service_user_description","target":target(OTHER),"description":"x"}})).await)["kind"],"unsupported_target");
    finish(p, s, &c).await;
}
#[tokio::test]
async fn base_permission_changes_preserve_authentication_typewide_grants_and_other_flags() {
    let c = Core::start().await;
    {
        c.data.lock().unwrap().users.get_mut(USER).unwrap()["all"] =
            json!({"Build":{"level":"Execute","specific":["Attach"]}});
    }
    let before = c.data.lock().unwrap().users[USER].clone();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(
        &s,
        &json!({"kind":"set_user_base","target":target(USER),"create_servers":true}),
    )
    .await;
    assert_eq!(r["saved_state_matches"], true);
    let after = c.data.lock().unwrap().users[USER].clone();
    for (k, v) in before.as_object().unwrap() {
        if k != "create_server_permissions" {
            assert_eq!(&after[k], v);
        }
    }
    assert_eq!(
        save(&s, &base(USER, false)).await["saved_state_matches"],
        true
    );
    assert_eq!(
        save(&s, &base(USER, true)).await["saved_state_matches"],
        true
    );
    assert_eq!(c.writes().len(), 3);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn ordinary_admin_and_super_admin_rules_and_disabled_targets_remain_distinct() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let change = json!({"kind":"set_user_admin","target":target(USER),"admin":true});
    assert_eq!(
        error(call(&s, "preview_access_change", json!({"change":change})).await)["kind"],
        "permission_denied"
    );
    assert!(c.writes().is_empty());
    c.data.lock().unwrap().role["super_admin"] = json!(true);
    assert_eq!(save(&s, &change).await["saved_state_matches"], true);
    let change = json!({"kind":"set_user_admin","target":target(USER),"admin":false});
    assert_eq!(save(&s, &change).await["saved_state_matches"], true);
    c.data.lock().unwrap().users.get_mut(USER).unwrap()["enabled"] = json!(false);
    assert_eq!(
        error(call(&s, "preview_access_change", json!({"change":change})).await)["kind"],
        "permission_denied"
    );
    assert_eq!(c.writes().len(), 2);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn self_disable_self_delete_admin_revocation_and_super_admin_targets_are_refused() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for change in [
        base(ACTOR, false),
        json!({"kind":"delete_user","target":target(ACTOR)}),
        json!({"kind":"set_user_admin","target":target(ACTOR),"admin":false}),
    ] {
        assert_eq!(
            error(call(&s, "preview_access_change", json!({"change":change})).await)["kind"],
            "permission_denied"
        );
    }
    {
        let mut d = c.data.lock().unwrap();
        d.role["super_admin"] = json!(true);
        d.users.get_mut(USER).unwrap()["super_admin"] = json!(true);
    }
    for change in [
        base(USER, false),
        json!({"kind":"delete_user","target":target(USER)}),
        json!({"kind":"set_user_admin","target":target(USER),"admin":true}),
    ] {
        assert_eq!(
            error(call(&s, "preview_access_change", json!({"change":change})).await)["kind"],
            "permission_denied"
        );
    }
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn group_create_rename_and_additive_membership_preserve_unrelated_members_and_grants() {
    let c = Core::start().await;
    let before = c.data.lock().unwrap().groups[GROUP].clone();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(&s, &json!({"kind":"create_group","name":"New Owned Group"})).await;
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(r["after"]["details"]["name"], "New Owned Group");
    assert_eq!(r["after"]["details"]["everyone"], false);
    assert_eq!(
        save(
            &s,
            &json!({"kind":"rename_group","target":target(GROUP),"name":"Renamed Group"})
        )
        .await["saved_state_matches"],
        true
    );
    assert_eq!(
        save(&s, &member(USER, false)).await["saved_state_matches"],
        true
    );
    let g = c.data.lock().unwrap().groups[GROUP].clone();
    assert_eq!(g["users"], json!([OTHER]));
    assert_eq!(g["all"], before["all"]);
    assert_eq!(g["private_extension"], before["private_extension"]);
    assert_eq!(
        save(&s, &member(USER, true)).await["saved_state_matches"],
        true
    );
    assert_eq!(
        c.data.lock().unwrap().groups[GROUP]["users"],
        json!([OTHER, USER])
    );
    assert_eq!(c.writes().len(), 4);
    assert!(
        c.writes()
            .iter()
            .all(|r| r.operation != "SetUsersInUserGroup")
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn everyone_and_typewide_grants_are_explicit_broad_scopes_with_preserved_flags() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(
        &s,
        &json!({"kind":"set_group_everyone","target":target(GROUP),"everyone":true}),
    )
    .await;
    assert_eq!(r["broad_scope"], true);
    assert_eq!(r["saved_state_matches"], true);
    let change = json!({"kind":"change_permission","subject":group_subject(),"scope":{"kind":"resource_type","resource_kind":"server"},"patch":{"level":"execute","add_specific":["terminal"],"remove_specific":["logs"]}});
    let r = save(&s, &change).await;
    assert_eq!(r["broad_scope"], true);
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(
        c.data.lock().unwrap().groups[GROUP]["all"]["Server"],
        json!({"level":"Execute","specific":["Terminal","Inspect"]})
    );
    assert_eq!(
        c.data.lock().unwrap().groups[GROUP]["users"],
        json!([USER, OTHER])
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn scoped_grants_merge_specifics_and_revocation_leaves_unrelated_flags_and_records() {
    let c = Core::start().await;
    let group_record = c.data.lock().unwrap().permissions[1].clone();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(
        &s,
        &grant(
            subject(USER),
            json!({"level":"execute","add_specific":["terminal"],"remove_specific":["logs"]}),
        ),
    )
    .await;
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(r["broad_scope"], false);
    {
        let d = c.data.lock().unwrap();
        let rec = d
            .permissions
            .iter()
            .find(|v| v["user_target"]["type"] == "User")
            .unwrap();
        assert_eq!(rec["specific"], json!(["Terminal", "Inspect"]));
        assert_eq!(rec["level"], "Execute");
        assert!(d.permissions.contains(&group_record));
    }
    let r = save(
        &s,
        &grant(
            subject(USER),
            json!({"level":"none","remove_specific":["terminal"]}),
        ),
    )
    .await;
    assert_eq!(r["after"]["details"]["grant"]["level"], "none");
    assert_eq!(
        r["after"]["details"]["grant"]["specific"],
        json!(["inspect"])
    );
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(c.writes().len(), 2);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn effective_access_uses_core_and_no_access_ids_disclose_no_resource_metadata() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r = data(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":target(SERVER)}),
        )
        .await,
    );
    assert_eq!(r["grant"]["level"], "read");
    assert_eq!(r["target_identity_verified"], true);
    assert_eq!(r["caller_id"], ACTOR);
    c.data.lock().unwrap().effective = json!("None");
    let start = c.data.lock().unwrap().requests.len();
    let r = data(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":target(SERVER)}),
        )
        .await,
    );
    assert_eq!(r["grant"]["level"], "none");
    assert_eq!(r["target_identity_verified"], false);
    assert!(r["target"]["name"].is_null());
    assert!(
        c.data.lock().unwrap().requests[start..]
            .iter()
            .all(|r| matches!(r.operation.as_str(), "CurrentCaller" | "GetPermission"))
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn group_only_fixture_access_then_membership_revocation_is_observed_through_current_credential()
 {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.role = user(USER, "owned-service", true);
        d.inherited = true;
        d.permissions
            .retain(|p| p["user_target"]["type"] == "UserGroup");
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let r = data(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":target(SERVER)}),
        )
        .await,
    );
    assert_eq!(r["grant"]["specific"], json!(["inspect", "logs"]));
    c.data.lock().unwrap().groups.get_mut(GROUP).unwrap()["users"] = json!([OTHER]);
    let r = data(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":target(SERVER)}),
        )
        .await,
    );
    assert_eq!(r["grant"]["level"], "none");
    assert_eq!(r["target_identity_verified"], false);
    let r = data(call(&s, "list_user_groups", json!({})).await);
    assert!(r["items"].as_array().unwrap().is_empty());
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn nonadmin_reads_and_applies_never_borrow_administrator_credentials() {
    let c = Core::start().await;
    c.data.lock().unwrap().role["admin"] = json!(false);
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    assert_eq!(
        error(call(&s, "list_users", json!({})).await)["kind"],
        "permission_denied"
    );
    assert_eq!(
        error(call(&s, "get_user", target(USER)).await)["kind"],
        "permission_denied"
    );
    assert_eq!(
        error(
            call(
                &s,
                "preview_access_change",
                json!({"change":base(USER,false)})
            )
            .await
        )["kind"],
        "permission_denied"
    );
    let r = data(
        call(
            &s,
            "list_direct_permissions",
            json!({"subject":{"kind":"current_caller"}}),
        )
        .await,
    );
    assert_eq!(r["subject"]["id"], ACTOR);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn disabled_and_admin_user_grant_targets_cannot_be_changed() {
    for (enabled, admin) in [(false, false), (true, true)] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            let u = d.users.get_mut(USER).unwrap();
            u["enabled"] = json!(enabled);
            u["admin"] = json!(admin);
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        assert_eq!(
            error(
                call(
                    &s,
                    "preview_access_change",
                    json!({"change":grant(subject(USER),json!({"level":"write"}))})
                )
                .await
            )["kind"],
            "permission_denied"
        );
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn private_auth_membership_grant_and_authority_drift_invalidate_consumed_previews() {
    for field in ["auth", "member", "grant", "authority"] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let ch = if field == "member" {
            member(USER, false)
        } else {
            base(USER, false)
        };
        let v = preview(&s, &ch).await;
        {
            let mut d = c.data.lock().unwrap();
            match field {
                "auth" => {
                    d.users.get_mut(USER).unwrap()["totp"]["secret"] = json!("changed-secret")
                }
                "member" => d.groups.get_mut(GROUP).unwrap()["users"] = json!([USER]),
                "grant" => d.users.get_mut(USER).unwrap()["all"] = json!({"Build":"Read"}),
                _ => d.role["super_admin"] = json!(true),
            }
        }
        assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "stale_preview");
        assert!(c.writes().is_empty());
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn final_shared_transport_rechecks_private_auth_after_handler_preflight() {
    let c = Core::start().await;
    let mut changed = c.data.lock().unwrap().users[USER].clone();
    changed["passkey"]["passkey"] = json!({"changed":PRIVATE[3]});
    c.data.lock().unwrap().drift_user = Some((3, changed));
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = base(USER, false);
    let v = preview(&s, &ch).await;
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "stale_preview");
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn private_tokens_are_single_use_input_bound_and_invalid_after_restart() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = base(USER, true);
    let v = preview(&s, &ch).await;
    assert_eq!(data(apply(&s, &ch, &v).await)["outcome"], "no_op");
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "invalid_preview");
    let v = preview(&s, &base(USER, false)).await;
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "invalid_preview");
    let v = preview(&s, &ch).await;
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "invalid_preview");
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn unknown_specific_flags_are_omitted_on_reads_and_never_dropped_by_writes() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.permissions[0]["specific"] = json!(["Logs", PRIVATE[4]]);
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = data(
        call(
            &s,
            "list_direct_permissions",
            json!({"subject":{"kind":"selected","subject":subject(USER)}}),
        )
        .await,
    );
    assert_eq!(r["complete"], false);
    assert_eq!(r["records"][0]["grant"]["unknown_specific_count"], 1);
    assert_eq!(
        error(
            call(
                &s,
                "preview_access_change",
                json!({"change":grant(subject(USER),json!({"level":"write"}))})
            )
            .await
        )["kind"],
        "unsupported_target"
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn malformed_and_lost_write_acknowledgements_keep_matching_state_separate_without_retry() {
    for kind in [
        Reply::Lost,
        Reply::Malformed,
        Reply::PrivateNoData,
        Reply::Oversized,
        Reply::Redirect,
        Reply::Denied,
        Reply::BeforeError,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().reply = kind;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = save(&s, &base(USER, false)).await;
        assert_eq!(r["acknowledgement_valid"], false);
        assert_eq!(
            r["outcome"],
            if matches!(kind, Reply::Denied) {
                "rejected"
            } else {
                "submission_unknown"
            }
        );
        if matches!(
            kind,
            Reply::Lost
                | Reply::Malformed
                | Reply::PrivateNoData
                | Reply::Oversized
                | Reply::Redirect
        ) {
            assert_eq!(r["saved_state_matches"], true);
        }
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn acknowledged_save_survives_denied_or_changed_readback_without_verified_state() {
    for denied in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if denied {
                d.after_write_read_status = Some(403);
            } else {
                let mut u = d.users[USER].clone();
                u["enabled"] = json!(false);
                u["extra_private"] = json!("changed");
                d.after_write_user = Some(u);
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = save(&s, &base(USER, false)).await;
        assert_eq!(r["outcome"], "saved");
        assert_eq!(r["acknowledgement_valid"], true);
        assert_eq!(r["saved_state_matches"], false);
        assert!(!r["observation_error"].is_null());
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn deletion_reports_orphan_permissions_and_partial_group_cleanup_without_extra_writes() {
    for partial in [false, true] {
        let c = Core::start().await;
        if partial {
            c.data.lock().unwrap().reply = Reply::PartialCleanup;
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = save(&s, &json!({"kind":"delete_user","target":target(USER)})).await;
        assert_eq!(
            r["outcome"],
            if partial {
                "submission_unknown"
            } else {
                "deleted"
            }
        );
        assert_eq!(r["saved_state_matches"], true);
        assert_eq!(r["cleanup"]["direct_permission_records_remaining"], 1);
        assert_eq!(r["cleanup"]["group_reference_count"], usize::from(partial));
        assert!(
            r["cleanup"]["credential_cleanup"]
                .as_str()
                .unwrap()
                .contains("not_performed")
        );
        assert_eq!(c.writes().len(), 1);
        assert!(c.data.lock().unwrap().users.contains_key(OTHER));
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn group_deletion_can_remove_record_before_permission_cleanup_fails() {
    let c = Core::start().await;
    c.data.lock().unwrap().reply = Reply::PartialCleanup;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(&s, &json!({"kind":"delete_group","target":target(GROUP)})).await;
    assert_eq!(r["outcome"], "submission_unknown");
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(r["cleanup"]["direct_permission_records_remaining"], 1);
    assert!(c.data.lock().unwrap().groups.is_empty());
    assert_eq!(c.writes().len(), 1);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn oid_shaped_names_ambiguity_and_missing_scopes_never_select_a_different_entity() {
    let c = Core::start().await;
    {
        c.data.lock().unwrap().users.get_mut(USER).unwrap()["username"] = json!(OTHER);
    }
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = data(call(&s, "get_user", json!({"name":OTHER})).await);
    assert_eq!(r["id"], USER);
    assert_eq!(r["username"], OTHER);
    c.data.lock().unwrap().users.get_mut(OTHER).unwrap()["username"] = json!(OTHER);
    assert_eq!(
        error(call(&s, "get_user", json!({"name":OTHER})).await)["kind"],
        "ambiguous_target"
    );
    for ch in [
        json!({"kind":"change_permission","subject":subject(USER),"patch":{"level":"write"}}),
        json!({"kind":"change_permission","subject":subject(USER),"scope":{"kind":"resource","resource_kind":"server"},"patch":{"level":"write"}}),
        json!({"kind":"set_user_base","target":target(USER),"enabled":null}),
        json!({"kind":"create_local_user","username":"x","password":PRIVATE[0]}),
        json!({"kind":"set_users_in_group","target":target(GROUP),"users":[]}),
    ] {
        assert_eq!(
            error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"],
            "invalid_arguments"
        );
    }
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}

#[tokio::test]
async fn typewide_scalar_grants_preserve_unrelated_unknown_specifics_and_auth() {
    let c = Core::start().await;
    {
        c.data.lock().unwrap().users.get_mut(USER).unwrap()["all"] =
            json!({"Build":{"level":"Read","specific":[PRIVATE[4]]}});
    }
    let before = c.data.lock().unwrap().users[USER].clone();
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = json!({"kind":"change_permission","subject":subject(USER),"scope":{"kind":"resource_type","resource_kind":"server"},"patch":{"level":"execute"}});
    let r = save(&s, &ch).await;
    assert_eq!(r["saved_state_matches"], true);
    assert_eq!(c.writes()[0].params["permission"], "Execute");
    let after = c.data.lock().unwrap().users[USER].clone();
    assert_eq!(after["all"]["Build"], before["all"]["Build"]);
    assert_eq!(after["config"], before["config"]);
    assert_eq!(after["totp"], before["totp"]);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn ordinary_admin_cannot_disable_or_delete_an_admin_but_super_admin_can() {
    let c = Core::start().await;
    c.data.lock().unwrap().users.get_mut(USER).unwrap()["admin"] = json!(true);
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for ch in [
        base(USER, false),
        json!({"kind":"delete_user","target":target(USER)}),
    ] {
        assert_eq!(
            error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"],
            "permission_denied"
        );
    }
    c.data.lock().unwrap().role["super_admin"] = json!(true);
    assert_eq!(
        save(&s, &base(USER, false)).await["saved_state_matches"],
        true
    );
    assert_eq!(
        save(&s, &json!({"kind":"delete_user","target":target(USER)})).await["saved_state_matches"],
        true
    );
    assert_eq!(c.writes().len(), 2);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn everyone_inheritance_can_apply_without_a_visible_explicit_membership_group() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.role = user(USER, "owned-service", true);
        d.inherited = true;
        let g = d.groups.get_mut(GROUP).unwrap();
        g["users"] = json!([OTHER]);
        g["everyone"] = json!(true);
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let groups = data(call(&s, "list_user_groups", json!({})).await);
    assert!(groups["items"].as_array().unwrap().is_empty());
    let r = data(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":target(SERVER)}),
        )
        .await,
    );
    assert_eq!(r["grant"]["level"], "read");
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn deletion_absence_and_cleanup_are_not_verified_after_loss_of_admin_visibility() {
    let c = Core::start().await;
    let mut role = c.data.lock().unwrap().role.clone();
    role["admin"] = json!(false);
    c.data.lock().unwrap().drift_role = Some((5, role));
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(&s, &json!({"kind":"delete_group","target":target(GROUP)})).await;
    assert_eq!(r["outcome"], "deleted");
    assert_eq!(r["acknowledgement_valid"], true);
    assert_eq!(r["saved_state_matches"], false);
    assert_eq!(r["observation_error"]["kind"], "permission_denied");
    assert_eq!(r["cleanup"]["observations_complete"], false);
    assert_eq!(c.writes().len(), 1);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn resource_secret_drift_and_phantom_admin_shortcut_cannot_validate_an_existing_target() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = grant(subject(USER), json!({"level":"write"}));
    let v = preview(&s, &ch).await;
    c.data.lock().unwrap().resource["config"]["passkey"] = json!("changed-reference");
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "stale_preview");
    let r = error(
        call(
            &s,
            "get_effective_permission",
            json!({"resource_kind":"server","target":{"id":"000000000000000000000099"}}),
        )
        .await,
    );
    assert!(matches!(
        r["kind"].as_str(),
        Some("not_found" | "invalid_response")
    ));
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn bounds_missing_role_fields_invalid_grant_patches_and_name_collisions_fail_closed() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for ch in [
        grant(
            subject(USER),
            json!({"add_specific":["logs"],"remove_specific":["logs"]}),
        ),
        grant(subject(USER), json!({"level":null})),
        json!({"kind":"create_group","name":"owned-group"}),
    ] {
        assert!(matches!(
            error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"].as_str(),
            Some("invalid_arguments" | "stale_preview")
        ));
    }
    let mut a = c.data.lock().unwrap().users[USER].clone();
    a.as_object_mut().unwrap().remove("admin");
    c.data
        .lock()
        .unwrap()
        .read_override
        .insert("FindUser".into(), a);
    assert_eq!(
        error(call(&s, "get_user", target(USER)).await)["kind"],
        "invalid_response"
    );
    c.data.lock().unwrap().read_override.clear();
    let mut big = group(GROUP, "owned-group");
    big["private_extension"] = json!("x".repeat(256 * 1024));
    c.data.lock().unwrap().groups.insert(GROUP.into(), big);
    assert_eq!(
        error(call(&s, "get_user_group", target(GROUP)).await)["kind"],
        "response_too_large"
    );
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
}
#[tokio::test]
async fn scoped_mutation_preserves_other_records_and_rejects_duplicate_direct_scope_evidence() {
    let c = Core::start().await;
    let mut other = permission("User", USER, "Build", "000000000000000000000011");
    other["_id"]["$oid"] = json!("0000000000000000000000a2");
    other["specific"] = json!([PRIVATE[4]]);
    c.data.lock().unwrap().permissions.push(other.clone());
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = grant(subject(USER), json!({"level":"write"}));
    assert_eq!(save(&s, &ch).await["saved_state_matches"], true);
    assert!(c.data.lock().unwrap().permissions.contains(&other));
    {
        let mut d = c.data.lock().unwrap();
        let duplicate = d
            .permissions
            .iter()
            .find(|v| {
                v["resource_target"]["type"] == "Server" && v["user_target"]["type"] == "User"
            })
            .unwrap()
            .clone();
        d.permissions.push(duplicate);
    }
    assert_eq!(
        error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"],
        "invalid_response"
    );
    assert_eq!(c.writes().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn a_matching_scoped_grant_does_not_verify_a_failed_parent_observation() {
    let c = Core::start().await;
    c.data.lock().unwrap().after_write_read_override = Some(("FindUser".into(), Value::Null));
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let r = save(&s, &grant(subject(USER), json!({"level":"write"}))).await;
    assert_eq!(r["acknowledgement_valid"], true);
    assert_eq!(r["outcome"], "saved");
    assert_eq!(r["saved_state_matches"], false);
    assert_eq!(r["observation_error"]["kind"], "invalid_response");
    assert_eq!(c.writes().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn service_descriptions_enforce_protected_admin_and_super_admin_targets() {
    let c = Core::start().await;
    c.data.lock().unwrap().users.get_mut(USER).unwrap()["admin"] = json!(true);
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let ch = json!({"kind":"set_service_user_description","target":target(USER),"description":"reviewed metadata"});
    assert_eq!(
        error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"],
        "permission_denied"
    );
    assert!(c.writes().is_empty());
    c.data.lock().unwrap().role["super_admin"] = json!(true);
    let v = preview(&s, &ch).await;
    assert_eq!(v["requires_super_admin"], true);
    assert_eq!(data(apply(&s, &ch, &v).await)["saved_state_matches"], true);
    let v = preview(&s, &ch).await;
    c.data.lock().unwrap().users.get_mut(USER).unwrap()["super_admin"] = json!(true);
    assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "permission_denied");
    assert_eq!(
        error(call(&s, "preview_access_change", json!({"change":ch})).await)["kind"],
        "permission_denied"
    );
    assert_eq!(c.writes().len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn membership_and_everyone_changes_bind_separate_scoped_group_grants() {
    for final_transport in [false, true] {
        for ch in [
            member(USER, true),
            member(USER, false),
            json!({"kind":"set_group_everyone","target":target(GROUP),"everyone":true}),
        ] {
            let c = Core::start().await;
            if ch["present"] == true {
                c.data.lock().unwrap().groups.get_mut(GROUP).unwrap()["users"] = json!([OTHER]);
            }
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let v = preview(&s, &ch).await;
            {
                let mut d = c.data.lock().unwrap();
                let mut changed = d.permissions.clone();
                let rec = changed
                    .iter_mut()
                    .find(|v| v["user_target"]["type"] == "UserGroup")
                    .unwrap();
                rec["specific"] = json!(["Logs", "Terminal"]);
                if final_transport {
                    d.drift_permissions = Some((3, changed));
                } else {
                    d.permissions = changed;
                }
            }
            assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "stale_preview");
            assert!(c.writes().is_empty());
            assert_eq!(error(apply(&s, &ch, &v).await)["kind"], "invalid_preview");
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn reenable_binds_direct_membership_everyone_and_inherited_grants_at_both_checks() {
    for final_transport in [false, true] {
        for field in [
            "direct",
            "group_grant",
            "membership",
            "everyone",
            "everyone_grant",
            "typewide",
        ] {
            let c = Core::start().await;
            {
                let mut d = c.data.lock().unwrap();
                d.users.get_mut(USER).unwrap()["enabled"] = json!(false);
                if matches!(field, "membership" | "everyone" | "everyone_grant") {
                    d.groups.get_mut(GROUP).unwrap()["users"] = json!([OTHER]);
                }
                if field == "everyone_grant" {
                    d.groups.get_mut(GROUP).unwrap()["everyone"] = json!(true);
                }
            }
            let (p, s) = Process::start_profile(&c.address, true, true).await;
            let ch = base(USER, true);
            let v = preview(&s, &ch).await;
            {
                let mut d = c.data.lock().unwrap();
                if matches!(field, "direct" | "group_grant" | "everyone_grant") {
                    let mut changed = d.permissions.clone();
                    let kind = if field == "direct" {
                        "User"
                    } else {
                        "UserGroup"
                    };
                    changed
                        .iter_mut()
                        .find(|v| v["user_target"]["type"] == kind)
                        .unwrap()["level"] = json!("Write");
                    if final_transport {
                        d.drift_permissions =
                            Some((if field == "direct" { 5 } else { 6 }, changed));
                    } else {
                        d.permissions = changed;
                    }
                } else {
                    let mut changed = d.groups.clone();
                    match field {
                        "membership" => {
                            changed.get_mut(GROUP).unwrap()["users"] = json!([OTHER, USER])
                        }
                        "everyone" => changed.get_mut(GROUP).unwrap()["everyone"] = json!(true),
                        _ => changed.get_mut(GROUP).unwrap()["all"]["Build"] = json!("Write"),
                    }
                    if final_transport {
                        d.drift_group_directory = Some((3, changed));
                    } else {
                        d.groups = changed;
                    }
                }
            }
            assert_eq!(
                error(apply(&s, &ch, &v).await)["kind"],
                "stale_preview",
                "{field}, final={final_transport}"
            );
            assert!(c.writes().is_empty());
            assert_eq!(c.data.lock().unwrap().users[USER]["enabled"], false);
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn reenable_refuses_incomplete_or_unreadable_inherited_access() {
    for case in ["limit", "denied", "membership"] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.users.get_mut(USER).unwrap()["enabled"] = json!(false);
            match case {
                "limit" => {
                    for n in 0x300..0x311 {
                        let gid = format!("{n:024x}");
                        d.groups
                            .insert(gid.clone(), group(&gid, &format!("owned-group-{n}")));
                    }
                }
                "denied" => d.group_permission_status = Some(403),
                _ => d.groups.get_mut(GROUP).unwrap()["users"] = json!([{}]),
            }
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let r = error(
            call(
                &s,
                "preview_access_change",
                json!({"change":base(USER,true)}),
            )
            .await,
        );
        assert_eq!(
            r["kind"],
            match case {
                "limit" => "incomplete_resolution",
                "denied" => "permission_denied",
                _ => "invalid_response",
            }
        );
        assert!(c.writes().is_empty());
        assert_eq!(c.data.lock().unwrap().users[USER]["enabled"], false);
        finish(p, s, &c).await;
    }
}
