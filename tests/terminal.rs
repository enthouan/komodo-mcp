//! Production stdio protocol tests. The fixture does not execute a real shell.
#[path = "support/terminal.rs"]
mod fixture;
mod support;
use fixture::{Core, Create, DEPLOYMENT, PRIVATE, SERVER, STACK, Stream, cid};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::Peer,
};
use serde_json::{Value, json};
use std::time::Duration;
use support::operations::{Process, Session, call};
fn input() -> Value {
    json!({"target":{"kind":"server","target":{"id":SERVER}},"command":"printf terminal-command-marker","observation_seconds":5})
}
fn safe(r: &CallToolResult) {
    let text = serde_json::to_string(r).unwrap();
    support::assert_safe(&text);
    for secret in [PRIVATE, "application-private"] {
        assert!(!text.contains(secret));
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
async fn start(c: &Core) -> (Process, Session) {
    Process::start_profile_terminal(&c.address, true, false, false, true).await
}
async fn run(s: &Session, v: Value) -> Value {
    data(call(s, "execute_terminal", v).await)
}
async fn finish(p: Process, s: Session, c: &Core) {
    {
        let d = c.data.lock().unwrap();
        assert!(!d.bad_credentials);
        assert!(d.requests.iter().all(|r| r.path != "unexpected"));
    }
    p.finish_excluding(
        s,
        &[
            PRIVATE,
            "terminal-command-marker",
            "harmless-output",
            "nonzero-output",
            "application-private",
            "partial-private-output",
        ],
    )
    .await;
}
fn mutations(c: &Core) -> usize {
    c.requests("write").len() + c.requests("terminal").len()
}
async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn sixteen_profiles_keep_terminal_and_prune_independent_and_deny_hidden_calls_without_io() {
    for operate in [false, true] {
        for edit in [false, true] {
            for prune in [false, true] {
                for terminal in [false, true] {
                    let c = Core::start().await;
                    let (p, s) =
                        Process::start_profile_terminal(&c.address, operate, edit, prune, terminal)
                            .await;
                    let tools = s.list_tools(None).await.unwrap().tools;
                    assert_eq!(
                        tools.len(),
                        64 + 20 * usize::from(operate)
                            + 55 * usize::from(edit)
                            + 4 * usize::from(operate && edit)
                            + 2 * usize::from(operate && prune)
                            + usize::from(operate && terminal)
                    );
                    assert_eq!(
                        tools.iter().any(|t| t.name == "execute_terminal"),
                        operate && terminal
                    );
                    assert_eq!(
                        tools.iter().any(|t| t.name == "apply_docker_prune"),
                        operate && prune
                    );
                    let before = c.data.lock().unwrap().requests.len();
                    if operate && terminal {
                        assert_eq!(
                            error(call(&s, "execute_terminal", json!({})).await)["kind"],
                            "invalid_arguments"
                        );
                    } else {
                        assert!(
                            s.call_tool(
                                CallToolRequestParams::new("execute_terminal")
                                    .with_arguments(input().as_object().unwrap().clone())
                            )
                            .await
                            .is_err()
                        );
                    }
                    assert_eq!(c.data.lock().unwrap().requests.len(), before);
                    finish(p, s, &c).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn four_exact_targets_use_original_inspection_authority_and_only_one_command() {
    for target in [
        json!({"kind":"server","target":{"id":SERVER}}),
        json!({"kind":"container","server":{"id":SERVER},"container":{"name":"fixture-web"}}),
        json!({"kind":"stack","target":{"id":STACK},"service":"web"}),
        json!({"kind":"deployment","target":{"id":DEPLOYMENT}}),
    ] {
        let c = Core::start().await;
        let (p, s) = start(&c).await;
        let mut v = input();
        v["target"] = target.clone();
        let r = run(&s, v).await;
        assert_eq!(r["observation"]["reported_exit_code"], 0);
        assert_eq!(r["cleanup"]["state"], "absence_observed");
        assert_eq!(r["attempts"], json!({"create":1,"command":1,"delete":1}));
        assert!(c.data.lock().unwrap().records.is_empty());
        let writes = c.requests("write");
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].operation, "CreateTerminal");
        assert_eq!(writes[0].params["recreate"], "Never");
        assert!(
            writes[0].params["name"]
                .as_str()
                .unwrap()
                .starts_with("mcp-")
        );
        assert!(
            writes[0].params["command"]
                .as_str()
                .unwrap()
                .starts_with("timeout -k 1 90 ")
        );
        let command = c.requests("terminal");
        assert_eq!(command.len(), 1);
        assert!(command[0].params["init"].is_null());
        assert!(
            command[0].params["command"]
                .as_str()
                .unwrap()
                .starts_with("sh -c '")
        );
        if target["kind"] == "container" {
            assert_eq!(command[0].params["target"]["params"]["container"], cid());
        }
        let reads = c.requests("read");
        if target["kind"] == "stack" {
            assert!(reads.iter().any(|r| r.operation == "InspectStackContainer"));
            assert!(!reads.iter().any(|r| r.operation == "InspectContainer"));
            assert!(
                reads
                    .iter()
                    .filter(|r| r.operation == "ListTerminals")
                    .all(|r| r.params["target"]["params"]["service"].is_null())
            );
        }
        if target["kind"] == "deployment" {
            assert!(
                reads
                    .iter()
                    .any(|r| r.operation == "InspectDeploymentContainer")
            );
            assert!(!reads.iter().any(|r| r.operation == "InspectContainer"));
        }
        assert!(
            reads
                .iter()
                .filter(|r| r.operation == "ListTerminals")
                .all(|r| r.params["limit"] == 2
                    && r.params["page"] == 0
                    && r.params["use_names"] == false)
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn strict_input_rejects_unknown_session_controls_prefixes_and_swarm_target_variants_before_io()
 {
    let c = Core::start().await;
    let (p, s) = start(&c).await;
    for patch in [
        json!({"command":"echo one\necho two"}),
        json!({"command":"echo\rno"}),
        json!({"command":"x".repeat(4097)}),
        json!({"command":""}),
        json!({"observation_seconds":0}),
        json!({"observation_seconds":31}),
        json!({"max_output_bytes":16385}),
        json!({"terminal":"default"}),
        json!({"init":{}}),
        json!({"recreate":"Always"}),
        json!({"mode":"attach"}),
        json!({"target":{"kind":"swarm","target":{"id":SERVER}}}),
        json!({"target":{"kind":"container","server":{"id":SERVER},"container":{"id":"ab123"}}}),
        json!({"target":{"kind":"container","server":{"id":SERVER},"container":{"id":cid(),"name":"fixture-web"}}}),
        json!({"target":{"kind":"stack","target":{"id":STACK},"service":""}}),
    ] {
        let mut v = input();
        v.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let before = c.data.lock().unwrap().requests.len();
        assert_eq!(
            error(call(&s, "execute_terminal", v).await)["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.data.lock().unwrap().requests.len(), before);
    }
    finish(p, s, &c).await;
}

#[tokio::test]
async fn shell_escaping_and_periphery_envelope_are_bounded_before_any_core_io() {
    let c = Core::start().await;
    let (p, s) = start(&c).await;
    for command in [
        format!("printf %s \"{}\"", "'".repeat(1500)),
        format!("printf %s {}", "a".repeat(4000)),
        format!("printf %s \"{}\"", "é".repeat(2000)),
    ] {
        assert!(command.len() <= 4096);
        let mut v = input();
        v["command"] = json!(command);
        let before = c.data.lock().unwrap().requests.len();
        let error = error(call(&s, "execute_terminal", v).await);
        assert_eq!(error["kind"], "invalid_arguments");
        assert_eq!(c.data.lock().unwrap().requests.len(), before);
    }
    // A substantial quoted UTF-8 command still takes the one-shot path when
    // its complete physical line fits; quoting is preserved in the actual wire.
    let mut v = input();
    v["command"] = json!(format!("printf %s \"{}\"", "é'".repeat(300)));
    assert_eq!(
        run(&s, v).await["attempts"],
        json!({"create":1,"command":1,"delete":1})
    );
    let sent = c.requests("terminal");
    let wrapped = sent[0].params["command"].as_str().unwrap();
    let envelope = format!(
        "printf '\\n__KOMODO_START_OF_OUTPUT__\\n\\n'; {wrapped}; rc=$?; printf '\\n__KOMODO_EXIT_CODE:%d\\n__KOMODO_END_OF_OUTPUT__\\n' \"$rc\"\n"
    );
    assert!(envelope.len() <= 4095);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn core_read_and_terminal_are_required_execute_write_and_admin_payloads_cannot_substitute() {
    for permission in [
        json!("Write"),
        json!({"level":"Execute","specific":[]}),
        json!({"level":"Write","specific":["Inspect"]}),
        json!({"level":"None","specific":["Terminal"]}),
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().permission = permission;
        let (p, s) = start(&c).await;
        assert_eq!(
            error(call(&s, "execute_terminal", input()).await)["kind"],
            "permission_denied"
        );
        assert_eq!(mutations(&c), 0);
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    c.data.lock().unwrap().caller["enabled"] = json!(false);
    let (p, s) = start(&c).await;
    assert_eq!(
        error(call(&s, "execute_terminal", input()).await)["kind"],
        "permission_denied"
    );
    assert_eq!(mutations(&c), 0);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn denied_parent_inspection_never_falls_back_to_a_server_request() {
    for kind in ["stack", "deployment"] {
        let c = Core::start().await;
        c.data.lock().unwrap().inspect_status = 403;
        let (p, s) = start(&c).await;
        let mut v = input();
        v["target"] = if kind == "stack" {
            json!({"kind":"stack","target":{"id":STACK},"service":"web"})
        } else {
            json!({"kind":"deployment","target":{"id":DEPLOYMENT}})
        };
        assert_eq!(
            error(call(&s, "execute_terminal", v).await)["kind"],
            "permission_denied"
        );
        assert_eq!(mutations(&c), 0);
        assert!(
            !c.requests("read")
                .iter()
                .any(|r| r.operation == "InspectContainer")
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn swarm_parents_tasks_stale_container_names_and_duplicate_services_never_execute() {
    for fault in 0..6 {
        let c = Core::start().await;
        let mut v = input();
        v["target"] = json!({"kind":"stack","target":{"id":STACK},"service":"web"});
        {
            let mut d = c.data.lock().unwrap();
            match fault {
                0 => d.stack["config"]["swarm_id"] = json!("000000000000000000000088"),
                1 => d.container["Config"]["Labels"]["com.docker.swarm.task.id"] = json!("task"),
                2 => d.container["Id"] = json!("cd".repeat(32)),
                3 => {
                    let duplicate = d.services[0].clone();
                    d.services.as_array_mut().unwrap().push(duplicate);
                }
                4 => d.container["Config"]["Labels"]["com.docker.compose.project"] = json!("other"),
                _ => d.container["State"]["Running"] = json!(false),
            }
        }
        let (p, s) = start(&c).await;
        error(call(&s, "execute_terminal", v).await);
        assert_eq!(mutations(&c), 0);
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    c.data.lock().unwrap().container["Config"]["Labels"]["com.docker.swarm.service.id"] =
        json!("service");
    let (p, s) = start(&c).await;
    let mut v = input();
    v["target"] = json!({"kind":"container","server":{"id":SERVER},"container":{"id":cid()}});
    error(call(&s, "execute_terminal", v).await);
    assert_eq!(mutations(&c), 0);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn incomplete_container_labels_never_mean_not_a_swarm_task_and_a_complete_empty_map_is_supported()
 {
    for config in [
        Value::Null,
        json!({}),
        json!({"Labels":null}),
        json!({"Labels":[]}),
        json!({"Labels":{"invalid":false}}),
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().container["Config"] = config;
        let (p, s) = start(&c).await;
        let mut v = input();
        v["target"] = json!({"kind":"container","server":{"id":SERVER},"container":{"id":cid()}});
        assert_eq!(
            error(call(&s, "execute_terminal", v).await)["kind"],
            "invalid_response"
        );
        assert_eq!(mutations(&c), 0);
        finish(p, s, &c).await;
    }
    let c = Core::start().await;
    c.data.lock().unwrap().container["Config"]["Labels"] = json!({});
    let (p, s) = start(&c).await;
    let mut v = input();
    v["target"] = json!({"kind":"container","server":{"id":SERVER},"container":{"id":cid()}});
    assert_eq!(
        run(&s, v).await["attempts"],
        json!({"create":1,"command":1,"delete":1})
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn foreign_collision_is_preserved_and_does_not_attach_create_execute_or_delete() {
    let c = Core::start().await;
    c.data.lock().unwrap().collision = true;
    let (p, s) = start(&c).await;
    let r = run(&s, input()).await;
    assert_eq!(r["attempts"], json!({"create":0,"command":0,"delete":0}));
    assert_eq!(r["cleanup"]["state"], "not_created");
    assert_eq!(mutations(&c), 0);
    assert_eq!(c.data.lock().unwrap().records.len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn unconfirmed_create_is_not_replayed_or_followed_by_an_application_command() {
    for response in [
        Create::Lost,
        Create::Malformed,
        Create::WrongTarget,
        Create::Denied,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().create = response;
        let (p, s) = start(&c).await;
        let r = run(&s, input()).await;
        assert_eq!(r["attempts"]["create"], 1);
        assert_eq!(r["attempts"]["command"], 0);
        assert!(c.requests("terminal").is_empty());
        assert_eq!(
            c.requests("write")
                .iter()
                .filter(|r| r.operation == "CreateTerminal")
                .count(),
            1
        );
        if matches!(response, Create::WrongTarget) {
            assert_eq!(
                r["cleanup"]["reason"],
                "foreign_or_changed_terminal_preserved"
            );
            assert_eq!(c.data.lock().unwrap().records.len(), 1);
        }
        if matches!(response, Create::Lost | Create::Malformed) {
            assert_eq!(r["cleanup"]["state"], "absence_observed");
            assert!(c.data.lock().unwrap().records.is_empty());
        }
        if matches!(response, Create::Denied) {
            assert_eq!(r["cleanup"]["state"], "unconfirmed");
            assert_eq!(r["cleanup"]["late_creation_possible"], true);
        }
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn late_private_or_permission_drift_prevents_command_and_does_not_reroute_cleanup() {
    for permission in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            if permission {
                d.permission_after_create = Some(json!({"level":"Read","specific":["Inspect"]}));
            } else {
                d.drift_after_create = Some(json!("changed-private-baseline"));
            }
        }
        let (p, s) = start(&c).await;
        let r = run(&s, input()).await;
        assert_eq!(r["attempts"], json!({"create":1,"command":0,"delete":0}));
        assert_eq!(r["cleanup"]["reason"], "routing_or_authority_changed");
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn nonzero_split_utf8_and_controls_are_complete_bounded_reported_evidence() {
    for (stream, code) in [(Stream::Nonzero, 7), (Stream::Split, 0)] {
        let c = Core::start().await;
        c.data.lock().unwrap().stream = stream;
        let (p, s) = start(&c).await;
        let r = run(&s, input()).await;
        assert_eq!(r["observation"]["reported_exit_code"], code);
        assert_eq!(r["observation"]["end"], "upstream_reported_exit");
        assert_eq!(r["attempts"]["command"], 1);
        assert_eq!(r["cleanup"]["state"], "absence_observed");
        assert!(
            !r["observation"]["output"]
                .as_str()
                .unwrap()
                .contains('\u{1b}')
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn invalid_missing_duplicate_nonfinal_and_incomplete_streams_omit_all_partial_output() {
    for stream in [
        Stream::Bytes(b"unknown-private-output".to_vec()),
        Stream::Bytes(b"__KOMODO_EXIT_CODE:0\nlate".to_vec()),
        Stream::Bytes(b"__KOMODO_EXIT_CODE:0\n__KOMODO_EXIT_CODE:1".to_vec()),
        Stream::Bytes(b"__KOMODO_EXIT_CODE:999".to_vec()),
        Stream::Bytes(vec![255]),
        Stream::Bytes(b"\x1b]unfinished".to_vec()),
        Stream::Interrupted,
        Stream::Oversized,
        Stream::Denied,
        Stream::Redirect,
    ] {
        let c = Core::start().await;
        c.data.lock().unwrap().stream = stream;
        let (p, s) = start(&c).await;
        let r = run(&s, input()).await;
        assert!(
            r["observation"]["reported_exit_code"].is_null()
                && r["observation"]["output"].is_null()
        );
        assert_eq!(r["attempts"]["command"], 1);
        assert_eq!(r["cleanup"]["state"], "absence_observed");
        assert_eq!(c.requests("terminal").len(), 1);
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn deadline_stops_observation_without_replay_and_still_removes_only_the_owned_session() {
    let c = Core::start().await;
    c.data.lock().unwrap().stream = Stream::Slow;
    let (p, s) = start(&c).await;
    let mut v = input();
    v["observation_seconds"] = json!(1);
    let r = run(&s, v).await;
    assert_eq!(
        r["observation"]["end"],
        "deadline_remote_completion_unknown"
    );
    assert_eq!(r["attempts"], json!({"create":1,"command":1,"delete":1}));
    assert_eq!(r["cleanup"]["state"], "absence_observed");
    finish(p, s, &c).await;
}

#[tokio::test]
async fn cleanup_failure_preserves_reported_result_and_unknown_delete_is_never_repeated() {
    for lost in [false, true] {
        let c = Core::start().await;
        {
            let mut d = c.data.lock().unwrap();
            d.delete_lost = lost;
            if !lost {
                d.delete_status = 503;
            }
        }
        let (p, s) = start(&c).await;
        let r = run(&s, input()).await;
        assert_eq!(r["observation"]["reported_exit_code"], 0);
        assert_eq!(r["attempts"]["delete"], 1);
        assert_eq!(
            r["cleanup"]["state"],
            if lost {
                "absence_observed"
            } else {
                "unconfirmed"
            }
        );
        assert_eq!(
            c.requests("write")
                .iter()
                .filter(|r| r.operation == "DeleteTerminal")
                .count(),
            1
        );
        finish(p, s, &c).await;
    }
}

#[tokio::test]
async fn concurrently_replaced_terminal_is_preserved_instead_of_deleted() {
    let c = Core::start().await;
    c.data.lock().unwrap().replace_after_command = true;
    let (p, s) = start(&c).await;
    let r = run(&s, input()).await;
    assert_eq!(r["observation"]["reported_exit_code"], 0);
    assert_eq!(r["attempts"]["delete"], 0);
    assert_eq!(
        r["cleanup"]["reason"],
        "foreign_or_changed_terminal_preserved"
    );
    assert_eq!(c.data.lock().unwrap().records.len(), 1);
    finish(p, s, &c).await;
}

async fn peer_call(peer: Peer<rmcp::RoleClient>) -> Result<CallToolResult, rmcp::ServiceError> {
    peer.call_tool(
        CallToolRequestParams::new("execute_terminal")
            .with_arguments(input().as_object().unwrap().clone()),
    )
    .await
}
#[tokio::test]
async fn two_mcp_processes_have_separate_names_and_preserve_an_existing_application_terminal() {
    let c = Core::start().await;
    c.data.lock().unwrap().records.insert(
        "default".into(),
        json!({"name":"default","command":"foreign application shell","created_at":1}),
    );
    let (p1, s1) = start(&c).await;
    let (p2, s2) = start(&c).await;
    let (r1, r2) = tokio::join!(run(&s1, input()), run(&s2, input()));
    assert_ne!(r1["terminal"], r2["terminal"]);
    assert_eq!(
        c.data
            .lock()
            .unwrap()
            .records
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["default"]
    );
    finish(p1, s1, &c).await;
    finish(p2, s2, &c).await;
}

#[tokio::test]
async fn dropped_rpc_cancels_observation_and_worker_finishes_owned_cleanup() {
    let c = Core::start().await;
    c.data.lock().unwrap().stream = Stream::Slow;
    let (p, s) = start(&c).await;
    let task = tokio::spawn(peer_call(s.peer().clone()));
    until(|| c.requests("terminal").len() == 1).await;
    task.abort();
    let _ = task.await;
    until(|| c.data.lock().unwrap().records.is_empty()).await;
    assert_eq!(c.requests("terminal").len(), 1);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn graceful_stdio_disconnect_drains_owned_cleanup_before_process_exit() {
    let c = Core::start().await;
    c.data.lock().unwrap().stream = Stream::Slow;
    let (p, s) = start(&c).await;
    let task = tokio::spawn(peer_call(s.peer().clone()));
    until(|| c.requests("terminal").len() == 1).await;
    finish(p, s, &c).await;
    let _ = task.await;
    assert!(c.data.lock().unwrap().records.is_empty());
    assert_eq!(c.requests("terminal").len(), 1);
}

#[tokio::test]
async fn interrupted_process_restart_never_adopts_or_deletes_the_old_namespace() {
    let c = Core::start().await;
    c.data.lock().unwrap().stream = Stream::Slow;
    let (p, s) = start(&c).await;
    let task = tokio::spawn(peer_call(s.peer().clone()));
    until(|| c.requests("terminal").len() == 1).await;
    let old = c
        .data
        .lock()
        .unwrap()
        .records
        .keys()
        .next()
        .unwrap()
        .clone();
    p.interrupt(s).await;
    let _ = task.await;
    c.data.lock().unwrap().stream = Stream::Valid;
    let (p, s) = start(&c).await;
    let r = run(&s, input()).await;
    assert_ne!(r["terminal"], old);
    assert!(c.data.lock().unwrap().records.contains_key(&old));
    finish(p, s, &c).await;
}

#[tokio::test]
async fn fifth_active_call_is_rejected_without_queuing_or_a_fifth_command() {
    let c = Core::start().await;
    c.data.lock().unwrap().stream = Stream::Slow;
    let (p, s) = start(&c).await;
    let tasks = (0..4)
        .map(|_| tokio::spawn(peer_call(s.peer().clone())))
        .collect::<Vec<_>>();
    until(|| c.requests("terminal").len() == 4).await;
    assert_eq!(
        error(call(&s, "execute_terminal", input()).await)["kind"],
        "invalid_arguments"
    );
    assert_eq!(c.requests("terminal").len(), 4);
    for task in tasks {
        data(task.await.unwrap().unwrap());
    }
    assert!(c.data.lock().unwrap().records.is_empty());
    finish(p, s, &c).await;
}
