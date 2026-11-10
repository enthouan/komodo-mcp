use std::{
    io,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rmcp::model::{ErrorCode, ProtocolVersion, RequestId, ServerJsonRpcMessage, ServerResult};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc::{self, UnboundedReceiver},
    task::JoinHandle,
    time::timeout,
};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_TIMEOUT: Duration = Duration::from_secs(40);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
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
    fn spawn() -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env("RUST_LOG", "komodo_mcp=info,rmcp=info")
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

    async fn finish(&mut self, force_kill: bool) -> Result<()> {
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
        let status = exit?;
        ensure!(status.success(), "komodo-mcp exited with {status}");
        Ok(())
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
    // The empty registry does not change and emits no unsolicited notifications.
    ensure!(
        matches!(
            message,
            ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
        ),
        "unexpected MCP request or notification: {message:?}"
    );
    Ok(message)
}

async fn list_empty_tools(process: &mut Process, id: &str) -> Result<()> {
    process
        .send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}}))
        .await
        .context("send tools/list")?;
    match process.receive(id).await.context("receive tools/list")? {
        ServerJsonRpcMessage::Response(response) => match response.result {
            ServerResult::ListToolsResult(result) => {
                ensure!(result.tools.is_empty(), "expected empty tools array");
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
                "clientInfo": {"name": "stdio-smoke", "version": "1.0.0"}
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
    list_empty_tools(process, "smoke-tools-before").await?;

    process
        .send(json!({
            "jsonrpc": "2.0",
            "id": "smoke-unknown-tool",
            "method": "tools/call",
            "params": {"name": "nonexistent-tool", "arguments": {}}
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
    list_empty_tools(process, "smoke-tools-after").await
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
        count == 4,
        "expected exactly four MCP responses, got {count}"
    );
    let stderr = std::str::from_utf8(&captured.stderr).context("stderr is not UTF-8")?;
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
    let mut process = Process::spawn()?;
    let protocol = timeout(SESSION_TIMEOUT, exchange(&mut process))
        .await
        .context("whole-session exchange deadline")
        .and_then(|result| result);
    // Always clean up before asserting, including timeout and malformed-frame paths.
    let shutdown = process.finish(protocol.is_err()).await;
    let captured = process.captured.lock().unwrap();
    let output = validate_captured_output(&captured);
    ensure!(
        protocol.is_ok() && shutdown.is_ok() && output.is_ok(),
        "stdio smoke failed\nprotocol: {protocol:#?}\nshutdown: {shutdown:#?}\noutput: {output:#?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&captured.stdout),
        String::from_utf8_lossy(&captured.stderr)
    );
    Ok(())
}
