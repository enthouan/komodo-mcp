mod support;

use komodo_mcp::operations::{self, StackOperationInput, WorkloadOperation};
use serde_json::{Value, json};
use support::{
    assert_safe,
    operations::{Ack, Core, DEPLOYMENT, Options, Outcome, Process, STACK, call, close},
};

fn args(kind: &str, service: Option<&str>) -> Value {
    if kind == "stack" {
        json!({"target":{"name":"fixture-stack"},"scope":service.map_or(json!({"kind":"whole_stack"}), |s| json!({"kind":"service","service":s}))})
    } else {
        json!({"name":"fixture-deployment"})
    }
}
fn receipt(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_safe(&serde_json::to_string(&result).unwrap());
    result.structured_content.unwrap()
}

#[tokio::test]
async fn every_lifecycle_action_uses_its_typed_request_exact_id_scope_and_update() {
    for action in ["start", "stop", "restart"] {
        for (kind, service) in [
            ("stack", None),
            ("stack", Some("web")),
            ("deployment", None),
        ] {
            let core = Core::start(Options {
                outcome: Outcome::Succeeded,
                ..Default::default()
            })
            .await;
            let (session, task) = core.session(true).await;
            let r = receipt(call(&session, &format!("{action}_{kind}"), args(kind, service)).await);
            let id = if kind == "stack" { STACK } else { DEPLOYMENT };
            let title = match action {
                "start" => "Start",
                "stop" => "Stop",
                _ => "Restart",
            };
            let typed = format!(
                "{title}{}",
                if kind == "stack" {
                    "Stack"
                } else {
                    "Deployment"
                }
            );
            assert_eq!(r["resource"]["id"], id);
            assert_eq!(r["operation"], action);
            assert_eq!(r["submission"], "accepted");
            assert_eq!(r["latest_update"]["id"], r["update_id"]);
            assert_eq!(r["latest_update"]["outcome"], "succeeded");
            assert_eq!(
                r["core_operation"],
                format!("{typed}{}", if service.is_some() { "Service" } else { "" })
            );
            assert_eq!(
                r["requested_scope"]["kind"],
                if service.is_some() {
                    "service"
                } else if kind == "stack" {
                    "whole_stack"
                } else {
                    "whole_deployment"
                }
            );
            if let Some(service) = service {
                assert_eq!(r["requested_scope"]["service"], service);
            }
            assert_eq!(r["automatic_resubmission"], false);
            let requests = core.requests();
            let executions: Vec<_> = requests.iter().filter(|r| r.path == "execute").collect();
            assert_eq!(executions.len(), 1);
            assert_eq!(executions[0].operation, typed);
            let mut expected = if kind == "stack" {
                json!({"stack":id,"services":service.into_iter().collect::<Vec<_>>()})
            } else {
                json!({"deployment":id})
            };
            if action == "stop" {
                if kind == "stack" {
                    expected["stop_time"] = Value::Null;
                } else {
                    expected["signal"] = Value::Null;
                    expected["time"] = Value::Null;
                }
            }
            assert_eq!(executions[0].params, expected);
            let effects = &r["configured_effects"];
            assert_eq!(effects["pre_deploy_hook"], false);
            assert_eq!(effects["post_deploy_hook"], false);
            assert_eq!(effects["auto_pull"], Value::Null);
            assert_eq!(effects["saved_extra_args"], false);
            assert_eq!(effects["saved_command_wrapper"], false);
            assert_eq!(
                effects["compose_dependencies"],
                if kind == "stack" {
                    json!(match action {
                        "start" => "start_existing_with_dependencies",
                        "stop" => "stop_selected_only",
                        _ => "restart_with_configured_dependents",
                    })
                } else {
                    Value::Null
                }
            );
            close(session, task).await;
            core.assert_credentials();
        }
    }
}

#[tokio::test]
async fn service_inventory_tracks_deployed_project_instead_of_new_saved_sources() {
    for action in ["start", "stop", "restart"] {
        for options in [
            Options {
                no_deployed_services: true,
                ..Default::default()
            },
            Options {
                service: Some("db".into()),
                ..Default::default()
            },
        ] {
            let selected = if options.no_deployed_services {
                "web"
            } else {
                "db"
            };
            let core = Core::start(options).await;
            let (session, task) = core.session(true).await;
            assert_eq!(
                call(
                    &session,
                    &format!("{action}_stack"),
                    args("stack", Some(selected))
                )
                .await
                .is_error,
                Some(true)
            );
            assert_eq!(core.executions(), 0);
            close(session, task).await;
        }
        let core = Core::start(Options {
            saved_service: Some("future".into()),
            ..Default::default()
        })
        .await;
        let (session, task) = core.session(true).await;
        let tool = format!("{action}_stack");
        assert_eq!(
            call(&session, &tool, args("stack", Some("future")))
                .await
                .is_error,
            Some(true)
        );
        assert_eq!(core.executions(), 0);
        assert_eq!(
            receipt(call(&session, &tool, args("stack", Some("web"))).await)["submission"],
            "accepted"
        );
        assert_eq!(core.executions(), 1);
        close(session, task).await;
    }
}

#[tokio::test]
async fn disabled_invalid_ambiguous_and_unsupported_lifecycle_never_execute() {
    for action in [
        WorkloadOperation::Start,
        WorkloadOperation::Stop,
        WorkloadOperation::Restart,
    ] {
        let core = Core::start(Options::default()).await;
        let input: StackOperationInput =
            serde_json::from_value(args("stack", Some("web"))).unwrap();
        let error = operations::stack_operation(
            &core.client(false, std::time::Duration::from_secs(1)),
            input,
            action,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, komodo_mcp::error::ErrorKind::CapabilityDisabled);
        assert!(core.requests().is_empty());
    }
    for action in ["start", "stop", "restart"] {
        for options in [
            Options {
                swarm: true,
                ..Default::default()
            },
            Options {
                template: true,
                ..Default::default()
            },
            Options {
                unassigned: true,
                ..Default::default()
            },
            Options {
                ambiguous: true,
                ..Default::default()
            },
            Options {
                missing: true,
                ..Default::default()
            },
            Options {
                changed_identity: true,
                ..Default::default()
            },
        ] {
            let core = Core::start(options).await;
            let (session, task) = core.session(true).await;
            for (kind, service) in [
                ("stack", None),
                ("stack", Some("web")),
                ("deployment", None),
            ] {
                assert_eq!(
                    call(&session, &format!("{action}_{kind}"), args(kind, service))
                        .await
                        .is_error,
                    Some(true)
                );
            }
            assert_eq!(core.executions(), 0);
            close(session, task).await;
        }
        for options in [
            Options::default(),
            Options {
                no_deployed_services: true,
                ..Default::default()
            },
        ] {
            let core = Core::start(options).await;
            let (session, task) = core.session(true).await;
            for value in [
                json!({"kind":"service","service":""}),
                json!({"kind":"service","service":"*"}),
                json!({"kind":"service","service":"web;db"}),
                json!({"kind":"service","service":"absent"}),
                json!({"kind":"whole_stack","service":"web"}),
                json!({"kind":"service","service":"web","extra":true}),
            ] {
                assert_eq!(
                    call(
                        &session,
                        &format!("{action}_stack"),
                        json!({"target":{"id":STACK},"scope":value})
                    )
                    .await
                    .is_error,
                    Some(true)
                );
            }
            assert_eq!(core.executions(), 0);
            close(session, task).await;
        }
    }
}

#[tokio::test]
async fn runtime_state_does_not_substitute_actions_or_hide_core_failure() {
    for action in ["start", "stop", "restart"] {
        for kind in ["stack", "deployment"] {
            for (state, outcome, result) in [
                ("running", Outcome::Succeeded, "succeeded"),
                ("stopped", Outcome::Succeeded, "succeeded"),
                ("paused", Outcome::Paused, "failed"),
                ("unknown", Outcome::Unavailable, "failed"),
            ] {
                let core = Core::start(Options {
                    state: Some(state.into()),
                    outcome,
                    ..Default::default()
                })
                .await;
                let (session, task) = core.session(true).await;
                let r =
                    receipt(call(&session, &format!("{action}_{kind}"), args(kind, None)).await);
                assert_eq!(r["latest_update"]["outcome"], result);
                assert_eq!(r["operation"], action);
                assert_eq!(core.executions(), 1);
                let requests = core.requests();
                assert!(!requests.iter().any(
                    |r| r.operation.starts_with("Deploy") || r.operation.starts_with("Unpause")
                ));
                if result == "failed" {
                    assert!(
                        r["latest_update"]["stages"]
                            .as_array()
                            .is_some_and(|s| !s.is_empty())
                    );
                }
                close(session, task).await;
            }
        }
    }
}

#[tokio::test]
async fn independent_callers_keep_own_busy_outcomes_and_explicit_repeat_is_new() {
    for action in ["start", "stop", "restart"] {
        for kind in ["stack", "deployment"] {
            let core = Core::start(Options {
                outcome: Outcome::SecondBusy,
                ..Default::default()
            })
            .await;
            let (one, task_one) = core.session(true).await;
            let (two, task_two) = core.session(true).await;
            let tool = format!("{action}_{kind}");
            let (a, b) = tokio::join!(
                call(&one, &tool, args(kind, None)),
                call(&two, &tool, args(kind, None))
            );
            let a = receipt(a);
            let b = receipt(b);
            assert_ne!(a["update_id"], b["update_id"]);
            let mut outcomes = vec![
                a["latest_update"]["outcome"].as_str().unwrap(),
                b["latest_update"]["outcome"].as_str().unwrap(),
            ];
            outcomes.sort();
            assert_eq!(outcomes, ["failed", "succeeded"]);
            assert_eq!(core.executions(), 2);
            let repeated = receipt(call(&one, &tool, args(kind, None)).await);
            assert_ne!(repeated["update_id"], a["update_id"]);
            assert_ne!(repeated["update_id"], b["update_id"]);
            assert_eq!(core.executions(), 3);
            close(one, task_one).await;
            close(two, task_two).await;
        }
    }
}

#[tokio::test]
async fn production_stdio_preserves_unknown_lifecycle_receipts_without_retrying() {
    for ack in [Ack::Malformed, Ack::Oversized, Ack::WrongTarget] {
        let core = Core::start(Options {
            ack,
            ..Default::default()
        })
        .await;
        let (process, session) = Process::start(&core, true).await;
        for action in ["start", "stop", "restart"] {
            for kind in ["stack", "deployment"] {
                let r =
                    receipt(call(&session, &format!("{action}_{kind}"), args(kind, None)).await);
                assert_eq!(r["submission"], "unknown");
                assert_eq!(r["update_id"], Value::Null);
                assert_eq!(r["automatic_resubmission"], false);
            }
        }
        assert_eq!(core.executions(), 6);
        process.finish(session).await;
    }
}
