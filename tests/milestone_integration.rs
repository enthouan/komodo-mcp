//! Cross-family compiled stdio observations; no live cluster or account.
#[path = "support/terminal.rs"]
mod fixture;
mod support;

use fixture::{Core, SERVER};
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use support::operations::{Process, call};

#[tokio::test]
async fn every_capability_profile_keeps_cross_family_reads_free_of_implicit_effects() {
    for operate in [false, true] {
        for edit in [false, true] {
            for prune in [false, true] {
                for terminal in [false, true] {
                    let core = Core::start().await;
                    {
                        let mut d = core.data.lock().unwrap();
                        d.caller["admin"] = json!(true);
                        d.permission =
                            json!({"level":"Execute","specific":["Inspect","Logs","Terminal"]});
                        d.server["config"]["auto_prune"] = json!(false);
                        for operation in [
                            "ListSwarms",
                            "ListImages",
                            "ListNetworks",
                            "ListVolumes",
                            "ListVariables",
                            "ListSecrets",
                            "ListAlerters",
                            "ListUsers",
                            "ListUserGroups",
                            "ListContainers",
                        ] {
                            d.reads.insert(operation.into(), json!([]));
                        }
                    }
                    let (process, session) = Process::start_profile_terminal(
                        &core.address,
                        operate,
                        edit,
                        prune,
                        terminal,
                    )
                    .await;
                    for (tool, args) in [
                        ("list_swarms", json!({})),
                        ("list_images", json!({"target":{"id":SERVER}})),
                        ("list_networks", json!({"target":{"id":SERVER}})),
                        ("list_volumes", json!({"target":{"id":SERVER}})),
                        ("list_variables", json!({})),
                        ("list_core_secret_names", json!({})),
                        ("list_alerters", json!({})),
                        ("list_users", json!({})),
                        ("list_user_groups", json!({})),
                    ] {
                        let result = call(&session, tool, args).await;
                        support::assert_safe(&serde_json::to_string(&result).unwrap());
                        assert_eq!(result.is_error, Some(false), "{tool}: {result:?}");
                    }
                    // Every family shares one transport/caller in this process.
                    // Discovery and invalid direct calls must not submit a test,
                    // prune, shell, save, deployment or administration mutation.
                    for (tool, visible) in [
                        ("apply_variable_change", edit),
                        ("apply_access_change", edit && operate),
                        ("apply_alerter_update", edit),
                        ("test_alerter", operate),
                        ("apply_docker_prune", operate && prune),
                        ("execute_terminal", operate && terminal),
                        ("deploy_scaled_deployment", operate),
                        ("preview_deployment_scale", edit),
                    ] {
                        let before = core.data.lock().unwrap().requests.len();
                        let result = session
                            .call_tool(
                                CallToolRequestParams::new(tool)
                                    .with_arguments(json!({}).as_object().unwrap().clone()),
                            )
                            .await;
                        if visible {
                            let result = result.unwrap();
                            assert_eq!(
                                result.structured_content.unwrap()["kind"],
                                "invalid_arguments"
                            );
                        } else {
                            assert!(result.is_err(), "disabled {tool} dispatched");
                        }
                        assert_eq!(core.data.lock().unwrap().requests.len(), before, "{tool}");
                    }
                    assert!(core.requests("write").is_empty());
                    assert!(core.requests("execute").is_empty());
                    assert!(core.requests("terminal").is_empty());
                    assert!(core.requests("unexpected").is_empty());
                    assert!(!core.data.lock().unwrap().bad_credentials);
                    process.finish_excluding(session, &[fixture::PRIVATE]).await;
                }
            }
        }
    }
}
