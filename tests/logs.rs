use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use komodo_client::entities::update::Log;
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    error::ErrorKind,
    logs::{self, ContainerLogsInput, DeploymentLogsInput, StackLogsInput},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

const KEY: &str = "logs-fixture-api-key";
const SECRET: &str = "logs-fixture-api-secret";

fn incident() -> Value {
    serde_json::from_str(include_str!("fixtures/core-2.3.2/incident.json")).unwrap()
}

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<Value>>>,
    status: StatusCode,
    response: Value,
    delay: Duration,
    key: String,
    secret: String,
}

struct Core {
    address: String,
    client: CoreClient,
    state: MockState,
    task: JoinHandle<()>,
}

impl Core {
    async fn start(
        status: StatusCode,
        response: Value,
        delay: Duration,
        deadline: Duration,
    ) -> Self {
        Self::start_with_credentials(status, response, delay, deadline, KEY, SECRET).await
    }

    async fn start_with_credentials(
        status: StatusCode,
        response: Value,
        delay: Duration,
        deadline: Duration,
        key: &str,
        secret: &str,
    ) -> Self {
        let state = MockState {
            requests: Arc::default(),
            status,
            response,
            delay,
            key: key.into(),
            secret: secret.into(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/read", post(read))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = CoreClient::new(
            Config::parse(&address, key, secret).unwrap(),
            RequestPolicy {
                connect_timeout: deadline,
                operation_timeout: deadline,
            },
        )
        .unwrap();
        Self {
            address,
            client,
            state,
            task,
        }
    }

    async fn log(stdout: &str, stderr: &str) -> Self {
        let log = Log {
            stdout: stdout.into(),
            stderr: stderr.into(),
            success: true,
            command: "should-never-return-command-with-password".into(),
            stage: "should-never-return-stage".into(),
            start_ts: 10,
            end_ts: 20,
        };
        Self::start(
            StatusCode::OK,
            serde_json::to_value(log).unwrap(),
            Duration::ZERO,
            Duration::from_secs(2),
        )
        .await
    }

    fn requests(&self) -> Vec<Value> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    assert_eq!(headers["x-api-key"], state.key);
    assert_eq!(headers["x-api-secret"], state.secret);
    if request["type"] == "GetVersion" {
        assert_eq!(request["params"], json!({}));
        state.requests.lock().unwrap().push(request);
        return (StatusCode::OK, Json(json!({"version":"2.3.2"})));
    }
    assert!(matches!(
        request["type"].as_str(),
        Some("GetContainerLog" | "GetDeploymentLog" | "GetStackLog")
    ));
    assert!(
        request["params"]["tail"]
            .as_u64()
            .is_some_and(|tail| (1..=logs::MAX_TAIL).contains(&tail))
    );
    state.requests.lock().unwrap().push(request);
    tokio::time::sleep(state.delay).await;
    (state.status, Json(state.response))
}

fn stack_input() -> StackLogsInput {
    serde_json::from_value(
        json!({"id":incident()["stack"]["id"],"services":[incident()["affected_service"]]}),
    )
    .unwrap()
}

#[tokio::test]
async fn shared_incident_filters_service_and_exposes_only_bounded_selected_fields() {
    let fixture = incident();
    let excerpt = fixture["expected_log_excerpt"].as_str().unwrap();
    let core = Core::log(excerpt, "").await;
    let result = logs::stack_logs(&core.client, stack_input()).await.unwrap();
    assert_eq!(result.stdout, excerpt);
    assert_eq!(result.stderr, "");
    assert_eq!(result.target.id, fixture["stack"]["id"]);
    assert_eq!(result.target.services, vec!["web"]);
    assert_eq!(result.returned_lines, 1);
    assert_eq!(result.returned_bytes, excerpt.len());
    assert!(
        !result.truncation.lines
            && !result.truncation.bytes
            && !result.truncation.upstream_may_have_more
    );
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("should-never-return"));
    assert_eq!(
        core.requests(),
        vec![
            json!({"type":"GetStackLog","params":{"stack":fixture["stack"]["id"],"services":["web"],"tail":100,"timestamps":false}})
        ]
    );
}

#[tokio::test]
async fn container_deployment_and_all_service_stack_requests_keep_positive_upstream_tails() {
    let fixture = incident();
    let core = Core::log("last line\n", "").await;
    let container: ContainerLogsInput = serde_json::from_value(json!({"server_id":fixture["server"]["id"],"container":fixture["container"],"tail":7,"max_bytes":99,"timestamps":true})).unwrap();
    let result = logs::container_logs(&core.client, container).await.unwrap();
    assert_eq!(
        result.target.container.as_deref(),
        fixture["container"].as_str()
    );
    let deployment: DeploymentLogsInput =
        serde_json::from_value(json!({"id":fixture["deployment"]["id"],"tail":8,"max_bytes":99}))
            .unwrap();
    assert_eq!(
        logs::deployment_logs(&core.client, deployment)
            .await
            .unwrap()
            .target
            .id,
        fixture["deployment"]["id"]
    );
    let all: StackLogsInput = serde_json::from_value(json!({"id":fixture["stack"]["id"]})).unwrap();
    assert!(
        logs::stack_logs(&core.client, all)
            .await
            .unwrap()
            .target
            .services
            .is_empty()
    );
    assert_eq!(
        core.requests(),
        vec![
            json!({"type":"GetContainerLog","params":{"server":fixture["server"]["id"],"container":fixture["container"],"tail":7,"timestamps":true}}),
            json!({"type":"GetDeploymentLog","params":{"deployment":fixture["deployment"]["id"],"tail":8,"timestamps":false}}),
            json!({"type":"GetStackLog","params":{"stack":fixture["stack"]["id"],"services":[],"tail":100,"timestamps":false}}),
        ]
    );
}

#[tokio::test]
async fn empty_logs_remain_empty_and_do_not_trigger_fallback() {
    let core = Core::log("", "").await;
    let result = logs::stack_logs(&core.client, stack_input()).await.unwrap();
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    assert_eq!((result.returned_lines, result.returned_bytes), (0, 0));
    assert!(!result.truncation.upstream_may_have_more);
    assert_eq!(core.requests().len(), 1);
}

#[tokio::test]
async fn joint_stream_line_limit_keeps_the_tail_and_reports_uncertain_upstream_completeness() {
    let core = Core::log("old\nnew\n", "error\n").await;
    let mut input = stack_input();
    input.tail = 2;
    let result = logs::stack_logs(&core.client, input).await.unwrap();
    assert_eq!(
        (result.stdout.as_str(), result.stderr.as_str()),
        ("new\n", "error\n")
    );
    assert_eq!(result.returned_lines, 2);
    assert!(result.truncation.lines && result.truncation.upstream_may_have_more);
    assert!(!result.truncation.bytes);
    let core = Core::log("full\ntail\n", "").await;
    let mut input = stack_input();
    input.tail = 2;
    let result = logs::stack_logs(&core.client, input).await.unwrap();
    assert!(!result.truncation.lines && result.truncation.upstream_may_have_more);
}

#[tokio::test]
async fn oversized_single_line_and_joint_byte_budgets_preserve_utf8() {
    let core = Core::log(&"💥é".repeat(40_000), "").await;
    let mut input = stack_input();
    input.max_bytes = 13;
    let result = logs::stack_logs(&core.client, input).await.unwrap();
    assert!(result.returned_bytes <= 13);
    assert_eq!(
        result.returned_bytes,
        result.stdout.len() + result.stderr.len()
    );
    assert!(result.truncation.bytes);
    assert_eq!(result.returned_lines, 1);
    assert!(serde_json::to_string(&result).is_ok());
    let core = Core::log("12345\n", "67890\n").await;
    let mut input = stack_input();
    input.max_bytes = 8;
    let result = logs::stack_logs(&core.client, input).await.unwrap();
    assert_eq!(
        (result.stdout.as_str(), result.stderr.as_str()),
        ("5\n", "67890\n")
    );
    assert_eq!(result.returned_bytes, 8);
    assert!(result.truncation.bytes);
}

#[tokio::test]
async fn configured_credentials_and_sensitive_assignments_are_redacted_before_clipping() {
    let marker = incident()["credential_marker"].as_str().unwrap().to_owned();
    let text = format!(
        "{KEY} {SECRET}\nTOKEN={marker}\npassword: 'space secret'\n{{\"api_key\":\"json-secret\"}}\nAuthorization: Bearer bearer-secret\nBearer standalone-secret\nDATABASE_URL=postgres://user:password@db/app\npassword=escaped\\\"secret\n"
    );
    let core = Core::log(&text, "").await;
    let result = logs::stack_logs(&core.client, stack_input()).await.unwrap();
    let encoded = serde_json::to_string(&result).unwrap();
    for credential in [
        KEY,
        SECRET,
        marker.as_str(),
        "space secret",
        "json-secret",
        "bearer-secret",
        "standalone-secret",
        "postgres://",
        "escaped",
    ] {
        assert!(!encoded.contains(credential), "leaked {credential}");
    }
    assert!(result.stdout.contains("[redacted]"));
    assert_eq!(
        result.returned_bytes,
        result.stdout.len() + result.stderr.len()
    );
    let core = Core::log(&format!("TOKEN={marker}"), "").await;
    let mut input = stack_input();
    input.max_bytes = 4;
    let result = logs::stack_logs(&core.client, input).await.unwrap();
    assert!(!result.stdout.contains(&marker[marker.len() - 4..]));
    assert_eq!(result.stdout, "ted]");
}

#[tokio::test]
async fn stdio_redacts_assignment_values_when_a_configured_key_is_the_assignment_label() {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    timeout(Duration::from_secs(15), async {
        let marker = incident()["credential_marker"].as_str().unwrap().to_owned();
        for key in ["token", "password", "api_key", "authorization"] {
            let log = Log {
                stdout: format!("{key}={marker}\n"),
                success: true,
                ..Default::default()
            };
            let core = Core::start_with_credentials(
                StatusCode::OK,
                serde_json::to_value(log).unwrap(),
                Duration::ZERO,
                Duration::from_secs(2),
                key,
                SECRET,
            )
            .await;
            let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
                .env_clear()
                .env("KOMODO_ADDRESS", &core.address)
                .env("KOMODO_API_KEY", key)
                .env("KOMODO_API_SECRET", SECRET)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut session = ClientConfig::new(
                Default::default(),
                Implementation::new("log-redaction-regression", "1"),
            )
            .serve_with_lifecycle(
                (child.stdout.take().unwrap(), child.stdin.take().unwrap()),
                ClientLifecycleMode::Initialize,
            )
            .await
            .unwrap();
            let result = session
                .call_tool(
                    CallToolRequestParams::new("get_stack_logs").with_arguments(
                        json!({"id":incident()["stack"]["id"],"max_bytes":32})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false));
            let value = serde_json::to_value(&result).unwrap();
            assert!(
                !value.to_string().contains(&marker),
                "assignment value leaked with key {key}"
            );
            let data = &value["structuredContent"];
            let actual =
                data["stdout"].as_str().unwrap().len() + data["stderr"].as_str().unwrap().len();
            assert_eq!(data["returned_bytes"], actual);
            assert!(actual <= 32);
            let text: Value =
                serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(data, &text);
            session.close().await.unwrap();
            assert!(child.wait().await.unwrap().success());
            assert_eq!(core.requests().len(), 2);
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stdio_redacts_complete_authorization_values_for_all_schemes() {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    timeout(Duration::from_secs(10), async {
        let quoted = json!({"authorization":r#"Digest username="json-user", response="json-response""#,"z_after":"keep-json-marker"}).to_string();
        let stdout = format!("Authorization: Basic dXNlcjpwYXNz\r\nAuthorization: Digest username=\"digest-user\", realm=\"digest-realm\", nonce=\"digest-nonce\", response=\"digest-response\"\nAuthorization: Negotiate negotiate-payload more-payload\n{quoted}\nAuthorization: \r\nfollowing-ready\n");
        let stderr = "Authorization: Digest username=\"stderr-user\", response=\"stderr-response\"\r\n";
        let core = Core::log(&stdout, stderr).await;
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", KEY)
            .env("KOMODO_API_SECRET", SECRET)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut session = ClientConfig::new(
            Default::default(),
            Implementation::new("authorization-redaction-regression", "1"),
        )
        .serve_with_lifecycle(
            (child.stdout.take().unwrap(), child.stdin.take().unwrap()),
            ClientLifecycleMode::Initialize,
        )
        .await
        .unwrap();
        for max_bytes in [4096, 7] {
            let result = session
                .call_tool(
                    CallToolRequestParams::new("get_stack_logs").with_arguments(
                        json!({"id":incident()["stack"]["id"],"max_bytes":max_bytes})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(false));
            let value = serde_json::to_value(result).unwrap();
            for marker in ["dXNlcjpwYXNz", "digest-user", "digest-realm", "digest-nonce", "digest-response", "negotiate-payload", "more-payload", "json-user", "json-response", "stderr-user", "stderr-response"] {
                assert!(!value.to_string().contains(marker), "authorization value leaked: {marker}");
            }
            let data = &value["structuredContent"];
            let actual = data["stdout"].as_str().unwrap().len() + data["stderr"].as_str().unwrap().len();
            assert_eq!(data["returned_bytes"], actual);
            assert!(actual <= max_bytes);
            let text: Value = serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(data, &text);
            if max_bytes == 4096 {
                let redacted_json = json!({"authorization":"[redacted]","z_after":"keep-json-marker"}).to_string();
                assert_eq!(data["stdout"], format!("Authorization: [redacted]\r\nAuthorization: [redacted]\nAuthorization: [redacted]\n{redacted_json}\nAuthorization: \r\nfollowing-ready\n"));
                assert_eq!(data["stderr"], "Authorization: [redacted]\r\n");
                assert_eq!(data["returned_lines"], 7);
            } else {
                assert_eq!(data["truncation"]["bytes"], true);
            }
        }
        session.close().await.unwrap();
        assert!(child.wait().await.unwrap().success());
        assert_eq!(core.requests().len(), 3, "one startup read and two bounded tails");
    }).await.unwrap();
}

#[tokio::test]
async fn stdio_redacts_repeatedly_encoded_json_and_masks_unclosed_values() {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    timeout(Duration::from_secs(10), async {
        let mut rows = Vec::new();
        let mut expected = Vec::new();
        for depth in 0..=4 {
            for authorization in ["Basic nested-basic-marker", r#"Digest username="nested-user", response="nested-response"\"#] {
                let mut original = json!({"authorization":authorization,"token":"nested-token \"embedded\" \\","password":"nested-password\nnext-line","api_key":"nested-api-key","z_after":"visible é💥"}).to_string();
                let mut redacted = json!({"authorization":"[redacted]","token":"[redacted]","password":"[redacted]","api_key":"[redacted]","z_after":"visible é💥"}).to_string();
                for _ in 0..depth {
                    original = serde_json::to_string(&original).unwrap();
                    redacted = serde_json::to_string(&redacted).unwrap();
                }
                rows.push(original);
                expected.push(redacted);
            }
        }
        for depth in 0..=3 {
            let quote = format!("{}\"", "\\".repeat((1 << depth) - 1));
            rows.push(format!("{{{quote}authorization{quote}:{quote}Basic unclosed-marker é💥"));
            expected.push(format!("{{{quote}authorization{quote}:{quote}[redacted]"));
        }
        rows.push(r#"{\"password\":"inconsistent-marker" remainder-marker"#.into());
        expected.push(r#"{\"password\":"[redacted]"#.into());
        rows.push(r#"{\"token\":\\"malformed-marker with-spaces-marker"#.into());
        expected.push(r#"{\"token\":[redacted]"#.into());
        let stdout = format!("{}\r\n", rows.join("\r\n"));
        let expected = format!("{}\r\n", expected.join("\r\n"));
        let core = Core::log(&stdout, "").await;
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", KEY)
            .env("KOMODO_API_SECRET", SECRET)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .kill_on_drop(true).spawn().unwrap();
        let mut session = ClientConfig::new(Default::default(), Implementation::new("encoded-redaction-regression", "1"))
            .serve_with_lifecycle((child.stdout.take().unwrap(), child.stdin.take().unwrap()), ClientLifecycleMode::Initialize).await.unwrap();
        let result = session.call_tool(CallToolRequestParams::new("get_stack_logs").with_arguments(json!({"id":incident()["stack"]["id"],"max_bytes":65536}).as_object().unwrap().clone())).await.unwrap();
        assert_eq!(result.is_error, Some(false));
        let value = serde_json::to_value(result).unwrap();
        let data = &value["structuredContent"];
        assert_eq!(data["stdout"], expected);
        assert_eq!(data["stderr"], "");
        assert_eq!(data["returned_bytes"], expected.len());
        assert_eq!(data["returned_lines"], rows.len());
        assert_eq!(data["truncation"]["bytes"], false);
        let text: Value = serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data, &text);
        session.close().await.unwrap();
        assert!(child.wait().await.unwrap().success());
        assert_eq!(core.requests().len(), 2);
    }).await.unwrap();
}

#[tokio::test]
async fn unavailable_command_http_error_and_timeout_never_retry_or_return_raw_failure_payloads() {
    for (status, body, delay, kind) in [
        (
            StatusCode::OK,
            json!({"stage":"private stage","command":"private command","stdout":"private stdout","stderr":"private stderr","success":false,"start_ts":0,"end_ts":1}),
            Duration::ZERO,
            ErrorKind::Upstream,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"error":format!("private {KEY} {SECRET}"),"trace":[]}),
            Duration::ZERO,
            ErrorKind::PermissionDenied,
        ),
        (
            StatusCode::OK,
            serde_json::to_value(Log {
                success: true,
                ..Default::default()
            })
            .unwrap(),
            Duration::from_millis(200),
            ErrorKind::Timeout,
        ),
    ] {
        let core = Core::start(status, body, delay, Duration::from_millis(60)).await;
        let result = logs::stack_logs(&core.client, stack_input())
            .await
            .unwrap_err();
        assert_eq!(result.kind, kind);
        let text = serde_json::to_string(&result).unwrap();
        assert!(!text.contains("private") && !text.contains(KEY) && !text.contains(SECRET));
        assert_eq!(core.requests().len(), 1);
    }
}

#[tokio::test]
async fn invalid_limits_targets_filters_and_argument_types_fail_before_io() {
    let core = Core::log("", "").await;
    let original = json!({"id":incident()["stack"]["id"]});
    let mut invalid = vec![json!({"id":"bad"}), json!({"id":"-x"})];
    for (key, value) in [
        ("tail", json!(0)),
        ("tail", json!(501)),
        ("max_bytes", json!(0)),
        ("max_bytes", json!(65_537)),
        ("services", json!(["--all"])),
        ("services", json!(["a b"])),
        ("services", json!(["web", "web"])),
        ("services", json!(["x".repeat(129)])),
        (
            "services",
            json!((0..17).map(|x| format!("s{x}")).collect::<Vec<_>>()),
        ),
    ] {
        let mut value_input = original.clone();
        value_input[key] = value;
        invalid.push(value_input);
    }
    for value in invalid {
        let input: StackLogsInput = serde_json::from_value(value).unwrap();
        assert_eq!(
            logs::stack_logs(&core.client, input)
                .await
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArguments
        );
    }
    for value in [
        json!({"id":null}),
        json!({"id":incident()["stack"]["id"],"unknown":1}),
        json!({"id":incident()["stack"]["id"],"services":null}),
        json!({"id":incident()["stack"]["id"],"tail":-1}),
    ] {
        assert!(serde_json::from_value::<StackLogsInput>(value).is_err());
    }
    let input: ContainerLogsInput = serde_json::from_value(
        json!({"server_id":incident()["server"]["id"],"container":"web;cat /etc/passwd"}),
    )
    .unwrap();
    assert_eq!(
        logs::container_logs(&core.client, input)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArguments
    );
    assert!(core.requests().is_empty());
}
