//! Independent adversarial coverage of encoded JSON credentials at the MCP edge.
//! The oracle uses serde_json serialization, never the redaction parser.

mod support;

use std::{collections::BTreeMap, process::Stdio, time::Duration};

use serde_json::{Value, json};
use support::{
    SECRET,
    incident::{IncidentCore, Options, fixture, wire},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};

const DEADLINE: Duration = Duration::from_secs(5);
const PRIVATE: &str = "private-escaped-marker";

fn encode_fragment(mut text: String, depth: usize) -> String {
    for _ in 0..depth {
        let quoted = serde_json::to_string(&text).unwrap();
        text = quoted[1..quoted.len() - 1].to_owned();
    }
    text
}

fn json_assignment(label: &str, value: &str) -> String {
    let fields = BTreeMap::from([
        ("adjacent_before", "keep-before💥"),
        (label, value),
        ("neighbor", "keep-afteré"),
    ]);
    serde_json::to_string(&fields).unwrap()
}

fn options(stdout: &str, stderr: &str, api_key: Option<&str>) -> Options {
    let mut log = wire()["log"].clone();
    log["stdout"] = json!(stdout);
    log["stderr"] = json!(stderr);
    Options {
        overrides: BTreeMap::from([("GetStackLog".into(), log)]),
        api_key: api_key.map(str::to_owned),
        ..Default::default()
    }
}

struct StdioClient {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    stderr: JoinHandle<Vec<u8>>,
    next_id: u64,
}

impl StdioClient {
    async fn start(core: &IncidentCore) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", core.api_key())
            .env("KOMODO_API_SECRET", SECRET)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut pipe = child.stderr.take().unwrap();
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            while let Ok(count) = pipe.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                let keep = count.min((64 * 1024usize).saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..keep]);
            }
            bytes
        });
        let mut client = Self {
            child,
            input,
            output,
            stderr,
            next_id: 0,
        };
        let initialized = client
            .request(
                "initialize",
                json!({
                    "protocolVersion":"2025-03-26", "capabilities":{},
                    "clientInfo":{"name":"escaped-log-test","version":"1"}
                }),
            )
            .await;
        assert_eq!(initialized["serverInfo"]["name"], "komodo-mcp");
        client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        client
    }

    async fn send(&mut self, message: Value) {
        let mut encoded = serde_json::to_vec(&message).unwrap();
        encoded.push(b'\n');
        let input = self.input.as_mut().unwrap();
        timeout(DEADLINE, async {
            input.write_all(&encoded).await.unwrap();
            input.flush().await.unwrap();
        })
        .await
        .unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        timeout(DEADLINE, async {
            for _ in 0..32 {
                let mut line = String::new();
                assert!(
                    self.output.read_line(&mut line).await.unwrap() > 0,
                    "MCP EOF"
                );
                let response: Value = serde_json::from_str(&line).unwrap();
                if response.get("id").is_none() {
                    continue;
                }
                assert_eq!(response["id"], id);
                assert!(
                    response.get("error").is_none(),
                    "MCP protocol error: {response}"
                );
                return response["result"].clone();
            }
            panic!("too many MCP notifications");
        })
        .await
        .unwrap()
    }

    async fn logs(&mut self, max_bytes: usize) -> Value {
        let result = self.request("tools/call", json!({"name":"get_stack_logs","arguments":{
            "id":fixture()["stack"]["id"], "services":["web"], "tail":500, "max_bytes":max_bytes
        }})).await;
        assert_eq!(result["isError"], false, "{result}");
        let projected = result["structuredContent"].clone();
        let texts = result["content"].as_array().unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0]["type"], "text");
        assert_eq!(
            serde_json::from_str::<Value>(texts[0]["text"].as_str().unwrap()).unwrap(),
            projected,
            "MCP text must match structured output"
        );
        assert!(
            !result.to_string().contains(PRIVATE),
            "credential leaked through MCP: {result}"
        );
        let stdout = projected["stdout"].as_str().unwrap();
        let stderr = projected["stderr"].as_str().unwrap();
        assert_eq!(projected["returned_bytes"], stdout.len() + stderr.len());
        assert_eq!(
            projected["returned_lines"],
            stdout.lines().count() + stderr.lines().count()
        );
        assert!(stdout.len() + stderr.len() <= max_bytes);
        projected
    }

    async fn close(mut self) {
        self.input.take();
        let status = timeout(DEADLINE, self.child.wait()).await.unwrap().unwrap();
        assert!(status.success(), "child did not shut down cleanly");
        let stderr = timeout(DEADLINE, self.stderr).await.unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&stderr).contains(PRIVATE));
    }
}

#[tokio::test]
async fn encoded_assignments_mask_complete_values_and_preserve_adjacent_context_through_stdio() {
    let secrets = [
        format!("Basic {PRIVATE}-basic"),
        format!(
            "Digest username=\"{PRIVATE}-user\", response=\"{PRIVATE}-response\", nonce=\"{PRIVATE}-nonce\""
        ),
        format!("{PRIVATE}-internal-\"quote\"-then-backslash\\"),
        format!("{PRIVATE}-ending-two\\\\"),
        format!("{PRIVATE}-ending-three\\\\\\"),
        format!("{PRIVATE}-emoji💥é-\"quoted\"-slash\\\\"),
    ];
    let mut stdout = String::new();
    let mut expected = String::new();
    for depth in 0..=4 {
        for label in ["authorization", "token", "password"] {
            for (index, secret) in secrets.iter().enumerate() {
                let prefix = format!("case-{depth}-{label}-{index} ");
                stdout.push_str(&prefix);
                stdout.push_str(&encode_fragment(json_assignment(label, secret), depth));
                stdout.push_str("\r\n");
                expected.push_str(&prefix);
                expected.push_str(&encode_fragment(
                    json_assignment(label, "[redacted]"),
                    depth,
                ));
                expected.push_str("\r\n");
            }
        }
    }
    let stderr = format!(
        "context {}\n",
        encode_fragment(json_assignment("authorization", &secrets[1]), 4)
    );
    let expected_stderr = format!(
        "context {}\n",
        encode_fragment(json_assignment("authorization", "[redacted]"), 4)
    );
    assert!(
        expected.len() + expected_stderr.len() < 65_536,
        "test output exceeds declared log budget"
    );
    let core = IncidentCore::start(options(&stdout, &stderr, None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(65_536).await;
    assert_eq!(
        result["stdout"], expected,
        "encoded values or harmless context changed"
    );
    assert_eq!(result["stderr"], expected_stderr);
    assert_eq!(result["truncation"]["bytes"], false);
    assert_eq!(result["truncation"]["lines"], false);
    client.close().await;
    assert_eq!(
        core.requests()
            .iter()
            .map(|r| r.operation.as_str())
            .collect::<Vec<_>>(),
        ["GetVersion", "GetStackLog"]
    );
}

#[tokio::test]
async fn encoded_credentials_are_removed_before_tiny_utf8_budgets_are_applied() {
    let stdout = format!(
        "{}💥é",
        encode_fragment(
            json_assignment(
                "authorization",
                &format!("Digest user=\"{PRIVATE}\", response=\"{PRIVATE}-response\\\"")
            ),
            3
        )
    );
    let core = IncidentCore::start(options(&stdout, "", None)).await;
    let mut client = StdioClient::start(&core).await;
    for budget in 1..=40 {
        let result = client.logs(budget).await;
        assert_eq!(result["truncation"]["bytes"], true);
        let returned = result["stdout"].as_str().unwrap();
        assert!(std::str::from_utf8(returned.as_bytes()).is_ok());
        assert!(
            !returned.contains("private"),
            "partially exposed credential at {budget} bytes"
        );
        assert_eq!(result["returned_lines"], usize::from(!returned.is_empty()));
    }
    client.close().await;
    let requests = core.requests();
    assert_eq!(requests.len(), 41);
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.operation == "GetStackLog" && r.params["tail"] == 500)
    );
}

#[tokio::test]
async fn encoded_sensitive_labels_are_recognized_before_same_named_api_keys_are_redacted() {
    for label in ["authorization", "token", "password"] {
        let source = format!(
            "{}\n",
            encode_fragment(json_assignment(label, &format!("Basic {PRIVATE}")), 4)
        );
        let core = IncidentCore::start(options(&source, "", Some(label))).await;
        let mut client = StdioClient::start(&core).await;
        let result = client.logs(4096).await;
        let output = result["stdout"].as_str().unwrap();
        assert!(output.contains("keep-before💥") && output.contains("keep-afteré"));
        assert_eq!(result["returned_lines"], 1);
        client.close().await;
    }
}

#[tokio::test]
async fn malformed_encoded_value_openers_mask_ambiguous_credentials_through_line_end() {
    let mut stdout = String::new();
    for label in ["token", "password"] {
        for slashes in [2, 4] {
            // The key is encoded once (one slash); the value opener's run is
            // noncanonical at every JSON encoding depth. Preserve CR/LF while
            // conservatively hiding the ambiguous remainder, including spaces.
            stdout.push_str(&format!(
                "{{\\\"{label}\\\":{}\"{PRIVATE}-first {PRIVATE}-second\\\",\\\"neighbor\\\":\\\"ambiguous-context\\\"}}\r\n",
                "\\".repeat(slashes)
            ));
            stdout.push_str(&format!(
                "{label}={}\"{PRIVATE}-first {PRIVATE}-second\r\n",
                "\\".repeat(slashes)
            ));
        }
    }
    let core = IncidentCore::start(options(&stdout, "", None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(4096).await;
    assert_eq!(result["returned_lines"], 8);
    assert_eq!(
        result["stdout"].as_str().unwrap().matches("\r\n").count(),
        8,
        "malformed quote fallback must retain physical line terminators"
    );
    client.close().await;
}
