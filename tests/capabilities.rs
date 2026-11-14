//! Future mutations are registered only in this test harness, using the same
//! production router restriction. No configuration-edit tool ships in v0.3.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use komodo_client::{
    api::{execute::DeployStack, read::GetStack},
    entities::update::Update,
};
use komodo_mcp::{
    capabilities::{Capability, CapabilityPolicy, restrict_router},
    client::{CoreClient, RequestPolicy},
    config::Config,
    history::{self, UpdateInput},
    server::KomodoServer,
};
use rmcp::{
    ClientServiceExt, RoleClient, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
        JsonObject, ProtocolVersion,
    },
    service::{ClientLifecycleMode, RunningService},
    tool, tool_handler, tool_router,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};

const KEY: &str = "capability-test-key";
const SECRET: &str = "capability-test-secret";
const ID: &str = "000000000000000000000002";
const UPDATE_ID: &str = "100000000000000000000001";
const DEADLINE: Duration = Duration::from_secs(5);
type Session = RunningService<RoleClient, ClientConfig>;

struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
struct StateData {
    requests: Vec<(String, String)>,
    bad_credentials: bool,
}
struct Core {
    address: String,
    state: Arc<Mutex<StateData>>,
    _task: Task,
}
impl Core {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(StateData::default()));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/execute", post(execute))
            .route("/komodo/write", post(write))
            .with_state(state.clone());
        let task = Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Self {
            address,
            state,
            _task: task,
        }
    }
    fn client(&self, policy: CapabilityPolicy) -> CoreClient {
        CoreClient::new(
            Config::parse(&self.address, KEY, SECRET)
                .unwrap()
                .with_capabilities(policy),
            RequestPolicy::default(),
        )
        .unwrap()
    }
    fn requests(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().requests.clone()
    }
}
fn record(state: &Mutex<StateData>, headers: HeaderMap, path: &str, envelope: &Value) {
    let mut state = state.lock().unwrap();
    state.bad_credentials |= headers.get("x-api-key").and_then(|h| h.to_str().ok()) != Some(KEY)
        || headers.get("x-api-secret").and_then(|h| h.to_str().ok()) != Some(SECRET);
    state
        .requests
        .push((path.into(), envelope["type"].as_str().unwrap().into()));
}
fn denied_update() -> Value {
    let mut update: Value =
        serde_json::from_str(include_str!("fixtures/core-2.3.2/incident-wire.json")).unwrap();
    let update = &mut update["update"];
    update["status"] = json!("Complete");
    update["success"] = json!(false);
    update["logs"][0]["stage"] = json!("Task Error");
    update["logs"][0]["success"] = json!(false);
    update["logs"][0]["stderr"] = json!(format!(
        "User does not have required permissions on this Stack. Must have at least Execute permissions\nAuthorization: Bearer {SECRET}\n{}",
        "bounded evidence ".repeat(300)
    ));
    update.clone()
}
async fn read(
    State(state): State<Arc<Mutex<StateData>>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> (StatusCode, Json<Value>) {
    record(&state, headers, "read", &envelope);
    match envelope["type"].as_str().unwrap() {
        "GetVersion" => (StatusCode::OK, Json(json!({"version":"2.3.2"}))),
        "GetStack" => (
            StatusCode::FORBIDDEN,
            Json(json!({"error":format!("denied {KEY} {SECRET}")})),
        ),
        "GetUpdate" => {
            assert_eq!(envelope["params"]["id"], UPDATE_ID);
            (StatusCode::OK, Json(denied_update()))
        }
        "ListUpdates" => (StatusCode::OK, Json(json!({"updates":[],"next_page":null}))),
        _ => (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"unexpected read"})),
        ),
    }
}
async fn execute(
    State(state): State<Arc<Mutex<StateData>>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> Json<Value> {
    record(&state, headers, "execute", &envelope);
    let request: DeployStack = serde_json::from_value(envelope["params"].clone()).unwrap();
    assert_eq!(request.stack, ID);
    assert!(request.services.is_empty());
    let mut update = denied_update();
    update["status"] = json!("InProgress");
    update["logs"] = json!([]);
    update["end_ts"] = Value::Null;
    Json(update)
}
async fn write(
    State(state): State<Arc<Mutex<StateData>>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> Json<Value> {
    record(&state, headers, "write", &envelope);
    Json(json!({"ok":true}))
}

struct Harness {
    client: CoreClient,
    address: String,
    denied_read: bool,
}
impl Harness {
    fn enabled_tools(&self) -> rmcp::handler::server::tool::ToolRouter<Self> {
        restrict_router(
            Self::tool_router(),
            self.client.capabilities(),
            |name| match name {
                "read_probe" => Some(Capability::Read),
                "operate_probe" => Some(Capability::Operate),
                "configuration_edit_probe" => Some(Capability::ConfigurationEdit),
                _ => None,
            },
        )
    }
    async fn post(&self, path: &str, envelope: Value) -> Value {
        // Test-only mutation stub. Production execution transport/receipts arrive
        // in #12; here this observes whether the policy permits dispatch at all.
        reqwest::Client::new()
            .post(format!("{}/{path}", self.address))
            .header("x-api-key", KEY)
            .header("x-api-secret", SECRET)
            .timeout(DEADLINE)
            .json(&envelope)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
#[tool_router]
impl Harness {
    #[tool(
        description = "Read an update",
        annotations(
            read_only_hint = true,
            idempotent_hint = true,
            destructive_hint = false
        )
    )]
    async fn read_probe(&self, _: JsonObject) -> CallToolResult {
        match history::get_update(
            &self.client,
            UpdateInput {
                id: UPDATE_ID.into(),
            },
        )
        .await
        {
            Ok(update) => CallToolResult::structured(serde_json::to_value(update).unwrap()),
            Err(error) => error.into_tool_result(),
        }
    }
    #[tool(
        description = "Test-only execution: mutates; never automatically retry. A new explicit call is a new operation; acknowledgement is not completion.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            destructive_hint = true
        )
    )]
    async fn operate_probe(&self, _: JsonObject) -> CallToolResult {
        if self.denied_read {
            return self
                .client
                .read(GetStack { stack: ID.into() })
                .await
                .unwrap_err()
                .into_tool_result();
        }
        let request = DeployStack {
            stack: ID.into(),
            services: vec![],
            stop_time: None,
        };
        let update: Update = serde_json::from_value(
            self.post("execute", json!({"type":"DeployStack","params":request}))
                .await,
        )
        .unwrap();
        CallToolResult::structured(json!({"update_id":update.id,"submission":"accepted"}))
    }
    #[tool(
        description = "Test-only configuration mutation; never automatically retry.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            destructive_hint = true
        )
    )]
    async fn configuration_edit_probe(&self, _: JsonObject) -> CallToolResult {
        CallToolResult::structured(
            self.post("write", json!({"type":"TestConfigurationEdit","params":{}}))
                .await,
        )
    }
    #[tool(description = "A newly registered but unclassified route must fail closed")]
    async fn unclassified_probe(&self, _: JsonObject) -> CallToolResult {
        panic!("unclassified route dispatched")
    }
}
#[tool_handler(router = self.enabled_tools())]
impl ServerHandler for Harness {}

async fn session(handler: impl ServerHandler) -> (Session, Task) {
    let (server_io, client_io) = tokio::io::duplex(256 * 1024);
    let task = Task(tokio::spawn(async move {
        handler
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    }));
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("capability-test", "1"),
    )
    .with_protocol_version(ProtocolVersion::V_2025_11_25);
    let session = timeout(
        DEADLINE,
        config.serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize),
    )
    .await
    .unwrap()
    .unwrap();
    (session, task)
}
async fn call(session: &Session, name: &str) -> Result<CallToolResult, rmcp::ServiceError> {
    timeout(
        DEADLINE,
        session.call_tool(
            CallToolRequestParams::new(name.to_owned()).with_arguments(JsonObject::new()),
        ),
    )
    .await
    .unwrap()
}
async fn close(mut session: Session, mut task: Task) {
    timeout(DEADLINE, session.close()).await.unwrap().unwrap();
    timeout(DEADLINE, &mut task.0).await.unwrap().unwrap();
}

#[tokio::test]
async fn discovery_lookup_and_direct_dispatch_share_all_four_grants() {
    for operate in [false, true] {
        for edit in [false, true] {
            let core = Core::start().await;
            let harness = Harness {
                client: core.client(CapabilityPolicy::new(operate, edit)),
                address: core.address.clone(),
                denied_read: false,
            };
            for (name, allowed) in [
                ("read_probe", true),
                ("operate_probe", operate),
                ("configuration_edit_probe", edit),
                ("unclassified_probe", false),
            ] {
                assert_eq!(harness.get_tool(name).is_some(), allowed);
            }
            let (session, task) = session(harness).await;
            let tools = timeout(DEADLINE, session.list_tools(None))
                .await
                .unwrap()
                .unwrap()
                .tools;
            assert_eq!(tools.len(), 1 + usize::from(operate) + usize::from(edit));
            for (name, allowed) in [
                ("read_probe", true),
                ("operate_probe", operate),
                ("configuration_edit_probe", edit),
                ("unclassified_probe", false),
                ("unknown_mutation", false),
            ] {
                assert_eq!(tools.iter().any(|t| t.name == name), allowed);
                let before = core.requests().len();
                let result = call(&session, name).await;
                assert_eq!(result.is_ok(), allowed, "{name}");
                if !allowed {
                    assert_eq!(
                        core.requests().len(),
                        before,
                        "disabled/unknown call reached Core"
                    );
                }
            }
            for tool in tools {
                let annotations = tool.annotations.unwrap();
                let is_read = tool.name == "read_probe";
                assert_eq!(annotations.read_only_hint, Some(is_read));
                assert_eq!(annotations.idempotent_hint, Some(is_read));
                assert_eq!(annotations.destructive_hint, Some(!is_read));
            }
            let requests = core.requests();
            assert_eq!(
                requests
                    .iter()
                    .filter(|(path, _)| path == "execute")
                    .count(),
                usize::from(operate)
            );
            assert_eq!(
                requests.iter().filter(|(path, _)| path == "write").count(),
                usize::from(edit)
            );
            assert!(!core.state.lock().unwrap().bad_credentials);
            close(session, task).await;
        }
    }
}

#[tokio::test]
async fn local_grant_preserves_upstream_denial_and_failed_update_after_acknowledgement() {
    let core = Core::start().await;
    for denied_read in [true, false] {
        let (session, task) = session(Harness {
            client: core.client(CapabilityPolicy::new(true, false)),
            address: core.address.clone(),
            denied_read,
        })
        .await;
        let result = call(&session, "operate_probe").await.unwrap();
        if denied_read {
            assert_eq!(result.is_error, Some(true));
            assert_eq!(
                result.structured_content.unwrap()["kind"],
                "permission_denied"
            );
        } else {
            assert_eq!(result.structured_content.unwrap()["update_id"], UPDATE_ID);
            let result = call(&session, "read_probe").await.unwrap();
            let safe = serde_json::to_string(&result).unwrap();
            assert!(!safe.contains(KEY) && !safe.contains(SECRET));
            let data = result.structured_content.unwrap();
            assert_eq!(data["outcome"], "failed");
            assert!(
                data["stages"][0]["stderr"]
                    .as_str()
                    .unwrap()
                    .starts_with("User does not have required permissions")
            );
            assert!(
                data["stages"][0]["stderr"].as_str().unwrap().len()
                    <= history::MAX_STAGE_STDERR_BYTES
            );
        }
        close(session, task).await;
    }
    assert_eq!(
        core.requests(),
        [
            ("read".into(), "GetStack".into()),
            ("execute".into(), "DeployStack".into()),
            ("read".into(), "GetUpdate".into())
        ]
    );
    assert!(!core.state.lock().unwrap().bad_credentials);
}

#[tokio::test]
async fn production_keeps_all_reads_and_recovery_history_for_every_policy() {
    for operate in [false, true] {
        for edit in [false, true] {
            let core = Core::start().await;
            let client = core.client(CapabilityPolicy::new(operate, edit));
            let core_version = client.detect_version().await.unwrap();
            let handler = KomodoServer {
                client,
                core_version,
            };
            assert!(handler.get_tool("get_update").is_some());
            assert!(handler.get_tool("wait_for_update").is_some());
            assert!(handler.get_tool("reconcile_operation").is_some());
            assert!(handler.get_tool("list_updates").is_some());
            let (session, task) = session(handler).await;
            let tools = timeout(DEADLINE, session.list_tools(None))
                .await
                .unwrap()
                .unwrap()
                .tools;
            assert_eq!(
                tools.len(),
                32 + 16 * usize::from(operate)
                    + 46 * usize::from(edit)
                    + 2 * usize::from(operate && edit)
            );
            for tool in tools {
                let mutation = [
                    "deploy_", "start_", "stop_", "restart_", "clone_", "pull_", "run_", "cancel_",
                ]
                .iter()
                .any(|prefix| tool.name.starts_with(prefix))
                    || tool.name.starts_with("apply_");
                assert_eq!(tool.annotations.unwrap().read_only_hint, Some(!mutation));
            }
            for name in ["deploy_stack", "edit_stack", "unknown_mutation"] {
                let result = call(&session, name).await;
                if operate && name == "deploy_stack" {
                    assert_eq!(
                        result.unwrap().structured_content.unwrap()["kind"],
                        "invalid_arguments"
                    );
                } else {
                    assert!(result.is_err());
                }
            }
            assert_eq!(core.requests().len(), 1);
            let result = timeout(
                DEADLINE,
                session.call_tool(
                    CallToolRequestParams::new("get_update")
                        .with_arguments(json!({"id":UPDATE_ID}).as_object().unwrap().clone()),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result.structured_content.unwrap()["outcome"], "failed");
            let result = call(&session, "list_updates").await.unwrap();
            assert_eq!(result.is_error, Some(false));
            assert!(!core.state.lock().unwrap().bad_credentials);
            close(session, task).await;
        }
    }
}
