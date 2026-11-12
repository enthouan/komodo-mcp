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
        self.logs_with_tail(max_bytes, 500).await
    }

    async fn logs_with_tail(&mut self, max_bytes: usize, tail: u64) -> Value {
        let result = self.request("tools/call", json!({"name":"get_stack_logs","arguments":{
            "id":fixture()["stack"]["id"], "services":["web"], "tail":tail, "max_bytes":max_bytes
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

fn multiline_assignment(
    label: &str,
    value: &str,
    before_colon: &str,
    before_value: &str,
) -> String {
    // JSON whitespace is intentionally generated independently from production
    // scanning. Verify the source remains valid JSON before encoding it again.
    let key = serde_json::to_string(label).unwrap();
    let value = serde_json::to_string(value).unwrap();
    let source = format!(
        "{{\"adjacent_before\":\"keep-before💥\",{key}{before_colon}:{before_value}{value},\"neighbor\":\"keep-afteré\"}}"
    );
    assert!(serde_json::from_str::<Value>(&source).is_ok());
    source
}

#[tokio::test]
async fn multiline_json_separators_preserve_context_and_hide_values_across_physical_lines() {
    let layouts = [("", "\n"), ("\r\n\t", "\r\n\t"), (" \n \t", " \r\n \t")];
    let mut stdout = String::new();
    let mut expected_stdout = String::new();
    let mut stderr = String::new();
    let mut expected_stderr = String::new();
    for depth in 0..=4 {
        for label in ["password", "token", "authorization"] {
            for (before_colon, before_value) in layouts {
                let secret =
                    format!("Digest user=\"{PRIVATE}\", response=\"{PRIVATE}-response\\\"");
                let source = encode_fragment(
                    multiline_assignment(label, &secret, before_colon, before_value),
                    depth,
                );
                let expected = encode_fragment(
                    multiline_assignment(label, "[redacted]", before_colon, before_value),
                    depth,
                );
                stdout.push_str(&source);
                stdout.push_str("\r\n");
                expected_stdout.push_str(&expected);
                expected_stdout.push_str("\r\n");
                stderr.push_str("error-context ");
                stderr.push_str(&source);
                stderr.push('\n');
                expected_stderr.push_str("error-context ");
                expected_stderr.push_str(&expected);
                expected_stderr.push('\n');
            }
        }
    }
    assert!(expected_stdout.len() + expected_stderr.len() < 65_536);
    let core = IncidentCore::start(options(&stdout, &stderr, None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(65_536).await;
    assert_eq!(result["stdout"], expected_stdout);
    assert_eq!(result["stderr"], expected_stderr);
    assert_eq!(
        result["stdout"].as_str().unwrap().matches('\n').count(),
        stdout.matches('\n').count()
    );
    assert_eq!(
        result["stderr"].as_str().unwrap().matches('\n').count(),
        stderr.matches('\n').count()
    );
    assert_eq!(result["truncation"]["lines"], false);
    assert_eq!(result["truncation"]["bytes"], false);
    client.close().await;
    assert_eq!(core.requests().len(), 2);
}

#[tokio::test]
async fn local_tail_keeps_redaction_context_without_extra_or_unbounded_core_reads() {
    for label in ["password", "token", "authorization"] {
        for use_stderr in [false, true] {
            let source = format!("{{\"{label}\"\r\n:\r\n\"{PRIVATE}\"}}");
            assert!(serde_json::from_str::<Value>(&source).is_ok());
            let core = IncidentCore::start(if use_stderr {
                options("", &source, None)
            } else {
                options(&source, "", None)
            })
            .await;
            let mut client = StdioClient::start(&core).await;
            let result = client.logs_with_tail(4096, 1).await;
            let field = if use_stderr { "stderr" } else { "stdout" };
            assert_eq!(
                result[field], "\"[redacted]\"}",
                "key context was dropped before masking"
            );
            assert_eq!(result["returned_lines"], 1);
            assert_eq!(result["truncation"]["lines"], true);
            client.close().await;
            let requests = core.requests();
            assert_eq!(
                requests.len(),
                2,
                "no fallback or supplemental read is allowed"
            );
            assert_eq!(requests[1].operation, "GetStackLog");
            assert_eq!(
                requests[1].params["tail"], 1,
                "upstream tail must stay bounded as requested"
            );
        }
    }
}

#[tokio::test]
async fn stream_boundaries_and_empty_authorization_headers_do_not_consume_harmless_lines() {
    // A partial key in stdout must not borrow a value from the other stream.
    // A bare empty HTTP Authorization header must retain its next line.
    let stdout = "Authorization:\r\nkeep-http-next-line💥\r\n{\"password\":\r\n";
    let stderr = format!("\"keep-independent-stderré\"\r\n{{\"token\":\r\n\"{PRIVATE}\"}}\r\n");
    let core = IncidentCore::start(options(stdout, &stderr, None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(4096).await;
    assert_eq!(result["stdout"], stdout);
    assert_eq!(
        result["stderr"],
        "\"keep-independent-stderré\"\r\n{\"token\":\r\n\"[redacted]\"}\r\n"
    );
    client.close().await;
}

#[tokio::test]
async fn multiline_context_survives_local_tail_and_tiny_utf8_byte_clipping() {
    let stdout = format!("{{\"authorization\"\r\n:\r\n\"Basic {PRIVATE}\"}}💥é");
    let core = IncidentCore::start(options(&stdout, "", None)).await;
    let mut client = StdioClient::start(&core).await;
    for bytes in 1..=24 {
        let result = client.logs_with_tail(bytes, 1).await;
        assert_eq!(
            result["returned_lines"],
            usize::from(!result["stdout"].as_str().unwrap().is_empty())
        );
        assert_eq!(result["truncation"]["lines"], true);
        assert!(!result["stdout"].as_str().unwrap().contains("private"));
    }
    client.close().await;
    let requests = core.requests();
    assert_eq!(requests.len(), 25);
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.operation == "GetStackLog" && r.params["tail"] == 1)
    );
}

#[tokio::test]
async fn ambiguous_authorization_inner_quotes_mask_complete_digest_credentials_through_stdio() {
    let cases = [
        // Keep the review's exact reproduction alongside uniquely marked cases.
        r#"authorization="Digest username="victim", response="secret"""#.to_owned(),
        format!(
            r#"authorization="Digest username="{PRIVATE}-victim", response="{PRIVATE}-response"""#
        ),
        format!(
            "authorization='Digest username='{PRIVATE}-victim', response='{PRIVATE}-response''"
        ),
        format!(
            r#"{{"authorization":"Digest username="{PRIVATE}-victim", response="{PRIVATE}-response"","neighbor":"ambiguous-context"}}"#
        ),
        format!(
            "{{'authorization':'Digest username='{PRIVATE}-victim', response='{PRIVATE}-response'','neighbor':'ambiguous-context'}}"
        ),
        // A comma after the premature quote is not sufficient evidence that the
        // Authorization value ended: Digest itself contains comma separators.
        format!(r#"authorization="Digest username=", response="{PRIVATE}-response"""#),
        format!(
            r#"{{"authorization":"Digest username=", response="{PRIVATE}-response"","neighbor":"ambiguous-context"}}"#
        ),
        // Valid-looking field/container trailers can still be part of malformed
        // Digest wrapping. They must not make a premature quote authoritative.
        format!(r#"{{"authorization":"Digest username=", "response":"{PRIVATE}-response""}}"#),
        format!(r#"{{"authorization":"Digest username="}}, response="{PRIVATE}-response""}}"#),
    ];
    let mut stdout = String::new();
    for depth in 0..=4 {
        for (index, source) in cases.iter().enumerate() {
            stdout.push_str(&format!("ambiguous-case-{depth}-{index} "));
            stdout.push_str(&encode_fragment(source.clone(), depth));
            stdout.push_str("\r\n");
        }
    }
    let stderr = format!("{}\r\n", encode_fragment(cases[3].clone(), 4));
    let core = IncidentCore::start(options(&stdout, &stderr, None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(65_536).await;
    let rendered = result.to_string();
    assert!(
        !rendered.contains("victim"),
        "Digest username survived ambiguous quoting"
    );
    assert!(
        !rendered.contains("secret"),
        "Digest response survived ambiguous quoting"
    );
    assert_eq!(
        result["stdout"].as_str().unwrap().matches("\r\n").count(),
        cases.len() * 5
    );
    assert_eq!(
        result["stderr"].as_str().unwrap().matches("\r\n").count(),
        1
    );
    for depth in 0..=4 {
        for index in 0..cases.len() {
            assert!(
                result["stdout"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("ambiguous-case-{depth}-{index} "))
            );
        }
    }
    assert_eq!(result["truncation"]["lines"], false);
    assert_eq!(result["truncation"]["bytes"], false);
    client.close().await;
    assert_eq!(core.requests().len(), 2);
}

#[tokio::test]
async fn structured_credential_values_hide_nested_secrets_and_preserve_outer_json_fields() {
    let values = [
        json!({"first": format!("{PRIVATE}-object"), "details": {
            "credential": format!("{PRIVATE}-nested"), "punctuation": "quoted \" } ] { ["
        }, "nested_array": [format!("{PRIVATE}-deep"), {"v":format!("{PRIVATE}-deeper")}]}),
        json!([format!("{PRIVATE}-array"), {"credential": format!("{PRIVATE}-nested"),
            "nested": [format!("{PRIVATE}-deep"), [format!("{PRIVATE}-deeper")]]},
            "quoted \" } ] { ["]),
    ];
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut neighbors = Vec::new();
    for depth in 0..=4 {
        for label in ["authorization", "token", "password"] {
            for (index, value) in values.iter().enumerate() {
                let key = serde_json::to_string(label).unwrap();
                let value = serde_json::to_string_pretty(value).unwrap();
                let neighbor = format!("keep-structured-{depth}-{label}-{index}💥é");
                let suffix = format!(
                    "\"z_neighbor\":{}",
                    serde_json::to_string(&neighbor).unwrap()
                );
                let source = format!(
                    "{{\"adjacent_before\":\"keep-structured-before\",{key}:{value},{suffix}}}"
                );
                assert!(serde_json::from_str::<Value>(&source).is_ok());
                let encoded = encode_fragment(source, depth);
                stdout.push_str(&encoded);
                stdout.push_str("\r\n");
                stderr.push_str(&encoded);
                stderr.push('\n');
                neighbors.push(encode_fragment(suffix, depth));
            }
        }
    }
    let core = IncidentCore::start(options(&stdout, &stderr, None)).await;
    let mut client = StdioClient::start(&core).await;
    let result = client.logs(65_536).await;
    for field in ["stdout", "stderr"] {
        let text = result[field].as_str().unwrap();
        for neighbor in &neighbors {
            assert!(
                text.contains(neighbor),
                "unambiguous neighboring field was lost in {field}"
            );
        }
    }
    assert_eq!(
        result["stdout"].as_str().unwrap().matches('\n').count(),
        stdout.matches('\n').count()
    );
    assert_eq!(
        result["stderr"].as_str().unwrap().matches('\n').count(),
        stderr.matches('\n').count()
    );
    assert_eq!(result["truncation"]["lines"], false);
    assert_eq!(result["truncation"]["bytes"], false);
    client.close().await;
    assert_eq!(core.requests().len(), 2);
}
