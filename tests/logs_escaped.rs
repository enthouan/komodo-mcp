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
        self.logs_tool("get_stack_logs", max_bytes, tail).await
    }

    async fn call_logs_tool(&mut self, tool: &str, max_bytes: usize, tail: u64) -> Value {
        let f = fixture();
        let mut arguments = match tool {
            "get_stack_logs" => json!({"id":f["stack"]["id"],"services":["web"]}),
            "get_container_logs" => {
                json!({"server_id":f["server"]["id"],"container":f["container"]})
            }
            "get_deployment_logs" => json!({"id":f["deployment"]["id"]}),
            _ => panic!("unsupported test tool"),
        };
        arguments["tail"] = json!(tail);
        arguments["max_bytes"] = json!(max_bytes);
        self.request("tools/call", json!({"name":tool,"arguments":arguments}))
            .await
    }

    async fn logs_tool(&mut self, tool: &str, max_bytes: usize, tail: u64) -> Value {
        let result = self.call_logs_tool(tool, max_bytes, tail).await;
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
async fn malformed_encoded_value_openers_each_mask_ambiguous_credentials_through_stdio() {
    let mut cases = Vec::new();
    for label in ["token", "password"] {
        for slashes in [2, 4] {
            // The key is encoded once (one slash); the value opener's run is
            // noncanonical at every JSON encoding depth. Preserve CR/LF while
            // conservatively hiding the ambiguous remainder, including spaces.
            cases.push(format!(
                "{{\\\"{label}\\\":{}\"{PRIVATE}-first {PRIVATE}-second\\\",\\\"neighbor\\\":\\\"ambiguous-context\\\"}}\r\n",
                "\\".repeat(slashes)
            ));
            cases.push(format!(
                "{label}={}\"{PRIVATE}-first {PRIVATE}-second\r\n",
                "\\".repeat(slashes)
            ));
        }
    }
    for source in cases {
        let core = IncidentCore::start(options(&source, &source, None)).await;
        let mut client = StdioClient::start(&core).await;
        let result = client.logs(4096).await;
        assert_eq!(result["returned_lines"], 2);
        for stream in ["stdout", "stderr"] {
            assert!(result[stream].as_str().unwrap().contains("[redacted]"));
            assert_eq!(result[stream].as_str().unwrap().matches("\r\n").count(), 1);
        }
        client.close().await;
        assert_eq!(core.requests().len(), 2);
    }
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
    for depth in 0..=4 {
        for (index, source) in cases.iter().enumerate() {
            let prefix = format!("ambiguous-case-{depth}-{index} ");
            let source = format!("{prefix}{}\r\n", encode_fragment(source.clone(), depth));
            let core = IncidentCore::start(options(&source, &source, None)).await;
            let mut client = StdioClient::start(&core).await;
            let result = client.logs(65_536).await;
            let rendered = result.to_string();
            assert!(!rendered.contains("victim"), "Digest username survived");
            assert!(!rendered.contains("secret"), "Digest response survived");
            for stream in ["stdout", "stderr"] {
                let text = result[stream].as_str().unwrap();
                assert!(text.starts_with(&prefix));
                assert!(text.contains("[redacted]"));
                assert_eq!(text.matches("\r\n").count(), 1);
            }
            assert_eq!(result["truncation"]["lines"], false);
            assert_eq!(result["truncation"]["bytes"], false);
            client.close().await;
            assert_eq!(core.requests().len(), 2);
        }
    }
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

fn all_log_options(stdout: &str, stderr: &str) -> Options {
    let mut configured = options(stdout, stderr, None);
    let log = configured.overrides["GetStackLog"].clone();
    configured
        .overrides
        .insert("GetContainerLog".into(), log.clone());
    configured.overrides.insert("GetDeploymentLog".into(), log);
    configured
}

const LOG_TOOLS: [(&str, &str); 3] = [
    ("get_stack_logs", "GetStackLog"),
    ("get_container_logs", "GetContainerLog"),
    ("get_deployment_logs", "GetDeploymentLog"),
];

#[tokio::test]
async fn yaml_comments_tags_anchors_and_plain_continuations_keep_all_sensitive_values_hidden() {
    for source in [
        format!("password:\n  # rotation\n  {PRIVATE}\ncontext-after\n"),
        format!("apiKey: # rotation\n\n  {PRIVATE}\ncontext-after\n"),
        format!("clientSecret: !!str\n  {PRIVATE}\ncontext-after\n"),
        format!("private_key: &anchor\n  {PRIVATE}\ncontext-after\n"),
        format!("databaseUrl:\n  {PRIVATE}-first\n\n  {PRIVATE}-second\ncontext-after\n"),
        format!("TOKEN={PRIVATE}-first\\\n  {PRIVATE}-second\ncontext-after\n"),
    ] {
        let core = IncidentCore::start(all_log_options(&source, &source)).await;
        let mut client = StdioClient::start(&core).await;
        for (tool, _) in LOG_TOOLS {
            let result = client.logs_tool(tool, 65_536, 500).await;
            for stream in ["stdout", "stderr"] {
                let text = result[stream].as_str().unwrap();
                assert!(text.contains("[redacted]"));
                assert!(!text.contains("context-after"));
                assert_eq!(text.matches('\n').count(), source.matches('\n').count());
            }
        }
        client.close().await;
        assert_eq!(core.requests().len(), 4);
    }
}

#[tokio::test]
async fn structured_prefixes_do_not_prove_where_an_unquoted_credential_ends() {
    for source in [
        format!("PASSWORD={{}}{PRIVATE}\n"),
        format!("token=[1,2]{PRIVATE}\n"),
        format!("privateKey={{\"nested\":\"{PRIVATE}\"}} trailer-{PRIVATE}\n"),
        format!("{{\"password\":{{}}\"{PRIVATE}\"}}\n"),
        format!("password={{}},\"response\":\"{PRIVATE}\"\n"),
    ] {
        let core = IncidentCore::start(all_log_options(&source, &source)).await;
        let mut client = StdioClient::start(&core).await;
        for (tool, _) in LOG_TOOLS {
            let result = client.logs_tool(tool, 4096, 500).await;
            for stream in ["stdout", "stderr"] {
                assert!(result[stream].as_str().unwrap().contains("[redacted]"));
                assert!(!result[stream].as_str().unwrap().contains("trailer-"));
            }
        }
        client.close().await;
        assert_eq!(core.requests().len(), 4);
    }
}

#[tokio::test]
async fn configured_credentials_are_masked_at_nested_json_depths_in_nonsensitive_fields() {
    let key = format!("{PRIVATE}-configured-\"key\\suffix");
    let mut source = String::new();
    let mut expected = String::new();
    for depth in 0..=8 {
        source.push_str(&encode_fragment(json_assignment("message", &key), depth));
        source.push('\n');
        expected.push_str(&encode_fragment(
            json_assignment("message", "[redacted]"),
            depth,
        ));
        expected.push('\n');
    }
    let mut configured = all_log_options(&source, &source);
    configured.api_key = Some(key);
    let core = IncidentCore::start(configured).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 65_536, 500).await;
        assert_eq!(result["stdout"], expected);
        assert_eq!(result["stderr"], expected);
    }
    client.close().await;
    assert_eq!(core.requests().len(), 4);
}

#[tokio::test]
async fn dense_standalone_bearer_tokens_finish_within_the_stdio_deadline() {
    let source = "Bearer x ".repeat(64_000);
    let core = IncidentCore::start(all_log_options(&source, "")).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 1, 1).await;
        assert_eq!(result["stdout"], " ");
        assert_eq!(result["truncation"]["bytes"], true);
    }
    client.close().await;
    assert_eq!(core.requests().len(), 4);
}

#[tokio::test]
async fn folded_authorization_values_are_masked_without_consuming_empty_headers() {
    for source in [
        format!("Authorization:\n  Basic {PRIVATE}\ncontext-after\n"),
        format!(
            "Authorization:\r\n\tDigest username=\"{PRIVATE}\", response=\"{PRIVATE}\"\r\ncontext-after\r\n"
        ),
        format!("Authorization:\n \tNegotiate {PRIVATE}\ncontext-after\n"),
        format!(
            "Authorization: Digest username=\"{PRIVATE}\"\n  response=\"{PRIVATE}\"\ncontext-after\n"
        ),
    ] {
        let core = IncidentCore::start(all_log_options(&source, &source)).await;
        let mut client = StdioClient::start(&core).await;
        for (tool, _) in LOG_TOOLS {
            let result = client.logs_tool(tool, 65_536, 500).await;
            for stream in ["stdout", "stderr"] {
                let text = result[stream].as_str().unwrap();
                assert!(text.contains("[redacted]"));
                assert!(!text.contains("context-after"));
                assert_eq!(text.matches('\n').count(), source.matches('\n').count());
            }
            for bytes in 1..=8 {
                client.logs_tool(tool, bytes, 1).await;
            }
        }
        client.close().await;
        assert_eq!(core.requests().len(), 28);
    }
    let empty = "Authorization:\r\n \t\r\nkeep-after-empty💥\nAuthorization:\nkeep-unindented\n";
    let core = IncidentCore::start(all_log_options(empty, empty)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 4096, 500).await;
        assert_eq!(result["stdout"], empty);
        assert_eq!(result["stderr"], empty);
    }
    client.close().await;
}

#[tokio::test]
async fn oversized_bodies_and_decoded_streams_fail_safely_before_redaction_without_fallback() {
    for chunked in [false, true] {
        for decoded_limit in [false, true] {
            let mut configured = all_log_options("", "");
            configured.chunked = chunked;
            let payload = if decoded_limit {
                let text = format!("password={}\n", PRIVATE.repeat(26_000));
                let mut log = wire()["log"].clone();
                log["stdout"] = json!(text);
                log["stderr"] = log["stdout"].clone();
                assert!(log["stdout"].as_str().unwrap().len() * 2 > 1024 * 1024);
                assert!(serde_json::to_vec(&log).unwrap().len() < 2 * 1024 * 1024);
                log
            } else {
                // An oversized non-Log shape must hit the body limit before
                // typed decoding, not merely fail the Log projection afterward.
                json!({"unstructured":PRIVATE.repeat(100_000)})
            };
            for (_, operation) in LOG_TOOLS {
                configured
                    .overrides
                    .insert(operation.into(), payload.clone());
            }
            let core = IncidentCore::start(configured).await;
            let mut client = StdioClient::start(&core).await;
            for (tool, _) in LOG_TOOLS {
                let result = client.call_logs_tool(tool, 1, 1).await;
                assert_eq!(
                    result["isError"], true,
                    "{tool}, chunked={chunked}, decoded={decoded_limit}"
                );
                assert_eq!(
                    result["structuredContent"]["kind"],
                    "response_too_large",
                    "{tool}, chunked={chunked}, decoded={decoded_limit}, payload_bytes={}: {result}",
                    serde_json::to_vec(&payload).unwrap().len()
                );
                assert!(!result.to_string().contains(PRIVATE));
            }
            client.close().await;
            let requests = core.requests();
            assert_eq!(requests.len(), 4);
            for (request, (_, operation)) in requests[1..].iter().zip(LOG_TOOLS) {
                assert_eq!(request.operation, operation);
                assert_eq!(request.params["tail"], 1);
            }
        }
    }
}

#[tokio::test]
async fn decoded_input_budget_accepts_its_exact_boundary_with_bounded_output() {
    let source = format!(
        "password={}",
        "x".repeat(komodo_mcp::logs::MAX_INPUT_BYTES - "password=".len())
    );
    let core = IncidentCore::start(all_log_options(&source, "")).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 1, 1).await;
        assert_eq!(result["stdout"], "]");
        assert_eq!(result["truncation"]["bytes"], true);
    }
    client.close().await;
    assert_eq!(core.requests().len(), 4);
}

#[tokio::test]
async fn unquoted_credentials_keep_no_suffix_after_ambiguous_delimiters() {
    let values = [
        format!("{PRIVATE}-first,{PRIVATE}-second"),
        format!("{PRIVATE}-first;{PRIVATE}-second"),
        format!("{PRIVATE}-first {PRIVATE}-second"),
        format!("postgres://alice:{PRIVATE}-first,{PRIVATE}-second@db/app"),
        format!("{PRIVATE}-first\\\"{PRIVATE}-second"),
        "alpha,beta".into(),
    ];
    let mut source = String::new();
    let mut expected = String::new();
    for label in [
        "PASSWORD",
        "DATABASE_URL",
        "connectionString",
        "TOKEN",
        "clientSecret",
    ] {
        for value in &values {
            source.push_str(&format!("probe {label}={value}\r\n"));
            expected.push_str(&format!("probe {label}=[redacted]\r\n"));
        }
    }
    let core = IncidentCore::start(all_log_options(&source, &source)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 65_536, 500).await;
        assert_eq!(result["stdout"], expected, "{tool}");
        assert_eq!(result["stderr"], expected, "{tool}");
    }
    client.close().await;
    assert_eq!(core.requests().len(), 4);
}

#[tokio::test]
async fn utf8_clipping_keeps_a_contiguous_latest_suffix_across_streams() {
    for (stdout, stderr, bytes, expected_stdout, expected_stderr) in [
        ("old\n💥\n", "", 2, "\n", ""),
        ("old\n💥", "", 2, "", ""),
        ("old\n", "💥\n", 2, "", "\n"),
        ("old\n", "💥", 2, "", ""),
        ("old\n💥\nend\n", "", 6, "\nend\n", ""),
    ] {
        let core = IncidentCore::start(all_log_options(stdout, stderr)).await;
        let mut client = StdioClient::start(&core).await;
        for (tool, _) in LOG_TOOLS {
            let result = client.logs_tool(tool, bytes, 500).await;
            assert_eq!(result["stdout"], expected_stdout, "{tool}");
            assert_eq!(result["stderr"], expected_stderr, "{tool}");
            assert_eq!(result["truncation"]["bytes"], true);
        }
        client.close().await;
        assert_eq!(core.requests().len(), 4);
    }
}

#[tokio::test]
async fn long_malformed_and_dense_structured_values_complete_within_stdio_deadline() {
    for source in [
        format!("password=[{}{PRIVATE}]\n", "\\".repeat(250_000)),
        format!("password={}\n", json!(vec![PRIVATE; 12_000])),
    ] {
        let core = IncidentCore::start(all_log_options(&source, &source)).await;
        let mut client = StdioClient::start(&core).await;
        for (tool, _) in LOG_TOOLS {
            // The MCP helper enforces a five-second exchange deadline; a tiny
            // output budget must not hide unbounded work on the received payload.
            let result = client.logs_tool(tool, 32, 1).await;
            assert_eq!(result["stdout"], "");
            assert_eq!(result["stderr"], "password=\"[redacted]\"\n");
        }
        client.close().await;
        let requests = core.requests();
        assert_eq!(requests.len(), 4);
        for (request, (_, operation)) in requests[1..].iter().zip(LOG_TOOLS) {
            assert_eq!(request.operation, operation);
            assert_eq!(request.params["tail"], 1);
        }
    }
}

#[tokio::test]
async fn prefixed_credential_keys_and_horizontal_bearer_spacing_are_masked_in_all_log_tools() {
    let stdout = format!(
        "POSTGRES_PASSWORD={PRIVATE}-postgres\r\nAWS_SECRET_ACCESS_TOKEN=\"{PRIVATE}-aws\"\nservice-CuStOm-ApI_KeY: '{PRIVATE}-key'\r\nMY-SECRET={PRIVATE}-hyphen\nrequest Bearer  {PRIVATE}-two-spaces keep-context💥\nrequest bEaReR\t{PRIVATE}-tab\r\nrequest BEARER \t \t{PRIVATE}-mixed\ncompassword=keep-embedded-one\npretoken=keep-embedded-two\nAPI_KEY_SUFFIX=keep-unrelated-suffix\nBearer  \r\nkeep-empty-bearer-next-line💥\r\n"
    );
    let expected_stdout = "POSTGRES_PASSWORD=[redacted]\r\nAWS_SECRET_ACCESS_TOKEN=\"[redacted]\"\nservice-CuStOm-ApI_KeY: '[redacted]'\r\nMY-SECRET=[redacted]\nrequest Bearer  [redacted] keep-context💥\nrequest bEaReR\t[redacted]\r\nrequest BEARER \t \t[redacted]\ncompassword=keep-embedded-one\npretoken=keep-embedded-two\nAPI_KEY_SUFFIX=keep-unrelated-suffix\nBearer  \r\nkeep-empty-bearer-next-line💥\r\n";
    let stderr = format!(
        "database-PASSWORD=\"{PRIVATE}-stderr-password\"\r\nworker_api_key={PRIVATE}-stderr-key\naws_secret_access_token='{PRIVATE}-stderr-token'\ntrace Bearer\t\t{PRIVATE}-stderr-tabs keep-contexté\nBearer\t\r\nkeep-empty-tab-next-lineé\r\nnotapikey=keep-embedded-three\nPASSWORD_SUFFIX=keep-unrelated-right-boundary\n"
    );
    let expected_stderr = "database-PASSWORD=\"[redacted]\"\r\nworker_api_key=[redacted]\naws_secret_access_token='[redacted]'\ntrace Bearer\t\t[redacted] keep-contexté\nBearer\t\r\nkeep-empty-tab-next-lineé\r\nnotapikey=keep-embedded-three\nPASSWORD_SUFFIX=keep-unrelated-right-boundary\n";
    let core = IncidentCore::start(all_log_options(&stdout, &stderr)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 4096, 100).await;
        assert_eq!(result["stdout"], expected_stdout, "{tool} stdout");
        assert_eq!(result["stderr"], expected_stderr, "{tool} stderr");
        assert_eq!(result["truncation"]["lines"], false);
        assert_eq!(result["truncation"]["bytes"], false);
    }
    client.close().await;
    let requests = core.requests();
    assert_eq!(requests.len(), 4, "no fallback reads are allowed");
    assert_eq!(requests[0].operation, "GetVersion");
    for (request, (_, operation)) in requests[1..].iter().zip(LOG_TOOLS) {
        assert_eq!(request.operation, operation);
        assert_eq!(request.params["tail"], 100);
    }
}

#[tokio::test]
async fn prefixed_keys_and_spaced_bearer_are_redacted_before_all_tools_apply_tiny_utf8_tails() {
    let stdout = format!("POSTGRES_PASSWORD=\"{PRIVATE}-password\"; keep-context💥é");
    let stderr = format!("trace Bearer \t {PRIVATE}-bearer keep-context💥é");
    let core = IncidentCore::start(all_log_options(&stdout, &stderr)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        for bytes in 1..=24 {
            let result = client.logs_tool(tool, bytes, 1).await;
            assert_eq!(result["stdout"], "", "joint tail=1 selects the stderr line");
            assert_eq!(result["truncation"]["lines"], true);
            assert!(!result["stderr"].as_str().unwrap().contains("private"));
        }
    }
    client.close().await;
    let requests = core.requests();
    assert_eq!(
        requests.len(),
        73,
        "one upstream read per MCP call, plus startup"
    );
    for (index, (tool, operation)) in LOG_TOOLS.into_iter().enumerate() {
        let reads = &requests[1 + index * 24..1 + (index + 1) * 24];
        assert!(
            reads
                .iter()
                .all(|r| r.operation == operation && r.params["tail"] == 1),
            "{tool} changed upstream bounds or retried"
        );
    }
}

#[tokio::test]
async fn camel_case_credentials_are_masked_in_both_streams_of_every_log_tool() {
    let mut source = String::new();
    let mut expected = String::new();
    for depth in 0..=4 {
        for label in [
            "dbPassword",
            "clientSecret",
            "DBPassword",
            "apiKey",
            "apiSecret",
            "accessToken",
            "refreshToken",
            "databaseUrl",
            "connectionString",
            "privateKey",
            "servicePrivateKey",
            "httpAuthorization",
        ] {
            source.push_str(&encode_fragment(json_assignment(label, PRIVATE), depth));
            source.push_str("\r\n");
            expected.push_str(&encode_fragment(
                json_assignment(label, "[redacted]"),
                depth,
            ));
            expected.push_str("\r\n");
        }
    }
    let harmless = "dbPasswordFile=keep-file\nclientSecretSuffix=keep-suffix\nAPI_KEY_SUFFIX=keep-env\ncompassword=keep-embedded\npretoken=keep-other\n";
    source.push_str(harmless);
    expected.push_str(harmless);
    let core = IncidentCore::start(all_log_options(&source, &source)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 65_536, 500).await;
        assert_eq!(result["stdout"], expected, "{tool} stdout");
        assert_eq!(result["stderr"], expected, "{tool} stderr");
        assert_eq!(result["truncation"]["lines"], false);
        assert_eq!(result["truncation"]["bytes"], false);
    }
    client.close().await;
    let requests = core.requests();
    assert_eq!(
        requests.len(),
        4,
        "one positive-tail read per tool plus startup"
    );
    for (request, (_, operation)) in requests[1..].iter().zip(LOG_TOOLS) {
        assert_eq!(request.operation, operation);
        assert_eq!(request.params["tail"], 500);
    }
}

#[tokio::test]
async fn compound_sensitive_keys_are_classified_completely_with_file_and_suffix_exclusions() {
    let mut source = String::new();
    let mut expected = String::new();
    for label in [
        "AWS_SECRET_ACCESS_KEY",
        "secretKey",
        "secret_key",
        "mySecretKey",
        "awsSecretAccessKey",
        "service-secret-key",
        "AWS_ACCESS_KEY",
    ] {
        for depth in 0..=4 {
            source.push_str(&encode_fragment(json_assignment(label, PRIVATE), depth));
            source.push_str("\r\n");
            expected.push_str(&encode_fragment(
                json_assignment(label, "[redacted]"),
                depth,
            ));
            expected.push_str("\r\n");
        }
        source.push_str(&format!("{label}={PRIVATE}\n"));
        expected.push_str(&format!("{label}=[redacted]\n"));
    }
    for label in [
        "AWS_SECRET_ACCESS_KEY_FILE",
        "secretKeyFile",
        "secret_key_suffix",
        "PASSWORD_FILE",
        "clientSecretSuffix",
        "API_KEY_SUFFIX",
        "compassword",
    ] {
        let row = format!("{}\n", json_assignment(label, "keep-harmless"));
        source.push_str(&row);
        expected.push_str(&row);
    }
    let core = IncidentCore::start(all_log_options(&source, &source)).await;
    let mut client = StdioClient::start(&core).await;
    for (tool, _) in LOG_TOOLS {
        let result = client.logs_tool(tool, 65_536, 500).await;
        assert_eq!(result["stdout"], expected, "{tool}");
        assert_eq!(result["stderr"], expected, "{tool}");
    }
    client.close().await;
    assert_eq!(core.requests().len(), 4);
}

#[tokio::test]
async fn multiline_secret_values_fail_closed_before_tail_and_utf8_clipping_in_every_log_tool() {
    let mut cases = vec![
        format!("password=\"{PRIVATE}-first\n{PRIVATE}-second\"\ncontext-after💥\n"),
        format!("dbPassword='{PRIVATE}-first\r\n{PRIVATE}-second'\r\ncontext-afteré\r\n"),
    ];
    for block in ["|", "|-", "|+", ">", ">-", ">+", "|2-"] {
        cases.push(format!("privateKey: {block}\n  -----BEGIN PRIVATE KEY-----\n  {PRIVATE}-pem-é💥\n  -----END PRIVATE KEY-----\ncontext-after\n"));
    }
    for depth in [1, 4] {
        let quote = format!("{}\"", "\\".repeat((1 << depth) - 1));
        cases.push(format!(
            "{{{quote}clientSecret{quote}:{quote}{PRIVATE}-first\n{PRIVATE}-second{quote}}}\n"
        ));
    }
    for (index, source) in cases.iter().enumerate() {
        for is_stderr in [false, true] {
            let (stdout, stderr) = if is_stderr {
                ("", source.as_str())
            } else {
                (source.as_str(), "")
            };
            let core = IncidentCore::start(all_log_options(stdout, stderr)).await;
            let mut client = StdioClient::start(&core).await;
            for (tool, _) in LOG_TOOLS {
                let result = client.logs_tool(tool, 65_536, 500).await;
                let text = result[if is_stderr { "stderr" } else { "stdout" }]
                    .as_str()
                    .unwrap();
                assert!(text.contains("[redacted]"), "unmasked case {index}, {tool}");
                assert!(
                    !text.contains("context-after"),
                    "ambiguous multiline remainder must be hidden"
                );
                assert!(!text.contains("PRIVATE KEY"));
                assert_eq!(text.matches('\n').count(), source.matches('\n').count());
                for bytes in 1..=24 {
                    let result = client.logs_tool(tool, bytes, 1).await;
                    assert_eq!(result["returned_lines"], 1);
                    assert_eq!(result["truncation"]["lines"], true);
                }
            }
            client.close().await;
            let requests = core.requests();
            assert_eq!(requests.len(), 76, "one read per call, no context fallback");
            for (tool_index, (_, operation)) in LOG_TOOLS.into_iter().enumerate() {
                let reads = &requests[1 + tool_index * 25..1 + (tool_index + 1) * 25];
                assert_eq!(reads[0].operation, operation);
                assert_eq!(reads[0].params["tail"], 500);
                assert!(
                    reads[1..]
                        .iter()
                        .all(|r| r.operation == operation && r.params["tail"] == 1)
                );
            }
        }
    }
}

#[tokio::test]
async fn unclosed_inconsistent_and_ambiguous_values_are_each_exercised_through_stdio() {
    let mut rows = Vec::<String>::new();
    let mut expected = Vec::<String>::new();
    for depth in 0..=3 {
        let quote = format!("{}\"", "\\".repeat((1 << depth) - 1));
        rows.push(format!(
            "{{{quote}authorization{quote}:{quote}Basic unclosed-marker é💥"
        ));
        expected.push(format!("{{{quote}authorization{quote}:{quote}[redacted]"));
    }
    for depth in 0..=4 {
        for (source, first) in [
            (
                r#"authorization="Digest username="victim", response="secret"""#,
                "Digest username=",
            ),
            (r#"password="prefix="private-password"""#, "prefix="),
            (r#"authorization=""private-empty"""#, ""),
            (
                r#"{"authorization":"Digest username=", "response":"private-response""}"#,
                "Digest username=",
            ),
        ] {
            let mut original = source.to_owned();
            for _ in 0..depth {
                original = serde_json::to_string(&original).unwrap();
            }
            let value_start = if first.is_empty() {
                original.find("private-empty").unwrap() - (1 << depth)
            } else {
                original.find(first).unwrap()
            };
            expected.push(format!("{}[redacted]", &original[..value_start]));
            rows.push(original);
        }
    }
    rows.push(r#"{\"password\":"inconsistent-marker" remainder-marker"#.into());
    expected.push(r#"{\"password\":"[redacted]"#.into());
    rows.push(r#"{\"token\":\\"malformed-marker with-spaces-marker"#.into());
    expected.push(r#"{\"token\":[redacted]"#.into());
    // Each uncertain value has its own received stream, so a conservative
    // earlier mask cannot prevent a later regression from reaching the parser.
    for (source, expected) in rows.into_iter().zip(expected) {
        let source = format!("{source}\r\n");
        let core = IncidentCore::start(options(&source, &source, None)).await;
        let mut client = StdioClient::start(&core).await;
        let result = client.logs(4096).await;
        assert_eq!(result["stdout"], format!("{expected}\r\n"));
        assert_eq!(result["stderr"], format!("{expected}\r\n"));
        client.close().await;
        assert_eq!(core.requests().len(), 2);
    }
}
