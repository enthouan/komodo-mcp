//! Real production stdio; pinned-wire faults are deterministic, not a live Swarm.
#[path = "support/configuration.rs"]
mod fixture;
mod support;
use fixture::{Core, WriteReply, scaling::*};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};
use support::operations::{Process, Session, call};

fn safe(r: &CallToolResult) {
    let text = serde_json::to_string(r).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(
            !text.contains(secret),
            "private definition/Docker field escaped"
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
fn input(replicas: i32) -> Value {
    json!({"target":{"id":DEPLOYMENT},"replicas":replicas})
}
fn apply(replicas: i32, p: &Value) -> Value {
    let mut v = input(replicas);
    v["preview_id"] = p["preview_id"].clone();
    v
}
fn executions(c: &Core) -> usize {
    c.data
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.path == "execute")
        .count()
}
async fn save(s: &Session, n: i32) -> Value {
    let p = data(call(s, "preview_deployment_scale", input(n)).await);
    data(call(s, "apply_deployment_scale", apply(n, &p)).await)
}
async fn review(s: &Session, n: i32) -> Value {
    data(call(s, "review_deployment_scale_deploy", input(n)).await)
}
async fn observe(s: &Session, n: i32) -> Value {
    let mut a = input(n);
    a["timeout_seconds"] = json!(1);
    data(call(s, "observe_deployment_scale", a).await)
}
async fn finish(p: Process, s: Session, c: &Core) {
    assert!(!c.data.lock().unwrap().bad_credentials);
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn independent_profiles_restrict_discovery_and_direct_calls_before_any_request() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start_scaling().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            for (name, allowed) in [
                ("preview_deployment_scale", edit),
                ("apply_deployment_scale", edit),
                ("review_deployment_scale_deploy", operate),
                ("deploy_scaled_deployment", operate),
                ("observe_deployment_scale", true),
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), allowed);
                if !allowed {
                    let before = c.data.lock().unwrap().requests.len();
                    assert!(
                        s.call_tool(
                            CallToolRequestParams::new(name)
                                .with_arguments(input(1).as_object().unwrap().clone())
                        )
                        .await
                        .is_err()
                    );
                    assert_eq!(c.data.lock().unwrap().requests.len(), before);
                }
            }
            assert!(c.writes().is_empty());
            assert_eq!(executions(&c), 0);
            finish(p, s, &c).await;
        }
    }
}

#[tokio::test]
async fn scale_up_down_and_zero_preserve_every_other_field_and_separate_all_stages() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for n in [3, 1, 0] {
        let before = c.data.lock().unwrap().deployment["config"].clone();
        let writes = c.writes().len();
        let runs = executions(&c);
        let saved = save(&s, n).await;
        assert_eq!(saved["configuration"]["outcome"], "saved");
        assert_eq!(saved["configuration"]["saved_configuration_verified"], true);
        assert_eq!(saved["deployment_submitted"], false);
        assert_eq!(executions(&c), runs);
        let after = c.data.lock().unwrap().deployment["config"].clone();
        for (k, v) in before.as_object().unwrap() {
            if k != "replicas" && k != "extra_args" {
                assert_eq!(&after[k], v, "unrelated field changed: {k}");
            }
        }
        assert_eq!(after["extra_args"][0], before["extra_args"][0]);
        assert_eq!(after["extra_args"][2], before["extra_args"][2]);
        assert_eq!(after["extra_args"][1], format!("--replicas={n}"));
        assert_eq!(c.writes().len(), writes + 1);
        assert_eq!(
            c.writes().last().unwrap().params["config"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        let reviewed = review(&s, n).await;
        assert_eq!(reviewed["writes_submitted"], 0);
        assert_eq!(executions(&c), runs);
        let deployed = data(call(&s, "deploy_scaled_deployment", apply(n, &reviewed)).await);
        assert_eq!(deployed["receipt"]["submission"], "accepted");
        assert_eq!(deployed["receipt"]["update_id"], UPDATE);
        assert_eq!(deployed["convergence_observed"], false);
        assert_eq!(executions(&c), runs + 1);
        let observed = observe(&s, n).await;
        assert_eq!(observed["end"], "matching_observations");
        assert_eq!(observed["latest"]["current_running"], n);
        assert_eq!(observed["latest"]["historical_tasks"], 1);
        assert_eq!(observed["latest"]["other_service_tasks_ignored"], 1);
        assert_eq!(observed["latest"]["application_health"], "not_verified");
        error(call(&s, "deploy_scaled_deployment", apply(n, &reviewed)).await);
        assert_eq!(executions(&c), runs + 1);
    }
    finish(p, s, &c).await;
}

#[tokio::test]
async fn config_only_save_and_operate_only_deploy_survive_restart_without_persisting_reviews() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    save(&s, 2).await;
    assert_eq!(executions(&c), 0);
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, true, false).await;
    let reviewed = review(&s, 2).await;
    data(call(&s, "deploy_scaled_deployment", apply(2, &reviewed)).await);
    assert_eq!(executions(&c), 1);
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let recovered = data(call(&s, "get_update", json!({"id":UPDATE})).await);
    assert_eq!(recovered["outcome"], "succeeded");
    data(
        call(
            &s,
            "wait_for_update",
            json!({"id":UPDATE,"timeout_seconds":1}),
        )
        .await,
    );
    assert_eq!(executions(&c), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn denied_failed_and_lost_deployments_remain_partial_and_are_never_replayed() {
    for reply in [
        ExecuteReply::Denied,
        ExecuteReply::Failed,
        ExecuteReply::Lost,
    ] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        save(&s, 2).await;
        let reviewed = review(&s, 2).await;
        c.data
            .lock()
            .unwrap()
            .scaling
            .as_mut()
            .unwrap()
            .execute_reply = reply;
        let r = data(call(&s, "deploy_scaled_deployment", apply(2, &reviewed)).await);
        assert_eq!(r["partial_outcome"], true);
        assert_eq!(r["saved_definition_verified"], true);
        assert_eq!(c.data.lock().unwrap().deployment["config"]["replicas"], 2);
        assert_eq!(executions(&c), 1);
        match reply {
            ExecuteReply::Denied => assert_eq!(r["receipt"]["submission"], "rejected"),
            ExecuteReply::Failed => {
                assert_eq!(r["receipt"]["update_id"], UPDATE);
                assert_eq!(r["receipt"]["latest_update"]["outcome"], "failed");
            }
            _ => {
                assert_eq!(r["receipt"]["submission"], "unknown");
                assert!(r["receipt"]["update_id"].is_null());
            }
        }
        error(call(&s, "deploy_scaled_deployment", apply(2, &reviewed)).await);
        assert_eq!(executions(&c), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn raw_secret_only_definition_reference_and_target_drift_invalidate_single_use_save() {
    for change in ["environment", "extension", "swarm", "target", "build"] {
        let c = Core::start_scaling().await;
        if change == "build" {
            c.data.lock().unwrap().deployment["config"]["image"] = json!({"type":"Build","params":{"build_id":fixture::BUILD,"version":{"major":0,"minor":0,"patch":0}}});
        }
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let pview = data(call(&s, "preview_deployment_scale", input(2)).await);
        {
            let mut d = c.data.lock().unwrap();
            match change {
                "environment" => {
                    d.deployment["config"]["environment"] =
                        json!("CHANGED=scale-environment-private\n")
                }
                "extension" => d.deployment["config"]["future_secret"] = json!(PRIVATE[0]),
                "swarm" => d.swarm["config"]["labels"]["private"] = json!("changed"),
                "target" => d.deployment["config"]["server_id"] = json!("000000000000000000000099"),
                _ => d.build["config"]["build_args"] = json!("TOKEN=scale-reference-private\n"),
            };
        }
        let e = error(call(&s, "apply_deployment_scale", apply(2, &pview)).await);
        assert!(
            matches!(
                e["kind"].as_str(),
                Some("stale_preview" | "invalid_response")
            ),
            "{e}"
        );
        assert!(c.writes().is_empty());
        assert_eq!(executions(&c), 0);
        error(call(&s, "apply_deployment_scale", apply(2, &pview)).await);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn saved_config_is_checked_again_inside_the_existing_final_deploy_resolution() {
    for late in [false, true] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let reviewed = review(&s, 1).await;
        if late {
            c.data
                .lock()
                .unwrap()
                .scaling
                .as_mut()
                .unwrap()
                .drift_after_runtime = true;
        } else {
            c.data.lock().unwrap().deployment["config"]["environment"] =
                json!("CHANGED=scale-environment-private\n");
        }
        let r = data(call(&s, "deploy_scaled_deployment", apply(1, &reviewed)).await);
        assert_eq!(r["partial_outcome"], true);
        assert_eq!(r["not_submitted_error"]["kind"], "stale_preview");
        assert!(r["receipt"].is_null());
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn noop_and_supported_replica_forms_preserve_order_without_extra_writes() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, false, true).await;
    let r = save(&s, 1).await;
    assert_eq!(r["configuration"]["outcome"], "no_op");
    assert_eq!(r["configuration"]["saved_configuration_verified"], true);
    assert!(c.writes().is_empty());
    finish(p, s, &c).await;
    for replica in [
        json!(["--replicas=1"]),
        json!(["--replicas 1"]),
        json!(["--replicas", "1"]),
        json!([]),
    ] {
        let c = Core::start_scaling().await;
        let mut args = vec![json!("--constraint=node.role==worker")];
        args.extend(replica.as_array().unwrap().clone());
        args.push(json!("--label=preserve"));
        args.push(json!("--replicas-max-per-node=1"));
        args.push(json!("--label='message --replicas=999'"));
        c.data.lock().unwrap().deployment["config"]["extra_args"] = json!(args);
        let (p, s) = Process::start_profile(&c.address, false, true).await;
        save(&s, 2).await;
        let args = c.data.lock().unwrap().deployment["config"]["extra_args"].clone();
        assert_eq!(args[0], "--constraint=node.role==worker");
        assert_eq!(
            args.as_array()
                .unwrap()
                .iter()
                .filter(|a| a.as_str().unwrap().starts_with("--replicas="))
                .count(),
            1
        );
        assert!(
            args.as_array()
                .unwrap()
                .contains(&json!("--label=preserve"))
        );
        assert!(
            args.as_array()
                .unwrap()
                .contains(&json!("--replicas-max-per-node=1"))
        );
        assert!(
            args.as_array()
                .unwrap()
                .contains(&json!("--label='message --replicas=999'"))
        );
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn complex_replica_mode_name_and_shell_forms_are_rejected_without_rewriting() {
    for args in [
        json!(["--replicas=1 --label=x"]),
        json!(["--replicas=1", "--replicas=1"]),
        json!(["--mode=global"]),
        json!(["--name=foreign"]),
        json!(["--replicas=$(id)"]),
        json!(["--label=x; echo forbidden"]),
    ] {
        let c = Core::start_scaling().await;
        c.data.lock().unwrap().deployment["config"]["extra_args"] = args.clone();
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        error(call(&s, "preview_deployment_scale", input(0)).await);
        assert_eq!(
            c.data.lock().unwrap().deployment["config"]["extra_args"],
            args
        );
        assert!(c.writes().is_empty());
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn pending_failed_placement_timeout_and_task_health_are_separate_from_deploy_success() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    save(&s, 3).await;
    let r = review(&s, 3).await;
    data(call(&s, "deploy_scaled_deployment", apply(3, &r)).await);
    {
        let mut d = c.data.lock().unwrap();
        let s = d.scaling.as_mut().unwrap();
        set_runtime(s, 3, 1, false);
        let pending = task(10, SERVICE, "pending", "running");
        let failed = task(11, SERVICE, "failed", "running");
        for t in [&pending, &failed] {
            s.details.insert(t["ID"].as_str().unwrap().into(),json!({"ID":t["ID"],"ServiceID":SERVICE,"DesiredState":"running","Status":{"State":t["State"],"Err":format!("no suitable node (scheduling constraints not satisfied) {}",PRIVATE[2])}}));
        }
        s.tasks.extend([pending, failed]);
    }
    let r = observe(&s, 3).await;
    assert_ne!(r["end"], "matching_observations");
    assert_eq!(r["latest"]["pending_current"], 1);
    assert_eq!(r["latest"]["failed_current"], 1);
    assert_eq!(r["latest"]["core_task_health"], "unhealthy");
    assert!(
        r["latest"]["fresh_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["failure_reason"] == "placement_constraints_unsatisfied")
    );
    {
        let mut d = c.data.lock().unwrap();
        set_runtime(d.scaling.as_mut().unwrap(), 3, 3, false);
    }
    let r = observe(&s, 3).await;
    assert_eq!(r["end"], "matching_observations");
    assert_eq!(r["latest"]["core_task_health"], "unhealthy");
    assert_eq!(r["latest"]["application_health"], "not_verified");
    assert_eq!(executions(&c), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn zero_needs_an_existing_service_and_cannot_ignore_running_shutdown_tasks() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    save(&s, 0).await;
    let r = review(&s, 0).await;
    data(call(&s, "deploy_scaled_deployment", apply(0, &r)).await);
    {
        let mut d = c.data.lock().unwrap();
        let s = d.scaling.as_mut().unwrap();
        s.tasks.push(task(20, SERVICE, "running", "shutdown"));
        s.details.insert(format!("t{:024}",20),json!({"ID":format!("t{:024}",20),"ServiceID":SERVICE,"DesiredState":"shutdown","Status":{"State":"running"}}));
    }
    let r = observe(&s, 0).await;
    assert_ne!(r["end"], "matching_observations");
    assert_eq!(r["latest"]["terminating_running"], 1);
    c.data
        .lock()
        .unwrap()
        .scaling
        .as_mut()
        .unwrap()
        .services
        .clear();
    let r = observe(&s, 0).await;
    assert_eq!(r["end"], "read_failed");
    assert_eq!(r["observation_error"]["kind"], "not_found");
    finish(p, s, &c).await;
}

#[tokio::test]
async fn permission_missing_target_stack_mode_and_oversized_evidence_never_broaden() {
    for denied in [
        "GetDeployment",
        "GetSwarm",
        "ListSwarmServices",
        "InspectSwarmService",
        "InspectSwarmTask",
    ] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        c.data
            .lock()
            .unwrap()
            .scaling
            .as_mut()
            .unwrap()
            .denied
            .insert(denied.into());
        let r = observe(&s, 1).await;
        assert_ne!(r["end"], "matching_observations");
        if denied == "InspectSwarmTask" {
            assert_eq!(r["latest"]["gaps"][0]["kind"], "permission_denied");
        } else {
            assert_eq!(r["observation_error"]["kind"], "permission_denied");
        }
        assert!(c.writes().is_empty());
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
    for kind in ["server", "stack", "global", "oversized"] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        {
            let mut d = c.data.lock().unwrap();
            match kind {
                "server" => d.deployment["config"]["swarm_id"] = json!(""),
                "stack" => {
                    d.scaling.as_mut().unwrap().service["Spec"]["Labels"]["com.docker.stack.namespace"] =
                        json!("owned-by-stack")
                }
                "global" => d.scaling.as_mut().unwrap().service["Mode"] = json!("Global"),
                _ => d.scaling.as_mut().unwrap().oversized = Some("ListSwarmTasks".into()),
            };
        }
        let r = observe(&s, 0).await;
        assert_ne!(r["end"], "matching_observations");
        if kind != "oversized" {
            error(call(&s, "preview_deployment_scale", input(2)).await);
        }
        assert!(c.writes().is_empty());
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn lost_or_unreadable_save_acknowledgements_preserve_uncertainty_without_execution() {
    for reply in [
        WriteReply::Lost,
        WriteReply::Malformed,
        WriteReply::WriteDenied,
    ] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let reviewed = data(call(&s, "preview_deployment_scale", input(2)).await);
        c.data.lock().unwrap().reply = reply;
        let r = data(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
        assert_eq!(r["configuration"]["outcome"], "submission_unknown");
        assert_eq!(r["configuration"]["saved_configuration_verified"], false);
        assert_eq!(c.writes().len(), 1);
        assert_eq!(executions(&c), 0);
        error(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
        assert_eq!(c.writes().len(), 1);
        finish(p, s, &c).await;
    }
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let reviewed = data(call(&s, "preview_deployment_scale", input(2)).await);
    c.data.lock().unwrap().post_write_read_status = Some(403);
    let r = data(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
    assert_eq!(r["configuration"]["outcome"], "saved");
    assert_eq!(r["configuration"]["saved_configuration_verified"], false);
    assert_eq!(c.writes().len(), 1);
    assert_eq!(executions(&c), 0);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn service_replacement_and_spec_changes_cannot_reuse_a_deploy_review() {
    for replace in [false, true] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let reviewed = review(&s, 1).await;
        {
            let mut d = c.data.lock().unwrap();
            let state = d.scaling.as_mut().unwrap();
            if replace {
                set_runtime(state, 1, 1, true);
            } else {
                state.service["Version"]["Index"] = json!(10);
            }
        }
        let r = data(call(&s, "deploy_scaled_deployment", apply(1, &reviewed)).await);
        assert_eq!(r["not_submitted_error"]["kind"], "stale_preview");
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    save(&s, 2).await;
    c.data
        .lock()
        .unwrap()
        .scaling
        .as_mut()
        .unwrap()
        .swap_after_tasks = Some(2);
    let r = observe(&s, 2).await;
    assert_eq!(r["end"], "identity_changed");
    assert_eq!(r["observation_error"]["kind"], "stale_preview");
    assert_eq!(executions(&c), 0);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn private_review_binding_rejects_proposal_cross_family_and_restart_replay() {
    let c = Core::start_scaling().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let reviewed = data(call(&s, "preview_deployment_scale", input(2)).await);
    error(call(&s, "apply_deployment_scale", apply(3, &reviewed)).await);
    error(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
    let ordinary = data(
        call(
            &s,
            "preview_deployment_update",
            json!({"target":{"id":DEPLOYMENT},"patch":{"replicas":2}}),
        )
        .await,
    );
    error(call(&s, "apply_deployment_scale", apply(2, &ordinary)).await);
    let reviewed = data(call(&s, "preview_deployment_scale", input(2)).await);
    finish(p, s, &c).await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    error(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
    let reads = c.data.lock().unwrap().requests.len();
    for value in [
        json!({"target":{"id":DEPLOYMENT},"replicas":-1}),
        json!({"target":{"id":DEPLOYMENT},"replicas":1001}),
        json!({"target":{"id":DEPLOYMENT},"replicas":null}),
        json!({"target":{},"replicas":1}),
        json!({"target":{"id":SERVICE},"replicas":1}),
        json!({"target":{"id":DEPLOYMENT,"name":"fixture-deployment"},"replicas":1}),
        json!({"target":{"name":"x".repeat(129)},"replicas":1}),
    ] {
        error(call(&s, "preview_deployment_scale", value).await);
    }
    assert_eq!(c.data.lock().unwrap().requests.len(), reads);
    assert!(c.writes().is_empty());
    assert_eq!(executions(&c), 0);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn task_scan_fresh_sampling_unknown_counts_and_private_baseline_limits_are_honest() {
    for case in ["sampling", "scan", "counts", "private"] {
        let c = Core::start_scaling().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        {
            let mut d = c.data.lock().unwrap();
            if case == "private" {
                d.deployment["config"]["future_private"] = json!("q".repeat(256 * 1024));
            } else {
                let state = d.scaling.as_mut().unwrap();
                match case {
                    "sampling" => set_runtime(state, 20, 20, true),
                    "scan" => state
                        .tasks
                        .extend((1000..1513).map(|n| task(n, OLD, "shutdown", "shutdown"))),
                    _ => state.services[0]["RunningTasks"] = Value::Null,
                };
            }
        }
        if case == "private" {
            let r = error(call(&s, "preview_deployment_scale", input(2)).await);
            assert_eq!(r["kind"], "incomplete_resolution");
        } else {
            let r = observe(&s, if case == "sampling" { 20 } else { 1 }).await;
            assert_ne!(r["end"], "matching_observations");
            if case == "sampling" {
                assert_eq!(r["latest"]["fresh_tasks"].as_array().unwrap().len(), 16);
                assert_eq!(r["latest"]["fresh_task_coverage_incomplete"], true);
            } else if case == "scan" {
                assert_eq!(r["latest"]["scan_incomplete"], true);
            }
        }
        assert!(c.data.lock().unwrap().requests.len() < 100);
        assert!(c.writes().is_empty());
        assert_eq!(executions(&c), 0);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn shared_scaling_entry_points_enforce_grants_without_router_or_core_io() {
    use komodo_mcp::{
        capabilities::CapabilityPolicy,
        client::{CoreClient, RequestPolicy},
        config::Config,
        error::ErrorKind,
        scaling,
    };
    for (operate, edit) in [(false, false), (false, true), (true, false)] {
        let c = Core::start_scaling().await;
        let client = CoreClient::new(
            Config::parse(&c.address, support::KEY, support::SECRET)
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
            RequestPolicy::default(),
        )
        .unwrap();
        if !edit {
            let args: scaling::ScaleInput = serde_json::from_value(input(1)).unwrap();
            assert_eq!(
                scaling::preview(&client, args).await.err().unwrap().kind,
                ErrorKind::CapabilityDisabled
            );
            let args: scaling::ApplyInput = serde_json::from_value(
                json!({"target":{"id":DEPLOYMENT},"replicas":1,"preview_id":"absent"}),
            )
            .unwrap();
            assert_eq!(
                scaling::save(&client, args).await.err().unwrap().kind,
                ErrorKind::CapabilityDisabled
            );
        }
        if !operate {
            let args: scaling::ScaleInput = serde_json::from_value(input(1)).unwrap();
            assert_eq!(
                scaling::review_deploy(&client, args)
                    .await
                    .err()
                    .unwrap()
                    .kind,
                ErrorKind::CapabilityDisabled
            );
            let args: scaling::ApplyInput = serde_json::from_value(
                json!({"target":{"id":DEPLOYMENT},"replicas":1,"preview_id":"absent"}),
            )
            .unwrap();
            assert_eq!(
                scaling::deploy(&client, args).await.err().unwrap().kind,
                ErrorKind::CapabilityDisabled
            );
        }
        assert!(c.data.lock().unwrap().requests.is_empty());
    }
}

#[tokio::test]
async fn credential_bearing_display_names_are_omitted_at_every_scale_stage() {
    let c = Core::start_scaling().await;
    c.data.lock().unwrap().deployment["name"] =
        json!(format!("https://display-user:{}@host", PRIVATE[2]));
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let reviewed = data(call(&s, "preview_deployment_scale", input(2)).await);
    assert_eq!(reviewed["target"]["name"], "[omitted]");
    assert_eq!(reviewed["configuration"]["resource"]["name"], "[omitted]");
    let saved = data(call(&s, "apply_deployment_scale", apply(2, &reviewed)).await);
    assert_eq!(saved["configuration"]["resource"]["name"], "[omitted]");
    let reviewed = review(&s, 2).await;
    let deployed = data(call(&s, "deploy_scaled_deployment", apply(2, &reviewed)).await);
    assert_eq!(deployed["receipt"]["resource"]["name"], "[omitted]");
    let observed = observe(&s, 2).await;
    assert_eq!(observed["target"]["name"], "[omitted]");
    finish(p, s, &c).await;
}
