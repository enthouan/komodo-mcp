mod support;

use std::time::{Duration, Instant};

use komodo_mcp::{
    error::ErrorKind,
    inventory::DetailInput,
    operations::{self, StackOperationInput, StackScope},
};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use support::{
    KEY, assert_safe,
    operations::{Ack, Core, DEPLOYMENT, Options, Outcome, Process, STACK, call, close},
};

fn stack(scope: Value) -> Value {
    json!({"target":{"id":STACK},"scope":scope})
}
fn whole() -> Value {
    stack(json!({"kind":"whole_stack"}))
}
fn service(name: &str) -> Value {
    stack(json!({"kind":"service","service":name}))
}
fn success(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let text = serde_json::to_string(&result).unwrap();
    assert_safe(&text);
    let data = result.structured_content.unwrap();
    assert!(serde_json::to_vec(&data).unwrap().len() <= operations::MAX_RECEIPT_BYTES);
    data
}

#[tokio::test]
async fn escaped_receipt_evidence_keeps_the_id_and_some_failed_stages_within_budget() {
    let core = Core::start(Options {
        escaped_evidence: true,
        ..Default::default()
    })
    .await;
    let (session, task) = core.session(true).await;
    let data = success(call(&session, "deploy_stack", whole()).await);
    assert_eq!(data["update_id"], data["latest_update"]["id"]);
    let stages = data["latest_update"]["stages"].as_array().unwrap();
    assert!(!stages.is_empty() && stages.len() < 20);
    assert_eq!(data["latest_update"]["stages_truncated"], true);
    assert_eq!(core.executions(), 1);
    close(session, task).await;
}

#[tokio::test]
async fn deploy_matrix_uses_typed_stable_ids_and_explicit_scope() {
    for swarm in [false, true] {
        for (tool, args, kind, id, core_operation, scope) in [
            (
                "deploy_stack",
                whole(),
                "stack",
                STACK,
                "DeployStack",
                "whole_stack",
            ),
            (
                "deploy_deployment",
                json!({"name":"fixture-deployment"}),
                "deployment",
                DEPLOYMENT,
                "Deploy",
                "whole_deployment",
            ),
        ] {
            let core = Core::start(Options {
                swarm,
                outcome: Outcome::Succeeded,
                ..Default::default()
            })
            .await;
            let (session, task) = core.session(true).await;
            let data = success(call(&session, tool, args).await);
            assert_eq!(data["resource"]["kind"], kind);
            assert_eq!(data["resource"]["id"], id);
            assert_eq!(data["requested_scope"]["kind"], scope);
            assert_eq!(data["core_operation"], core_operation);
            assert_eq!(data["operation"], "deploy");
            assert_eq!(data["submission"], "accepted");
            assert_eq!(data["latest_update"]["id"], data["update_id"]);
            assert_eq!(data["latest_update"]["outcome"], "succeeded");
            assert_eq!(data["automatic_resubmission"], false);
            assert_eq!(core.executions(), 1);
            let requests = core.requests();
            let executed = requests.iter().find(|r| r.path == "execute").unwrap();
            if kind == "stack" {
                assert_eq!(
                    executed.params,
                    json!({"stack":STACK,"services":[],"stop_time":null})
                );
            } else {
                assert_eq!(
                    executed.params,
                    json!({"deployment":DEPLOYMENT,"stop_signal":null,"stop_time":null})
                );
            }
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r.operation == "GetUpdate")
                    .count(),
                1
            );
            core.assert_credentials();
            close(session, task).await;
        }
    }
}

#[tokio::test]
async fn named_service_preserves_saved_hooks_dependencies_and_pull_policy() {
    let core = Core::start(Options {
        outcome: Outcome::Succeeded,
        ..Default::default()
    })
    .await;
    let (session, task) = core.session(true).await;
    let data = success(
        call(
            &session,
            "deploy_stack",
            json!({"target":{"name":"fixture-stack"},"scope":{"kind":"service","service":"web"}}),
        )
        .await,
    );
    assert_eq!(
        data["requested_scope"],
        json!({"kind":"service","service":"web"})
    );
    assert_eq!(data["core_operation"], "DeployStackService");
    assert_eq!(data["configured_effects"]["pre_deploy_hook"], true);
    assert_eq!(data["configured_effects"]["post_deploy_hook"], true);
    assert_eq!(data["configured_effects"]["auto_pull"], false);
    assert_eq!(data["configured_effects"]["saved_extra_args"], true);
    assert_eq!(data["configured_effects"]["broader_effects_possible"], true);
    assert_eq!(
        data["latest_update"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["stage"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Service/s", "Pre Deploy", "Compose Up", "Post Deploy"]
    );
    let requests = core.requests();
    let executed = requests.iter().find(|r| r.path == "execute").unwrap();
    assert_eq!(
        executed.params,
        json!({"stack":STACK,"services":["web"],"stop_time":null})
    );
    // The saved dependencies/hooks are fixture evidence, never hidden overrides
    // such as no_deps, no_pull, skip_hooks, a replacement definition or stop flags.
    assert_eq!(executed.params.as_object().unwrap().len(), 3);
    assert_eq!(core.executions(), 1);
    close(session, task).await;
}

#[tokio::test]
async fn invalid_or_disabled_selection_never_executes_or_expands_scope() {
    let core = Core::start(Options::default()).await;
    let client = core.client(false, Duration::from_secs(1));
    let error = operations::deploy_stack(
        &client,
        StackOperationInput {
            target: DetailInput {
                id: Some(STACK.into()),
                name: None,
            },
            scope: StackScope::WholeStack {},
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::CapabilityDisabled);
    assert!(core.requests().is_empty());
    let (session, task) = core.session(true).await;
    let invalid = [
        json!({}),
        json!({"target":{"id":STACK}}),
        json!({"id":STACK,"scope":{"kind":"whole_stack"}}),
        json!({"target":{"id":STACK,"name":"fixture-stack"},"scope":{"kind":"whole_stack"}}),
        json!({"target":{"id":null,"name":"fixture-stack"},"scope":{"kind":"whole_stack"}}),
        json!({"target":{"id":"invalid"},"scope":{"kind":"whole_stack"}}),
        json!({"target":{"name":" "},"scope":{"kind":"whole_stack"}}),
        json!({"target":{"id":STACK},"scope":{"kind":"whole_stack","service":"web"}}),
        json!({"target":{"id":STACK},"scope":{"kind":"service","service":["web"]}}),
        json!({"target":{"id":STACK},"scope":{"kind":"service"}}),
        json!({"target":{"id":STACK},"scope":{"kind":"whole_stack"},"config":{}}),
        service(""),
        service(" "),
        service("*"),
        service("web*"),
        service("web;echo"),
        service("web db"),
        service("-web"),
        service(&"x".repeat(129)),
    ];
    let before = core.requests().len();
    for (case, args) in invalid.into_iter().enumerate() {
        let r = call(&session, "deploy_stack", args).await;
        assert_eq!(r.is_error, Some(true), "invalid scope/selector case {case}");
        assert_eq!(r.structured_content.unwrap()["kind"], "invalid_arguments");
    }
    assert_eq!(core.requests().len(), before);
    for args in [
        service("missing"),
        json!({"target":{"name":"missing"},"scope":{"kind":"whole_stack"}}),
        json!({"target":{"name":"fixture-*"},"scope":{"kind":"whole_stack"}}),
    ] {
        assert_eq!(
            call(&session, "deploy_stack", args).await.is_error,
            Some(true)
        );
    }
    let r = call(
        &session,
        "deploy_deployment",
        json!({"id":DEPLOYMENT,"services":["web"]}),
    )
    .await;
    assert_eq!(r.structured_content.unwrap()["kind"], "invalid_arguments");
    assert_eq!(core.executions(), 0);
    close(session, task).await;
}

#[tokio::test]
async fn unsupported_missing_ambiguous_and_changed_targets_make_zero_execute_requests() {
    for (options, args, expected) in [
        (
            Options {
                swarm: true,
                ..Default::default()
            },
            service("web"),
            "unsupported_target",
        ),
        (
            Options {
                template: true,
                ..Default::default()
            },
            whole(),
            "unsupported_target",
        ),
        (
            Options {
                unassigned: true,
                ..Default::default()
            },
            whole(),
            "unsupported_target",
        ),
        (
            Options {
                no_services: true,
                ..Default::default()
            },
            service("web"),
            "invalid_arguments",
        ),
        (
            Options {
                missing: true,
                ..Default::default()
            },
            whole(),
            "not_found",
        ),
        (
            Options {
                ambiguous: true,
                ..Default::default()
            },
            json!({"target":{"name":"fixture-stack"},"scope":{"kind":"whole_stack"}}),
            "ambiguous_target",
        ),
        (
            Options {
                changed_identity: true,
                ..Default::default()
            },
            whole(),
            "invalid_response",
        ),
    ] {
        let core = Core::start(options).await;
        let (session, task) = core.session(true).await;
        let r = call(&session, "deploy_stack", args).await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(r.structured_content.unwrap()["kind"], expected);
        assert_eq!(core.executions(), 0);
        close(session, task).await;
    }
    for options in [
        Options {
            template: true,
            ..Default::default()
        },
        Options {
            unassigned: true,
            ..Default::default()
        },
    ] {
        let core = Core::start(options).await;
        let (session, task) = core.session(true).await;
        assert_eq!(
            call(&session, "deploy_deployment", json!({"id":DEPLOYMENT}))
                .await
                .structured_content
                .unwrap()["kind"],
            "unsupported_target"
        );
        assert_eq!(core.executions(), 0);
        close(session, task).await;
    }
}

#[tokio::test]
async fn rejection_uncertainty_and_acknowledged_failure_are_distinct_without_retries() {
    for (ack, expected) in [
        (Ack::Reject(401), "rejected"),
        (Ack::Reject(403), "rejected"),
        (Ack::Reject(409), "rejected"),
        (Ack::Malformed, "unknown"),
        (Ack::Oversized, "unknown"),
        (Ack::WrongTarget, "unknown"),
    ] {
        for chunked in [false, true] {
            let core = Core::start(Options {
                ack,
                chunked,
                ..Default::default()
            })
            .await;
            let (session, task) = core.session(true).await;
            let data = success(call(&session, "deploy_stack", service("web")).await);
            assert_eq!(data["submission"], expected);
            assert!(data["update_id"].is_null());
            assert!(data["submission_error"].is_object());
            assert_eq!(core.executions(), 1);
            assert!(!core.requests().iter().any(|r| r.operation == "GetUpdate"));
            core.assert_credentials();
            close(session, task).await;
        }
    }
    for outcome in [Outcome::Denied, Outcome::Busy, Outcome::RolloutFailed] {
        let core = Core::start(Options {
            outcome,
            ..Default::default()
        })
        .await;
        let (session, task) = core.session(true).await;
        let data = success(call(&session, "deploy_stack", whole()).await);
        assert_eq!(data["submission"], "accepted");
        assert!(data["update_id"].is_string());
        assert_eq!(data["latest_update"]["outcome"], "failed");
        assert!(
            data["latest_update"]["stages"][0]["stderr"]
                .as_str()
                .unwrap()
                .len()
                > 5
        );
        assert_eq!(core.executions(), 1);
        core.assert_credentials();
        close(session, task).await;
    }
}

#[tokio::test]
async fn an_observation_failure_or_deadline_preserves_the_acknowledged_update() {
    for options in [
        Options {
            observation_status: Some(403),
            ..Default::default()
        },
        Options {
            observation_delay: Duration::from_secs(4),
            ..Default::default()
        },
    ] {
        let core = Core::start(options).await;
        let (session, task) = core.session(true).await;
        let started = Instant::now();
        let data = success(call(&session, "deploy_deployment", json!({"id":DEPLOYMENT})).await);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(data["submission"], "accepted");
        assert_eq!(data["latest_update"]["id"], data["update_id"]);
        assert_eq!(data["latest_update"]["outcome"], "running");
        assert!(data["observation_error"].is_object());
        assert_eq!(core.executions(), 1);
        close(session, task).await;
    }
}

#[tokio::test]
async fn independent_clients_retain_their_own_ids_and_actual_busy_outcomes() {
    let core = Core::start(Options {
        outcome: Outcome::SecondBusy,
        ..Default::default()
    })
    .await;
    let (one, one_task) = core.session(true).await;
    let (two, two_task) = core.session(true).await;
    let (a, b) = tokio::join!(
        call(&one, "deploy_stack", service("web")),
        call(&two, "deploy_stack", service("web"))
    );
    let a = success(a);
    let b = success(b);
    assert_ne!(a["update_id"], b["update_id"]);
    for result in [&a, &b] {
        assert_eq!(result["submission"], "accepted");
        assert_eq!(result["update_id"], result["latest_update"]["id"]);
    }
    let mut outcomes = vec![
        a["latest_update"]["outcome"].as_str().unwrap(),
        b["latest_update"]["outcome"].as_str().unwrap(),
    ];
    outcomes.sort();
    assert_eq!(outcomes, ["failed", "succeeded"]);
    assert_eq!(core.executions(), 2);
    close(one, one_task).await;
    close(two, two_task).await;
}

#[tokio::test]
async fn exact_raw_identifiers_are_used_before_display_redaction_or_clipping() {
    let raw_service = format!("web-{KEY}");
    let raw_name = format!("{}-{KEY}", "a".repeat(300));
    let core = Core::start(Options {
        service: Some(raw_service.clone()),
        name: Some(raw_name.clone()),
        ..Default::default()
    })
    .await;
    let (session, task) = core.session(true).await;
    let data = success(
        call(
            &session,
            "deploy_stack",
            json!({"target":{"name":raw_name},"scope":{"kind":"service","service":raw_service}}),
        )
        .await,
    );
    assert_eq!(data["submission"], "accepted");
    assert!(data["resource"]["name"].as_str().unwrap().len() <= 256);
    let requests = core.requests();
    let executed = requests.iter().find(|r| r.path == "execute").unwrap();
    assert_eq!(executed.params["stack"], STACK);
    assert_eq!(executed.params["services"], json!([raw_service]));
    close(session, task).await;
}

#[tokio::test]
async fn production_binary_discovers_mutations_only_when_enabled_and_returns_receipts() {
    for operate in [false, true] {
        let core = Core::start(Options {
            outcome: Outcome::Succeeded,
            ..Default::default()
        })
        .await;
        let (process, session) = Process::start(&core, operate).await;
        let tools = session.list_tools(None).await.unwrap().tools;
        assert_eq!(tools.len(), 66 + 20 * usize::from(operate));
        for tool in tools.iter().filter(|t| {
            ["deploy_", "start_", "stop_", "restart_"]
                .iter()
                .any(|prefix| t.name.starts_with(prefix))
        }) {
            let a = tool.annotations.as_ref().unwrap();
            assert_eq!(a.read_only_hint, Some(false));
            assert_eq!(a.idempotent_hint, Some(false));
            assert_eq!(a.destructive_hint, Some(true));
            assert!(tool.description.as_ref().unwrap().contains("retry"));
        }
        if operate {
            for (tool, args) in [
                ("deploy_stack", service("web")),
                ("deploy_deployment", json!({"id":DEPLOYMENT})),
                ("start_stack", service("web")),
                ("stop_stack", whole()),
                ("restart_stack", service("db")),
                ("start_deployment", json!({"id":DEPLOYMENT})),
                ("stop_deployment", json!({"id":DEPLOYMENT})),
                ("restart_deployment", json!({"id":DEPLOYMENT})),
            ] {
                assert_eq!(
                    success(call(&session, tool, args).await)["submission"],
                    "accepted"
                );
            }
            assert_eq!(core.executions(), 8);
        } else {
            let before = core.requests().len();
            for name in [
                "deploy_stack",
                "start_stack",
                "stop_stack",
                "restart_stack",
                "start_deployment",
                "stop_deployment",
                "restart_deployment",
            ] {
                assert!(
                    session
                        .call_tool(
                            CallToolRequestParams::new(name)
                                .with_arguments(whole().as_object().unwrap().clone())
                        )
                        .await
                        .is_err()
                );
            }
            assert_eq!(core.requests().len(), before);
            assert_eq!(core.executions(), 0);
        }
        process.finish(session).await;
        core.assert_credentials();
    }
}
