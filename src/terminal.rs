//! One command, one private terminal, no attach, replay or cross-call state.
mod stream;
mod target;

use crate::{
    capabilities::Capability,
    client::CoreClient,
    error::{AppError, ErrorKind},
    export,
    inventory::DetailInput,
};
use komodo_client::{
    api::{
        read::ListTerminals,
        write::{CreateTerminal, DeleteTerminal},
    },
    entities::terminal::{ContainerTerminalMode, TerminalRecreateMode, TerminalTarget},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, oneshot, watch};

const OP: &str = "execute_terminal";
const GUARD_TIME: Duration = Duration::from_secs(10);
const LIFECYCLE_TIME: Duration = Duration::from_secs(10);
const TTL: u64 = 90;
// Pinned Periphery 2.3.2 writes this complete physical line into a default
// canonical Linux PTY. Count UTF-8 bytes after POSIX quoting, including its
// status envelope and newline, conservatively within the 4095-byte capacity.
const PTY_LINE_BYTES: usize = 4095;
const PTY_PREFIX: &str = "printf '\\n__KOMODO_START_OF_OUTPUT__\\n\\n'; ";
const PTY_SUFFIX: &str =
    "; rc=$?; printf '\\n__KOMODO_EXIT_CODE:%d\\n__KOMODO_END_OF_OUTPUT__\\n' \"$rc\"\n";

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ContainerSelector {
    Id { id: String },
    Name { name: String },
}
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Server {
        target: DetailInput,
    },
    Container {
        server: DetailInput,
        container: ContainerSelector,
    },
    Stack {
        target: DetailInput,
        service: String,
    },
    Deployment {
        target: DetailInput,
    },
}
fn observation_default() -> u64 {
    15
}
fn output_default() -> usize {
    4096
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub target: Target,
    pub command: String,
    #[serde(default = "observation_default")]
    pub observation_seconds: u64,
    #[serde(default = "output_default")]
    pub max_output_bytes: usize,
}
fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
fn require(client: &CoreClient) -> Result<(), AppError> {
    if client.capabilities().allows(Capability::Terminal) {
        Ok(())
    } else {
        Err(AppError::new(ErrorKind::CapabilityDisabled, OP, None))
    }
}
impl Input {
    fn validate(&self) -> Result<(), AppError> {
        if self.command.is_empty()
            || self.command.len() > 4096
            || self.command.chars().any(char::is_control)
            || !(1..=30).contains(&self.observation_seconds)
            || !(1..=16384).contains(&self.max_output_bytes)
        {
            return Err(AppError::arguments(
                OP,
                "Use one nonempty physical command line of at most 4096 bytes, observation_seconds 1–30 and max_output_bytes 1–16384",
            ));
        }
        let selector = match &self.target {
            Target::Server { target } | Target::Deployment { target } => target,
            Target::Stack { target, service } => {
                if !target::service_name(service) {
                    return Err(AppError::arguments(
                        OP,
                        "Select one exact Stack service name",
                    ));
                }
                target
            }
            Target::Container { server, container } => {
                let valid = match container {
                    ContainerSelector::Id { id } => target::docker_id(id),
                    ContainerSelector::Name { name } => target::docker_name(name),
                };
                if !valid {
                    return Err(AppError::arguments(
                        OP,
                        "Use a complete 64-hex Docker ID or one exact Docker name; prefixes are unsupported",
                    ));
                }
                server
            }
        };
        wrapped_command(&self.command)?;
        export::validate_selector(selector)
    }
}

#[derive(Default)]
struct State {
    closing: bool,
    active: BTreeMap<String, watch::Sender<bool>>,
}
#[derive(Default)]
pub(crate) struct Runtime {
    state: Mutex<State>,
    finished: Notify,
}
struct Registration {
    runtime: Arc<Runtime>,
    name: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut state) = self.runtime.state.lock() {
            state.active.remove(&self.name);
        }
        self.runtime.finished.notify_waiters();
    }
}
struct CancelOnDrop(watch::Sender<bool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}
impl Runtime {
    fn register(
        self: &Arc<Self>,
    ) -> Result<(Registration, CancelOnDrop, watch::Receiver<bool>, String), AppError> {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|_| invalid())?;
        let hex = |s: &[u8]| s.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let name = format!("mcp-{}", hex(&bytes[..16]));
        let nonce = hex(&bytes[16..]);
        let mut state = self.state.lock().map_err(|_| invalid())?;
        if state.closing || state.active.len() >= 4 || state.active.contains_key(&name) {
            return Err(AppError::arguments(
                OP,
                "At most four terminal calls may be active; closing sessions cannot accept work",
            ));
        }
        let (tx, rx) = watch::channel(false);
        state.active.insert(name.clone(), tx.clone());
        Ok((
            Registration {
                runtime: Arc::clone(self),
                name,
            },
            CancelOnDrop(tx),
            rx,
            nonce,
        ))
    }
    pub async fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closing = true;
            for tx in state.active.values() {
                let _ = tx.send(true);
            }
        }
        let drained = async {
            loop {
                let notified = self.finished.notified();
                if self.state.lock().is_ok_and(|s| s.active.is_empty()) {
                    break;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(25), drained)
            .await
            .is_err()
        {
            tracing::warn!(
                "terminal cleanup drain exceeded its budget; remote lifetime limits remain in effect"
            );
        }
    }
}
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    let _ = rx.wait_for(|v| *v).await;
}

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Create,
    Command,
    Delete,
}
/// Sealed authority: only this module can construct the target, initializer,
/// original private baseline and owned record used by the shared transport.
pub(crate) struct Owned {
    pinned: target::Pinned,
    name: String,
    initializer: String,
    expected_command: String,
    command: String,
    record: Option<Value>,
    create_attempts: AtomicUsize,
    command_attempts: AtomicUsize,
    delete_attempts: AtomicUsize,
}
impl Owned {
    pub(crate) fn mark_attempt(&self, phase: Phase) {
        match phase {
            Phase::Create => &self.create_attempts,
            Phase::Command => &self.command_attempts,
            Phase::Delete => &self.delete_attempts,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn body(&self, phase: Phase) -> Result<Value, AppError> {
        match phase {
            Phase::Create => serde_json::to_value(CreateTerminal {
                name: Some(self.name.clone()),
                target: self.pinned.target.clone(),
                command: Some(self.initializer.clone()),
                mode: if matches!(self.pinned.target, TerminalTarget::Server { .. }) {
                    None
                } else {
                    Some(ContainerTerminalMode::Exec)
                },
                recreate: TerminalRecreateMode::Never,
            }),
            Phase::Delete => serde_json::to_value(DeleteTerminal {
                target: self.pinned.target.clone(),
                terminal: self.name.clone(),
            }),
            Phase::Command => {
                serde_json::to_value(komodo_client::api::terminal::ExecuteTerminalBody {
                    target: self.pinned.target.clone(),
                    terminal: Some(self.name.clone()),
                    command: self.command.clone(),
                    init: None,
                })
            }
        }
        .map_err(|_| invalid())
    }
    pub(crate) async fn guard(&self, client: &CoreClient, phase: Phase) -> Result<(), AppError> {
        require(client)?;
        self.pinned
            .check(client, !matches!(phase, Phase::Delete))
            .await?;
        let records = self.list(client).await?;
        match phase {
            Phase::Create
                if records.is_empty() && self.create_attempts.load(Ordering::Relaxed) == 0 =>
            {
                Ok(())
            }
            Phase::Command | Phase::Delete
                if self.record.is_some()
                    && records.len() == 1
                    && self.identity(&records[0]).as_ref() == self.record.as_ref() =>
            {
                let attempted = match phase {
                    Phase::Command => &self.command_attempts,
                    _ => &self.delete_attempts,
                };
                if attempted.load(Ordering::Relaxed) == 0 {
                    Ok(())
                } else {
                    Err(invalid())
                }
            }
            _ => Err(AppError::new(ErrorKind::StalePreview, OP, None)),
        }
    }
    fn identity(&self, record: &Value) -> Option<Value> {
        let mut actual = record["target"].clone();
        // Pinned scoped Server lists return server=None on Periphery. This
        // normalization is valid only in this positively resolved Server scope.
        if matches!(self.pinned.target, TerminalTarget::Server { .. })
            && actual == json!({"type":"Server","params":{"server":null}})
        {
            actual = serde_json::to_value(&self.pinned.target).ok()?;
        }
        if record["name"] != self.name
            || record["command"] != self.expected_command
            || actual != serde_json::to_value(&self.pinned.target).ok()?
            || !record["created_at"].as_i64().is_some_and(|v| v > 0)
        {
            return None;
        }
        Some(
            json!({"name":record["name"],"target":actual,"command":record["command"],"created_at":record["created_at"]}),
        )
    }
    async fn list(&self, client: &CoreClient) -> Result<Vec<Value>, AppError> {
        require(client)?;
        let raw = client
            .read_references(ListTerminals {
                target: Some(self.pinned.scope()),
                terms: vec![self.name.clone()],
                page: 0,
                limit: Some(2),
                use_names: false,
                ..Default::default()
            })
            .await?;
        let entries = raw.as_array().filter(|v| v.len() < 2).ok_or_else(invalid)?;
        // A contains-term response which is not the exact private name is a
        // collision/incomplete proof, never a reason to pick the first terminal.
        if entries.iter().any(|v| v["name"] != self.name) {
            return Err(invalid());
        }
        Ok(entries.clone())
    }
}

#[derive(Serialize)]
pub struct Output {
    pub target: Value,
    pub terminal: String,
    pub observation: Value,
    pub cleanup: Value,
    pub attempts: Value,
    pub duration_ms: u128,
    pub remote_shell_lifetime_seconds: u64,
    pub limitations: Vec<&'static str>,
}

pub async fn execute(
    client: &CoreClient,
    input: Input,
    cancel: impl Future<Output = ()>,
) -> Result<Output, AppError> {
    require(client)?;
    input.validate()?;
    let (registration, drop_cancel, rx, nonce) = client.terminal_runtime().register()?;
    let client = client.clone();
    let (tx, result) = oneshot::channel();
    tokio::spawn(async move {
        let result = worker(&client, input, registration.name.clone(), nonce, rx).await;
        let _ = tx.send(result);
        drop(registration);
    });
    tokio::pin!(result);
    let response = tokio::select! {
        r=&mut result=>r,
        _=cancel=> {let _=drop_cancel.0.send(true);result.await},
    };
    drop(drop_cancel);
    response.map_err(|_| invalid())?
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn wrapped_command(line: &str) -> Result<String, AppError> {
    let command = format!("sh -c {}", quote(line));
    if PTY_PREFIX.len() + command.len() + PTY_SUFFIX.len() > PTY_LINE_BYTES {
        return Err(AppError::arguments(
            OP,
            "The fully shell-escaped command plus pinned Periphery status envelope must fit within 4095 UTF-8 bytes; shorten the command or reduce literal apostrophes",
        ));
    }
    Ok(command)
}
async fn worker(
    client: &CoreClient,
    input: Input,
    name: String,
    nonce: String,
    mut cancel: watch::Receiver<bool>,
) -> Result<Output, AppError> {
    let start = Instant::now();
    let command = wrapped_command(&input.command)?;
    let pinned = tokio::select! {
        _=cancelled(&mut cancel)=>return Err(AppError::new(ErrorKind::Timeout,OP,None)),
        r=tokio::time::timeout(GUARD_TIME,target::prepare(client,input.target,true))=>r.map_err(|_|AppError::new(ErrorKind::Timeout,OP,None))??,
    };
    let initializer = format!(
        "timeout -k 1 {TTL} sh -c 'while IFS= read -r mcp_line; do eval \"$mcp_line\"; done' {nonce}"
    );
    let expected_command = match &pinned.container_name {
        Some(container) => format!("docker exec -it {container} {initializer}"),
        None => initializer.clone(),
    };
    let mut owned = Owned {
        pinned,
        name,
        initializer,
        expected_command,
        command,
        record: None,
        create_attempts: AtomicUsize::new(0),
        command_attempts: AtomicUsize::new(0),
        delete_attempts: AtomicUsize::new(0),
    };
    let create = tokio::select! {
        _=cancelled(&mut cancel)=>None,
        r=tokio::time::timeout(GUARD_TIME+LIFECYCLE_TIME,client.terminal_step(&owned,Phase::Create))=>Some(r),
    };
    let mut observation = stream::Observation::unknown("creation_unconfirmed");
    let mut confirmed = false;
    match create {
        Some(Ok(Ok(record))) => {
            owned.record = owned.identity(&record);
            if owned.record.is_some() {
                confirmed = matches!(tokio::time::timeout(GUARD_TIME,owned.list(client)).await,Ok(Ok(records)) if records.len()==1 && owned.identity(&records[0]).as_ref()==owned.record.as_ref());
            }
        }
        Some(Ok(Err(error))) => {
            observation.error_kind = Some(error.kind);
            if owned.create_attempts.load(Ordering::Relaxed) == 0 {
                observation.end = "creation_rejected_before_submission";
            }
        }
        _ => {}
    }
    if confirmed {
        let command = client.terminal_stream(&owned);
        observation = tokio::select! {
            _=cancelled(&mut cancel)=>stream::Observation::unknown("cancelled_remote_completion_unknown"),
            r=tokio::time::timeout(Duration::from_secs(input.observation_seconds),command)=>match r {
                Err(_)=>stream::Observation::unknown("deadline_remote_completion_unknown"),
                Ok(Err(error))=> {
                    let mut observation=stream::Observation::unknown(if owned.command_attempts.load(Ordering::Relaxed)==0 {"command_rejected_before_submission"} else {"upstream_failure_remote_completion_unknown"});
                    observation.error_kind=Some(error.kind);
                    observation
                },
                Ok(Ok(bytes))=>stream::decode(client,bytes,input.max_output_bytes),
            },
        };
    }
    // Cleanup continues even when the RPC future/stdin disappears. A cancelled
    // create can finish late; an empty list then proves only current absence.
    let cleanup = tokio::time::timeout(LIFECYCLE_TIME, cleanup(client, &mut owned, confirmed))
        .await
        .unwrap_or_else(|_| json!({"state":"unconfirmed","reason":"cleanup_deadline"}));
    let attempts = json!({"create":owned.create_attempts.load(Ordering::Relaxed),"command":owned.command_attempts.load(Ordering::Relaxed),"delete":owned.delete_attempts.load(Ordering::Relaxed)});
    tracing::info!(caller_id=%owned.pinned.caller_id,target_id=%owned.pinned.resource_id,terminal=%owned.name,duration_ms=start.elapsed().as_millis(),end=observation.end,create_attempts=owned.create_attempts.load(Ordering::Relaxed),command_attempts=owned.command_attempts.load(Ordering::Relaxed),delete_attempts=owned.delete_attempts.load(Ordering::Relaxed),cleanup=%cleanup["state"],"terminal observation ended");
    Ok(Output {
        target: serde_json::to_value(&owned.pinned.target).map_err(|_| invalid())?,
        terminal: owned.name,
        observation: serde_json::to_value(observation).map_err(|_| invalid())?,
        cleanup,
        attempts,
        duration_ms: start.elapsed().as_millis(),
        remote_shell_lifetime_seconds: TTL,
        limitations: vec![
            "An exit marker is spoofable upstream-reported evidence, not independently verified completion; no Core update ID exists.",
            "Cleanup observes session removal, not descendant termination or rollback. The remote timeout bounds the normal shell from its start, not Core queue delay, escaped children or arbitrary commands which interfere with it.",
            "Core has no terminal identity CAS or atomic reservation. Private names and identity checks cannot exclude a concurrently privileged upstream replacement or final container-name race.",
            "Core/Periphery can log commands. Known-secret redaction cannot guarantee arbitrary output is secret-free.",
        ],
    })
}

async fn cleanup(client: &CoreClient, owned: &mut Owned, confirmed: bool) -> Value {
    if owned.create_attempts.load(Ordering::Relaxed) == 0 {
        return json!({"state":"not_created"});
    }
    if owned.pinned.check(client, false).await.is_err() {
        return json!({"state":"unconfirmed","reason":"routing_or_authority_changed"});
    }
    let records = match owned.list(client).await {
        Ok(v) => v,
        Err(_) => return json!({"state":"unconfirmed","reason":"ownership_read_failed"}),
    };
    if records.is_empty() {
        return if confirmed {
            json!({"state":"absence_observed","late_creation_possible":false})
        } else {
            json!({"state":"unconfirmed","absence_observed":true,"late_creation_possible":true})
        };
    }
    let identity = owned.identity(&records[0]);
    if identity.is_none()
        || owned
            .record
            .as_ref()
            .is_some_and(|r| Some(r) != identity.as_ref())
    {
        return json!({"state":"unconfirmed","reason":"foreign_or_changed_terminal_preserved"});
    }
    if owned.record.is_none() {
        owned.record = identity;
    }
    let _ = client.terminal_step(owned, Phase::Delete).await;
    match owned.list(client).await {
        Ok(records) if records.is_empty() => {
            json!({"state":"absence_observed","late_creation_possible":false,"delete_acknowledgement_required":false})
        }
        _ => json!({"state":"unconfirmed","reason":"removal_not_observed"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities::CapabilityPolicy, client::RequestPolicy, config::Config};
    #[tokio::test]
    async fn closed_transport_never_uses_configuration_edit_as_terminal_authority_or_accepts_generic_writes()
     {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        for operate in [false, true] {
            for edit in [false, true] {
                for prune in [false, true] {
                    for terminal in [false, true] {
                        let client = CoreClient::new(
                            Config::parse(
                                &format!("http://{}", listener.local_addr().unwrap()),
                                "test-key",
                                "test-secret",
                            )
                            .unwrap()
                            .with_capabilities(
                                CapabilityPolicy::new(operate, edit)
                                    .with_pruning(prune)
                                    .with_terminal(terminal),
                            ),
                            RequestPolicy::default(),
                        )
                        .unwrap();
                        let target = TerminalTarget::Server {
                            server: Some("invalid-id".into()),
                        };
                        let owned = Owned {
                            pinned: target::Pinned {
                                spec: Target::Server {
                                    target: DetailInput {
                                        id: Some("invalid-id".into()),
                                        name: None,
                                    },
                                },
                                target: target.clone(),
                                baseline: Value::Null,
                                container: None,
                                container_name: None,
                                caller_id: String::new(),
                                resource_id: String::new(),
                            },
                            name: "mcp-fixture".into(),
                            initializer: String::new(),
                            expected_command: String::new(),
                            command: String::new(),
                            record: None,
                            create_attempts: AtomicUsize::new(0),
                            command_attempts: AtomicUsize::new(0),
                            delete_attempts: AtomicUsize::new(0),
                        };
                        for phase in [Phase::Create, Phase::Delete] {
                            let error = client.terminal_step(&owned, phase).await.unwrap_err();
                            assert_eq!(
                                error.kind,
                                if operate && terminal {
                                    ErrorKind::InvalidArguments
                                } else {
                                    ErrorKind::CapabilityDisabled
                                }
                            );
                        }
                        assert_eq!(
                            client.terminal_stream(&owned).await.unwrap_err().kind,
                            if operate && terminal {
                                ErrorKind::InvalidArguments
                            } else {
                                ErrorKind::CapabilityDisabled
                            }
                        );
                        assert_eq!(
                            client
                                .write_checked(CreateTerminal {
                                    name: Some("foreign".into()),
                                    target: target.clone(),
                                    command: None,
                                    mode: None,
                                    recreate: TerminalRecreateMode::Always
                                })
                                .await
                                .unwrap_err()
                                .kind,
                            ErrorKind::UnsupportedTarget
                        );
                        assert_eq!(
                            client
                                .write_checked(DeleteTerminal {
                                    target,
                                    terminal: "foreign".into()
                                })
                                .await
                                .unwrap_err()
                                .kind,
                            ErrorKind::UnsupportedTarget
                        );
                        assert_eq!(owned.command_attempts.load(Ordering::Relaxed), 0);
                    }
                }
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    }
}
