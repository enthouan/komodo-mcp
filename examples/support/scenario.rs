//! Official rmcp client over the compiled production process's stdio pipes.
#![allow(dead_code)] // Shared by the executable example and integration tests.

use super::mock_core::{DEADLINE, KEY, MockCore, RequestEvidence, SECRET};
use anyhow::{Context, Result, bail, ensure};
use rmcp::{
    ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
    },
    service::{ClientLifecycleMode, RunningService},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::JoinHandle,
    time::timeout,
};

const PAGE_SIZE: u64 = 2;
const MAX_PAGES: u64 = 32;
const SCENARIO_DEADLINE: Duration = Duration::from_secs(90);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Server,
    Swarm,
}
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    pub id: String,
}
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StackState {
    Deploying,
    Running,
    Paused,
    Stopped,
    Created,
    Restarting,
    Dead,
    Removing,
    Unhealthy,
    Down,
    Unknown,
}
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Stack {
    pub id: String,
    pub name: String,
    pub state: StackState,
    #[serde(deserialize_with = "required_option")]
    pub target: Option<Target>,
}
#[derive(Debug)]
pub struct Answer {
    pub server: Server,
    pub stacks: Vec<Stack>,
    pub pages: u64,
    pub visible: usize,
}
impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Stacks on {} ({}):", self.server.name, self.server.id)?;
        if self.stacks.is_empty() {
            writeln!(
                f,
                "  No matching stacks in the {0} visible stacks.",
                self.visible
            )?;
        }
        for stack in &self.stacks {
            writeln!(
                f,
                "  {} ({}) — {}",
                stack.name,
                stack.id,
                format!("{:?}", stack.state).to_lowercase()
            )?;
        }
        write!(
            f,
            "Scanned {} pages ({} visible stacks).",
            self.pages, self.visible
        )
    }
}
#[derive(Deserialize)]
struct Page {
    items: Vec<Stack>,
    page: u64,
    page_size: u64,
    has_more: bool,
    #[serde(deserialize_with = "required_option")]
    next_page: Option<u64>,
}

fn required_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
impl Page {
    fn continuation(&self, requested: u64) -> Result<Option<u64>> {
        ensure!(
            self.page == requested
                && self.page_size == PAGE_SIZE
                && self.items.len() as u64 <= PAGE_SIZE,
            "list_stacks returned inconsistent page metadata; incomplete answer"
        );
        match (self.has_more, self.next_page) {
            (false, None) => Ok(None),
            (true, Some(next))
                if Some(next) == requested.checked_add(1)
                    && self.items.len() as u64 == PAGE_SIZE =>
            {
                Ok(Some(next))
            }
            _ => bail!("list_stacks continuation made no valid progress; incomplete answer"),
        }
    }
}

pub fn check_safe(text: &str) -> Result<()> {
    for marker in [
        KEY,
        SECRET,
        "wrong-synthetic-key",
        "wrong-synthetic-secret",
        "sensitive-config-credential-marker",
        "sensitive-environment-marker",
        "sensitive-compose-marker",
        "sensitive-stored-file-marker",
        "sensitive-raw-trace-marker",
        "sensitive-description-marker",
    ] {
        ensure!(
            !text.contains(marker),
            "credential or sensitive fixture marker escaped the server"
        );
    }
    Ok(())
}
fn check_resource(id: &str, name: &str) -> Result<()> {
    ensure!(
        id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()) && !name.trim().is_empty(),
        "invalid resource identity in MCP response"
    );
    Ok(())
}

#[derive(Clone, Copy, Default)]
pub enum Configuration {
    #[default]
    Synthetic,
    MissingSecret,
    WrongKey,
    WrongSecret,
}

pub struct Report {
    pub result: Result<Answer>,
    pub address: String,
    pub child_id: u32,
    pub status: ExitStatus,
    pub forced_kill: bool,
    pub stderr: String,
    pub transcript: Vec<Value>,
    pub requests: Vec<RequestEvidence>,
    pub rejected_credentials: usize,
}
struct Process {
    child: Child,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<std::io::Result<()>>,
}
impl Process {
    fn spawn(executable: &Path, address: &str, config: Configuration) -> Result<Self> {
        let mut command = Command::new(executable);
        for field in [
            "KOMODO_ADDRESS",
            "KOMODO_API_KEY",
            "KOMODO_API_SECRET",
            "KOMODO_ALLOW_OPERATE",
            "KOMODO_ALLOW_CONFIGURATION_EDIT",
        ] {
            command.env_remove(field);
        }
        command
            .env("KOMODO_ADDRESS", address)
            .env("KOMODO_API_KEY", KEY)
            .env("KOMODO_API_SECRET", SECRET);
        match config {
            Configuration::Synthetic => {}
            Configuration::MissingSecret => {
                command.env_remove("KOMODO_API_SECRET");
            }
            Configuration::WrongKey => {
                command.env("KOMODO_API_KEY", "wrong-synthetic-key");
            }
            Configuration::WrongSecret => {
                command.env("KOMODO_API_SECRET", "wrong-synthetic-secret");
            }
        }
        Self::start(command)
    }
    fn start(mut command: Command) -> Result<Self> {
        let mut child = command
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("launch compiled komodo-mcp; build it first and pass its executable path")?;
        let mut stderr_pipe = child.stderr.take().context("piped server stderr")?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let output = stderr.clone();
        let reader = tokio::spawn(async move {
            let mut bytes = [0; 4096];
            loop {
                let count = stderr_pipe.read(&mut bytes).await?;
                if count == 0 {
                    return Ok(());
                }
                let mut output = output.lock().unwrap();
                // Always drain, even after the bounded diagnostic buffer fills.
                let keep = count.min((1024 * 1024_usize).saturating_sub(output.len()));
                output.extend_from_slice(&bytes[..keep]);
            }
        });
        Ok(Self {
            child,
            stderr,
            reader,
        })
    }
    async fn session(&mut self, name: &str, transcript: &mut Vec<Value>) -> Result<Answer> {
        let stdout = self
            .child
            .stdout
            .take()
            .context("server stdout already taken")?;
        let stdin = self
            .child
            .stdin
            .take()
            .context("server stdin already taken")?;
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("stacks-on-server", "1"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25);
        // Own Child separately: the SDK's child-process convenience transport
        // hides its exit status. Its official async-rw transport accepts these
        // exact child pipes, while we retain responsibility for wait/kill/reap.
        let mut session = timeout(
            DEADLINE,
            config.serve_with_lifecycle((stdout, stdin), ClientLifecycleMode::Initialize),
        )
        .await
        .context("initialize deadline; inspect captured server stderr")?
        .context("MCP initialize failed; inspect captured server stderr")?;
        let result = async {
            let info = session.peer_info().context("missing initialize response")?;
            ensure!(
                info.protocol_version == ProtocolVersion::V_2025_11_25,
                "unexpected initialized protocol"
            );
            ensure!(
                info.server_info
                    .as_ref()
                    .is_some_and(|info| info.name == "komodo-mcp")
                    && info.capabilities.tools.is_some(),
                "wrong server or missing tools capability"
            );
            transcript.push(json!({"method":"initialize","result":info}));
            solve(&session, name, transcript).await
        }
        .await;
        let closed = timeout(SHUTDOWN_DEADLINE, session.close())
            .await
            .context("MCP close deadline")?
            .context("close MCP session");
        result.and_then(|answer| {
            closed?;
            Ok(answer)
        })
    }
    async fn finish(&mut self) -> Result<(ExitStatus, bool)> {
        drop(self.child.stdin.take());
        let (exit, forced_kill) = match timeout(SHUTDOWN_DEADLINE, self.child.wait()).await {
            Ok(result) => (result.context("reap server"), false),
            Err(_) => {
                // Keep the actual exit status and original scenario failure even
                // when cancellation needs a forced shutdown. Always drain below.
                self.child.start_kill().context("kill stalled server")?;
                (
                    timeout(SHUTDOWN_DEADLINE, self.child.wait())
                        .await
                        .context("kill/reap deadline")?
                        .context("reap killed server"),
                    true,
                )
            }
        };
        let drained = timeout(SHUTDOWN_DEADLINE, &mut self.reader).await;
        if drained.is_err() {
            self.reader.abort();
        }
        drained
            .context("stderr drain deadline")?
            .context("stderr reader task")?
            .context("capture server stderr")?;
        Ok((exit?, forced_kill))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn call(
    session: &RunningService<RoleClient, ClientConfig>,
    name: &'static str,
    args: Value,
    transcript: &mut Vec<Value>,
) -> Result<Value> {
    let result = timeout(
        DEADLINE,
        session.call_tool(
            CallToolRequestParams::new(name).with_arguments(
                args.as_object()
                    .context("tool arguments must be an object")?
                    .clone(),
            ),
        ),
    )
    .await
    .with_context(|| format!("{name} exchange deadline"))?
    .with_context(|| format!("MCP {name} request failed"))?;
    check_safe(&serde_json::to_string(&result)?)?;
    transcript.push(json!({"method":"tools/call","name":name,"arguments":args,"result":result}));
    if result.is_error == Some(true) {
        let data = result
            .structured_content
            .context("tool failed without structured error")?;
        // Only allowlisted, server-owned error categories enter diagnostics.
        let kind = data["kind"].as_str().unwrap_or("unknown");
        ensure!(
            matches!(
                kind,
                "not_found"
                    | "authentication"
                    | "permission_denied"
                    | "upstream"
                    | "timeout"
                    | "unreachable"
                    | "invalid_response"
                    | "invalid_arguments"
                    | "incomplete_resolution"
                    | "ambiguous_target"
            ),
            "tool returned unknown error kind"
        );
        bail!(
            "{name} failed ({kind}); check visible inventory, Read grants, and Core connectivity"
        );
    }
    result
        .structured_content
        .context("successful tool result lacks structuredContent")
}

async fn solve(
    session: &RunningService<RoleClient, ClientConfig>,
    name: &str,
    transcript: &mut Vec<Value>,
) -> Result<Answer> {
    let tools = timeout(DEADLINE, session.list_tools(None))
        .await
        .context("tools/list deadline")?
        .context("discover tools")?;
    check_safe(&serde_json::to_string(&tools)?)?;
    ensure!(
        tools.next_cursor.is_none(),
        "unexpected tool discovery continuation"
    );
    let names: BTreeSet<_> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    ensure!(
        names
            == BTreeSet::from([
                "get_server",
                "list_servers",
                "get_stack",
                "list_stacks",
                "get_deployment",
                "list_deployments",
                "fleet_health",
                "list_alerts",
                "list_updates",
                "get_update",
                "wait_for_update",
                "reconcile_operation",
                "get_container_logs",
                "get_deployment_logs",
                "get_stack_logs",
                "stack_diagnose",
                "find_resources_by_tag",
                "list_repos",
                "get_repo",
                "list_builders",
                "get_builder",
                "list_builds",
                "get_build",
                "list_procedures",
                "get_procedure",
                "list_actions",
                "get_action",
                "list_schedules",
                "get_schedule",
                "export_resources_toml"
            ]),
        "missing expected inventory tools"
    );
    transcript.push(json!({"method":"tools/list","result":tools}));
    let server: Server = serde_json::from_value(
        call(session, "get_server", json!({"name":name}), transcript).await?,
    )
    .context("invalid get_server structuredContent")?;
    check_resource(&server.id, &server.name)?;
    ensure!(
        server.name == name,
        "get_server returned a different exact name"
    );
    let mut page = 0;
    let mut matches = Vec::new();
    let mut ids = BTreeSet::new();
    for count in 1..=MAX_PAGES {
        let data = call(
            session,
            "list_stacks",
            json!({"page":page,"page_size":PAGE_SIZE}),
            transcript,
        )
        .await?;
        let result: Page =
            serde_json::from_value(data).context("invalid list_stacks structuredContent")?;
        let continuation = result.continuation(page)?;
        for item in result.items {
            check_resource(&item.id, &item.name)?;
            ensure!(
                ids.insert(item.id.clone()),
                "duplicate stack across inventory pages; incomplete answer"
            );
            if let Some(target) = &item.target {
                check_resource(&target.id, "target")?;
            }
            if item
                .target
                .as_ref()
                .is_some_and(|t| t.kind == TargetKind::Server && t.id == server.id)
            {
                matches.push(item);
            }
        }
        match continuation {
            None => {
                matches.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
                return Ok(Answer {
                    server,
                    stacks: matches,
                    pages: count,
                    visible: ids.len(),
                });
            }
            Some(next) => {
                page = next;
            }
        }
    }
    bail!("inventory exceeded {MAX_PAGES} pages; incomplete answer")
}

async fn interrupted() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    bail!("demonstration interrupted")
}

pub async fn run(
    executable: &Path,
    name: &str,
    config: Configuration,
    fail_operation: Option<&'static str>,
) -> Result<Report> {
    run_with_control(
        executable,
        name,
        config,
        fail_operation,
        SCENARIO_DEADLINE,
        interrupted(),
    )
    .await
}

pub async fn run_with_control(
    executable: &Path,
    name: &str,
    config: Configuration,
    fail_operation: Option<&'static str>,
    deadline: Duration,
    cancellation: impl Future<Output = Result<()>>,
) -> Result<Report> {
    let core = MockCore::start_with_failure("127.0.0.1:0".parse()?, fail_operation).await?;
    let mut process = match Process::spawn(executable, &core.address, config) {
        Ok(process) => process,
        Err(error) => {
            core.shutdown().await?;
            return Err(error);
        }
    };
    let child_id = process.child.id().context("missing child process ID")?;
    let address = core.address.clone();
    let mut transcript = Vec::new();
    let result = tokio::select! {
        result = timeout(deadline, process.session(name, &mut transcript)) => result.context("overall scenario deadline; incomplete answer").and_then(|r| r),
        result = cancellation => result.and_then(|_| anyhow::bail!("demonstration interrupted")),
    };
    // Always finish both resources, including startup/tool failures and cancellation.
    let exit = process.finish().await;
    let stderr = String::from_utf8_lossy(&process.stderr.lock().unwrap()).into_owned();
    let requests = core.requests();
    let rejected_credentials = core.rejected_credentials();
    let verified = core.verify_reads();
    let stopped = core.shutdown().await;
    stopped?;
    verified?;
    check_safe(&stderr)?;
    check_safe(&serde_json::to_string(&transcript)?)?;
    let (status, forced_kill) =
        exit.with_context(|| format!("captured server stderr:\n{stderr}"))?;
    let result = result.and_then(|answer| {
        ensure!(
            !forced_kill,
            "server did not exit after closing MCP stdin; killed and reaped it"
        );
        ensure!(
            status.success(),
            "server exited unsuccessfully after the scenario"
        );
        Ok(answer)
    });
    Ok(Report {
        result,
        address,
        child_id,
        status,
        forced_kill,
        stderr,
        transcript,
        requests,
        rejected_credentials,
    })
}

pub async fn demonstrate(executable: &Path, name: &str) -> Result<Answer> {
    let report = run(executable, name, Configuration::Synthetic, None).await?;
    eprintln!(
        "Cleanup: MCP child exited {}{}; mock stopped.",
        report.status,
        if report.forced_kill {
            " (shutdown deadline; killed and reaped)"
        } else {
            ""
        }
    );
    if report.result.is_err() {
        eprintln!("Captured komodo-mcp stderr:\n{}", report.stderr);
    }
    let answer = report.result?;
    println!(
        "Initialized MCP 2025-11-25; discovered read-only tools; called get_server and list_stacks over stdio."
    );
    println!(
        "Core reads: {}",
        report
            .requests
            .iter()
            .map(|r| r.operation.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(answer)
}

/// Use the same official client, fixture, deadlines and cleanup as the walkthrough.
/// The requested application version is independent of MCP and Core versions.
pub async fn verify_release(executable: &Path, expected: &str) -> Result<Value> {
    let executable = executable
        .canonicalize()
        .context("resolve release executable")?;
    let report = run(
        &executable,
        super::mock_core::DEFAULT_SERVER,
        Configuration::Synthetic,
        None,
    )
    .await?;
    eprintln!(
        "Cleanup: MCP child exited {}; mock stopped; forced kill: {}.",
        report.status, report.forced_kill
    );
    if report.result.is_err() {
        eprintln!("Captured komodo-mcp stderr:\n{}", report.stderr);
    }
    let answer = report.result?;
    ensure!(
        answer.pages == 3
            && answer.visible == 6
            && answer
                .stacks
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                == ["alpha-web", "gamma-jobs", "zeta-metrics"],
        "release inventory differs from the synthetic oracle"
    );
    let observed = report.transcript[0]["result"]["serverInfo"]["version"]
        .as_str()
        .context("initialize omitted serverInfo.version")?;
    ensure!(
        observed == expected,
        "MCP version mismatch: expected {expected}, observed {observed}"
    );
    Ok(json!({
        "executable": executable,
        "expected_version": expected,
        "observed_version": observed,
        "inventory": answer.stacks.iter().map(|s| &s.name).collect::<Vec<_>>(),
        "child_exit_success": report.status.success(),
        "forced_kill": report.forced_kill,
        "mock_stopped": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn forced_shutdown_reaps_the_child_and_finishes_stderr_capture() -> Result<()> {
        let mut command = Command::new("/bin/sh");
        // The shell stops itself without spawning a descendant. Closing stdin
        // cannot wake it; the cleanup deadline must kill and reap this process.
        command.args(["-c", "kill -STOP $$"]);
        let mut process = Process::start(command)?;
        let (status, forced) = process.finish().await?;
        assert!(forced);
        assert!(!status.success());
        assert!(process.child.id().is_none(), "child must be reaped");
        assert!(process.reader.is_finished(), "stderr capture must finish");
        Ok(())
    }

    #[test]
    fn rejects_stalled_skipped_inconsistent_and_missing_continuations() -> Result<()> {
        let item =
            json!({"id":"000000000000000000000002","name":"stack","state":"running","target":null});
        let valid = json!({"items":[item.clone(),item],"page":0,"page_size":2,"has_more":true,"next_page":1});
        assert_eq!(
            serde_json::from_value::<Page>(valid.clone())?.continuation(0)?,
            Some(1)
        );
        for (field, value) in [
            ("next_page", json!(0)),
            ("next_page", json!(2)),
            ("next_page", Value::Null),
            ("page", json!(1)),
            ("page_size", json!(3)),
            ("has_more", json!(false)),
            ("items", json!([])),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                serde_json::from_value::<Page>(invalid)?
                    .continuation(0)
                    .is_err()
            );
        }
        let mut missing = valid;
        missing.as_object_mut().unwrap().remove("next_page");
        assert!(serde_json::from_value::<Page>(missing).is_err());
        let missing_target =
            json!({"id":"000000000000000000000002","name":"stack","state":"running"});
        assert!(serde_json::from_value::<Stack>(missing_target).is_err());
        Ok(())
    }
}
