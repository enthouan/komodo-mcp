#[path = "support/configuration.rs"]
mod fixture;
mod support;

use fixture::{Core, WriteReply};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{DEPLOYMENT, Process, STACK, Session, call},
};

fn data(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{r:?}");
    assert_safe(&serde_json::to_string(&r).unwrap());
    let v = r.structured_content.unwrap();
    for value in [
        "deletion-private-archived-definition",
        "deletion-private-container-environment",
        "deletion-private-cleanup-error",
        "never-return-this",
    ] {
        assert!(!v.to_string().contains(value));
    }
    v
}
fn input(kind: &str, id: &str) -> Value {
    json!({"target":{"kind":kind,"id":id},"action":"delete"})
}
fn apply(input: &Value, preview: &Value) -> Value {
    let mut v = input.clone();
    v["preview_id"] = preview["preview_id"].clone();
    v
}
async fn preview(s: &Session, input: &Value) -> Value {
    data(call(s, "preview_resource_delete", input.clone()).await)
}

#[tokio::test]
async fn stack_and_deployment_delete_use_deployed_identity_and_keep_removal_separate_from_fresh_cleanup()
 {
    for (kind, id, count) in [("stack", STACK, 2), ("deployment", DEPLOYMENT, 1)] {
        let core = Core::start_deletion().await;
        let (process, s) = Process::start_profile(&core.address, true, true).await;
        let request = input(kind, id);
        let p = preview(&s, &request).await;
        assert_eq!(p["writes_submitted"], 0);
        assert!(core.writes().is_empty());
        assert_eq!(p["known_workloads"].as_array().unwrap().len(), count);
        assert!(
            p["known_workloads"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["name"].as_str().unwrap().starts_with("running-"))
        );
        let result = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
        assert_eq!(result["outcome"], "acknowledged");
        assert_eq!(result["definition_removal"]["reported_by_core"], true);
        assert_eq!(
            result["definition_removal"]["observation"],
            "definition_absent_at_direct_read"
        );
        assert!(
            result["known_workloads"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["observation"] == "fresh_inspection_reports_known_id_absent")
        );
        assert_eq!(result["runtime_cleanup_fully_verified"], false);
        assert!(result["cleanup_updates"].as_array().unwrap().is_empty());
        assert!(
            result["evidence_gaps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("history"))
        );
        assert_eq!(core.writes().len(), 1);
        assert_eq!(core.writes()[0].params, json!({"id":id}));
        assert_eq!(
            core.writes()[0].operation,
            if kind == "stack" {
                "DeleteStack"
            } else {
                "DeleteDeployment"
            }
        );
        let before = core.data.lock().unwrap().requests.len();
        assert_eq!(
            call(&s, "apply_resource_delete", apply(&request, &p))
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
        process
            .finish_excluding(
                s,
                &[
                    "deletion-private-archived-definition",
                    "deletion-private-container-environment",
                    "never-return-this",
                ],
            )
            .await;
    }
}

#[tokio::test]
async fn server_delete_detaches_visible_references_reports_current_builder_gap_and_leaves_workloads()
 {
    let core = Core::start_deletion().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let request = input("server", fixture::SERVER);
    let p = preview(&s, &request).await;
    assert_eq!(p["known_workloads"].as_array().unwrap().len(), 3);
    for family in ["stack", "deployment", "repo", "builder", "alerter"] {
        assert!(
            p["visible_references"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == family)
        );
    }
    assert!(
        p["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["hidden_records_possible"] == true)
    );
    let result = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
    assert_eq!(result["outcome"], "acknowledged");
    assert_eq!(
        result["definition_removal"]["observation"],
        "definition_absent_at_direct_read"
    );
    let refs = result["visible_references"].as_array().unwrap();
    assert!(
        refs.iter()
            .filter(|r| r["kind"] != "builder")
            .all(|r| r["after"] == "no_longer_referencing_target")
    );
    assert!(
        refs.iter()
            .filter(|r| r["kind"] == "builder")
            .all(|r| r["after"] == "still_referencing_target")
    );
    assert_eq!(core.data.lock().unwrap().containers.len(), 3);
    assert_eq!(core.writes().len(), 1);
    assert_eq!(core.writes()[0].params, json!({"id":fixture::SERVER}));
    assert!(
        result["known_workloads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["observation"] == "server_record_removed_runtime_cleanup_not_requested")
    );
    assert!(
        core.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.path != "execute")
    );
    process
        .finish_excluding(
            s,
            &[
                "deletion-private-archived-definition",
                "deletion-private-container-environment",
            ],
        )
        .await;
}

#[tokio::test]
async fn deletion_and_configuration_previews_share_capacity_and_large_private_snapshots_are_rejected()
 {
    let core = Core::start_deletion().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    for _ in 0..8 {
        preview(&s, &input("stack", STACK)).await;
        data(
            call(
                &s,
                "preview_stack_update",
                json!({"target":{"id":STACK},"patch":{"auto_pull":false}}),
            )
            .await,
        );
    }
    assert_eq!(
        call(&s, "preview_resource_delete", input("stack", STACK))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_arguments"
    );
    assert!(core.writes().is_empty());
    process.finish(s).await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    core.data.lock().unwrap().stack["config"]["file_contents"] =
        json!(format!("{}\n", "private-large-definition".repeat(14000)));
    let r = call(&s, "preview_resource_delete", input("stack", STACK)).await;
    assert_eq!(r.is_error, Some(true));
    assert_eq!(r.structured_content.unwrap()["kind"], "invalid_arguments");
    assert!(core.writes().is_empty());
    process
        .finish_excluding(s, &["private-large-definition"])
        .await;
}

#[tokio::test]
async fn four_profiles_enforce_discovery_and_direct_dispatch_before_reads() {
    let core = Core::start_deletion().await;
    for (operate, edit) in [(false, false), (true, false), (false, true), (true, true)] {
        let (process, s) = Process::start_profile(&core.address, operate, edit).await;
        let tools = s.list_tools(None).await.unwrap().tools;
        for (name, enabled) in [
            ("preview_resource_delete", edit),
            ("apply_resource_delete", operate && edit),
        ] {
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
            let request = input("stack", STACK);
            let p = preview(&s, &request).await;
            let before = core.data.lock().unwrap().requests.len();
            assert!(
                s.call_tool(
                    CallToolRequestParams::new("apply_resource_delete")
                        .with_arguments(apply(&request, &p).as_object().unwrap().clone())
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
async fn invalid_selectors_actions_and_bulk_inputs_make_zero_core_requests() {
    let core = Core::start_deletion().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.data.lock().unwrap().requests.len();
    for request in [
        input("stack", "*"),
        input("stack", "name"),
        input("stack", ""),
        json!({"target":{"kind":"repo","id":STACK},"action":"delete"}),
        json!({"target":{"kind":"stack","id":STACK},"action":"force"}),
        json!({"target":{"kind":"stack","id":STACK,"name":"alpha"},"action":"delete"}),
        json!({"targets":[{"kind":"stack","id":STACK}],"action":"delete"}),
    ] {
        assert_eq!(
            call(&s, "preview_resource_delete", request).await.is_error,
            Some(true)
        );
    }
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    process.finish(s).await;
}

#[tokio::test]
async fn raw_secrets_cleanup_identity_runtime_trust_and_reference_changes_invalidate_deletes() {
    for change in 0..6 {
        let core = Core::start_deletion().await;
        let (process, s) = Process::start_profile(&core.address, true, true).await;
        let request = input("stack", STACK);
        let p = preview(&s, &request).await;
        {
            let mut d = core.data.lock().unwrap();
            match change {
                0 => d.stack["config"]["environment"] = json!("PASSWORD=changed-private-secret"),
                1 => d.stack["info"]["deployed_project_name"] = json!("other-running-project"),
                2 => {
                    d.containers.get_mut(&"11".repeat(32)).unwrap()["State"]["Status"] =
                        json!("exited")
                }
                3 => d.server["info"]["public_key"] = json!("changed-private-trust"),
                4 => {
                    d.extra_references.get_mut("alerter").unwrap()[0]["config"]["resources"] =
                        json!([])
                }
                _ => d.stack["config"]["server_id"] = json!(""),
            }
        }
        let r = call(&s, "apply_resource_delete", apply(&request, &p)).await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            r.structured_content.unwrap()["kind"],
            "stale_preview",
            "case {change}"
        );
        assert!(core.writes().is_empty());
        process
            .finish_excluding(s, &["changed-private-secret", "changed-private-trust"])
            .await;
    }
}

#[tokio::test]
async fn type_binding_and_configuration_token_reuse_are_rejected_before_core_reads() {
    let core = Core::start_deletion().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let request = input("stack", STACK);
    let p = preview(&s, &request).await;
    let before = core.data.lock().unwrap().requests.len();
    let retarget = input("deployment", DEPLOYMENT);
    assert_eq!(
        call(&s, "apply_resource_delete", apply(&retarget, &p))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    let cfg = json!({"target":{"id":STACK},"patch":{"auto_pull":false}});
    let cp = data(call(&s, "preview_stack_update", cfg).await);
    let before = core.data.lock().unwrap().requests.len();
    assert_eq!(
        call(&s, "apply_resource_delete", apply(&request, &cp))
            .await
            .structured_content
            .unwrap()["kind"],
        "invalid_preview"
    );
    assert_eq!(core.data.lock().unwrap().requests.len(), before);
    assert!(core.writes().is_empty());
    process.finish(s).await;
}

#[tokio::test]
async fn cleanup_failure_and_skip_are_reported_even_when_core_success_is_true() {
    for (kind, id) in [("stack", STACK), ("deployment", DEPLOYMENT)] {
        for skip in [false, true] {
            let core = Core::start_deletion().await;
            {
                let mut d = core.data.lock().unwrap();
                d.deletion_history_visible = true;
                d.cleanup_skip = skip;
                d.stack_failure_success_log = kind == "stack" && !skip;
                d.cleanup_fail = kind == "deployment" && !skip;
            }
            let (process, s) = Process::start_profile(&core.address, true, true).await;
            let request = input(kind, id);
            let p = preview(&s, &request).await;
            let r = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
            assert_eq!(r["outcome"], "acknowledged");
            assert_eq!(
                r["definition_removal"]["observation"],
                "definition_absent_at_direct_read"
            );
            assert!(
                r["known_workloads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|w| w["observation"] == "fresh_inspection_still_present_after_attempt")
            );
            let updates = r["cleanup_updates"].as_array().unwrap();
            assert_eq!(updates.len(), 1);
            assert_eq!(
                updates[0]["attribution"],
                "new_matching_candidate_invocation_not_proven"
            );
            assert_eq!(
                updates[0]["reported_cleanup"],
                if skip {
                    "runtime_cleanup_skipped"
                } else {
                    "cleanup_or_post_delete_error_recorded"
                }
            );
            assert!(
                !updates[0]["failed_or_skipped_stages"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            if kind == "stack" {
                assert_eq!(
                    core.data.lock().unwrap().deletion_updates[0]["success"],
                    true
                );
            }
            assert_eq!(core.writes().len(), 1);
            process
                .finish_excluding(
                    s,
                    &[
                        "deletion-private-archived-definition",
                        "deletion-private-cleanup-error",
                    ],
                )
                .await;
        }
    }
}

#[tokio::test]
async fn incomplete_reference_scans_disclose_permission_and_page_limits() {
    for denied in [false, true] {
        let core = Core::start_deletion().await;
        {
            let mut d = core.data.lock().unwrap();
            if denied {
                d.reference_status = Some(403);
            } else {
                let source = d.extra_references["builder"][0].clone();
                for i in 1..44 {
                    let mut v = source.clone();
                    v["_id"] = json!({"$oid":format!("{:024x}",0x1000+i)});
                    v["name"] = json!(format!("builder-{i:03}"));
                    d.extra_references.get_mut("builder").unwrap().push(v);
                }
            }
        }
        let (process, s) = Process::start_profile(&core.address, true, true).await;
        let p = preview(&s, &input("server", fixture::SERVER)).await;
        let coverage = p["coverage"].as_array().unwrap();
        assert!(coverage.iter().any(|c| c["visible_scan_complete"] == false));
        assert!(
            coverage
                .iter()
                .all(|c| c["hidden_records_possible"] == true)
        );
        assert!(p["visible_references"].as_array().unwrap().len() <= 32);
        assert!(core.writes().is_empty());
        for r in &core.data.lock().unwrap().requests {
            if r.operation.starts_with("ListFull") {
                assert_eq!(r.params["limit"], 20);
                assert!(r.params["page"].as_u64().unwrap() < 2);
            }
        }
        process.finish(s).await;
    }
}

#[tokio::test]
async fn lost_invalid_denied_redirected_and_mismatched_acknowledgements_never_retry() {
    for (kind, id) in [
        ("stack", STACK),
        ("deployment", DEPLOYMENT),
        ("server", fixture::SERVER),
    ] {
        for reply in [
            WriteReply::Malformed,
            WriteReply::Oversized,
            WriteReply::Denied,
            WriteReply::WrongTarget,
            WriteReply::WrongName,
            WriteReply::Template,
            WriteReply::MissingConfig,
            WriteReply::MissingRequested,
            WriteReply::WrongTrust,
            WriteReply::MissingTrust,
            WriteReply::Redirect,
            WriteReply::Lost,
            WriteReply::WriteDenied,
            WriteReply::AttachDenied,
        ] {
            if kind != "server"
                && matches!(reply, WriteReply::WrongTrust | WriteReply::MissingTrust)
            {
                continue;
            }
            let denied = matches!(
                reply,
                WriteReply::Denied
                    | WriteReply::WriteDenied
                    | WriteReply::AttachDenied
                    | WriteReply::Redirect
            );
            let core = Core::start_deletion().await;
            let (process, s) = Process::start_profile(&core.address, true, true).await;
            let request = input(kind, id);
            let p = preview(&s, &request).await;
            core.data.lock().unwrap().reply = reply;
            let r = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
            assert_eq!(r["outcome"], "submission_unknown");
            assert_eq!(r["definition_removal"]["reported_by_core"], false);
            assert_eq!(
                r["definition_removal"]["observation"],
                if denied {
                    "definition_still_present_at_read"
                } else {
                    "definition_absent_at_direct_read"
                }
            );
            assert_eq!(core.writes().len(), 1);
            let before = core.data.lock().unwrap().requests.len();
            assert_eq!(
                call(&s, "apply_resource_delete", apply(&request, &p))
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
            process
                .finish_excluding(
                    s,
                    &[
                        "deletion-private-archived-definition",
                        "deletion-private-container-environment",
                    ],
                )
                .await;
        }
    }
}

#[tokio::test]
async fn post_delete_read_denial_keeps_acknowledgement_and_unknown_cleanup() {
    let core = Core::start_deletion().await;
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let request = input("deployment", DEPLOYMENT);
    let p = preview(&s, &request).await;
    core.data.lock().unwrap().post_write_read_status = Some(403);
    let r = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
    assert_eq!(r["outcome"], "acknowledged");
    assert_eq!(
        r["definition_removal"]["observation"],
        "observation_unavailable"
    );
    assert!(
        r["known_workloads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["observation"] == "post_attempt_inspection_unavailable")
    );
    assert_eq!(core.writes().len(), 1);
    process.finish(s).await;
}

#[tokio::test]
async fn deadline_keeps_submission_unknown_even_when_later_reads_observe_removal() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        deletion::{self, ApplyInput, PreviewInput},
    };
    use std::time::Duration;
    let core = Core::start_deletion().await;
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
    let request = input("deployment", DEPLOYMENT);
    let p = deletion::preview(
        &client,
        serde_json::from_value::<PreviewInput>(request.clone()).unwrap(),
    )
    .await
    .unwrap();
    core.data.lock().unwrap().reply = WriteReply::Delayed;
    let r = serde_json::to_value(
        deletion::apply(
            &client,
            serde_json::from_value::<ApplyInput>(apply(
                &request,
                &serde_json::to_value(p).unwrap(),
            ))
            .unwrap(),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(r["outcome"], "submission_unknown");
    assert_eq!(
        r["definition_removal"]["observation"],
        "definition_absent_at_direct_read"
    );
    assert_eq!(r["observation_errors"][0]["kind"], "timeout");
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(core.writes().len(), 1);
}

#[tokio::test]
async fn swarm_effects_remain_async_without_server_container_claims() {
    let core = Core::start_deletion().await;
    {
        let mut d = core.data.lock().unwrap();
        d.stack["config"]["server_id"] = json!("");
        d.stack["config"]["swarm_id"] = json!(fixture::SWARM);
    }
    let (process, s) = Process::start_profile(&core.address, true, true).await;
    let request = input("stack", STACK);
    let p = preview(&s, &request).await;
    assert!(p["known_workloads"].as_array().unwrap().is_empty());
    assert!(
        p["evidence_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("asynchronous"))
    );
    let r = data(call(&s, "apply_resource_delete", apply(&request, &p)).await);
    assert_eq!(
        r["definition_removal"]["observation"],
        "definition_absent_at_direct_read"
    );
    assert_eq!(core.data.lock().unwrap().containers.len(), 3);
    assert!(
        core.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.operation != "InspectContainer" && r.operation != "ListContainers")
    );
    process.finish(s).await;
}
