//! Compiled production stdio acceptance; no live Swarm required by CI.
#[path = "support/swarms.rs"]
mod fixture;
mod support;
use fixture::*;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
const TOOLS: [&str; 10] = [
    "list_swarms",
    "get_swarm",
    "list_swarm_nodes",
    "get_swarm_node",
    "list_swarm_services",
    "get_swarm_service",
    "list_swarm_tasks",
    "get_swarm_task",
    "get_swarm_service_logs",
    "diagnose_swarm_service",
];
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
    let s = serde_json::to_string(r).unwrap();
    support::assert_safe(&s);
    for secret in PRIVATE {
        assert!(!s.contains(secret), "sensitive Docker field leaked");
    }
}
fn target() -> Value {
    json!({"target":{"id":SWARM}})
}
fn object(id: &str) -> Value {
    json!({"target":{"id":SWARM},"object":{"id":id}})
}
async fn finish(p: Process, s: Session, c: &Core) {
    c.assert_readonly();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn discovery_and_direct_calls_are_readonly_for_every_profile() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            for name in TOOLS {
                let t = tools.iter().find(|t| t.name == name).unwrap();
                assert_eq!(t.annotations.as_ref().unwrap().read_only_hint, Some(true));
                let reads = c.requests("read").len();
                error(call(&s, name, json!({"unexpected":true})).await);
                assert_eq!(c.requests("read").len(), reads);
            }
            assert!(
                tools
                    .iter()
                    .all(|t| !t.name.contains("prune") && !t.name.contains("terminal"))
            );
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn list_inspect_and_exact_logs_project_only_safe_fields_and_distinguish_sources() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let list = data(call(&s, "list_swarms", json!({"page_size":1})).await);
    assert_eq!(list["items"][0]["id"], SWARM);
    assert_eq!(list["items"][0]["infrastructure_error_present"], true);
    assert!(list["next_page"].is_null());
    let swarm = data(call(&s, "get_swarm", json!({"name":"fixture-swarm"})).await);
    assert_eq!(swarm["source"], "core_cache");
    assert_eq!(
        swarm["data"]["docker_swarm_id"],
        "iiiiiiiiiiiiiiiiiiiiiiiii"
    );
    for name in [
        "list_swarm_nodes",
        "list_swarm_services",
        "list_swarm_tasks",
    ] {
        let v = data(call(&s, name, target()).await);
        assert_eq!(v["source"], "core_cache");
        assert!(!v["data"]["scan_incomplete"].as_bool().unwrap());
        assert!(v["evidence_limit"].as_str().unwrap().contains("suppressed"));
    }
    for (name, id) in [
        ("get_swarm_node", WORKER),
        ("get_swarm_service", SERVICE),
        ("get_swarm_task", FAILED),
    ] {
        let v = data(call(&s, name, object(id)).await);
        assert_eq!(v["source"], "fresh_inspect");
        assert_eq!(v["data"]["id"], id);
    }
    let n = data(
        call(
            &s,
            "get_swarm_node",
            json!({"target":{"id":SWARM},"object":{"name":"worker"}}),
        )
        .await,
    );
    assert_eq!(n["data"]["availability"], "drain");
    let v = data(call(&s, "get_swarm_task", object(FAILED)).await);
    assert_eq!(v["data"]["failure_reason"], "nonzero_exit");
    assert_eq!(v["data"]["exit_code"], 23);
    assert_eq!(v["data"]["replica_evidence"], "historical");
    let logs = data(
        call(
            &s,
            "get_swarm_service_logs",
            json!({"target":{"id":SWARM},"service":{"name":"fixture-web"},"tail":3,"max_bytes":64}),
        )
        .await,
    );
    assert_eq!(logs["target"]["kind"], "swarm_service");
    assert_eq!(logs["target"]["services"], json!([SERVICE]));
    assert!(logs["returned_bytes"].as_u64().unwrap() <= 64);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn diagnosis_traces_failed_and_pending_tasks_without_counting_historical_running_replicas() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "diagnose_swarm_service", object(SERVICE)).await);
    let d = &v["data"];
    assert_eq!(d["cached_current_running"], 1);
    assert_eq!(d["cached_pending"], 1);
    assert_eq!(d["historical_tasks"], 2);
    assert_eq!(d["under_replicated_in_cache"], true);
    assert!(
        d["fresh_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == PENDING
                && t["failure_reason"] == "placement_constraints_unsatisfied")
    );
    assert!(
        d["fresh_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == FAILED && t["node_id"] == WORKER && t["exit_code"] == 23)
    );
    assert!(
        d["fresh_nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == WORKER && n["availability"] == "drain")
    );
    assert!(d["gaps"].as_array().unwrap().is_empty());
    assert!(c.requests("read").len() <= 19);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn core_read_inspect_and_logs_denials_remain_distinct_without_privileged_fallback() {
    for denied in [
        "ListSwarms",
        "GetSwarm",
        "ListSwarmNodes",
        "InspectSwarm",
        "InspectSwarmNode",
        "InspectSwarmService",
        "InspectSwarmTask",
        "GetSwarmServiceLog",
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().denied.insert(denied.into());
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        let (tool, args) = match denied {
            "ListSwarms" => ("list_swarms", json!({})),
            "GetSwarm" | "InspectSwarm" => ("get_swarm", json!({"id":SWARM})),
            "ListSwarmNodes" => ("list_swarm_nodes", target()),
            "InspectSwarmNode" => ("get_swarm_node", object(NODE)),
            "InspectSwarmTask" => ("get_swarm_task", object(FAILED)),
            "GetSwarmServiceLog" => (
                "get_swarm_service_logs",
                json!({"target":{"id":SWARM},"service":{"id":SERVICE}}),
            ),
            _ => ("get_swarm_service", object(SERVICE)),
        };
        assert_eq!(
            error(call(&s, tool, args).await)["kind"],
            "permission_denied"
        );
        assert_eq!(
            c.requests("read")
                .iter()
                .filter(|q| q.operation == denied)
                .count(),
            1
        );
        finish(p, s, &c).await;
    }
    // Logs requires no inspect permission; inspect requires no log permission.
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.denied.insert("InspectSwarmService".into());
        d.denied.insert("InspectSwarm".into());
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    data(
        call(
            &s,
            "get_swarm_service_logs",
            json!({"target":{"id":SWARM},"service":{"id":SERVICE}}),
        )
        .await,
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn invalid_missing_ambiguous_and_stale_selectors_never_expand_scope() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    for selector in [
        json!({}),
        json!({"id":"cccc"}),
        json!({"name":"*"}),
        json!({"name":"-flag"}),
        json!({"id":SERVICE,"name":"fixture-web"}),
        json!({"id":null,"name":"fixture-web"}),
    ] {
        let reads = c.requests("read").len();
        assert_eq!(
            error(
                call(
                    &s,
                    "get_swarm_service",
                    json!({"target":{"id":SWARM},"object":selector})
                )
                .await
            )["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.requests("read").len(), reads);
    }
    for tool in [
        "get_swarm_service",
        "diagnose_swarm_service",
        "list_swarm_tasks",
        "get_swarm_service_logs",
    ] {
        let mut v = target();
        v[if tool == "list_swarm_tasks" || tool == "get_swarm_service_logs" {
            "service"
        } else {
            "object"
        }] = json!({"name":"missing"});
        assert_eq!(error(call(&s, tool, v).await)["kind"], "not_found");
    }
    let reads = c.requests("read").len();
    assert_eq!(
        error(
            call(
                &s,
                "list_swarm_tasks",
                json!({"target":{"id":SWARM},"service":null})
            )
            .await
        )["kind"],
        "invalid_arguments"
    );
    assert_eq!(c.requests("read").len(), reads);
    assert!(
        c.requests("read")
            .iter()
            .all(|r| !r.operation.starts_with("Inspect")
                && r.operation != "GetSwarmServiceLog"
                && r.operation != "ListSwarmTasks")
    );
    {
        let mut d = c.data.lock().unwrap();
        let mut other = d.services[1].clone();
        other["Name"] = json!("fixture-web");
        other["ID"] = json!("zzzzzzzzzzzzzzzzzzzzzzzzz");
        d.services.push(other);
    }
    assert_eq!(
        error(
            call(
                &s,
                "get_swarm_service",
                json!({"target":{"id":SWARM},"object":{"name":"fixture-web"}})
            )
            .await
        )["kind"],
        "ambiguous_target"
    );
    {
        c.data.lock().unwrap().details.get_mut(SERVICE).unwrap()["ID"] = json!(OTHER_SERVICE);
    }
    assert_eq!(
        error(call(&s, "get_swarm_service", object(SERVICE)).await)["kind"],
        "invalid_response"
    );
    {
        c.data.lock().unwrap().full["_id"]["$oid"] = json!("000000000000000000000099");
    }
    assert_eq!(
        error(call(&s, "get_swarm_service", object(SERVICE)).await)["kind"],
        "invalid_response"
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn bounded_pages_clipped_caches_oversized_bodies_and_unsupported_reads_are_explicit() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.services = (0..600)
            .map(|n| service(&format!("{:025}", n), &format!("service-{n}")))
            .collect();
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(
        call(
            &s,
            "list_swarm_services",
            json!({"target":{"id":SWARM},"page_size":100,"page":5}),
        )
        .await,
    );
    assert_eq!(v["data"]["items"].as_array().unwrap().len(), 12);
    assert_eq!(v["data"]["scan_incomplete"], true);
    assert!(v["data"]["next_page"].is_null());
    assert_eq!(
        error(call(&s, "get_swarm_service", object("0000000000000000000000000")).await)["kind"],
        "incomplete_resolution"
    );
    let reads = c.requests("read").len();
    for v in [
        json!({"target":{"id":SWARM},"page_size":0}),
        json!({"target":{"id":SWARM},"page":u64::MAX}),
        json!({"target":{"id":SWARM},"page":6,"page_size":100}),
    ] {
        error(call(&s, "list_swarm_services", v).await);
    }
    assert_eq!(c.requests("read").len(), reads);
    c.data.lock().unwrap().oversized = Some("ListSwarmNodes".into());
    assert_eq!(
        error(call(&s, "list_swarm_nodes", target()).await)["kind"],
        "response_too_large"
    );
    c.data
        .lock()
        .unwrap()
        .unsupported
        .insert("ListSwarmTasks".into());
    let e = error(call(&s, "list_swarm_tasks", target()).await);
    assert_eq!(e["kind"], "unsupported_target");
    assert_eq!(e["status"], 501);
    finish(p, s, &c).await;
}
#[tokio::test]
async fn empty_cache_and_partial_fresh_coverage_never_claim_live_health() {
    let c = Core::start().await;
    {
        let mut d = c.data.lock().unwrap();
        d.tasks.clear();
        d.nodes.clear();
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "list_swarm_tasks", target()).await);
    assert!(v["data"]["items"].as_array().unwrap().is_empty());
    assert!(
        v["evidence_limit"]
            .as_str()
            .unwrap()
            .contains("missing cache")
    );
    finish(p, s, &c).await;
    let c = Core::start().await;
    c.data
        .lock()
        .unwrap()
        .denied
        .insert("InspectSwarmTask".into());
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "diagnose_swarm_service", object(SERVICE)).await);
    assert_eq!(v["data"]["fresh_task_coverage_incomplete"], true);
    assert_eq!(v["data"]["gaps"][0]["kind"], "permission_denied");
    finish(p, s, &c).await;
}

#[tokio::test]
async fn name_drift_and_duplicate_cached_task_ids_are_rejected() {
    let c = Core::start().await;
    c.data.lock().unwrap().details.get_mut(SERVICE).unwrap()["Spec"]["Name"] =
        json!("renamed-service");
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    assert_eq!(
        error(
            call(
                &s,
                "get_swarm_service",
                json!({"target":{"id":SWARM},"object":{"name":"fixture-web"}})
            )
            .await
        )["kind"],
        "invalid_response"
    );
    {
        let mut d = c.data.lock().unwrap();
        let duplicate = d.tasks[0].clone();
        d.tasks.push(duplicate);
    }
    assert_eq!(
        error(call(&s, "list_swarm_tasks", target()).await)["kind"],
        "invalid_response"
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn unknown_task_state_is_not_reported_as_pending_or_complete_replica_evidence() {
    let c = Core::start().await;
    c.data.lock().unwrap().tasks[0]["State"] = Value::Null;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "diagnose_swarm_service", object(SERVICE)).await);
    assert_eq!(v["data"]["cached_pending"], 1);
    assert_eq!(v["data"]["cached_unclassified_tasks"], 1);
    assert!(v["data"]["under_replicated_in_cache"].is_null());
    finish(p, s, &c).await;
}
