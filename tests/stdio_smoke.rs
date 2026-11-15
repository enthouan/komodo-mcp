mod support;

use std::{
    io,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rmcp::model::{ErrorCode, ProtocolVersion, RequestId, ServerJsonRpcMessage, ServerResult};
use serde_json::{Value, json};
use support::{KEY, MockCore, Reply, SECRET, Step, assert_safe, inventory::*};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc::{self, UnboundedReceiver},
    task::JoinHandle,
    time::timeout,
};

// The production startup probe has a 15-second deadline.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(25);
const SESSION_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(25);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Default)]
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: UnboundedReceiver<Vec<u8>>,
    stdout_reader: JoinHandle<io::Result<()>>,
    stderr_reader: JoinHandle<io::Result<()>>,
    captured: Arc<Mutex<Captured>>,
}

impl Process {
    fn spawn(overrides: &[(&str, Option<&str>)]) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"));
        for field in [
            "KOMODO_ADDRESS",
            "KOMODO_API_KEY",
            "KOMODO_API_SECRET",
            "KOMODO_ALLOW_OPERATE",
            "KOMODO_ALLOW_CONFIGURATION_EDIT",
        ] {
            command.env_remove(field);
        }
        for (field, value) in overrides {
            if let Some(value) = value {
                command.env(field, value);
            } else {
                command.env_remove(field);
            }
        }
        let mut child = command
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("launch compiled komodo-mcp")?;

        // Command guarantees these handles because all three streams are piped.
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut stderr = child.stderr.take().expect("piped stderr");
        let captured = Arc::new(Mutex::new(Captured::default()));
        let (sender, frames) = mpsc::unbounded_channel();

        let output = Arc::clone(&captured);
        let stdout_reader = tokio::spawn(async move {
            loop {
                let mut frame = Vec::new();
                let read = stdout.read_until(b'\n', &mut frame).await;
                output.lock().unwrap().stdout.extend_from_slice(&frame);
                if read? == 0 {
                    return Ok(());
                }
                // Keep draining even if the test has stopped awaiting responses.
                let _ = sender.send(frame);
            }
        });

        let output = Arc::clone(&captured);
        let stderr_reader = tokio::spawn(async move {
            let mut buffer = [0; 4096];
            loop {
                let count = stderr.read(&mut buffer).await?;
                if count == 0 {
                    return Ok(());
                }
                output
                    .lock()
                    .unwrap()
                    .stderr
                    .extend_from_slice(&buffer[..count]);
            }
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            frames,
            stdout_reader,
            stderr_reader,
            captured,
        })
    }

    async fn send(&mut self, message: Value) -> Result<()> {
        let mut frame = serde_json::to_vec(&message)?;
        frame.push(b'\n');
        let stdin = self.stdin.as_mut().context("stdin already closed")?;
        timeout(EXCHANGE_TIMEOUT, async {
            stdin.write_all(&frame).await?;
            stdin.flush().await
        })
        .await
        .context("deadline writing to stdin")?
        .context("write MCP message")
    }

    async fn receive(&mut self, id: &str) -> Result<ServerJsonRpcMessage> {
        let frame = timeout(EXCHANGE_TIMEOUT, self.frames.recv())
            .await
            .context("deadline awaiting stdout response")?
            .context("stdout reached EOF before response")?;
        let message = decode_frame(&frame)?;
        let actual_id = match &message {
            ServerJsonRpcMessage::Response(response) => Some(&response.id),
            ServerJsonRpcMessage::Error(response) => response.id.as_ref(),
            other => bail!("unexpected server message: {other:?}"),
        };
        ensure!(
            actual_id == Some(&RequestId::String(id.into())),
            "expected response ID {id:?}, got {actual_id:?}"
        );
        Ok(message)
    }

    async fn wait_for_exit(&mut self, force_kill: bool) -> Result<ExitStatus> {
        // Drop the pipe itself: an MCP stdio client shuts down by closing stdin.
        drop(self.stdin.take());
        if force_kill {
            self.child
                .start_kill()
                .context("kill after failed exchange")?;
        }
        match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(status) => status.context("reap komodo-mcp"),
            Err(_) => {
                // kill() also waits, so the deadline path terminates and reaps it.
                timeout(SHUTDOWN_TIMEOUT, self.child.kill())
                    .await
                    .context("deadline killing and reaping komodo-mcp")?
                    .context("kill and reap komodo-mcp")?;
                bail!("deadline waiting for process exit after closing stdin")
            }
        }
    }

    async fn finish(&mut self, force_kill: bool) -> Result<ExitStatus> {
        let exit = self.wait_for_exit(force_kill).await;
        // Await both streams even when the exchange or process wait failed.
        let readers = timeout(DRAIN_TIMEOUT, async {
            tokio::join!(&mut self.stdout_reader, &mut self.stderr_reader)
        })
        .await;
        if readers.is_err() {
            self.stdout_reader.abort();
            self.stderr_reader.abort();
        }
        let (stdout, stderr) = readers.context("deadline draining stdout/stderr to EOF")?;
        stdout
            .context("stdout reader task")?
            .context("read stdout")?;
        stderr
            .context("stderr reader task")?
            .context("read stderr")?;
        exit
    }
}

fn decode_frame(frame: &[u8]) -> Result<ServerJsonRpcMessage> {
    ensure!(frame.ends_with(b"\n"), "stdout frame lacks final newline");
    let text = std::str::from_utf8(frame).context("stdout contains invalid UTF-8")?;
    ensure!(!text.trim().is_empty(), "stdout contains a blank line");
    let envelope: Value = serde_json::from_str(text).context("stdout contains malformed JSON")?;
    ensure!(
        envelope.is_object() && envelope["jsonrpc"] == "2.0",
        "stdout is not a JSON-RPC 2.0 envelope: {text:?}"
    );
    let message: ServerJsonRpcMessage =
        serde_json::from_str(text).context("stdout is not an SDK MCP server message")?;
    // The production registry is fixed and emits no unsolicited notifications.
    ensure!(
        matches!(
            message,
            ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
        ),
        "unexpected MCP request or notification: {message:?}"
    );
    Ok(message)
}

async fn list_tools(process: &mut Process, id: &str) -> Result<()> {
    process
        .send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}}))
        .await
        .context("send tools/list")?;
    match process.receive(id).await.context("receive tools/list")? {
        ServerJsonRpcMessage::Response(response) => match response.result {
            ServerResult::ListToolsResult(result) => {
                let names: std::collections::BTreeSet<_> =
                    result.tools.iter().map(|tool| tool.name.as_ref()).collect();
                let expected: std::collections::BTreeSet<_> = FAMILIES
                    .iter()
                    .flat_map(|family| [family.list_tool(), family.get_tool()])
                    .chain([
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
                        "export_resources_toml",
                        "list_resource_syncs",
                        "get_resource_sync",
                        "get_resource_sync_pending",
                        "review_resource_sync_pending",
                        "wait_for_resource_sync_update",
                    ])
                    .collect();
                ensure!(
                    names == expected,
                    "expected all thirty-five read-only tools"
                );
                for tool in &result.tools {
                    ensure!(tool.output_schema.is_some(), "missing output schema");
                    let metadata = serde_json::to_value(tool)?;
                    ensure!(
                        metadata["annotations"]["readOnlyHint"] == true
                            && metadata["annotations"]["idempotentHint"]
                                == (metadata["name"] != "review_resource_sync_pending"),
                        "missing read-only annotations"
                    );
                }
                ensure!(result.next_cursor.is_none(), "unexpected tools pagination");
                Ok(())
            }
            other => bail!("expected tools/list result, got {other:?}"),
        },
        other => bail!("expected successful tools/list response, got {other:?}"),
    }
}

async fn exchange(process: &mut Process) -> Result<()> {
    process
        .send(json!({
            "jsonrpc": "2.0",
            "id": "smoke-initialize",
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": KEY, "version": SECRET}
            }
        }))
        .await
        .context("send initialize")?;
    match process
        .receive("smoke-initialize")
        .await
        .context("receive initialize")?
    {
        ServerJsonRpcMessage::Response(response) => match response.result {
            ServerResult::InitializeResult(result) => {
                ensure!(
                    result.protocol_version == ProtocolVersion::V_2025_11_25,
                    "unexpected negotiated protocol: {}",
                    result.protocol_version
                );
                ensure!(result.server_info.name == "komodo-mcp", "wrong server name");
                ensure!(
                    result.server_info.version == env!("CARGO_PKG_VERSION"),
                    "server version differs from Cargo package version"
                );
                ensure!(
                    serde_json::to_value(result.capabilities)? == json!({"tools": {}}),
                    "server must advertise only the tools capability"
                );
            }
            other => bail!("expected initialize result, got {other:?}"),
        },
        other => bail!("expected successful initialize response, got {other:?}"),
    }

    process
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await
        .context("send notifications/initialized")?;
    list_tools(process, "smoke-tools-before").await?;

    process
        .send(json!({
            "jsonrpc": "2.0",
            "id": "smoke-unknown-tool",
            "method": "tools/call",
            "params": {"name": "nonexistent-tool", "arguments": {"key": KEY, "secret": SECRET}}
        }))
        .await
        .context("send unknown tools/call")?;
    match process
        .receive("smoke-unknown-tool")
        .await
        .context("receive unknown tools/call")?
    {
        ServerJsonRpcMessage::Error(response) => {
            // rmcp 3.8.0 ToolRouter::call returns this protocol error for a missing route.
            ensure!(
                response.error.code == ErrorCode::INVALID_PARAMS,
                "wrong error code"
            );
            ensure!(
                response.error.message == "tool not found",
                "wrong error message"
            );
        }
        other => bail!("expected SDK unknown-tool protocol error, got {other:?}"),
    }
    // Exercise the deployed router, projections, ID/name selectors and continuation.
    for family in FAMILIES {
        let listed = call(
            process,
            &format!("smoke-{}", family.list_tool()),
            family.list_tool(),
            json!({}),
        )
        .await?;
        let data = assert_success(&listed);
        assert_keys(&data["items"][0], &family.keys(false));
        let id = data["items"][0]["id"].clone();
        for (field, value) in [("id", id), ("name", family.summary()["name"].clone())] {
            let inspected = call(
                process,
                &format!("smoke-{}-{field}", family.get_tool()),
                family.get_tool(),
                json!({field:value}),
            )
            .await?;
            assert_keys(assert_success(&inspected), &family.keys(true));
        }
    }
    let first = call(
        process,
        "smoke-page-0",
        "list_servers",
        json!({"page_size":1}),
    )
    .await?;
    let data = assert_success(&first);
    ensure!(
        data["has_more"] == true && data["next_page"] == 1,
        "missing continuation"
    );
    let last = call(
        process,
        "smoke-page-1",
        "list_servers",
        json!({"page":data["next_page"],"page_size":1}),
    )
    .await?;
    let data = assert_success(&last);
    ensure!(
        data["has_more"] == false && data["next_page"].is_null(),
        "expected exhausted exact-boundary page"
    );

    let failure = call(
        process,
        "smoke-missing",
        "get_stack",
        json!({"name":"missing-stack"}),
    )
    .await?;
    assert_failure(&failure, "not_found");
    let failure = call(
        process,
        "smoke-arguments",
        "get_deployment",
        json!({"name":{"credential":SECRET},KEY:SECRET}),
    )
    .await?;
    assert_failure(&failure, "invalid_arguments");
    let failure = call(process, "smoke-upstream", "list_deployments", json!({})).await?;
    assert_failure(&failure, "upstream");
    let recovery = call(process, "smoke-recovery", "list_deployments", json!({})).await?;
    assert_success(&recovery);
    list_tools(process, "smoke-tools-after").await
}

async fn call(process: &mut Process, id: &str, tool: &str, arguments: Value) -> Result<Value> {
    process.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).await?;
    match process.receive(id).await? {
        ServerJsonRpcMessage::Response(response) => match response.result {
            ServerResult::CallToolResult(result) => Ok(serde_json::to_value(result)?),
            other => bail!("expected tool result, got {other:?}"),
        },
        other => bail!("expected tool response, got {other:?}"),
    }
}

fn validate_captured_output(captured: &Captured) -> Result<()> {
    let mut count = 0;
    // Revalidate the entire stream, including any output after the final response.
    for (index, frame) in captured
        .stdout
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
    {
        decode_frame(frame).with_context(|| format!("stdout frame {}", index + 1))?;
        count += 1;
    }
    ensure!(
        count == 19,
        "expected exactly nineteen MCP responses, got {count}"
    );
    let stderr = std::str::from_utf8(&captured.stderr).context("stderr is not UTF-8")?;
    assert_safe(stderr);
    assert_safe(std::str::from_utf8(&captured.stdout)?);
    ensure!(
        stderr.contains("connected to Komodo Core") && stderr.contains("2.3.2"),
        "missing observed Core version"
    );
    ensure!(
        stderr.contains("starting komodo-mcp over stdio"),
        "missing application startup diagnostic on stderr"
    );
    ensure!(
        stderr.contains("MCP session closed"),
        "missing shutdown diagnostic on stderr"
    );
    ensure!(
        !stderr.contains('\u{1b}'),
        "ANSI escapes in captured diagnostics"
    );
    Ok(())
}

#[tokio::test]
async fn stdio_initialize_tools_and_eof() -> Result<()> {
    let mut steps = vec![Step::version()];
    for family in FAMILIES {
        let summary = family.summary();
        steps.push(family.list(0, 30, std::slice::from_ref(&summary)));
        for field in ["id", "name"] {
            steps.push(family.lookup(
                summary[field].as_str().unwrap(),
                std::slice::from_ref(&summary),
            ));
            steps.push(family.get(
                summary["id"].as_str().unwrap(),
                Reply::new(200, family.detail().to_string()),
            ));
        }
    }
    let first = Family::Server.summary();
    let mut last = first.clone();
    last["id"] = json!("000000000000000000000004");
    last["name"] = json!("fixture-server-next");
    steps.extend([
        Family::Server.list(0, 1, &[first]),
        Family::Server.list(1, 1, std::slice::from_ref(&last)),
        Family::Server.list(1, 1, &[last]),
        Family::Server.list(2, 1, &[]),
        Family::Stack.lookup("missing-stack", &[]),
        Family::Deployment.list_step(
            0,
            30,
            &[],
            &[],
            Reply::new(
                502,
                json!({
                    "error": format!("proxy {KEY} {SECRET}"),
                    "trace": [support::SENSITIVE_MARKERS[4]]
                })
                .to_string(),
            ),
        ),
        Family::Deployment.list(0, 30, &[]),
    ]);
    let operations: Vec<_> = steps.iter().map(|step| step.operation).collect();
    let core = MockCore::start(steps).await?;
    let mut process = Process::spawn(&[
        ("KOMODO_ADDRESS", Some(&core.address)),
        ("KOMODO_API_KEY", Some(KEY)),
        ("KOMODO_API_SECRET", Some(SECRET)),
    ])?;
    let protocol = timeout(SESSION_TIMEOUT, exchange(&mut process))
        .await
        .context("whole-session exchange deadline")
        .and_then(|result| result);
    // Always clean up before asserting, including timeout and malformed-frame paths.
    let shutdown = process.finish(protocol.is_err()).await.and_then(|status| {
        ensure!(status.success(), "komodo-mcp exited with {status}");
        Ok(())
    });
    let captured = process.captured.lock().unwrap();
    let output = validate_captured_output(&captured);
    ensure!(
        protocol.is_ok() && shutdown.is_ok() && output.is_ok(),
        "stdio smoke failed\nprotocol: {protocol:#?}\nshutdown: {shutdown:#?}\noutput: {output:#?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&captured.stdout),
        String::from_utf8_lossy(&captured.stderr)
    );
    core.assert_finished(&operations);
    Ok(())
}

async fn startup_failure(overrides: &[(&str, Option<&str>)], expected: &str) -> Result<()> {
    let mut process = Process::spawn(overrides)?;
    let status = process.finish(false).await?;
    let captured = process.captured.lock().unwrap();
    ensure!(!status.success(), "startup must fail");
    ensure!(captured.stdout.is_empty(), "failed startup emitted stdout");
    let stderr = std::str::from_utf8(&captured.stderr)?;
    assert_safe(stderr);
    ensure!(
        stderr.contains(expected),
        "missing safe failure identifier: {stderr}"
    );
    Ok(())
}

#[tokio::test]
async fn missing_and_invalid_environment_fail_safely() -> Result<()> {
    for missing in ["KOMODO_ADDRESS", "KOMODO_API_KEY", "KOMODO_API_SECRET"] {
        startup_failure(
            &[
                ("KOMODO_ADDRESS", Some("http://127.0.0.1:1")),
                ("KOMODO_API_KEY", Some(KEY)),
                ("KOMODO_API_SECRET", Some(SECRET)),
                (missing, None),
            ],
            missing,
        )
        .await?;
    }
    for (field, value) in [
        (
            "KOMODO_ADDRESS",
            format!("https://{KEY}:{SECRET}@core.invalid"),
        ),
        ("KOMODO_API_KEY", format!("{KEY}\n{SECRET}")),
        ("KOMODO_API_SECRET", String::new()),
        ("KOMODO_ALLOW_OPERATE", format!("true {KEY} {SECRET}")),
        ("KOMODO_ALLOW_CONFIGURATION_EDIT", "TRUE".into()),
    ] {
        startup_failure(
            &[
                ("KOMODO_ADDRESS", Some("http://127.0.0.1:1")),
                ("KOMODO_API_KEY", Some(KEY)),
                ("KOMODO_API_SECRET", Some(SECRET)),
                (field, Some(&value)),
            ],
            field,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn upstream_startup_failures_have_safe_stderr() -> Result<()> {
    for (reply, expected) in [
        (
            Reply::echoed(401, support::AUTHENTICATION),
            "authentication",
        ),
        (Reply::echoed(500, support::PERMISSION), "permission_denied"),
        (Reply::new(502, format!("proxy {KEY} {SECRET}")), "upstream"),
        (
            Reply::new(200, format!("invalid {KEY} {SECRET}")),
            "invalid_response",
        ),
    ] {
        let core = MockCore::start(vec![Step::new("GetVersion", reply)]).await?;
        startup_failure(
            &[
                ("KOMODO_ADDRESS", Some(&core.address)),
                ("KOMODO_API_KEY", Some(KEY)),
                ("KOMODO_API_SECRET", Some(SECRET)),
            ],
            expected,
        )
        .await?;
        core.assert_finished(&["GetVersion"]);
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = format!("http://{}", listener.local_addr()?);
    drop(listener);
    startup_failure(
        &[
            ("KOMODO_ADDRESS", Some(&address)),
            ("KOMODO_API_KEY", Some(KEY)),
            ("KOMODO_API_SECRET", Some(SECRET)),
        ],
        "unreachable",
    )
    .await
}

#[tokio::test]
async fn stalled_startup_obeys_production_deadline() -> Result<()> {
    let mut reply = Reply::new(200, support::VERSION);
    reply.body_delay = Duration::from_secs(20);
    let core = MockCore::start(vec![Step::new("GetVersion", reply)]).await?;
    startup_failure(
        &[
            ("KOMODO_ADDRESS", Some(&core.address)),
            ("KOMODO_API_KEY", Some(KEY)),
            ("KOMODO_API_SECRET", Some(SECRET)),
        ],
        "timeout",
    )
    .await?;
    core.assert_finished(&["GetVersion"]);
    Ok(())
}
