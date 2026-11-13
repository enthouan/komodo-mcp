//! Exercise the production executable with the official rmcp stdio client.

mod support;

use std::{
    collections::BTreeSet,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rmcp::{
    ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
        ProtocolVersion,
    },
    service::{ClientLifecycleMode, RunningService},
};
use serde_json::{Value, json};
use support::{
    KEY, SECRET, SENSITIVE_MARKERS,
    incident::{IncidentCore, Mode, Options, fixture},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::JoinHandle,
    time::timeout,
};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(45);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_BYTES: usize = 1024 * 1024;
const TOOLS: [&str; 16] = [
    "list_servers",
    "get_server",
    "list_stacks",
    "get_stack",
    "list_deployments",
    "get_deployment",
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
];

type Session = RunningService<RoleClient, ClientConfig>;

#[derive(Clone, Copy)]
enum Scenario {
    Incident,
    DiscoverAndRejectInvalidArguments,
    Healthy,
    Inconclusive,
    MissingAlerts,
    FinalSerializedCap,
}

struct Process {
    child: Child,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<std::io::Result<()>>,
}

impl Process {
    fn spawn(core: &IncidentCore) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", core.api_key())
            .env("KOMODO_API_SECRET", SECRET)
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("launch production komodo-mcp")?;
        let mut pipe = child.stderr.take().context("missing child stderr")?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&stderr);
        let reader = tokio::spawn(async move {
            let mut buffer = [0; 4096];
            loop {
                let count = pipe.read(&mut buffer).await?;
                if count == 0 {
                    return Ok(());
                }
                let mut captured = captured.lock().unwrap();
                let keep = count.min(STDERR_BYTES.saturating_sub(captured.len()));
                captured.extend_from_slice(&buffer[..keep]);
                // Continue draining after the diagnostic buffer fills.
            }
        });
        Ok(Self {
            child,
            stderr,
            reader,
        })
    }

    async fn session(&mut self, core: &IncidentCore, scenario: Scenario) -> Result<()> {
        let stdout = self.child.stdout.take().context("stdout already taken")?;
        let stdin = self.child.stdin.take().context("stdin already taken")?;
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("incident-stdio-test", "1"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25);
        let mut session = timeout(
            EXCHANGE_TIMEOUT,
            config.serve_with_lifecycle((stdout, stdin), ClientLifecycleMode::Initialize),
        )
        .await
        .context("initialize deadline")?
        .context("initialize production MCP")?;
        let result = async {
            let info = session.peer_info().context("missing initialize result")?;
            ensure!(
                info.protocol_version == ProtocolVersion::V_2025_11_25,
                "wrong MCP protocol"
            );
            let server = info.server_info.as_ref().context("missing serverInfo")?;
            ensure!(
                server.name == "komodo-mcp" && server.version == env!("CARGO_PKG_VERSION"),
                "wrong executable identity/version"
            );
            ensure!(
                info.capabilities.tools.is_some(),
                "tools capability missing"
            );
            safe(&serde_json::to_string(&info)?)?;
            match scenario {
                Scenario::Incident => incident_workflow(&session, core).await,
                Scenario::DiscoverAndRejectInvalidArguments => {
                    discover_and_reject(&session, core).await
                }
                Scenario::FinalSerializedCap => final_serialized_cap(&session).await,
                Scenario::Healthy | Scenario::Inconclusive | Scenario::MissingAlerts => {
                    diagnose_variant(&session, core, scenario).await
                }
            }
        }
        .await;
        // Close the official transport on both successful and failed exchanges.
        let closed = timeout(SHUTDOWN_TIMEOUT, session.close())
            .await
            .context("MCP close deadline")?
            .context("close MCP session");
        result.and(closed.map(|_| ()))
    }

    async fn finish(&mut self) -> Result<(ExitStatus, bool)> {
        drop(self.child.stdin.take());
        let (status, forced) = match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(status) => (status.context("reap MCP child")?, false),
            Err(_) => {
                self.child.start_kill().context("kill stalled MCP child")?;
                (
                    timeout(SHUTDOWN_TIMEOUT, self.child.wait())
                        .await
                        .context("reap deadline")?
                        .context("reap killed MCP child")?,
                    true,
                )
            }
        };
        let drained = timeout(SHUTDOWN_TIMEOUT, &mut self.reader).await;
        if drained.is_err() {
            self.reader.abort();
        }
        drained
            .context("stderr drain deadline")?
            .context("stderr task")?
            .context("read child stderr")?;
        ensure!(self.child.id().is_none(), "child was not reaped");
        Ok((status, forced))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn safe(text: &str) -> Result<()> {
    for marker in [KEY, SECRET]
        .into_iter()
        .chain(SENSITIVE_MARKERS.iter().copied())
    {
        ensure!(!text.contains(marker), "a private fixture marker leaked");
    }
    let fixture = fixture();
    for key in [
        "credential_marker",
        "raw_config_marker",
        "other_data_marker",
    ] {
        ensure!(
            !text.contains(fixture[key].as_str().unwrap()),
            "incident private marker leaked"
        );
    }
    Ok(())
}

async fn call(session: &Session, tool: &str, arguments: Value) -> Result<CallToolResult> {
    let result = timeout(
        EXCHANGE_TIMEOUT,
        session.call_tool(
            CallToolRequestParams::new(tool.to_owned()).with_arguments(
                arguments
                    .as_object()
                    .context("tool arguments must be an object")?
                    .clone(),
            ),
        ),
    )
    .await
    .with_context(|| format!("{tool} exchange deadline"))?
    .with_context(|| format!("{tool} protocol failure"))?;
    safe(&serde_json::to_string(&result)?)?;
    Ok(result)
}

async fn success(session: &Session, tool: &str, arguments: Value) -> Result<Value> {
    let result = call(session, tool, arguments).await?;
    ensure!(
        result.is_error != Some(true),
        "{tool} returned a tool error"
    );
    let content = serde_json::to_value(&result.content)?;
    ensure!(
        content.as_array().is_some_and(|c| c.len() == 1) && content[0]["type"] == "text",
        "{tool} omitted compatible text output"
    );
    let text: Value =
        serde_json::from_str(content[0]["text"].as_str().context("missing text output")?)?;
    let structured = result
        .structured_content
        .context("missing structuredContent")?;
    ensure!(structured == text, "{tool} text/structured output mismatch");
    Ok(structured)
}

async fn discover_and_reject(session: &Session, core: &IncidentCore) -> Result<()> {
    let listed = timeout(EXCHANGE_TIMEOUT, session.list_tools(None))
        .await
        .context("tools/list deadline")??;
    safe(&serde_json::to_string(&listed)?)?;
    ensure!(
        listed.next_cursor.is_none(),
        "unexpected discovery continuation"
    );
    let actual: BTreeSet<_> = listed.tools.iter().map(|t| t.name.as_ref()).collect();
    ensure!(
        actual == BTreeSet::from(TOOLS),
        "expected all sixteen read-only tools"
    );
    let reads = core.requests().len();
    for tool in &listed.tools {
        let annotations = tool.annotations.as_ref().context("missing annotations")?;
        ensure!(
            annotations.read_only_hint == Some(true)
                && annotations.idempotent_hint == Some(true)
                && annotations.destructive_hint == Some(false),
            "unsafe tool annotations on {}",
            tool.name
        );
        ensure!(
            tool.input_schema.get("type") == Some(&json!("object")),
            "input schema must describe an object"
        );
        ensure!(
            tool.output_schema
                .as_ref()
                .is_some_and(|s| s.get("type") == Some(&json!("object"))),
            "output schema must describe an object"
        );
        let invalid = call(
            session,
            &tool.name,
            json!({"unexpected":format!("{KEY} {SECRET}")}),
        )
        .await?;
        ensure!(
            invalid.is_error == Some(true),
            "malformed arguments must be safe tool errors"
        );
        ensure!(
            invalid
                .structured_content
                .as_ref()
                .is_some_and(|data| data["kind"] == "invalid_arguments"),
            "wrong malformed-argument category"
        );
    }
    ensure!(
        core.requests().len() == reads,
        "malformed tool arguments reached Core"
    );
    Ok(())
}

async fn incident_workflow(session: &Session, core: &IncidentCore) -> Result<()> {
    discover_and_reject(session, core).await?;
    let f = fixture();
    let fleet = success(session, "fleet_health", json!({"page_size":10})).await?;
    let broken = fleet["stacks"]
        .as_array()
        .context("fleet stacks missing")?
        .iter()
        .find(|stack| stack["health"] == "unhealthy")
        .context("fleet did not identify an unhealthy stack")?;
    ensure!(
        broken["id"] == f["stack"]["id"] && broken["name"] == f["stack"]["name"],
        "fleet selected the wrong incident stack"
    );
    ensure!(
        broken["server_id"] == f["server"]["id"],
        "fleet lost the affected server association"
    );
    let id = broken["id"].as_str().context("missing stack ID")?;
    let before = core.requests().len();
    let diagnosis = success(session, "stack_diagnose", json!({"id":id})).await?;
    ensure!(
        core.requests().len() - before <= 7,
        "diagnosis exceeded its Core call budget"
    );
    check_incident(&diagnosis, &f)?;
    ensure!(
        diagnosis["gaps"].as_array().is_some_and(Vec::is_empty),
        "complete incident has unexpected evidence gaps"
    );
    let update_id = diagnosis["failed_update"]["id"]
        .as_str()
        .context("diagnosis omitted failed update ID")?;
    let update = success(session, "get_update", json!({"id":update_id})).await?;
    ensure!(
        update == diagnosis["failed_update"],
        "independent update lookup disagrees with diagnosis"
    );
    let alert_id = diagnosis["recent_alerts"][0]["id"]
        .as_str()
        .context("diagnosis omitted alert ID")?;
    ensure!(alert_id == f["alert"]["id"], "missing correlated alert");
    let alerts = success(
        session,
        "list_alerts",
        json!({"target":{"kind":"stack","id":id},"page":0}),
    )
    .await?;
    let alert = alerts["items"]
        .as_array()
        .context("missing alert page")?
        .iter()
        .find(|a| a["id"] == alert_id)
        .context("returned alert ID not independently discoverable")?;
    ensure!(
        alert == &diagnosis["recent_alerts"][0] && alert["target"]["id"] == id,
        "independent alert disagrees with diagnosis"
    );
    let empty = success(
        session,
        "list_alerts",
        json!({"target":{"kind":"stack","id":id},"page":1}),
    )
    .await?;
    ensure!(
        empty["items"] == json!([]),
        "exhausted alert history should be empty"
    );
    let service = diagnosis["affected_services"][0]
        .as_str()
        .context("missing affected service")?;
    let logs = success(
        session,
        "get_stack_logs",
        json!({"id":id,"services":[service],"tail":40,"max_bytes":4096}),
    )
    .await?;
    ensure!(
        logs == diagnosis["logs"],
        "independent filtered logs disagree with diagnosis"
    );
    let other_logs = success(
        session,
        "get_stack_logs",
        json!({"id":id,"services":[f["healthy_service"]],"tail":4,"max_bytes":512}),
    )
    .await?;
    ensure!(
        other_logs["stdout"] == "db | ready\n",
        "service filter did not select the healthy service"
    );
    ensure!(
        other_logs["returned_bytes"]
            .as_u64()
            .is_some_and(|n| n <= 512),
        "service logs exceeded their bound"
    );
    let filtered: Vec<_> = core
        .requests()
        .into_iter()
        .filter(|r| r.operation == "GetStackLog")
        .collect();
    ensure!(
        filtered.len() == 3
            && filtered[0].params["services"] == json!([service])
            && filtered[1].params["services"] == json!([service])
            && filtered[2].params["services"] == json!([f["healthy_service"]]),
        "log service filters were not forwarded to Core"
    );
    Ok(())
}

fn check_incident(diagnosis: &Value, f: &Value) -> Result<()> {
    ensure!(
        diagnosis["stack_id"] == f["stack"]["id"] && diagnosis["outcome"] == "unhealthy",
        "wrong incident diagnosis"
    );
    ensure!(
        diagnosis["affected_services"] == json!([f["affected_service"]]),
        "wrong affected service"
    );
    ensure!(
        diagnosis["failed_update"]["id"] == f["update"]["id"]
            && diagnosis["failed_update"]["outcome"] == "failed",
        "wrong failed update"
    );
    ensure!(
        diagnosis["failed_update"]["target"]["id"] == f["stack"]["id"],
        "failed update belongs to another resource"
    );
    let stage = &diagnosis["failed_update"]["stages"][0];
    ensure!(
        stage["stage"] == f["update"]["failed_stage"] && stage["success"] == false,
        "missing failed rollout stage"
    );
    ensure!(
        stage["stderr"]
            .as_str()
            .is_some_and(|s| s.contains(f["update"]["stage_stderr"].as_str().unwrap())),
        "missing stage evidence"
    );
    ensure!(
        diagnosis["logs"]["stdout"]
            .as_str()
            .is_some_and(|s| s.contains(f["expected_log_excerpt"].as_str().unwrap())),
        "missing expected log excerpt"
    );
    ensure!(
        diagnosis["logs"]["target"]["services"] == json!([f["affected_service"]]),
        "diagnostic logs omitted affected-service filter"
    );
    ensure!(
        serde_json::to_vec(diagnosis)?.len() <= 65_536,
        "diagnosis exceeded output bound"
    );
    Ok(())
}

async fn diagnose_variant(
    session: &Session,
    core: &IncidentCore,
    scenario: Scenario,
) -> Result<()> {
    let f = fixture();
    let before = core.requests().len();
    let diagnosis = success(session, "stack_diagnose", json!({"id":f["stack"]["id"]})).await?;
    ensure!(
        core.requests().len() - before <= 7,
        "diagnosis exceeded call budget"
    );
    match scenario {
        Scenario::Healthy => {
            ensure!(
                diagnosis["outcome"] == "healthy" && diagnosis["affected_services"] == json!([]),
                "healthy stack diagnosed incorrectly"
            );
            ensure!(
                diagnosis["failed_update"].is_null() && diagnosis["recent_alerts"] == json!([]),
                "healthy variant retained unrelated failure evidence"
            );
            ensure!(
                diagnosis["gaps"] == json!([]),
                "healthy variant unexpectedly lost a source"
            );
        }
        Scenario::Inconclusive => {
            ensure!(
                diagnosis["outcome"] == "inconclusive",
                "missing sources and unknown state must be inconclusive"
            );
            ensure!(
                diagnosis["services"]["services"]
                    .as_array()
                    .is_some_and(|s| s.len() == 2),
                "inconclusive diagnosis discarded available service observations"
            );
            let gaps: BTreeSet<_> = diagnosis["gaps"]
                .as_array()
                .context("missing source gaps")?
                .iter()
                .filter_map(|g| g["source"].as_str())
                .collect();
            ensure!(
                gaps.is_superset(&BTreeSet::from(["updates", "alerts", "logs"])),
                "inconclusive diagnosis omitted unavailable sources"
            );
        }
        Scenario::MissingAlerts => {
            check_incident(&diagnosis, &f)?;
            let gaps = diagnosis["gaps"]
                .as_array()
                .context("missing source gaps")?;
            ensure!(
                gaps.len() == 1 && gaps[0]["source"] == "alerts",
                "partial diagnosis reported the wrong gap"
            );
            ensure!(
                diagnosis["recent_alerts"] == json!([]),
                "unavailable alerts must remain explicitly absent"
            );
        }
        _ => unreachable!("variant scenario"),
    }
    Ok(())
}

async fn final_serialized_cap(session: &Session) -> Result<()> {
    // Use the original selector: the configured one-character credential also
    // occurs in stable IDs, so response identifiers are recursively redacted.
    let diagnosis = success(
        session,
        "stack_diagnose",
        json!({"id":fixture()["stack"]["id"]}),
    )
    .await?;
    ensure!(
        serde_json::to_vec(&diagnosis)?.len() <= 65_536,
        "final redacted structuredContent exceeded the serialized diagnosis cap"
    );
    ensure!(
        diagnosis["limits"]["evidence_truncated"] == true,
        "large escaped evidence must report truncation"
    );
    ensure!(
        diagnosis["services"]["truncated"] == true,
        "service evidence truncation was not disclosed"
    );
    ensure!(
        diagnosis["services"]["services"]
            .as_array()
            .is_some_and(|services| !services.is_empty()),
        "bounded diagnosis discarded all service evidence"
    );
    ensure!(
        diagnosis["failed_update"].is_object() && diagnosis["outcome"] == "unhealthy",
        "serialized cap discarded the confirmed failure evidence"
    );
    ensure!(
        diagnosis["stack_id"]
            .as_str()
            .is_some_and(|id| !id.contains('0')),
        "final string credential redaction did not run"
    );
    Ok(())
}

async fn run(options: Options, scenario: Scenario) -> Result<()> {
    let core = IncidentCore::start(options).await;
    let mut process = Process::spawn(&core)?;
    let result = timeout(SCENARIO_TIMEOUT, process.session(&core, scenario))
        .await
        .context("overall stdio incident deadline")
        .and_then(|r| r);
    // Run cleanup before returning any assertion or exchange error.
    let finished = process.finish().await;
    let stderr = String::from_utf8_lossy(&process.stderr.lock().unwrap()).into_owned();
    safe(&stderr)?;
    let (status, forced) = finished.with_context(|| format!("captured child stderr:\n{stderr}"))?;
    ensure!(
        !forced && status.success(),
        "MCP child failed graceful shutdown: {status}"
    );
    result.with_context(|| format!("captured child stderr:\n{stderr}"))?;
    let requests = core.requests();
    ensure!(
        requests
            .first()
            .is_some_and(|r| r.operation == "GetVersion"),
        "production startup skipped Core version probe"
    );
    ensure!(
        requests.iter().all(|r| matches!(
            r.operation.as_str(),
            "GetVersion"
                | "ListServers"
                | "ListStacks"
                | "ListDeployments"
                | "GetStack"
                | "GetServerState"
                | "GetSystemStats"
                | "ListAllStackServices"
                | "ListUpdates"
                | "ListAlerts"
                | "GetUpdate"
                | "GetStackLog"
        )),
        "observed an unexpected or mutating Core operation"
    );
    ensure!(
        core.peak() <= 4,
        "sequential triage exceeded its Core concurrency budget"
    );
    Ok(())
}

#[tokio::test]
async fn fleet_to_diagnosis_to_independent_evidence_through_stdio() -> Result<()> {
    run(Options::default(), Scenario::Incident).await
}

#[tokio::test]
async fn discovery_and_invalid_arguments_are_safe_through_stdio() -> Result<()> {
    run(
        Options::default(),
        Scenario::DiscoverAndRejectInvalidArguments,
    )
    .await
}

#[tokio::test]
async fn healthy_and_inconclusive_outcomes_through_stdio() -> Result<()> {
    run(
        Options {
            mode: Mode::Healthy,
            ..Default::default()
        },
        Scenario::Healthy,
    )
    .await?;
    run(
        Options {
            mode: Mode::Inconclusive,
            ..Default::default()
        },
        Scenario::Inconclusive,
    )
    .await
}

#[tokio::test]
async fn unavailable_alert_source_retains_other_evidence_through_stdio() -> Result<()> {
    run(
        Options {
            failures: BTreeSet::from(["ListAlerts".into()]),
            ..Default::default()
        },
        Scenario::MissingAlerts,
    )
    .await
}

#[tokio::test]
async fn diagnosis_caps_final_redacted_serialized_output_through_stdio() -> Result<()> {
    // NUL expands to six bytes in JSON. Vary the padding at the trimming
    // boundary so final recursive redaction's longer ID strings cannot escape
    // a cap measured only on the earlier report representation.
    for padding in 235..=240 {
        let nul = "\0".repeat(padding);
        let services: Vec<_> = (0..20)
            .map(|index| {
                json!({
                    "stack_id":fixture()["stack"]["id"],
                    "stack_name":"alpha-web",
                    "service":format!("{index:03}{nul}"),
                    "image":"fixture:latest",
                    "container":{"name":nul,"state":"running","status":nul},
                    "swarm_service":null,
                    "state":"Unhealthy",
                    "image_digests":null,
                })
            })
            .collect();
        run(
            Options {
                api_key: Some("0".into()),
                overrides: [("ListAllStackServices".into(), json!(services))].into(),
                ..Default::default()
            },
            Scenario::FinalSerializedCap,
        )
        .await
        .with_context(|| format!("NUL padding {padding}"))?;
    }
    Ok(())
}
