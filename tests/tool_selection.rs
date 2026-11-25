//! Exercise startup settings on real stdio and authenticated HTTP processes.
#[path = "support/http.rs"]
mod http;
mod support;

#[path = "support/peer.rs"]
mod peer;

use http::{DEADLINE, FIRST};
use peer::{Peer, TRANSPORTS, Transport};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Stdio};
use support::operations::{Core, Options, STACK};
use tokio::time::timeout;

const ENABLED: &str = "KOMODO_ENABLED_TOOLS";
const DISABLED: &str = "KOMODO_DISABLED_TOOLS";
const TRIAGE: [&str; 3] = ["get_stack", "list_stacks", "stack_diagnose"];

impl Peer {
    async fn assert_excluded(&self, core: &Core, name: &str, args: Value) {
        let before = core.requests().len();
        let excluded = self.call(name, args).await.unwrap_err();
        let unknown = self
            .call("not_a_registered_fixture_tool", json!({}))
            .await
            .unwrap_err();
        assert_eq!(excluded.code, unknown.code);
        assert_eq!(
            core.requests().len(),
            before,
            "excluded calls must be rejected before Core"
        );
    }
}

#[tokio::test]
async fn triage_exposes_exactly_three_unchanged_schemas_and_diagnoses_the_fixture() {
    let core = Core::start(Options::default()).await;
    let mut first_catalog = None;
    for transport in TRANSPORTS {
        let default = transport.connect(&core.address, &[]).await;
        let all = default.catalog().await;
        default.finish().await;
        let expected: BTreeMap<_, _> = TRIAGE
            .into_iter()
            .map(|name| (name.to_owned(), all[name].clone()))
            .collect();
        let triage = transport
            .connect(
                &core.address,
                &[(ENABLED, " list_stacks, get_stack , stack_diagnose ")],
            )
            .await;
        let catalog = triage.catalog().await;
        assert_eq!(
            catalog, expected,
            "the same three schemas must be discoverable"
        );
        if let Some(first) = &first_catalog {
            assert_eq!(&catalog, first, "transports must expose the same schemas");
        }
        first_catalog = Some(catalog);
        let stacks = triage.data("list_stacks", json!({})).await;
        assert_eq!(stacks["items"][0]["id"], STACK);
        assert_eq!(
            triage.data("get_stack", json!({"id":STACK})).await["id"],
            STACK
        );
        assert_eq!(
            triage.data("stack_diagnose", json!({"id":STACK})).await["outcome"],
            "unhealthy"
        );
        triage
            .assert_excluded(&core, "list_deployments", json!({}))
            .await;
        triage.finish().await;
    }
    core.assert_credentials();
}

#[tokio::test]
async fn disabling_one_tool_preserves_every_other_schema_and_its_core_permissions() {
    let core = Core::start(Options {
        history_status: Some(403),
        ..Options::default()
    })
    .await;
    for transport in TRANSPORTS {
        let default = transport.connect(&core.address, &[]).await;
        let mut expected = default.catalog().await;
        default.finish().await;
        assert!(expected.remove("get_image_history").is_some());
        let selected = transport
            .connect(&core.address, &[(DISABLED, " get_image_history ")])
            .await;
        assert_eq!(selected.catalog().await, expected);
        selected
            .assert_excluded(
                &core,
                "get_image_history",
                json!({
                    "target":{"id":"000000000000000000000001"},
                    "object":{"id":format!("sha256:{}", "a".repeat(64))}
                }),
            )
            .await;
        let before = core.requests().len();
        let denied = selected.call("list_updates", json!({})).await.unwrap();
        assert_eq!(denied.is_error, Some(true));
        assert_eq!(
            denied.structured_content.unwrap()["kind"],
            "permission_denied"
        );
        let requests = core.requests();
        assert_eq!(requests.len(), before + 1);
        assert_eq!(requests.last().unwrap().operation, "ListUpdates");
        selected.finish().await;
    }
    core.assert_credentials();
}

#[tokio::test]
async fn disabled_overlap_wins_and_allowlisting_cannot_enable_the_terminal_capability() {
    let core = Core::start(Options::default()).await;
    for transport in TRANSPORTS {
        let selected = transport
            .connect(
                &core.address,
                &[
                    (ENABLED, "list_stacks,get_stack,execute_terminal"),
                    (DISABLED, "get_stack"),
                ],
            )
            .await;
        assert_eq!(
            selected.catalog().await.into_keys().collect::<Vec<_>>(),
            ["list_stacks"]
        );
        selected
            .assert_excluded(&core, "get_stack", json!({"id":STACK}))
            .await;
        selected
            .assert_excluded(&core, "execute_terminal", json!({}))
            .await;
        selected.finish().await;
        let empty = transport
            .connect(
                &core.address,
                &[(ENABLED, "list_stacks"), (DISABLED, "list_stacks")],
            )
            .await;
        assert!(empty.catalog().await.is_empty());
        empty.assert_excluded(&core, "list_stacks", json!({})).await;
        empty.finish().await;
    }
    core.assert_credentials();
}

#[tokio::test]
async fn malformed_selections_fail_startup_before_any_core_connection() {
    let core = Core::start(Options::default()).await;
    for transport in TRANSPORTS {
        for key in [ENABLED, DISABLED] {
            for value in ["", "list_stacks, ", "fake_selection_tool"] {
                let mut command = support::binary(&core.address, &[(key, value)]);
                if let Transport::Http = transport {
                    command
                        .env("KOMODO_TRANSPORT", "http")
                        .env("KOMODO_HTTP_BIND", "127.0.0.1:0")
                        .env("KOMODO_HTTP_TOKENS", FIRST);
                }
                command.stdin(Stdio::null());
                let output = timeout(DEADLINE, command.output()).await.unwrap().unwrap();
                assert!(
                    !output.status.success(),
                    "invalid selection accepted at startup"
                );
                let error = String::from_utf8(output.stderr).unwrap();
                assert!(
                    error.contains(key),
                    "the failing setting must be identified"
                );
                let reason = error.to_ascii_lowercase();
                if value == "fake_selection_tool" {
                    assert!(
                        reason.contains("unknown") || reason.contains("unregistered"),
                        "missing unknown-tool diagnostic"
                    );
                } else {
                    assert!(
                        reason.contains("blank") || reason.contains("empty"),
                        "missing blank-entry diagnostic"
                    );
                }
                assert!(
                    core.requests().is_empty(),
                    "invalid configuration must fail before Core"
                );
            }
        }
    }
}
