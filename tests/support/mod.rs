#![allow(dead_code)] // Shared by integration binaries exercising different surfaces.

pub mod incident;
pub mod inventory;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use komodo_client::{
    api::read::{KomodoReadRequest, ListServers},
    entities::{
        ResourceTarget,
        permission::{Permission, PermissionLevel, UserTarget},
        user::User,
    },
};
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};

pub const KEY: &str = "fixture-API-KEY-7d42";
pub const SECRET: &str = "fixture-API-SECRET-a913";
pub const VERSION: &str = include_str!("../fixtures/core-2.3.2/version.json");
pub const SERVERS: &str = include_str!("../fixtures/core-2.3.2/servers.json");
pub const PERMISSION: &str = include_str!("../fixtures/core-2.3.2/permission.json");
pub const AUTHENTICATION: &str = include_str!("../fixtures/core-2.3.2/authentication.json");
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub header_delay: Duration,
    pub body_delay: Duration,
    pub location: Option<String>,
}

impl Reply {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            header_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            location: None,
        }
    }

    pub fn echoed(status: u16, body: &str) -> Self {
        let mut body: Value = serde_json::from_str(body).unwrap();
        body["trace"] = json!([format!("proxy echoed key={KEY} secret={SECRET}")]);
        Self::new(status, body.to_string())
    }
}

pub struct Step {
    pub operation: &'static str,
    pub params: Value,
    pub reply: Reply,
}

impl Step {
    pub fn new(operation: &'static str, reply: Reply) -> Self {
        let params = match operation {
            "GetVersion" => json!({}),
            "ListServers" => serde_json::to_value(server_page()).unwrap(),
            "GetServer" => json!({"server":"restricted-server"}),
            _ => panic!("use Step::read for explicit typed fixture requests"),
        };
        Self {
            operation,
            params,
            reply,
        }
    }
    pub fn read<T: serde::Serialize + KomodoReadRequest>(request: T, reply: Reply) -> Self {
        Self {
            operation: T::req_type(),
            params: serde_json::to_value(request).unwrap(),
            reply,
        }
    }
    pub fn version() -> Self {
        Self::new("GetVersion", Reply::new(200, VERSION))
    }
}

#[derive(Default)]
struct State {
    steps: VecDeque<Step>,
    operations: Vec<String>,
    connections: usize,
    errors: Vec<String>,
}

pub struct MockCore {
    pub address: String,
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    task: JoinHandle<()>,
}

impl MockCore {
    pub async fn start(steps: Vec<Step>) -> Result<Self> {
        let user: User = serde_json::from_str(include_str!("../fixtures/core-2.3.2/reader.json"))?;
        ensure!(
            user.enabled && !user.admin && !user.super_admin,
            "fixture must be an enabled non-admin"
        );
        let permission: Permission = serde_json::from_str(include_str!(
            "../fixtures/core-2.3.2/reader-permission.json"
        ))?;
        ensure!(
            permission.level == PermissionLevel::Read,
            "fixture must grant read access"
        );
        ensure!(
            matches!(&permission.user_target, UserTarget::User(id) if id == &user.id),
            "permission must belong to fixture identity"
        );
        ensure!(
            matches!(&permission.resource_target, ResourceTarget::Server(id) if id == "000000000000000000000001"),
            "permission must refer to inventory server"
        );
        let permissions: Vec<Permission> = serde_json::from_str(include_str!(
            "../fixtures/core-2.3.2/reader-permissions.json"
        ))?;
        for permission in &permissions {
            ensure!(
                permission.level == PermissionLevel::Read && permission.specific.is_empty(),
                "inventory grants must be Read only"
            );
            ensure!(
                matches!(&permission.user_target, UserTarget::User(id) if id == &user.id),
                "grant must belong to fixture identity"
            );
        }
        for resource in [
            ResourceTarget::Server("000000000000000000000001".into()),
            ResourceTarget::Stack("000000000000000000000002".into()),
            ResourceTarget::Deployment("000000000000000000000003".into()),
        ] {
            ensure!(
                permissions
                    .iter()
                    .any(|permission| permission.resource_target == resource),
                "inventory family must have a Read grant"
            );
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = format!("http://{}/komodo", listener.local_addr()?);
        let state = Arc::new(Mutex::new(State {
            steps: steps.into(),
            ..Default::default()
        }));
        let changed = Arc::new(Notify::new());
        let state_task = Arc::clone(&state);
        let changed_task = Arc::clone(&changed);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { return };
                        state_task.lock().unwrap().connections += 1;
                        let state = Arc::clone(&state_task);
                        let changed = Arc::clone(&changed_task);
                        connections.spawn(async move {
                            if let Err(error) = timeout(Duration::from_secs(30), serve(stream, &state, &changed)).await {
                                state.lock().unwrap().errors.push(format!("fixture connection deadline: {error}"));
                            }
                        });
                    }
                    Some(joined) = connections.join_next(), if !connections.is_empty() => {
                        if let Err(error) = joined { state_task.lock().unwrap().errors.push(format!("fixture task: {error}")); }
                    }
                }
            }
        });
        Ok(Self {
            address,
            state,
            changed,
            task,
        })
    }

    pub fn config(&self) -> Config {
        Config::parse(&format!("{}/", self.address), KEY, SECRET).unwrap()
    }

    pub fn client(&self, deadline: Duration) -> CoreClient {
        CoreClient::new(
            self.config(),
            RequestPolicy {
                connect_timeout: Duration::from_millis(200),
                operation_timeout: deadline,
            },
        )
        .unwrap()
    }

    pub async fn wait_requests(&self, count: usize) -> Result<()> {
        timeout(TEST_TIMEOUT, async {
            loop {
                let notified = self.changed.notified();
                if self.state.lock().unwrap().operations.len() >= count {
                    break;
                }
                notified.await;
            }
        })
        .await
        .context("wait for fixture request")
    }

    pub fn assert_finished(&self, expected: &[&str]) {
        let state = self.state.lock().unwrap();
        assert_eq!(state.operations, expected);
        assert!(state.steps.is_empty(), "unconsumed fixture steps");
        assert!(
            state.errors.is_empty(),
            "fixture errors: {:?}",
            state.errors
        );
    }

    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }
}

impl Drop for MockCore {
    fn drop(&mut self) {
        self.task.abort();
    } // Dropping JoinSet aborts every connection.
}

async fn serve(stream: TcpStream, state: &Mutex<State>, changed: &Notify) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let request = async {
            let mut start = String::new();
            if read.read_line(&mut start).await? == 0 {
                return Ok(None);
            }
            ensure!(
                start.trim_end() == "POST /komodo/read HTTP/1.1",
                "fixture rejects non-read endpoint"
            );
            let mut length = None;
            let mut key = None;
            let mut secret = None;
            loop {
                let mut line = String::new();
                ensure!(read.read_line(&mut line).await? > 0, "incomplete headers");
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').context("invalid header")?;
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = Some(value.trim().parse::<usize>()?),
                    "x-api-key" => key = Some(value.trim().to_owned()),
                    "x-api-secret" => secret = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
            ensure!(
                key.as_deref() == Some(KEY) && secret.as_deref() == Some(SECRET),
                "incorrect synthetic credential headers"
            );
            let length = length.context("missing content-length")?;
            ensure!(length < 64 * 1024, "oversized fixture request");
            let mut body = vec![0; length];
            read.read_exact(&mut body).await?;
            let envelope: Value = serde_json::from_slice(&body)?;
            let operation = envelope["type"]
                .as_str()
                .context("missing SDK request type")?;
            let step = {
                let mut state = state.lock().unwrap();
                state.operations.push(operation.to_owned());
                state
                    .steps
                    .pop_front()
                    .context("unexpected extra request")?
            };
            ensure!(step.operation == operation, "wrong fixture operation");
            ensure!(
                matches!(
                    operation,
                    "GetVersion"
                        | "ListServers"
                        | "ListStacks"
                        | "ListDeployments"
                        | "GetServer"
                        | "GetStack"
                        | "GetDeployment"
                        | "GetContainerLog"
                        | "GetDeploymentLog"
                        | "GetStackLog"
                ),
                "fixture rejects unexpected SDK request"
            );
            ensure!(
                envelope == json!({"type":operation,"params":step.params}),
                "incorrect SDK request envelope for {operation}"
            );
            Ok::<_, anyhow::Error>(Some(step.reply))
        }
        .await;
        changed.notify_one();
        let reply = match request {
            Ok(Some(reply)) => reply,
            Ok(None) => return,
            Err(error) => {
                state.lock().unwrap().errors.push(error.to_string());
                return;
            }
        };
        sleep(reply.header_delay).await;
        let location = reply
            .location
            .map(|location| format!("Location: {location}\r\n"))
            .unwrap_or_default();
        let header = format!(
            "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{location}\r\n",
            reply.status,
            reply.body.len()
        );
        if write.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        if write.flush().await.is_err() {
            return;
        }
        sleep(reply.body_delay).await;
        if write.write_all(reply.body.as_bytes()).await.is_err() {
            return;
        }
        if write.flush().await.is_err() {
            return;
        }
    }
}

pub fn assert_safe(text: &str) {
    assert!(!text.contains(KEY), "API key leaked");
    assert!(!text.contains(SECRET), "API secret leaked");
    for marker in SENSITIVE_MARKERS {
        assert!(
            !text.contains(marker),
            "sensitive fixture field leaked: {marker}"
        );
    }
}

pub const SENSITIVE_MARKERS: &[&str] = &[
    "sensitive-config-credential-marker",
    "sensitive-environment-marker",
    "sensitive-compose-marker",
    "sensitive-stored-file-marker",
    "sensitive-raw-trace-marker",
    "sensitive-description-marker",
];

pub fn server_page() -> ListServers {
    ListServers {
        limit: Some(30),
        ..Default::default()
    }
}
