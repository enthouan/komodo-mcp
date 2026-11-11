use super::*;
use komodo_client::api::read::{GetDeployment, GetServer, GetStack, ListDeployments, ListStacks};
use komodo_mcp::server::KomodoServer;
use rmcp::ServiceExt;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

#[derive(Debug, Clone, Copy)]
pub enum Family {
    Server,
    Stack,
    Deployment,
}
pub const FAMILIES: [Family; 3] = [Family::Server, Family::Stack, Family::Deployment];

impl Family {
    pub fn list_tool(self) -> &'static str {
        match self {
            Self::Server => "list_servers",
            Self::Stack => "list_stacks",
            Self::Deployment => "list_deployments",
        }
    }
    pub fn get_tool(self) -> &'static str {
        match self {
            Self::Server => "get_server",
            Self::Stack => "get_stack",
            Self::Deployment => "get_deployment",
        }
    }
    pub fn list_operation(self) -> &'static str {
        match self {
            Self::Server => "ListServers",
            Self::Stack => "ListStacks",
            Self::Deployment => "ListDeployments",
        }
    }
    pub fn get_operation(self) -> &'static str {
        match self {
            Self::Server => "GetServer",
            Self::Stack => "GetStack",
            Self::Deployment => "GetDeployment",
        }
    }
    pub fn summary(self) -> Value {
        let source = match self {
            Self::Server => SERVERS,
            Self::Stack => include_str!("../fixtures/core-2.3.2/stacks.json"),
            Self::Deployment => include_str!("../fixtures/core-2.3.2/deployments.json"),
        };
        serde_json::from_str::<Value>(source).unwrap()[0].clone()
    }
    pub fn detail(self) -> Value {
        serde_json::from_str(match self {
            Self::Server => include_str!("../fixtures/core-2.3.2/server-detail.json"),
            Self::Stack => include_str!("../fixtures/core-2.3.2/stack-detail.json"),
            Self::Deployment => include_str!("../fixtures/core-2.3.2/deployment-detail.json"),
        })
        .unwrap()
    }
    pub fn list_step(
        self,
        page: u64,
        limit: u64,
        names: &[&str],
        terms: &[&str],
        reply: Reply,
    ) -> Step {
        macro_rules! request {
            ($ty:ident) => {{
                let mut request = $ty {
                    page,
                    limit: Some(limit),
                    sort_desc: false,
                    ..Default::default()
                };
                request.query.names = names.iter().map(|s| (*s).into()).collect();
                request.query.terms = terms.iter().map(|s| (*s).into()).collect();
                Step::read(request, reply)
            }};
        }
        match self {
            Self::Server => request!(ListServers),
            Self::Stack => request!(ListStacks),
            Self::Deployment => request!(ListDeployments),
        }
    }
    pub fn list(self, page: u64, limit: u64, items: &[Value]) -> Step {
        self.list_step(
            page,
            limit,
            &[],
            &[],
            Reply::new(200, json!(items).to_string()),
        )
    }
    pub fn lookup(self, selector: &str, items: &[Value]) -> Step {
        self.list_step(
            0,
            100,
            &[selector],
            &[],
            Reply::new(200, json!(items).to_string()),
        )
    }
    pub fn get(self, id: &str, reply: Reply) -> Step {
        match self {
            Self::Server => Step::read(GetServer { server: id.into() }, reply),
            Self::Stack => Step::read(GetStack { stack: id.into() }, reply),
            Self::Deployment => Step::read(
                GetDeployment {
                    deployment: id.into(),
                },
                reply,
            ),
        }
    }
    pub fn keys(self, detail: bool) -> Vec<&'static str> {
        let mut keys = vec!["id", "name", "template", "state"];
        match self {
            Self::Server => {
                keys.extend(["region", "periphery_version"]);
                if detail {
                    keys.push("enabled");
                }
            }
            Self::Stack => {
                keys.extend(["target", "service_count"]);
                if detail {
                    keys.push("project_name");
                }
            }
            Self::Deployment => {
                keys.extend(["target", "image", "runtime_name", "update_available"])
            }
        }
        keys
    }
}

pub fn assert_keys(value: &Value, keys: &[&str]) {
    let mut actual: Vec<_> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    actual.sort_unstable();
    let mut expected = keys.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

pub fn assert_success(result: &Value) -> &Value {
    assert_eq!(result["isError"], false, "{result}");
    let data = &result["structuredContent"];
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(result["content"][0]["type"], "text");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        &text, data,
        "text and structured content must be the same projection"
    );
    assert_safe(&result.to_string());
    data
}

pub fn assert_failure<'a>(result: &'a Value, kind: &str) -> &'a Value {
    assert_eq!(result["isError"], true, "{result}");
    let data = &result["structuredContent"];
    assert_eq!(data["kind"], kind, "{result}");
    let mut keys = vec!["kind", "message", "operation"];
    for key in ["status", "correction", "candidates"] {
        if data.get(key).is_some() {
            keys.push(key);
        }
    }
    assert_keys(data, &keys);
    assert_safe(&result.to_string());
    data
}

pub struct McpSession {
    read: BufReader<ReadHalf<DuplexStream>>,
    write: Option<WriteHalf<DuplexStream>>,
    task: JoinHandle<Result<()>>,
    next_id: u64,
    captured: String,
}

impl McpSession {
    pub async fn start(core: &MockCore, deadline: Duration) -> Result<Self> {
        let client = core.client(deadline);
        let core_version = client.detect_version().await?;
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let task = tokio::spawn(async move {
            KomodoServer {
                client,
                core_version,
            }
            .serve(server_io)
            .await?
            .waiting()
            .await?;
            Ok(())
        });
        let (read, write) = tokio::io::split(client_io);
        let mut session = Self {
            read: BufReader::new(read),
            write: Some(write),
            task,
            next_id: 0,
            captured: String::new(),
        };
        session.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"inventory-fixture","version":"1"}})).await?;
        session
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        Ok(session)
    }
    async fn send(&mut self, message: Value) -> Result<()> {
        timeout(
            TEST_TIMEOUT,
            self.write
                .as_mut()
                .unwrap()
                .write_all(format!("{message}\n").as_bytes()),
        )
        .await??;
        Ok(())
    }
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let mut frame = String::new();
        ensure!(
            timeout(TEST_TIMEOUT, self.read.read_line(&mut frame))
                .await
                .with_context(|| format!("await MCP {method} response"))??
                > 0,
            "unexpected MCP EOF"
        );
        self.captured.push_str(&frame);
        let response: Value = serde_json::from_str(&frame)?;
        assert_eq!(response["id"], id);
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(
            response.get("error").is_none(),
            "unexpected protocol error: {response}"
        );
        assert_safe(&frame);
        Ok(response["result"].clone())
    }
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<Value> {
        self.request("tools/call", json!({"name":tool,"arguments":arguments}))
            .await
    }
    pub async fn finish(mut self) -> Result<()> {
        assert_safe(&self.captured);
        self.write.as_mut().unwrap().shutdown().await?;
        drop(self.write.take());
        timeout(TEST_TIMEOUT, &mut self.task)
            .await
            .context("MCP session shutdown")???;
        let mut tail = String::new();
        timeout(TEST_TIMEOUT, self.read.read_to_string(&mut tail)).await??;
        assert!(tail.is_empty(), "unexpected trailing MCP frames");
        Ok(())
    }
}
impl Drop for McpSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}
