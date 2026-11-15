//! Pending-plan acceptance through the compiled production stdio server.
#[path = "support/resource_syncs.rs"]
mod fixture;
mod support;
use fixture::{Core, Fault, PRIVATE, SYNC};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

const SERVER: &str = "000000000000000000000001";
fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    safe(&r);
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    safe(&r);
    r.structured_content.unwrap()
}
fn safe(r: &CallToolResult) {
    let text = serde_json::to_string(r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret), "private plan leaked");
    }
}
fn target() -> Value {
    json!({"id":SYNC})
}
fn review(selection: Value) -> Value {
    json!({"target":target(),"selection":selection})
}
fn whole() -> Value {
    json!({"kind":"whole_sync"})
}
fn typed() -> Value {
    json!({"kind":"resource_type","resource_type":"server"})
}
fn named(targets: Value) -> Value {
    json!({"kind":"resources","resource_type":"server","targets":targets})
}
fn doc(header: &str, name: &str) -> String {
    format!(
        "[[{header}]]\nname='{name}'\ndescription='{}'\n",
        PRIVATE[2]
    )
}
fn create(header: &str, name: &str) -> Value {
    json!({"type":"Create","data":{"name":name,"proposed":doc(header,name)}})
}
fn deletion(header: &str, name: &str) -> Value {
    json!({"type":"Delete","data":{"current":doc(header,name)}})
}
fn resource(kind: &str, id: &str, delta: Value) -> Value {
    json!({"target":{"type":kind,"id":id},"data":delta})
}
fn seed(core: &Core, source: &str) {
    let mut d = core.data.lock().unwrap();
    let s = d.syncs.get_mut(SYNC).unwrap();
    s["config"]["file_contents"] = json!(source);
    s["config"]["match_tags"] = json!([]);
    s["config"]["include_resources"] = json!(true);
    s["config"]["include_variables"] = json!(true);
    s["config"]["include_user_groups"] = json!(true);
    s["info"]["pending_error"] = Value::Null;
    s["info"]["remote_contents"] = json!([{"resource_path":"","path":"","contents":source}]);
    d.server["_id"]["$oid"] = json!(SERVER);
    d.server["name"] = json!("existing");
}
fn no_ref(result: &Value, reason: &str) {
    assert!(result["review_reference"].is_null(), "{result}");
    assert_eq!(result["plan"]["apply_coverage_complete"], false);
    assert_eq!(
        result["plan"]["reviewed_status"],
        if result["plan"]["cached_status"] == "errors" {
            "errors"
        } else {
            "incomplete"
        }
    );
    assert!(
        result["plan"]["coverage_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == reason),
        "{result}"
    );
}
fn read_only(core: &Core) {
    assert!(core.requests("write").is_empty());
    assert!(core.requests("execute").is_empty());
    assert!(!core.data.lock().unwrap().bad_credentials);
}
async fn finish(p: Process, s: Session) {
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn cached_observation_preserves_distinct_errors_and_never_mutates() {
    let core = Core::start().await;
    seed(&core, "# empty owned source\n");
    {
        let mut d = core.data.lock().unwrap();
        let s = d.syncs.get_mut(SYNC).unwrap();
        s["info"]["resource_updates"] = json!([
            resource("Server", "", create("server", "new")),
            resource(
                "Server",
                SERVER,
                json!({"type":"Update","data":{"current":doc("server","existing"),"proposed":doc("server","existing")}})
            ),
            resource(
                "Server",
                "000000000000000000000002",
                deletion("server", "removed")
            )
        ]);
        s["info"]["variable_updates"] = json!([create("variable", "var")]);
        s["config"]["managed"] = json!(true);
        s["info"]["user_group_updates"] = json!([deletion("user_group", "group")]);
        s["info"]["pending_deploys"] = json!([{"target":{"type":"Stack","id":"app"},"reason":PRIVATE[3],"after":[{"type":"Deployment","id":"database"}]}]);
        s["info"]["pending_error"] = json!(PRIVATE[3]);
        s["info"]["pending_deploy_error"] = json!(PRIVATE[3]);
        s["info"]["remote_errors"] =
            json!([{"resource_path":"private","path":"private","contents":PRIVATE[2]}]);
        s["info"]["pending_hash"] = json!("abcdef12");
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let v = data(call(&s, "get_resource_sync_pending", target()).await);
    assert_eq!(v["cached_status"], "errors");
    assert_eq!(v["observation"], "cached");
    assert_eq!(v["resource_changes"]["reported_total"], 3);
    let deleted = &v["resource_changes"]["items"][2];
    assert_eq!(deleted["action"], "delete");
    assert_eq!(deleted["template_before"], false);
    assert!(deleted["template_after"].is_null());
    assert_eq!(v["errors"]["pending_error"], true);
    assert_eq!(v["errors"]["pending_deploy_error"], true);
    assert_eq!(v["errors"]["source_file_errors"], 1);
    assert!(v["source"]["pending_git_revision"].is_null());
    assert_eq!(v["source"]["proves_current_external_revision"], false);
    assert_eq!(
        v["pending_deployments"]["items"][0]["dependencies"][0]["name"],
        "database"
    );
    assert_eq!(v["core_mutation_requests"], 0);
    read_only(&core);
    finish(p, s).await;
}

#[tokio::test]
async fn explicit_refresh_has_own_local_and_core_permission_gate_and_no_retry() {
    for (operate, edit) in [(false, false), (false, true), (true, false), (true, true)] {
        let core = Core::start().await;
        seed(&core, "# owned\n");
        let (p, s) = Process::start_profile(&core.address, operate, edit).await;
        let cached = data(call(&s, "get_resource_sync_pending", target()).await);
        assert_eq!(cached["sync"]["id"], SYNC);
        assert_eq!(cached["core_mutation_requests"], 0);
        let tools = s.list_tools(None).await.unwrap().tools;
        assert_eq!(
            tools
                .iter()
                .any(|t| t.name == "refresh_resource_sync_pending"),
            operate
        );
        assert!(
            tools
                .iter()
                .any(|t| t.name == "review_resource_sync_pending")
        );
        let before = core.requests("read").len();
        if operate {
            let r = call(&s, "refresh_resource_sync_pending", target()).await;
            let v = data(r);
            assert_eq!(v["outcome"], "refreshed");
            assert_eq!(v["pending"]["observation"], "refresh_response");
            assert_eq!(v["refresh_requests_submitted"], 1);
            assert_eq!(core.requests("write")[0].params, json!({"sync":SYNC}));
            assert_eq!(core.requests("write").len(), 1);
        } else {
            let rejected = s
                .call_tool(
                    CallToolRequestParams::new("refresh_resource_sync_pending")
                        .with_arguments(target().as_object().unwrap().clone()),
                )
                .await;
            assert!(
                rejected.is_err(),
                "disabled direct invocation must be rejected by the router"
            );
            assert_eq!(core.requests("read").len(), before);
            assert!(core.requests("write").is_empty());
        }
        assert!(core.requests("execute").is_empty());
        finish(p, s).await;
    }
    for fault in ["denied", "lost", "wrong_identity"] {
        let core = Core::start().await;
        seed(&core, "# owned\n");
        {
            let mut d = core.data.lock().unwrap();
            match fault {
                "denied" => d.deny_refresh = true,
                "lost" => d.fault = Fault::Lost,
                _ => {
                    let mut v = d.syncs[SYNC].clone();
                    v["name"] = json!("other");
                    d.refresh_reply = Some(v);
                }
            }
        }
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let v = data(call(&s, "refresh_resource_sync_pending", target()).await);
        assert_eq!(
            v["outcome"],
            if fault == "denied" {
                "denied"
            } else {
                "submission_unknown"
            }
        );
        assert_eq!(v["refresh_requests_submitted"], 1);
        assert_eq!(v["run_sync_requests"], 0);
        assert!(!v["error"].is_null());
        assert_eq!(core.requests("write").len(), 1);
        assert!(core.requests("execute").is_empty());
        finish(p, s).await;
    }
}

#[tokio::test]
async fn objectid_shaped_names_and_ambiguous_pending_targets_never_disappear_from_scope() {
    let core = Core::start().await;
    let shaped = "aaaaaaaaaaaaaaaaaaaaaaaa";
    seed(&core, "# empty owned source\n");
    {
        let mut d = core.data.lock().unwrap();
        d.syncs.get_mut(SYNC).unwrap()["config"]["managed"] = json!(true);
        d.syncs.get_mut(SYNC).unwrap()["info"]["resource_updates"] =
            json!([resource("Server", SERVER, deletion("server", shaped))]);
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let v = data(
        call(
            &s,
            "review_resource_sync_pending",
            review(named(json!([{"name":shaped}]))),
        )
        .await,
    );
    assert!(v["review_reference"].is_string(), "{v}");
    assert_eq!(v["apply_binding"]["selection"]["targets"][0]["id"], SERVER);
    {
        let mut d = core.data.lock().unwrap();
        let source = doc("server", shaped);
        d.syncs.get_mut(SYNC).unwrap()["config"]["file_contents"] = json!(source);
        d.syncs.get_mut(SYNC).unwrap()["info"]["remote_contents"][0]["contents"] = json!(source);
        d.syncs.get_mut(SYNC).unwrap()["info"]["resource_updates"] =
            json!([resource("Server", "", create("server", shaped))]);
    }
    assert_eq!(
        error(
            call(
                &s,
                "review_resource_sync_pending",
                review(named(json!([{"name":shaped}])))
            )
            .await
        )["kind"],
        "unsupported_target"
    );
    {
        let mut d = core.data.lock().unwrap();
        d.syncs.get_mut(SYNC).unwrap()["info"]["resource_updates"] = json!([
            resource("Server", SERVER, deletion("server", shaped)),
            resource(
                "Server",
                "000000000000000000000002",
                deletion("server", shaped)
            )
        ]);
    }
    assert_eq!(
        error(
            call(
                &s,
                "review_resource_sync_pending",
                review(named(json!([{"name":shaped}])))
            )
            .await
        )["kind"],
        "ambiguous_target"
    );
    read_only(&core);
    finish(p, s).await;
}

#[tokio::test]
async fn review_resolves_new_names_and_existing_ids_without_broadening_or_variables() {
    let core = Core::start().await;
    seed(
        &core,
        &format!("{}{}", doc("server", "new"), doc("variable", "var")),
    );
    {
        let mut d = core.data.lock().unwrap();
        let s = d.syncs.get_mut(SYNC).unwrap();
        s["config"]["managed"] = json!(true);
        s["info"]["resource_updates"] = json!([
            resource("Server", "", create("server", "new")),
            resource("Server", SERVER, deletion("server", "old"))
        ]);
        s["info"]["variable_updates"] = json!([create("variable", "var")]);
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    for selection in [
        whole(),
        typed(),
        named(json!([{"name":"new"},{"id":SERVER}])),
    ] {
        let v = data(
            call(
                &s,
                "review_resource_sync_pending",
                review(selection.clone()),
            )
            .await,
        );
        assert!(v["review_reference"].is_string(), "{v}");
        assert_eq!(v["expires_after_seconds"], 600);
        assert_eq!(v["apply_binding"]["sync_id"], SYNC);
        assert_eq!(v["apply_binding"]["selection"]["kind"], selection["kind"]);
        assert_eq!(
            v["plan"]["variables_in_review_scope"],
            selection["kind"] == "whole_sync"
        );
        if selection["kind"] != "whole_sync" {
            assert_eq!(v["plan"]["variable_changes"]["reported_total"], 0);
            assert_eq!(v["plan"]["variable_changes"]["excluded_by_selection"], 1);
        }
        if selection["kind"] == "resources" {
            let text = v["apply_binding"]["selection"]["targets"].to_string();
            assert!(text.contains("existing") && text.contains(SERVER) && text.contains("create"));
        }
    }
    for targets in [
        json!([]),
        json!([{"name":"new"},{"name":"new"}]),
        json!([{"name":"missing"}]),
        json!([{"id":"bad"}]),
    ] {
        error(call(&s, "review_resource_sync_pending", review(named(targets))).await);
    }
    read_only(&core);
    finish(p, s).await;
}

#[tokio::test]
async fn review_audits_template_and_variable_secret_flag_changes_missing_from_core_cache() {
    let core = Core::start().await;
    let source = format!(
        "[[server]]\nname='existing'\ntemplate=false\n[[variable]]\nname='var'\nis_secret=true\nvalue='{}'\n",
        PRIVATE[2]
    );
    seed(&core, &source);
    {
        let mut d = core.data.lock().unwrap();
        d.server["template"] = json!(true);
        d.variables =
            json!([{"name":"var","is_secret":false,"value":PRIVATE[2],"description":PRIVATE[3]}]);
    }
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let v = data(call(&s, "review_resource_sync_pending", review(whole())).await);
    assert_eq!(v["plan"]["cached_status"], "clean");
    assert_eq!(v["plan"]["reviewed_status"], "pending");
    assert_eq!(
        v["plan"]["resource_changes"]["items"][0]["origin"],
        "metadata_audit"
    );
    assert_eq!(
        v["plan"]["resource_changes"]["items"][0]["template_before"],
        true
    );
    assert_eq!(
        v["plan"]["resource_changes"]["items"][0]["template_after"],
        false
    );
    assert_eq!(
        v["plan"]["variable_changes"]["items"][0]["secret_flag_after"],
        true
    );
    assert!(v["review_reference"].is_string(), "{v}");
    core.data.lock().unwrap().deny_metadata = true;
    let denied = data(call(&s, "review_resource_sync_pending", review(whole())).await);
    no_ref(&denied, "resource_metadata_not_authoritatively_observed");
    assert!(
        !denied["plan"]["observation_errors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    read_only(&core);
    finish(p, s).await;
}

#[tokio::test]
async fn deployment_execution_coverage_rejects_filters_and_disabled_resources() {
    for (selection, tags, include, allowed) in [
        (whole(), json!([]), true, true),
        (typed(), json!([]), true, false),
        (whole(), json!(["owned"]), true, false),
        (whole(), json!([]), false, false),
        (whole(), json!(["nonmatching"]), true, false),
        (whole(), json!(["nonmatching"]), false, false),
    ] {
        let core = Core::start().await;
        let source = "[[stack]]\nname='app'\ndeploy=true\ntags=['owned']\nafter=['database']\n[[deployment]]\nname='database'\ndeploy=true\ntags=['owned']\n";
        seed(&core, source);
        {
            let mut d = core.data.lock().unwrap();
            let s = d.syncs.get_mut(SYNC).unwrap();
            let cached_rows = include && tags.as_array().unwrap().iter().all(|tag| tag == "owned");
            s["config"]["match_tags"] = tags;
            s["config"]["include_resources"] = json!(include);
            if cached_rows {
                s["info"]["resource_updates"] = json!([
                    resource("Stack", "", create("stack", "app")),
                    resource("Deployment", "", create("deployment", "database"))
                ]);
                s["info"]["pending_deploys"] = json!([{"target":{"type":"Stack","id":"app"},"reason":"deploy on creation","after":[{"type":"Deployment","id":"database"}]}]);
            }
        }
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let v = data(call(&s, "review_resource_sync_pending", review(selection)).await);
        if allowed {
            assert!(v["review_reference"].is_string(), "{v}");
        } else {
            no_ref(&v, "deployment_execution_scope_differs_from_cached_view");
        }
        read_only(&core);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn source_tag_ids_with_name_filters_never_authorize_unresolved_metadata_coverage() {
    for tags in [
        "'aaaaaaaaaaaaaaaaaaaaaaaa'",
        "'owned','aaaaaaaaaaaaaaaaaaaaaaaa'",
    ] {
        let core = Core::start().await;
        seed(
            &core,
            &format!("[[server]]\nname='existing'\ntemplate=false\ntags=[{tags}]\n"),
        );
        {
            let mut d = core.data.lock().unwrap();
            d.syncs.get_mut(SYNC).unwrap()["config"]["match_tags"] = json!(["owned"]);
            d.server["template"] = json!(true);
        }
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let v = data(call(&s, "review_resource_sync_pending", review(whole())).await);
        no_ref(
            &v,
            "objectid_shaped_source_tags_require_authoritative_resolution",
        );
        assert_eq!(v["plan"]["cached_status"], "clean");
        read_only(&core);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn missing_truncated_or_secret_bearing_malformed_evidence_never_authorizes() {
    for fault in [
        "missing_info",
        "stale_delete",
        "bad_source",
        "missing_config",
        "truncated",
        "malformed_id",
        "source_error",
        "duplicate",
        "nested_sync",
    ] {
        let core = Core::start().await;
        seed(&core, "# owned\n");
        let reason;
        {
            let mut d = core.data.lock().unwrap();
            let s = d.syncs.get_mut(SYNC).unwrap();
            reason = match fault {
                "missing_info" => {
                    s["info"]
                        .as_object_mut()
                        .unwrap()
                        .remove("resource_updates");
                    "missing_cache_fields"
                }
                "stale_delete" => {
                    s["info"]["resource_updates"] =
                        json!([resource("Server", SERVER, deletion("server", "old"))]);
                    "cached_deletions_disagree_with_definition_or_source"
                }
                "bad_source" => {
                    s["config"]["file_contents"] = json!("[[server] private parser error\n");
                    s["info"]["remote_contents"][0]["contents"] =
                        s["config"]["file_contents"].clone();
                    "source_document_unparseable_or_unsupported"
                }
                "missing_config" => {
                    s["config"]
                        .as_object_mut()
                        .unwrap()
                        .remove("include_resources");
                    "missing_or_unsupported_definition_fields"
                }
                "truncated" => {
                    s["info"]["resource_updates"] = json!(
                        (0..33)
                            .map(|i| resource(
                                "Server",
                                &format!("{i:024x}"),
                                deletion("server", &format!("old-{i}"))
                            ))
                            .collect::<Vec<_>>()
                    );
                    "selected_display_truncated"
                }
                "malformed_id" => {
                    s["info"]["resource_updates"] =
                        json!([resource("Server", PRIVATE[2], deletion("server", "old"))]);
                    "unparseable_cached_change"
                }
                "source_error" => {
                    s["info"]["pending_error"] = json!(PRIVATE[3]);
                    "core_pending_or_source_errors"
                }
                "duplicate" => {
                    s["info"]["remote_contents"] = json!([{"resource_path":"","path":"","contents":"# owned\n"},{"resource_path":"","path":"","contents":"# owned\n"}]);
                    "inline_cache_does_not_match_saved_definition"
                }
                _ => {
                    let t = "[[resource_sync]]\nname='nested'\n[resource_sync.config]\nfile_contents='secret inline'\n";
                    s["config"]["file_contents"] = json!(t);
                    s["info"]["remote_contents"][0]["contents"] = json!(t);
                    "inline_or_unconfigured_nested_sync_definitions_ignored_by_core"
                }
            };
        }
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let v = data(call(&s, "review_resource_sync_pending", review(whole())).await);
        no_ref(&v, reason);
        if fault == "truncated" {
            assert_eq!(v["plan"]["resource_changes"]["reported_total"], 33);
            assert_eq!(
                v["plan"]["resource_changes"]["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                32
            );
        }
        read_only(&core);
        finish(p, s).await;
    }
}

#[tokio::test]
async fn source_evidence_and_version_limits_are_honest_for_all_four_sources() {
    for kind in ["inline", "git", "linked_repo", "host"] {
        let core = Core::start().await;
        seed(&core, "# owned\n");
        {
            let mut d = core.data.lock().unwrap();
            let s = d.syncs.get_mut(SYNC).unwrap();
            s["info"]["pending_hash"] = json!("abcdef12");
            match kind {
                "git" => {
                    s["config"]["file_contents"] = json!("");
                    s["config"]["repo"] = json!("owned/source");
                }
                "linked_repo" => {
                    s["config"]["file_contents"] = json!("");
                    s["config"]["linked_repo"] = json!(fixture::REPO);
                }
                "host" => {
                    s["config"]["file_contents"] = json!("");
                    s["config"]["files_on_host"] = json!(true);
                }
                _ => {}
            }
        }
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let v = data(call(&s, "review_resource_sync_pending", review(whole())).await);
        assert!(v["review_reference"].is_string(), "{v}");
        assert_eq!(v["plan"]["source"]["kind"], kind);
        assert_eq!(
            v["plan"]["source"]["pending_git_revision"].is_string(),
            matches!(kind, "git" | "linked_repo")
        );
        assert_eq!(
            v["plan"]["source"]["proves_current_external_revision"],
            false
        );
        read_only(&core);
        finish(p, s).await;
    }
    let core = Core::start().await;
    seed(&core, "# owned\n");
    core.data.lock().unwrap().version = "2.3.3".into();
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let v = data(call(&s, "review_resource_sync_pending", review(whole())).await);
    no_ref(&v, "review_requires_inspected_core_2_3_2");
    read_only(&core);
    finish(p, s).await;
}
