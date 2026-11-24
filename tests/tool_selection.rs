//! Exercise startup settings on real stdio and authenticated HTTP processes.
#[path = "support/http.rs"]
mod http;
mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;

use http::{Binary, DEADLINE, FIRST};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, ListToolsResult, Tool},
    service::ServiceError,
};
use serde_json::{Value, json};
use support::operations::{Core, Options, Process, STACK, Session};
use tokio::time::timeout;

const ENABLED: &str = "KOMODO_ENABLED_TOOLS";
const DISABLED: &str = "KOMODO_DISABLED_TOOLS";
const TRIAGE: [&str; 3] = ["get_stack", "list_stacks", "stack_diagnose"];

#[derive(Clone, Copy)]
enum Transport {
    Stdio,
    Http,
}
const TRANSPORTS: [Transport; 2] = [Transport::Stdio, Transport::Http];

enum Peer {
    Stdio(Process, Box<Session>),
    Http(Binary, String),
}

impl Transport {
    async fn connect(self, core: &Core, settings: &[(&str, &str)]) -> Peer {
        match self {
            Self::Stdio => {
                let (process, session) =
                    Process::start_with_settings(&core.address, settings).await;
                Peer::Stdio(process, Box::new(session))
            }
            Self::Http => {
                let process = Binary::start(&core.address, settings).await;
                let session = process.http.session(FIRST, None).await;
                Peer::Http(process, session)
            }
        }
    }
}

impl Peer {
    async fn catalog(&self) -> BTreeMap<String, Tool> {
        let tools = match self {
            Self::Stdio(_, s) => timeout(DEADLINE, s.list_all_tools())
                .await
                .unwrap()
                .unwrap(),
            Self::Http(p, s) => {
                let mut tools = Vec::new();
                let mut cursors = BTreeSet::new();
                let mut params = json!({});
                loop {
                    let response = p.http.rpc(FIRST, s, "tools/list", params).await;
                    let page: ListToolsResult =
                        serde_json::from_value(response["result"].clone()).unwrap();
                    tools.extend(page.tools);
                    let Some(cursor) = page.next_cursor else {
                        break;
                    };
                    assert!(
                        cursors.insert(cursor.clone()),
                        "repeated cursor in discovery"
                    );
                    params = json!({"cursor":cursor});
                }
                tools
            }
        };
        let mut result = BTreeMap::new();
        for tool in tools {
            assert!(
                result.insert(tool.name.to_string(), tool).is_none(),
                "duplicate advertised tool"
            );
        }
        result
    }

    async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, ErrorData> {
        match self {
            Self::Stdio(_, s) => {
                let req = CallToolRequestParams::new(name.to_owned())
                    .with_arguments(args.as_object().unwrap().clone());
                match timeout(DEADLINE, s.call_tool(req)).await.unwrap() {
                    Ok(result) => Ok(result),
                    Err(ServiceError::McpError(error)) => Err(error),
                    Err(other) => panic!("unexpected stdio transport failure: {other}"),
                }
            }
            Self::Http(p, s) => {
                let response = p.http.call(FIRST, s, name, args).await;
                if let Some(error) = response.get("error") {
                    Err(serde_json::from_value(error.clone()).unwrap())
                } else {
                    Ok(serde_json::from_value(response["result"].clone()).unwrap())
                }
            }
        }
    }

    async fn data(&self, name: &str, args: Value) -> Value {
        let call = self.call(name, args).await.unwrap();
        assert_eq!(call.is_error, Some(false));
        call.structured_content.unwrap()
    }

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

    async fn finish(self) {
        match self {
            Self::Stdio(p, s) => p.finish(*s).await,
            Self::Http(p, _) => p.finish().await,
        }
    }
}

#[tokio::test]
async fn triage_exposes_exactly_three_unchanged_schemas_and_diagnoses_the_fixture() {
    let core = Core::start(Options::default()).await;
    let mut first_catalog = None;
    for transport in TRANSPORTS {
        let default = transport.connect(&core, &[]).await;
        let all = default.catalog().await;
        default.finish().await;
        let expected: BTreeMap<_, _> = TRIAGE
            .into_iter()
            .map(|name| (name.to_owned(), all[name].clone()))
            .collect();
        let triage = transport
            .connect(
                &core,
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
        let default = transport.connect(&core, &[]).await;
        let mut expected = default.catalog().await;
        default.finish().await;
        assert!(expected.remove("get_image_history").is_some());
        let selected = transport
            .connect(&core, &[(DISABLED, " get_image_history ")])
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
                &core,
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
                &core,
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
