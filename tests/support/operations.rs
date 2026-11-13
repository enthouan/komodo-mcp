//! Deterministic Core 2.3.2 workload-operation fixture and production stdio client.

use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_client::api::{
    execute::{
        Deploy, DeployStack, RestartDeployment, RestartStack, StartDeployment, StartStack,
        StopDeployment, StopStack,
    },
    read::{GetDeployment, GetStack, GetUpdate, ListDeployments, ListStacks},
};
use komodo_mcp::{
    capabilities::CapabilityPolicy,
    client::{CoreClient, RequestPolicy},
    config::Config,
    server::KomodoServer,
};
use rmcp::{
    ClientServiceExt, RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
        ProtocolVersion,
    },
    service::{ClientLifecycleMode, RunningService},
};
use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    net::TcpListener,
    process::{Child, Command},
    task::JoinHandle,
    time::timeout,
};

use super::{
    KEY, SECRET,
    inventory::{FAMILIES, Family},
};

pub const STACK: &str = "000000000000000000000002";
pub const DEPLOYMENT: &str = "000000000000000000000003";
pub const DEADLINE: Duration = Duration::from_secs(25);
const SHUTDOWN: Duration = Duration::from_secs(5);
pub type Session = RunningService<RoleClient, ClientConfig>;

#[derive(Clone, Copy, Default)]
pub enum Outcome {
    #[default]
    Running,
    Succeeded,
    Denied,
    Busy,
    RolloutFailed,
    SecondBusy,
    Paused,
    Unavailable,
}
#[derive(Clone, Copy, Default)]
pub enum Ack {
    #[default]
    Valid,
    Malformed,
    Oversized,
    Reject(u16),
    WrongTarget,
}
#[derive(Clone, Default)]
pub struct Options {
    pub outcome: Outcome,
    pub ack: Ack,
    pub swarm: bool,
    pub template: bool,
    pub unassigned: bool,
    pub missing: bool,
    pub ambiguous: bool,
    pub no_services: bool,
    pub no_deployed_services: bool,
    pub saved_service: Option<String>,
    pub state: Option<String>,
    pub service: Option<String>,
    pub name: Option<String>,
    pub observation_status: Option<u16>,
    pub observation_delay: Duration,
    pub chunked: bool,
    pub changed_identity: bool,
    pub escaped_evidence: bool,
}
#[derive(Debug, Clone)]
pub struct Request {
    pub path: &'static str,
    pub operation: String,
    pub params: Value,
}
struct Data {
    options: Options,
    requests: Vec<Request>,
    updates: BTreeMap<String, Value>,
    executions: usize,
    bad_credentials: bool,
}
pub struct Task(pub JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub struct Core {
    pub address: String,
    data: Arc<Mutex<Data>>,
    _task: Task,
}
impl Core {
    pub async fn start(options: Options) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let data = Arc::new(Mutex::new(Data {
            options,
            requests: vec![],
            updates: BTreeMap::new(),
            executions: 0,
            bad_credentials: false,
        }));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/execute", post(execute))
            .with_state(data.clone());
        let task = Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Self {
            address,
            data,
            _task: task,
        }
    }
    pub fn client(&self, operate: bool, deadline: Duration) -> CoreClient {
        CoreClient::new(
            Config::parse(&self.address, KEY, SECRET)
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, false)),
            RequestPolicy {
                connect_timeout: deadline,
                operation_timeout: deadline,
            },
        )
        .unwrap()
    }
    pub fn requests(&self) -> Vec<Request> {
        self.data.lock().unwrap().requests.clone()
    }
    pub fn executions(&self) -> usize {
        self.data.lock().unwrap().executions
    }
    pub fn assert_credentials(&self) {
        assert!(!self.data.lock().unwrap().bad_credentials);
    }
    pub async fn session(&self, operate: bool) -> (Session, Task) {
        let client = self.client(operate, Duration::from_secs(3));
        let core_version = client.detect_version().await.unwrap();
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let task = Task(tokio::spawn(async move {
            KomodoServer {
                client,
                core_version,
            }
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
        }));
        (connect(client_io).await, task)
    }
}
fn record(data: &mut Data, headers: &HeaderMap, path: &'static str, envelope: &Value) {
    data.bad_credentials |= headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some(KEY)
        || headers.get("x-api-secret").and_then(|v| v.to_str().ok()) != Some(SECRET);
    data.requests.push(Request {
        path,
        operation: envelope["type"].as_str().unwrap().into(),
        params: envelope["params"].clone(),
    });
}
fn summary(family: Family, options: &Options) -> Value {
    let mut value = family.summary();
    value["template"] = json!(options.template);
    if let Some(state) = &options.state {
        value["info"]["state"] = json!(
            if matches!(family, Family::Deployment) && state == "stopped" {
                "exited"
            } else {
                state
            }
        );
    }
    if let Some(name) = &options.name {
        value["name"] = json!(name);
    }
    if options.swarm {
        value["info"]["swarm_id"] = json!("000000000000000000000009");
    }
    if options.unassigned {
        value["info"]["server_id"] = json!("");
        value["info"]["swarm_id"] = json!("");
    }
    value
}
fn detail(family: Family, options: &Options) -> Value {
    let mut value = family.detail();
    value["template"] = json!(options.template);
    if let Some(name) = &options.name {
        value["name"] = json!(name);
    }
    if options.changed_identity {
        value["name"] = json!("changed-after-resolution");
    }
    if options.swarm {
        value["config"]["swarm_id"] = json!("000000000000000000000009");
    }
    if options.unassigned {
        value["config"]["server_id"] = json!("");
        value["config"]["swarm_id"] = json!("");
    }
    if matches!(family, Family::Stack) {
        let service = options.service.as_deref().unwrap_or("web");
        value["info"]["latest_services"] = if options.no_services {
            json!([])
        } else {
            json!([{"service_name":service,"container_name":"fixture-web"},{"service_name":"db","container_name":"fixture-db"}])
        };
        value["info"]["deployed_services"] = if options.no_deployed_services {
            Value::Null
        } else {
            value["info"]["latest_services"].clone()
        };
        if let Some(saved) = &options.saved_service {
            value["info"]["latest_services"] =
                json!([{"service_name":saved,"container_name":"new-saved-container"}]);
        }
        value["config"]["pre_deploy"] =
            json!({"command":"echo saved-pre-hook","path":".","shell_mode":true});
        value["config"]["post_deploy"] =
            json!({"command":"echo saved-post-hook","path":".","shell_mode":true});
        value["config"]["file_contents"] = json!(
            "services:\n  web:\n    image: busybox:1.37\n    depends_on: [db]\n  db:\n    image: busybox:1.37\n# sensitive-compose-marker"
        );
        value["config"]["auto_pull"] = json!(false);
        value["config"]["extra_args"] = json!(["--remove-orphans"]);
    }
    value
}
fn json_response(status: u16, value: Value, chunked: bool) -> Response {
    let response = (StatusCode::from_u16(status).unwrap(), Json(value)).into_response();
    if !chunked {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    parts.headers.remove("content-length");
    Response::from_parts(
        parts,
        axum::body::Body::from_stream(body.into_data_stream()),
    )
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> Response {
    let (options, operation, params) = {
        let mut data = data.lock().unwrap();
        record(&mut data, &headers, "read", &envelope);
        (
            data.options.clone(),
            envelope["type"].as_str().unwrap().to_owned(),
            envelope["params"].clone(),
        )
    };
    if operation == "GetVersion" {
        return json_response(200, json!({"version":"2.3.2"}), false);
    }
    if operation == "GetUpdate" {
        let _: GetUpdate = serde_json::from_value(params.clone()).unwrap();
        tokio::time::sleep(options.observation_delay).await;
        if let Some(status) = options.observation_status {
            return json_response(
                status,
                json!({"error":format!("denied {KEY} {SECRET}")}),
                options.chunked,
            );
        }
        let value = data
            .lock()
            .unwrap()
            .updates
            .get(params["id"].as_str().unwrap())
            .cloned();
        return json_response(
            if value.is_some() { 200 } else { 500 },
            value.unwrap_or(json!({"error":"no update exists with given id"})),
            options.chunked,
        );
    }
    for family in FAMILIES {
        if operation == family.list_operation() {
            match family {
                Family::Stack => {
                    let _: ListStacks = serde_json::from_value(params.clone()).unwrap();
                }
                Family::Deployment => {
                    let _: ListDeployments = serde_json::from_value(params.clone()).unwrap();
                }
                Family::Server => (),
            }
            let item = summary(family, &options);
            let selected = params["query"]["names"].as_array().is_none_or(|names| {
                names.is_empty()
                    || names
                        .iter()
                        .any(|name| *name == item["id"] || *name == item["name"])
            });
            let mut items = if !options.missing && selected && params["page"] == 0 {
                vec![item]
            } else {
                vec![]
            };
            if options.ambiguous && !items.is_empty() {
                let mut extra = items[0].clone();
                extra["id"] = json!("000000000000000000000004");
                items.push(extra);
            }
            return json_response(200, json!(items), options.chunked);
        }
        if operation == family.get_operation() {
            match family {
                Family::Stack => {
                    let request: GetStack = serde_json::from_value(params).unwrap();
                    assert_eq!(request.stack, STACK);
                }
                Family::Deployment => {
                    let request: GetDeployment = serde_json::from_value(params).unwrap();
                    assert_eq!(request.deployment, DEPLOYMENT);
                }
                Family::Server => (),
            }
            return json_response(200, detail(family, &options), options.chunked);
        }
    }
    json_response(403, json!({"error":"unexpected read"}), false)
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(envelope): Json<Value>,
) -> Response {
    let mut data = data.lock().unwrap();
    record(&mut data, &headers, "execute", &envelope);
    data.executions += 1;
    let n = data.executions;
    let (target, operation, services) = match envelope["type"].as_str().unwrap() {
        "DeployStack" => {
            let r: DeployStack = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.stack, STACK);
            assert!(r.stop_time.is_none());
            (
                json!({"type":"Stack","id":r.stack}),
                if r.services.is_empty() {
                    "DeployStack"
                } else {
                    "DeployStackService"
                },
                r.services,
            )
        }
        "Deploy" => {
            let r: Deploy = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.deployment, DEPLOYMENT);
            assert!(r.stop_time.is_none() && r.stop_signal.is_none());
            (
                json!({"type":"Deployment","id":r.deployment}),
                "Deploy",
                vec![],
            )
        }
        "StartStack" => {
            let r: StartStack = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.stack, STACK);

            (
                json!({"type":"Stack","id":r.stack}),
                if r.services.is_empty() {
                    "StartStack"
                } else {
                    "StartStackService"
                },
                r.services,
            )
        }
        "StartDeployment" => {
            let r: StartDeployment = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.deployment, DEPLOYMENT);

            (
                json!({"type":"Deployment","id":r.deployment}),
                "StartDeployment",
                vec![],
            )
        }
        "StopStack" => {
            let r: StopStack = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.stack, STACK);
            assert!(r.stop_time.is_none());
            (
                json!({"type":"Stack","id":r.stack}),
                if r.services.is_empty() {
                    "StopStack"
                } else {
                    "StopStackService"
                },
                r.services,
            )
        }
        "StopDeployment" => {
            let r: StopDeployment = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.deployment, DEPLOYMENT);
            assert!(r.signal.is_none() && r.time.is_none());
            (
                json!({"type":"Deployment","id":r.deployment}),
                "StopDeployment",
                vec![],
            )
        }
        "RestartStack" => {
            let r: RestartStack = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.stack, STACK);

            (
                json!({"type":"Stack","id":r.stack}),
                if r.services.is_empty() {
                    "RestartStack"
                } else {
                    "RestartStackService"
                },
                r.services,
            )
        }
        "RestartDeployment" => {
            let r: RestartDeployment = serde_json::from_value(envelope["params"].clone()).unwrap();
            assert_eq!(r.deployment, DEPLOYMENT);

            (
                json!({"type":"Deployment","id":r.deployment}),
                "RestartDeployment",
                vec![],
            )
        }
        _ => panic!("unexpected execution"),
    };
    if let Ack::Reject(status) = data.options.ack {
        return json_response(
            status,
            json!({"error":format!("rejected {KEY} {SECRET}")}),
            data.options.chunked,
        );
    }
    let id = format!("{:024x}", 0x100000000000000000000000_u128 + n as u128);
    let mut update: Value =
        serde_json::from_str(include_str!("../fixtures/core-2.3.2/incident-wire.json")).unwrap();
    let update = &mut update["update"];
    update["_id"] = json!({"$oid":id});
    update["operation"] = json!(operation);
    update["target"] = target;
    update["status"] = json!("InProgress");
    update["end_ts"] = Value::Null;
    update["success"] = json!(false);
    update["logs"] = json!([]);
    let ack = update.clone();
    let outcome = match data.options.outcome {
        Outcome::SecondBusy if n == 2 => Outcome::Busy,
        Outcome::SecondBusy => Outcome::Succeeded,
        other => other,
    };
    if !matches!(outcome, Outcome::Running) {
        update["status"] = json!("Complete");
        update["end_ts"] = json!(1735689605000_i64);
        update["success"] = json!(matches!(outcome, Outcome::Succeeded));
        let message = match outcome {
            Outcome::Succeeded => "",
            Outcome::Denied => {
                "User does not have required permissions on this Stack. Must have at least Execute permissions"
            }
            Outcome::Busy => "Stack is busy; cannot run another action",
            Outcome::Paused => "Container is paused; the requested action failed",
            Outcome::Unavailable => "Periphery unavailable; the requested action failed",
            _ => "Compose rollout failed",
        };
        update["logs"] = if matches!(outcome, Outcome::Succeeded)
            && !operation.starts_with("Deploy")
        {
            json!([{"stage":"Compose Command","success":true,"start_ts":1,"end_ts":2,"stdout":format!("requested {operation} {services:?}"),"stderr":"","command":"sensitive-compose-marker"}])
        } else if matches!(outcome, Outcome::Succeeded) {
            json!([
            {"stage":"Pre Deploy","success":true,"start_ts":1,"end_ts":2,"stdout":"saved hook ran outside the selected service","stderr":"","command":"sensitive-stored-file-marker"},
            {"stage":"Compose Up","success":true,"start_ts":2,"end_ts":3,"stdout":format!("requested {services:?}; dependency db started"),"stderr":"","command":"sensitive-compose-marker"},
            {"stage":"Post Deploy","success":true,"start_ts":3,"end_ts":4,"stdout":"saved hook completed","stderr":"","command":"sensitive-stored-file-marker"}])
        } else {
            json!([{"stage":"Task Error","success":false,"start_ts":1,"end_ts":2,"stdout":"sensitive-environment-marker","stderr":format!("{message}\nAuthorization: Bearer {SECRET}\npassword={KEY}"),"command":"sensitive-stored-file-marker"}])
        };
    }
    if data.options.escaped_evidence {
        update["status"] = json!("Complete");
        update["success"] = json!(false);
        update["logs"] = json!((0..20).map(|i|json!({"stage":format!("failure {i}"),"success":false,"start_ts":1,"end_ts":2,"stdout":"","stderr":"\u{0001}".repeat(1024),"command":"sensitive-stored-file-marker"})).collect::<Vec<_>>());
    }
    data.updates.insert(id, update.clone());
    match data.options.ack {
        Ack::Valid => json_response(200, ack, data.options.chunked),
        Ack::WrongTarget => {
            let mut ack = ack;
            ack["target"]["id"] = json!("000000000000000000000099");
            json_response(200, ack, data.options.chunked)
        }
        Ack::Malformed => (StatusCode::OK, format!("malformed {KEY} {SECRET}")).into_response(),
        Ack::Oversized => json_response(
            200,
            json!({"padding":"x".repeat(2*1024*1024)}),
            data.options.chunked,
        ),
        Ack::Reject(_) => unreachable!(),
    }
}
async fn connect(transport: tokio::io::DuplexStream) -> Session {
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("operation-test", "1"),
    )
    .with_protocol_version(ProtocolVersion::V_2025_11_25);
    timeout(
        DEADLINE,
        config.serve_with_lifecycle(transport, ClientLifecycleMode::Initialize),
    )
    .await
    .unwrap()
    .unwrap()
}
pub async fn call(session: &Session, name: &str, args: Value) -> CallToolResult {
    timeout(
        DEADLINE,
        session.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}
pub async fn close(mut session: Session, mut task: Task) {
    timeout(SHUTDOWN, session.close()).await.unwrap().unwrap();
    timeout(SHUTDOWN, &mut task.0).await.unwrap().unwrap();
}
pub struct Process {
    child: Child,
    stderr: Arc<Mutex<Vec<u8>>>,
    _reader: Task,
}
impl Process {
    pub async fn start(core: &Core, operate: bool) -> (Self, Session) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", KEY)
            .env("KOMODO_API_SECRET", SECRET)
            .env(
                "KOMODO_ALLOW_OPERATE",
                if operate { "true" } else { "false" },
            )
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut pipe = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(vec![]));
        let captured = stderr.clone();
        let reader = Task(tokio::spawn(async move {
            let mut buf = [0; 4096];
            loop {
                let n = pipe.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                let mut c = captured.lock().unwrap();
                let keep = n.min((1024 * 1024_usize).saturating_sub(c.len()));
                c.extend_from_slice(&buf[..keep]);
            }
        }));
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("operation-stdio-test", "1"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25);
        let session = timeout(
            DEADLINE,
            config.serve_with_lifecycle((stdout, stdin), ClientLifecycleMode::Initialize),
        )
        .await
        .unwrap()
        .unwrap();
        (
            Self {
                child,
                stderr,
                _reader: reader,
            },
            session,
        )
    }
    pub async fn finish(mut self, mut session: Session) {
        let result = timeout(SHUTDOWN, session.close()).await;
        if result.is_err() {
            self.child.start_kill().unwrap();
        }
        let status = match timeout(SHUTDOWN, self.child.wait()).await {
            Ok(s) => s.unwrap(),
            Err(_) => {
                self.child.start_kill().unwrap();
                timeout(SHUTDOWN, self.child.wait()).await.unwrap().unwrap()
            }
        };
        timeout(SHUTDOWN, &mut self._reader.0)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap().is_ok());
        assert!(status.success());
        assert!(self.child.id().is_none());
        super::assert_safe(std::str::from_utf8(&self.stderr.lock().unwrap()).unwrap());
    }
}
