#[path = "support/configuration.rs"]
mod fixture;
mod support;
use fixture::{Core, SERVER, WriteReply};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{DEPLOYMENT, Process, STACK, Session, call},
};
const KEEP: &str = "000000000000000000007001";
const OLD: &str = "000000000000000000007002";
fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    assert_safe(&serde_json::to_string(&r).unwrap());
    let v = r.structured_content.unwrap();
    for secret in [
        "tag-private-unrelated-metadata",
        "tag-private-cleanup-error",
        "tag-private-owner",
        "changed-private-config",
    ] {
        assert!(!v.to_string().contains(secret), "private tag state escaped");
    }
    v
}
fn apply(input: &Value, p: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = p["preview_id"].clone();
    v
}
fn assign(kind: &str, id: &str, tags: Value, mode: &str) -> Value {
    json!({"target":{"kind":kind,"id":id},"mode":mode,"tags":tags})
}
async fn preview(s: &Session, name: &str, input: &Value) -> Value {
    data(call(s, name, input.clone()).await)
}
fn tag_ids(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn create_rename_assign_and_find_by_new_name_preserve_identity_and_unrelated_metadata() {
    let core = Core::start_tags().await;
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let input = json!({"name":"New Tag"});
    let p = preview(&s, "preview_tag_create", &input).await;
    assert!(core.writes().is_empty());
    assert_eq!(p["tag_after"]["id"], Value::Null);
    let r = data(call(&s, "apply_tag_create", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(r["change_verified"], true);
    let id = r["tag_after"]["id"].as_str().unwrap().to_owned();
    assert_eq!(core.writes()[0].params, json!({"name":"New Tag"}));
    let input = json!({"tag":{"id":id},"name":"Renamed Tag"});
    let p = preview(&s, "preview_tag_rename", &input).await;
    let r = data(call(&s, "apply_tag_rename", apply(&input, &p)).await);
    assert_eq!(r["tag_before"]["id"], id);
    assert_eq!(r["tag_after"]["id"], id);
    assert_eq!(r["tag_after"]["name"], "Renamed Tag");
    assert_eq!(r["change_verified"], true);
    let before = core.data.lock().unwrap().stack.clone();
    let input = assign("stack", STACK, json!([{"name":"Renamed Tag"}]), "add");
    let p = preview(&s, "preview_resource_tags", &input).await;
    assert_eq!(tag_ids(&p, "before_tags"), [KEEP]);
    assert_eq!(tag_ids(&p, "after_tags"), [KEEP, &id]);
    let r = data(call(&s, "apply_resource_tags", apply(&input, &p)).await);
    assert_eq!(r["change_verified"], true);
    assert_eq!(tag_ids(&r, "observed_tags"), [KEEP, &id]);
    let mut after = core.data.lock().unwrap().stack.clone();
    after["tags"] = before["tags"].clone();
    assert_eq!(before, after);
    assert_eq!(
        core.writes()[2].params,
        json!({"target":{"type":"Stack","id":STACK},"tags":[KEEP,id]})
    );
    let found = data(
        call(
            &s,
            "find_resources_by_tag",
            json!({"tag":{"name":"Renamed Tag"},"kind":"stack","page_size":1}),
        )
        .await,
    );
    assert_eq!(found["tag"]["id"], id);
    assert_eq!(found["resources"][0]["id"], STACK);
    assert_eq!(found["has_more"], false);
    let missing = call(
        &s,
        "find_resources_by_tag",
        json!({"tag":{"name":"New Tag"},"kind":"stack"}),
    )
    .await;
    assert_eq!(missing.structured_content.unwrap()["kind"], "not_found");
    assert_eq!(core.writes().len(), 3);
    assert!(
        core.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "execute")
    );
    process.finish(s).await;
}

#[tokio::test]
async fn omitted_unchanged_add_and_explicit_empty_sets_have_deliberate_zero_write_or_clear_behavior()
 {
    for (kind, id) in [
        ("stack", STACK),
        ("deployment", DEPLOYMENT),
        ("server", SERVER),
    ] {
        let core = Core::start_tags().await;
        let (process, s) = Process::start_profile(&core.address, false, true).await;
        for input in [
            json!({"target":{"kind":kind,"id":id}}),
            assign(kind, id, json!([]), "add"),
        ] {
            let p = preview(&s, "preview_resource_tags", &input).await;
            assert_eq!(p["outcome"], "no_op");
            let r = data(call(&s, "apply_resource_tags", apply(&input, &p)).await);
            assert_eq!(r["outcome"], "no_op");
            assert_eq!(r["writes_submitted"], 0);
        }
        assert!(core.writes().is_empty());
        let input = assign(kind, id, json!([{"id":OLD}]), "add");
        let p = preview(&s, "preview_resource_tags", &input).await;
        let r = data(call(&s, "apply_resource_tags", apply(&input, &p)).await);
        assert_eq!(r["change_verified"], true);
        let expected = if kind == "stack" {
            vec![KEEP, OLD]
        } else {
            vec![OLD]
        };
        assert_eq!(tag_ids(&r, "observed_tags"), expected);
        let p = preview(&s, "preview_resource_tags", &input).await;
        assert_eq!(p["outcome"], "no_op");
        data(call(&s, "apply_resource_tags", apply(&input, &p)).await);
        assert_eq!(core.writes().len(), 1);
        let clear = assign(kind, id, json!([]), "replace");
        let p = preview(&s, "preview_resource_tags", &clear).await;
        assert_eq!(tag_ids(&p, "before_tags"), expected);
        assert!(tag_ids(&p, "after_tags").is_empty());
        let r = data(call(&s, "apply_resource_tags", apply(&clear, &p)).await);
        assert_eq!(r["change_verified"], true);
        assert!(tag_ids(&r, "observed_tags").is_empty());
        assert_eq!(core.writes().len(), 2);
        assert_eq!(
            core.writes()[1].params,
            json!({"target":{"type":if kind=="stack"{"Stack"}else if kind=="deployment"{"Deployment"}else{"Server"},"id":id},"tags":[]})
        );
        process.finish(s).await;
    }
}

#[tokio::test]
async fn guarded_tag_deletion_removes_visible_assignments_and_reports_pinned_swarm_omission() {
    let core = Core::start_tags().await;
    {
        let mut d = core.data.lock().unwrap();
        d.stack["tags"] = json!([KEEP, OLD]);
        d.server["tags"] = json!([KEEP, OLD]);
        d.deployment["tags"] = json!([KEEP, OLD]);
        d.swarm["tags"] = json!([KEEP, OLD]);
        d.repo["tags"] = json!([KEEP, OLD]);
        d.build["tags"] = json!([KEEP, OLD]);
        for v in d.extra_references.values_mut().flatten() {
            v["tags"] = json!([KEEP, OLD]);
        }
    }
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"tag":{"name":"old-tag"},"action":"delete"});
    let p = preview(&s, "preview_tag_delete", &input).await;
    assert_eq!(p["tag_before"]["id"], OLD);
    assert_eq!(p["coverage"].as_array().unwrap().len(), 11);
    assert_eq!(p["visible_assignments"].as_array().unwrap().len(), 11);
    assert!(core.writes().is_empty());
    let r = data(call(&s, "apply_tag_delete", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(r["tag_removal_reported_by_core"], true);
    assert_eq!(r["tag_record_observation"], "tag_absent_at_exact_id_read");
    assert_eq!(r["assignment_cleanup_fully_verified"], false);
    for a in r["visible_assignments"].as_array().unwrap() {
        assert_eq!(
            a["observation"],
            if a["kind"] == "swarm" {
                "tag_still_assigned"
            } else {
                "tag_no_longer_assigned"
            }
        );
        assert_eq!(a["unrelated_tags_preserved"], true);
    }
    assert!(!core.data.lock().unwrap().tags.contains_key(OLD));
    assert_eq!(core.data.lock().unwrap().swarm["tags"], json!([KEEP, OLD]));
    assert_eq!(core.data.lock().unwrap().containers.len(), 3);
    assert_eq!(core.writes().len(), 1);
    assert_eq!(core.writes()[0].params, json!({"id":OLD}));
    assert_eq!(core.writes()[0].operation, "DeleteTag");
    let reads = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(&s, "apply_tag_delete", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), reads);
    assert!(
        core.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "execute")
    );
    process.finish(s).await;
}

#[tokio::test]
async fn every_profile_enforces_discovery_and_direct_tag_calls_before_core_io() {
    let core = Core::start_tags().await;
    for (operate, edit) in [(false, false), (true, false), (false, true), (true, true)] {
        let (process, s) = Process::start_profile(&core.address, operate, edit).await;
        let tools = s.list_tools(None).await.unwrap().tools;
        assert_eq!(
            tools.len(),
            64 + 20 * usize::from(operate)
                + 55 * usize::from(edit)
                + 4 * usize::from(operate && edit)
        );
        for name in [
            "find_resources_by_tag",
            "preview_tag_create",
            "apply_tag_create",
            "preview_tag_rename",
            "apply_tag_rename",
            "preview_resource_tags",
            "apply_resource_tags",
            "preview_tag_delete",
            "apply_tag_delete",
        ] {
            let enabled = if name == "find_resources_by_tag" {
                true
            } else if name == "apply_tag_delete" {
                operate && edit
            } else {
                edit
            };
            assert_eq!(tools.iter().any(|t| t.name == name), enabled);
            let before = core.data.lock().unwrap().requests.len();
            let r = s
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(json!({}).as_object().unwrap().clone()),
                )
                .await;
            if enabled {
                assert_eq!(r.unwrap().is_error, Some(true));
            } else {
                assert!(r.is_err());
            }
            assert_eq!(core.data.lock().unwrap().requests.len(), before);
        }
        if edit && !operate {
            let input = json!({"tag":{"id":OLD},"action":"delete"});
            let p = preview(&s, "preview_tag_delete", &input).await;
            let before = core.data.lock().unwrap().requests.len();
            assert!(
                s.call_tool(
                    CallToolRequestParams::new("apply_tag_delete")
                        .with_arguments(apply(&input, &p).as_object().unwrap().clone())
                )
                .await
                .is_err()
            );
            assert_eq!(core.data.lock().unwrap().requests.len(), before);
        }
        process.finish(s).await;
    }
    assert!(core.writes().is_empty());
}

#[tokio::test]
async fn invalid_names_selectors_modes_nulls_metadata_and_page_limits_fail_before_reads() {
    let core = Core::start_tags().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.data.lock().unwrap().requests.len();
    for (tool, input) in [
        (
            "preview_resource_tags",
            json!({"target":{"kind":"stack","id":STACK},"tags":[{"id":"*"}]}),
        ),
        ("preview_tag_create", json!({"name":""})),
        ("preview_tag_create", json!({"name":"  "})),
        ("preview_tag_create", json!({"name":OLD})),
        ("preview_tag_create", json!({"name":"x".repeat(129)})),
        ("preview_tag_create", json!({"name":"private\nname"})),
        ("preview_tag_create", json!({"name":"new","color":"Red"})),
        (
            "preview_tag_rename",
            json!({"tag":{"id":OLD,"name":"old-tag"},"name":"new"}),
        ),
        ("preview_tag_rename", json!({"tag":{"id":"*"},"name":"new"})),
        (
            "preview_resource_tags",
            json!({"target":{"kind":"stack","id":STACK},"tags":null}),
        ),
        (
            "preview_resource_tags",
            json!({"target":{"kind":"stack","id":STACK},"mode":"append"}),
        ),
        (
            "preview_resource_tags",
            json!({"target":{"kind":"stack","id":STACK},"tags":[],"description":"private"}),
        ),
        (
            "preview_resource_tags",
            assign("repo", STACK, json!([]), "replace"),
        ),
        (
            "preview_tag_delete",
            json!({"tag":{"id":OLD},"action":"force"}),
        ),
        (
            "preview_tag_delete",
            json!({"tag":{"ids":[OLD]},"action":"delete"}),
        ),
        (
            "find_resources_by_tag",
            json!({"tag":{"id":OLD},"kind":"stack","page_size":0}),
        ),
        (
            "find_resources_by_tag",
            json!({"tag":{"id":OLD},"kind":"stack","page_size":101}),
        ),
        (
            "find_resources_by_tag",
            json!({"tag":{"id":OLD},"kind":"stack","page":u64::MAX,"page_size":100}),
        ),
    ] {
        assert_eq!(call(&s, tool, input).await.is_error, Some(true), "{tool}");
    }
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    assert!(core.writes().is_empty());
    process.finish(s).await;
}

#[tokio::test]
async fn changed_private_tag_owner_color_name_resource_configuration_and_assignments_invalidate() {
    for change in 0..7 {
        let core = Core::start_tags().await;
        let (process, s) = Process::start_profile(&core.address, true, true).await;
        let (rename, tool, input) = if change < 3 {
            (
                true,
                "preview_tag_rename",
                json!({"tag":{"id":OLD},"name":"new-name"}),
            )
        } else {
            (
                false,
                "preview_resource_tags",
                assign("stack", STACK, json!([{"id":OLD}]), "add"),
            )
        };
        let p = preview(&s, tool, &input).await;
        {
            let mut d = core.data.lock().unwrap();
            match change {
                0 => d.tags.get_mut(OLD).unwrap()["owner"] = json!("tag-private-owner"),
                1 => d.tags.get_mut(OLD).unwrap()["color"] = json!("Red"),
                2 => d.tags.get_mut(OLD).unwrap()["name"] = json!("other-name"),
                3 => d.stack["config"]["environment"] = json!("PASSWORD=changed-private-config\n"),
                4 => d.stack["tags"] = json!([]),
                5 => d.tags.get_mut(KEEP).unwrap()["owner"] = json!("tag-private-owner"),
                _ => d.stack["description"] = json!("tag-private-unrelated-metadata"),
            }
        }
        let r = call(
            &s,
            if rename {
                "apply_tag_rename"
            } else {
                "apply_resource_tags"
            },
            apply(&input, &p),
        )
        .await;
        assert_eq!(r.is_error, Some(true));
        assert_safe(&serde_json::to_string(&r).unwrap());
        assert_eq!(
            r.structured_content.unwrap()["kind"],
            "stale_preview",
            "case{change}"
        );
        assert!(core.writes().is_empty());
        process
            .finish_excluding(
                s,
                &[
                    "tag-private-owner",
                    "changed-private-config",
                    "tag-private-unrelated-metadata",
                ],
            )
            .await;
    }
    let core = Core::start_tags().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"tag":{"id":OLD},"action":"delete"});
    let p = preview(&s, "preview_tag_delete", &input).await;
    core.data.lock().unwrap().deployment["tags"] = json!([OLD]);
    assert_eq!(
        call(&s, "apply_tag_delete", apply(&input, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "stale_preview"
    );
    assert!(core.writes().is_empty());
    process.finish(s).await;
}

#[tokio::test]
async fn tokens_cannot_change_operation_tag_resource_or_mode_and_share_the_bounded_store() {
    let core = Core::start_tags().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let rename = json!({"tag":{"id":OLD},"name":"new-name"});
    let p = preview(&s, "preview_tag_rename", &rename).await;
    let before = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(
            &s,
            "apply_tag_create",
            apply(&json!({"name":"new-name"}), &p)
        )
        .await
        .structured_content
        .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let del = json!({"tag":{"id":OLD},"action":"delete"});
    let p = preview(&s, "preview_tag_delete", &del).await;
    let before = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(
            &s,
            "apply_tag_delete",
            apply(&json!({"tag":{"id":KEEP},"action":"delete"}), &p)
        )
        .await
        .structured_content
        .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let input = assign("stack", STACK, json!([{"id":OLD}]), "add");
    let p = preview(&s, "preview_resource_tags", &input).await;
    let before = core.data.lock().unwrap().requests.len();
    let changed = assign("server", SERVER, json!([{"id":OLD}]), "add");
    assert_eq!(
        call(&s, "apply_resource_tags", apply(&changed, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let p = preview(&s, "preview_resource_tags", &input).await;
    let before = core.data.lock().unwrap().requests.len();
    let changed = assign("stack", STACK, json!([{"id":OLD}]), "replace");
    assert_eq!(
        call(&s, "apply_resource_tags", apply(&changed, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    for _ in 0..8 {
        preview(
            &s,
            "preview_tag_rename",
            &json!({"tag":{"id":OLD},"name":"old-tag"}),
        )
        .await;
        preview(
            &s,
            "preview_stack_update",
            &json!({"target":{"id":STACK},"patch":{"auto_pull":false}}),
        )
        .await;
    }
    assert_eq!(
        call(&s, "preview_tag_create", json!({"name":"beyond-capacity"}))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_arguments"
    );
    assert!(core.writes().is_empty());
    process.finish(s).await;
}

#[tokio::test]
async fn unresolved_assignments_are_preserved_by_omission_and_block_unsafe_add_without_implicit_creation()
 {
    let core = Core::start_tags().await;
    let missing = "000000000000000000007099";
    core.data.lock().unwrap().stack["tags"] = json!([KEEP, missing]);
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let omitted = json!({"target":{"kind":"stack","id":STACK}});
    let p = preview(&s, "preview_resource_tags", &omitted).await;
    assert_eq!(p["outcome"], "no_op");
    assert_eq!(tag_ids(&p, "before_tags"), [KEEP, missing]);
    data(call(&s, "apply_resource_tags", apply(&omitted, &p)).await);
    let add = assign("stack", STACK, json!([{"id":OLD}]), "add");
    assert_eq!(
        call(&s, "preview_resource_tags", add)
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_arguments"
    );
    assert_eq!(
        call(
            &s,
            "preview_resource_tags",
            assign(
                "stack",
                STACK,
                json!([{"name":"not-created-implicitly"}]),
                "replace"
            )
        )
        .await
        .structured_content
        .unwrap()["kind"],
        "not_found"
    );
    assert!(core.writes().is_empty());
    let clear = assign("stack", STACK, json!([]), "replace");
    let p = preview(&s, "preview_resource_tags", &clear).await;
    let r = data(call(&s, "apply_resource_tags", apply(&clear, &p)).await);
    assert_eq!(r["change_verified"], true);
    assert_eq!(core.writes().len(), 1);
    assert_eq!(core.writes()[0].operation, "UpdateResourceMeta");
    process.finish(s).await;
}

#[tokio::test]
async fn authoritative_readback_detects_core_silent_tag_drops_and_unrelated_metadata_changes() {
    for drop in [true, false] {
        let core = Core::start_tags().await;
        let (process, s) = Process::start_profile(&core.address, false, true).await;
        let input = assign("stack", STACK, json!([{"id":OLD}]), "add");
        let p = preview(&s, "preview_resource_tags", &input).await;
        {
            let mut d = core.data.lock().unwrap();
            d.tag_drop_on_assign = drop;
            d.tag_change_metadata = !drop;
        }
        let r = data(call(&s, "apply_resource_tags", apply(&input, &p)).await);
        assert_eq!(r["outcome"], "acknowledged");
        assert_eq!(r["change_verified"], false);
        assert_eq!(r["errors"][0]["kind"], "invalid_response");
        assert_eq!(tag_ids(&r, "after_tags"), [KEEP, OLD]);
        if drop {
            assert_eq!(tag_ids(&r, "observed_tags"), [KEEP]);
        }
        assert_eq!(core.writes().len(), 1);
        assert_eq!(
            core.writes()[0].params,
            json!({"target":{"type":"Stack","id":STACK},"tags":[KEEP,OLD]})
        );
        assert!(
            core.data
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|r| r.operation != "CreateTag" && r.path != "execute")
        );
        process
            .finish_excluding(s, &["tag-private-unrelated-metadata"])
            .await;
    }
}

#[tokio::test]
async fn owner_denial_and_partial_cross_collection_cleanup_never_fallback_or_retry() {
    for delete in [false, true] {
        let core = Core::start_tags().await;
        let (process, s) = Process::start_profile(&core.address, true, true).await;
        let input = if delete {
            json!({"tag":{"id":OLD},"action":"delete"})
        } else {
            json!({"tag":{"id":OLD},"name":"owner-denied"})
        };
        let p = preview(
            &s,
            if delete {
                "preview_tag_delete"
            } else {
                "preview_tag_rename"
            },
            &input,
        )
        .await;
        core.data.lock().unwrap().tag_owner_denied = true;
        let r = data(
            call(
                &s,
                if delete {
                    "apply_tag_delete"
                } else {
                    "apply_tag_rename"
                },
                apply(&input, &p),
            )
            .await,
        );
        assert_eq!(r["outcome"], "submission_unknown");
        assert_eq!(r["errors"][0]["kind"], "permission_denied");
        assert_eq!(core.data.lock().unwrap().tags[OLD]["name"], "old-tag");
        assert_eq!(core.writes().len(), 1);
        process.finish(s).await;
    }
    let core = Core::start_tags().await;
    {
        let mut d = core.data.lock().unwrap();
        d.stack["tags"] = json!([KEEP, OLD]);
        d.deployment["tags"] = json!([OLD]);
    }
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let input = json!({"tag":{"id":OLD},"action":"delete"});
    let p = preview(&s, "preview_tag_delete", &input).await;
    core.data.lock().unwrap().tag_partial_cleanup_error = true;
    let r = data(call(&s, "apply_tag_delete", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "submission_unknown");
    assert_eq!(
        r["tag_record_observation"],
        "tag_still_present_at_exact_id_read"
    );
    assert_eq!(r["tag_removal_reported_by_core"], false);
    let effects = r["visible_assignments"].as_array().unwrap();
    assert!(
        effects
            .iter()
            .any(|a| a["kind"] == "stack" && a["observation"] == "tag_no_longer_assigned")
    );
    assert!(
        effects
            .iter()
            .any(|a| a["kind"] == "deployment" && a["observation"] == "tag_still_assigned")
    );
    assert_eq!(core.writes().len(), 1);
    process
        .finish_excluding(s, &["tag-private-cleanup-error"])
        .await;
}

#[tokio::test]
async fn malformed_lost_oversized_denied_redirected_and_mismatched_replies_retain_uncertainty() {
    for operation in ["create", "rename", "assign", "delete"] {
        for reply in [
            WriteReply::Malformed,
            WriteReply::Oversized,
            WriteReply::Denied,
            WriteReply::Redirect,
            WriteReply::Lost,
            WriteReply::WriteDenied,
            WriteReply::WrongTarget,
            WriteReply::WrongName,
            WriteReply::MissingTagOwner,
            WriteReply::WrongTagOwner,
            WriteReply::MissingTagColor,
            WriteReply::Template,
        ] {
            if operation == "assign"
                && matches!(
                    reply,
                    WriteReply::MissingTagOwner | WriteReply::MissingTagColor
                )
            {
                continue;
            }
            if operation != "assign" && matches!(reply, WriteReply::Template) {
                continue;
            }
            let core = Core::start_tags().await;
            let (process, s) = Process::start_profile(&core.address, true, true).await;
            let (pre, app, input) = match operation {
                "create" => (
                    "preview_tag_create",
                    "apply_tag_create",
                    json!({"name":"uncertain-tag"}),
                ),
                "rename" => (
                    "preview_tag_rename",
                    "apply_tag_rename",
                    json!({"tag":{"id":OLD},"name":"uncertain-rename"}),
                ),
                "assign" => (
                    "preview_resource_tags",
                    "apply_resource_tags",
                    assign("stack", STACK, json!([{"id":OLD}]), "add"),
                ),
                _ => (
                    "preview_tag_delete",
                    "apply_tag_delete",
                    json!({"tag":{"id":OLD},"action":"delete"}),
                ),
            };
            let p = preview(&s, pre, &input).await;
            core.data.lock().unwrap().reply = reply;
            let r = data(call(&s, app, apply(&input, &p)).await);
            assert_eq!(r["outcome"], "submission_unknown", "{operation}");
            assert_eq!(r["writes_submitted"], 1);
            assert_eq!(core.writes().len(), 1);
            let before = core.data.lock().unwrap().requests.len();
            assert_eq!(
                call(&s, app, apply(&input, &p))
                    .await
                    .structured_content
                    .unwrap()["kind"],
                "invalid_preview"
            );
            assert_eq!(core.data.lock().unwrap().requests.len(), before);
            assert!(
                core.data
                    .lock()
                    .unwrap()
                    .requests
                    .iter()
                    .all(|r| r.path != "execute")
            );
            process.finish(s).await;
        }
    }
}

#[tokio::test]
async fn observation_denial_keeps_valid_acknowledgement_without_a_verification_claim() {
    let core = Core::start_tags().await;
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let input = json!({"tag":{"id":OLD},"name":"read-back-denied"});
    let p = preview(&s, "preview_tag_rename", &input).await;
    core.data.lock().unwrap().post_write_read_status = Some(403);
    let r = data(call(&s, "apply_tag_rename", apply(&input, &p)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(r["change_verified"], false);
    assert_eq!(r["tag_record_observation"], "observation_unavailable");
    assert_eq!(r["errors"][0]["kind"], "permission_denied");
    assert_eq!(core.writes().len(), 1);
    process.finish(s).await;
}

#[tokio::test]
async fn find_pagination_and_deletion_coverage_remain_bounded_and_permission_gaps_are_explicit() {
    let core = Core::start_tags().await;
    {
        let mut d = core.data.lock().unwrap();
        let base = d.stack.clone();
        for i in 0..44 {
            let id = format!("{:024x}", 0x8000 + i);
            let mut v = base.clone();
            v["_id"] = json!({"$oid":id});
            v["name"] = json!(format!("tagged-stack-{i:03}"));
            v["tags"] = json!([OLD]);
            d.created.insert(id, v);
        }
    }
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let found = data(
        call(
            &s,
            "find_resources_by_tag",
            json!({"tag":{"name":"old-tag"},"kind":"stack","page_size":1}),
        )
        .await,
    );
    assert_eq!(found["resources"].as_array().unwrap().len(), 1);
    assert_eq!(found["has_more"], true);
    assert_eq!(found["next_page"], 1);
    let p = preview(
        &s,
        "preview_tag_delete",
        &json!({"tag":{"id":OLD},"action":"delete"}),
    )
    .await;
    assert_eq!(p["visible_assignments"].as_array().unwrap().len(), 32);
    let stack = p["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["family"] == "stack")
        .unwrap();
    assert_eq!(stack["visible_records_examined"], 40);
    assert_eq!(stack["visible_scan_complete"], false);
    assert_eq!(stack["hidden_records_possible"], true);
    assert!(core.writes().is_empty());
    process.finish(s).await;
    let core = Core::start_tags().await;
    core.data.lock().unwrap().reference_status = Some(403);
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let p = preview(
        &s,
        "preview_tag_delete",
        &json!({"tag":{"id":OLD},"action":"delete"}),
    )
    .await;
    assert!(
        p["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["visible_scan_complete"] == false
                && c["error"]["kind"] == "permission_denied")
    );
    assert!(core.writes().is_empty());
    process.finish(s).await;
}

#[tokio::test]
async fn timeout_keeps_unknown_submission_even_when_readback_observes_a_rename() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        tags::{self, ApplyRename, RenameInput},
    };
    use std::time::Duration;
    let core = Core::start_tags().await;
    let client = CoreClient::new(
        Config::parse(&core.address, support::KEY, support::SECRET)
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(false, true)),
        RequestPolicy {
            connect_timeout: Duration::from_millis(100),
            operation_timeout: Duration::from_millis(100),
        },
    )
    .unwrap();
    let input = json!({"tag":{"id":OLD},"name":"renamed-despite-timeout"});
    let p = tags::preview_rename(
        &client,
        serde_json::from_value::<RenameInput>(input.clone()).unwrap(),
    )
    .await
    .unwrap();
    core.data.lock().unwrap().reply = WriteReply::Delayed;
    let r = serde_json::to_value(
        tags::apply_rename(
            &client,
            serde_json::from_value::<ApplyRename>(apply(&input, &serde_json::to_value(p).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(r["outcome"], "submission_unknown");
    assert_eq!(r["change_verified"], true);
    assert_eq!(r["errors"][0]["kind"], "timeout");
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(core.writes().len(), 1);
}

#[tokio::test]
async fn private_snapshot_size_is_bounded_and_raw_owner_or_boundary_secrets_never_escape() {
    let core = Core::start_tags().await;
    {
        let mut d = core.data.lock().unwrap();
        d.stack["description"] = json!("private-large-metadata".repeat(16000));
    }
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let r = call(
        &s,
        "preview_resource_tags",
        json!({"target":{"kind":"stack","id":STACK}}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    assert_eq!(r.structured_content.unwrap()["kind"], "invalid_arguments");
    assert!(core.writes().is_empty());
    process
        .finish_excluding(s, &["private-large-metadata"])
        .await;
    let core = Core::start_tags().await;
    {
        let mut d = core.data.lock().unwrap();
        d.tags.get_mut(OLD).unwrap()["name"] = json!(format!(
            "{}{}{}",
            "é".repeat(124),
            support::SECRET,
            "z".repeat(200)
        ));
        d.tags.get_mut(OLD).unwrap()["owner"] = json!("tag-private-owner");
    }
    let (process, s) = Process::start_profile(&core.address, false, true).await;
    let p = preview(
        &s,
        "preview_tag_rename",
        &json!({"tag":{"id":OLD},"name":"public-new-name"}),
    )
    .await;
    assert!(p["tag_before"]["name"].as_str().unwrap().len() <= 256);
    assert!(!p.to_string().contains(support::SECRET));
    assert!(!p.to_string().contains("tag-private-owner"));
    assert!(core.writes().is_empty());
    process.finish_excluding(s, &["tag-private-owner"]).await;
}
