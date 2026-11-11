mod support;

use std::time::Duration;

use anyhow::{Context, Result};
use komodo_client::api::read::GetServer;
use komodo_mcp::client::CoreClient;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerConfig,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    task::JoinHandle,
    time::timeout,
};

use support::*;

struct TestHandler(CoreClient);

impl ServerHandler for TestHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let read = if request.name == "restricted" {
            self.0
                .read(GetServer {
                    server: "restricted-server".into(),
                })
                .await
                .map(|_| ())
        } else {
            self.0.list_servers().await.map(|_| ())
        };
        Ok(match read {
            Ok(()) => CallToolResult::success(vec![ContentBlock::text("read succeeded")]),
            Err(error) => error.into_tool_result(),
        }
        .into())
    }
}

struct AbortOnDrop(JoinHandle<Result<()>>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn real_mcp_messages_keep_typed_failures_safe() -> Result<()> {
    let mut stalled = Reply::new(200, "[]");
    stalled.body_delay = Duration::from_secs(3);
    let core = MockCore::start(vec![
        Step::version(),
        Step::new("ListServers", Reply::echoed(401, AUTHENTICATION)),
        Step::new("GetServer", Reply::echoed(500, PERMISSION)),
        Step::new("ListServers", stalled),
        Step::new(
            "ListServers",
            Reply::new(502, format!("proxy {KEY} {SECRET}")),
        ),
        Step::new("ListServers", Reply::new(200, "[]")),
    ])
    .await?;
    let client = core.client(Duration::from_millis(300));
    client.detect_version().await?;
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let mut task = AbortOnDrop(tokio::spawn(async move {
        TestHandler(client)
            .serve(server_io)
            .await?
            .waiting()
            .await?;
        Ok(())
    }));
    let (read, mut write) = tokio::io::split(client_io);
    let mut read = BufReader::new(read);
    let mut captured = String::new();
    let messages = [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"inventory","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"restricted","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"inventory","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"inventory","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"inventory","arguments":{}}}),
    ];
    for (index, message) in messages.into_iter().enumerate() {
        timeout(
            TEST_TIMEOUT,
            write.write_all(format!("{message}\n").as_bytes()),
        )
        .await??;
        let mut frame = String::new();
        timeout(TEST_TIMEOUT, read.read_line(&mut frame)).await??;
        captured.push_str(&frame);
        let response: Value = serde_json::from_str(&frame).context("decode actual rmcp frame")?;
        assert_eq!(response["id"], index);
        if index == 0 {
            timeout(
                TEST_TIMEOUT,
                write
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"),
            )
            .await??;
        } else if index <= 4 {
            let kind = ["authentication", "permission_denied", "timeout", "upstream"][index - 1];
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(response["result"]["structuredContent"]["kind"], kind);
            assert!(
                response["error"].is_null(),
                "execution failure must be a tool result"
            );
        } else {
            assert_eq!(response["result"]["isError"], false);
        }
    }
    assert_safe(&captured);
    drop(write);
    drop(read);
    timeout(TEST_TIMEOUT, &mut task.0)
        .await
        .context("MCP handler shutdown deadline")???;
    core.assert_finished(&[
        "GetVersion",
        "ListServers",
        "GetServer",
        "ListServers",
        "ListServers",
        "ListServers",
    ]);
    Ok(())
}
