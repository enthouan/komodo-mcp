use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use komodo_client::entities::stats::SystemStats;
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    error::ErrorKind,
    health::{self, FleetHealthInput, HealthStatus},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

type Responder = dyn Fn(&str, &Value) -> (StatusCode, Value, Duration) + Send + Sync;
#[derive(Clone)]
struct MockState {
    reply: Arc<Responder>,
    calls: Arc<Mutex<Vec<Value>>>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}
struct Core {
    address: String,
    state: MockState,
    task: JoinHandle<()>,
}
impl Core {
    async fn start(
        reply: impl Fn(&str, &Value) -> (StatusCode, Value, Duration) + Send + Sync + 'static,
    ) -> Self {
        let state = MockState {
            reply: Arc::new(reply),
            calls: Arc::default(),
            active: Arc::default(),
            peak: Arc::default(),
        };
        let app = Router::new()
            .route("/komodo/read", post(handle))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            address,
            state,
            task,
        }
    }
    fn client(&self) -> CoreClient {
        CoreClient::new(
            Config::parse(&self.address, "health-test-key", "health-test-secret").unwrap(),
            RequestPolicy::default(),
        )
        .unwrap()
    }
    fn calls(&self) -> Vec<Value> {
        self.state.calls.lock().unwrap().clone()
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handle(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(headers["x-api-key"], "health-test-key");
    assert_eq!(headers["x-api-secret"], "health-test-secret");
    state.calls.lock().unwrap().push(request.clone());
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    let (status, body, delay) =
        (state.reply)(request["type"].as_str().unwrap(), &request["params"]);
    tokio::time::sleep(delay).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    (status, Json(body))
}
fn incident() -> Value {
    serde_json::from_str(include_str!("fixtures/core-2.3.2/incident.json")).unwrap()
}
fn base(path: &str) -> Value {
    serde_json::from_str(match path {
        "servers" => include_str!("fixtures/core-2.3.2/servers.json"),
        "stacks" => include_str!("fixtures/core-2.3.2/stacks.json"),
        "deployments" => include_str!("fixtures/core-2.3.2/deployments.json"),
        _ => unreachable!(),
    })
    .unwrap()
}
fn ok(body: Value) -> (StatusCode, Value, Duration) {
    (StatusCode::OK, body, Duration::ZERO)
}
fn stats() -> Value {
    let mut stats = serde_json::to_value(SystemStats::default()).unwrap();
    stats["cpu_perc"] = json!(12.5);
    stats["mem_used_gb"] = json!(4.0);
    stats["mem_total_gb"] = json!(16.0);
    stats["disks"] = json!([{"mount":"/","file_system":"ext4","used_gb":40.0,"total_gb":100.0}]);
    stats["refresh_ts"] = json!(1735689600000_i64);
    stats
}
fn service(name: &str, state: &str, status: Option<&str>) -> Value {
    let f = incident();
    json!({"stack_id":f["stack"]["id"],"stack_name":f["stack"]["name"],"service":name,"image":"example:test","container":status.map(|status|json!({"name":f["container"],"state":"running","status":status})),"swarm_service":null,"state":state,"image_digests":null})
}

fn mixed_reply(operation: &str, params: &Value) -> (StatusCode, Value, Duration) {
    let f = incident();
    match operation {
        "GetVersion" => {
            assert_eq!(params, &json!({}));
            ok(serde_json::from_str(include_str!("fixtures/core-2.3.2/version.json")).unwrap())
        }

        "ListServers" => {
            assert_eq!(params["limit"], 30);
            assert_eq!(params["page"], 0);
            let items: Vec<_> = [
                "server",
                "healthy_server",
                "unavailable_server",
                "unknown_server",
            ]
            .iter()
            .map(|key| {
                let mut s = base("servers")[0].clone();
                s["id"] = f[key]["id"].clone();
                s["name"] = f[key]["name"].clone();
                s["info"]["state"] = json!("Ok");
                s
            })
            .collect();
            ok(json!(items))
        }
        "ListStacks" => {
            let mut broken = base("stacks")[0].clone();
            broken["id"] = f["stack"]["id"].clone();
            broken["name"] = f["stack"]["name"].clone();
            broken["info"]["state"] = json!("unhealthy");
            ok(json!([broken]))
        }
        "ListDeployments" => ok(base("deployments")),
        "GetServerState" if params["server"] == f["unavailable_server"]["id"] => (
            StatusCode::BAD_GATEWAY,
            json!({"error":"health-test-key health-test-secret"}),
            Duration::ZERO,
        ),
        "GetServerState" if params["server"] == f["unknown_server"]["id"] => {
            ok(json!({"status":"Disabled"}))
        }
        "GetServerState" => ok(json!({"status":"Ok"})),
        "GetSystemStats" if params["server"] == f["unavailable_server"]["id"] => (
            StatusCode::BAD_GATEWAY,
            json!({"error":"unavailable"}),
            Duration::ZERO,
        ),
        "GetSystemStats" => ok(stats()),
        "ListAllStackServices" => {
            assert_eq!(params["stacks"], json!([f["stack"]["id"]]));
            assert_eq!(params["limit"], 101);
            ok(json!([
                service("web", "Running", Some("Up 1 minute (unhealthy)")),
                service("db", "Running", Some("Up 1 hour (healthy)"))
            ]))
        }
        _ => panic!("unexpected read {operation}"),
    }
}

#[tokio::test]
async fn mixed_fleet_retains_results_and_resource_specific_gaps() {
    let core = Core::start(mixed_reply).await;
    let report = health::fleet_health(&core.client(), FleetHealthInput::default())
        .await
        .unwrap();
    assert!(report.gaps.is_empty());
    assert_eq!(report.servers.len(), 4);
    assert_eq!(report.servers[0].health, HealthStatus::Healthy);
    assert_eq!(report.servers[1].health, HealthStatus::Healthy);
    assert_eq!(report.servers[2].health, HealthStatus::Unavailable);
    assert_eq!(report.servers[3].health, HealthStatus::Unknown);
    assert_eq!(report.servers[2].gaps.len(), 2);
    assert!(
        report.servers[2]
            .gaps
            .iter()
            .all(|g| g.resource_id.as_deref() == Some("00000000000000000000000a"))
    );
    let util = report.servers[0].utilization.as_ref().unwrap();
    assert_eq!(util.cpu_percent, Some(12.5));
    assert_eq!(util.memory_percent, Some(25.0));
    assert_eq!(util.disk_percent, Some(40.0));
    assert_eq!(report.stacks[0].health, HealthStatus::Unhealthy);
    assert_eq!(
        report.stacks[0]
            .services
            .iter()
            .find(|s| s.service == "web")
            .unwrap()
            .health,
        HealthStatus::Unhealthy
    );
    assert_eq!(
        report.stacks[0]
            .services
            .iter()
            .find(|s| s.service == "db")
            .unwrap()
            .health,
        HealthStatus::Healthy
    );
    assert!(!report.has_more);
    assert_eq!(core.calls().len(), 12);
    assert!(core.state.peak.load(Ordering::SeqCst) <= health::MAX_CORE_CONCURRENCY);
    let out = serde_json::to_string(&report).unwrap();
    assert!(!out.contains("health-test-key"));
    assert!(!out.contains("sensitive-raw-trace-marker"));
}

#[tokio::test]
async fn production_stdio_fleet_call_retains_partial_results_and_safe_read_evidence() {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    timeout(Duration::from_secs(15), async {
        let core = Core::start(mixed_reply).await;
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
            .env_clear()
            .env("KOMODO_ADDRESS", &core.address)
            .env("KOMODO_API_KEY", "health-test-key")
            .env("KOMODO_API_SECRET", "health-test-secret")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut session = ClientConfig::new(
            Default::default(),
            Implementation::new("fleet-health-regression", "1"),
        )
        .serve_with_lifecycle(
            (child.stdout.take().unwrap(), child.stdin.take().unwrap()),
            ClientLifecycleMode::Initialize,
        )
        .await
        .unwrap();
        let response = session
            .call_tool(
                CallToolRequestParams::new("fleet_health").with_arguments(
                    json!({"page":0,"page_size":30})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.is_error, Some(false));
        let response = serde_json::to_value(response).unwrap();
        let data = &response["structuredContent"];
        assert_eq!(data["stacks"][0]["id"], incident()["stack"]["id"]);
        assert_eq!(data["stacks"][0]["health"], "unhealthy");
        let affected = data["stacks"][0]["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["service"] == "web")
            .unwrap();
        assert_eq!(affected["health"], "unhealthy");
        let unavailable = data["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == incident()["unavailable_server"]["id"])
            .unwrap();
        assert_eq!(unavailable["health"], "unavailable");
        assert_eq!(unavailable["gaps"].as_array().unwrap().len(), 2);
        assert_eq!(data["servers"][0]["utilization"]["cpu_percent"], 12.5);
        let text: Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data, &text);
        assert!(!response.to_string().contains("health-test-key"));
        assert!(!response.to_string().contains("health-test-secret"));
        let reads = core.calls().len();
        assert_eq!(
            reads, 13,
            "one startup read plus the bounded mixed-fleet reads"
        );
        let invalid = session
            .call_tool(
                CallToolRequestParams::new("fleet_health")
                    .with_arguments(json!({"page_size":0}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(invalid.is_error, Some(true));
        assert_eq!(
            core.calls().len(),
            reads,
            "invalid input must not read Core"
        );
        let allowed = [
            "GetVersion",
            "ListServers",
            "ListStacks",
            "ListDeployments",
            "GetServerState",
            "GetSystemStats",
            "ListAllStackServices",
        ];
        assert!(
            core.calls()
                .iter()
                .all(|r| allowed.contains(&r["type"].as_str().unwrap()))
        );
        assert!(core.state.peak.load(Ordering::SeqCst) <= health::MAX_CORE_CONCURRENCY);
        session.close().await.unwrap();
        assert!(child.wait().await.unwrap().success());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn healthy_stack_requires_complete_distinct_service_coverage() {
    for (services, expected) in [
        (vec![service("db", "Running", None)], HealthStatus::Unknown),
        (
            vec![
                service("db", "Running", None),
                service("db", "Running", None),
            ],
            HealthStatus::Unknown,
        ),
        (
            vec![
                service("db", "Running", None),
                service("worker", "Running", None),
            ],
            HealthStatus::Unknown,
        ),
        (
            vec![
                service("web", "Running", None),
                service("db", "Running", None),
                service("db", "Running", None),
            ],
            HealthStatus::Healthy,
        ),
        (
            vec![service("web", "Unhealthy", None)],
            HealthStatus::Unhealthy,
        ),
    ] {
        let observed_count = services.len();
        let core = Core::start(move |operation, _| match operation {
            "ListServers" | "ListDeployments" => ok(json!([])),
            "ListStacks" => {
                let mut stack = base("stacks")[0].clone();
                // Inventory may repeat a logical name as well. Both sides use
                // distinct service coverage rather than container/replica count.
                let duplicate = stack["info"]["services"][1].clone();
                stack["info"]["services"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
                ok(json!([stack]))
            }
            "ListAllStackServices" => ok(json!(services)),
            _ => panic!("unexpected read {operation}"),
        })
        .await;
        let report = health::fleet_health(&core.client(), FleetHealthInput::default())
            .await
            .unwrap();
        let stack = &report.stacks[0];
        assert_eq!(stack.service_count, Some(2));
        assert_eq!(stack.health, expected);
        assert_eq!(
            stack.services.len(),
            observed_count,
            "partial evidence and replicas must survive"
        );
        if expected == HealthStatus::Healthy {
            assert!(stack.gaps.is_empty());
        } else {
            assert_eq!(stack.gaps.len(), 1);
            assert_eq!(stack.gaps[0].source, "service_inventory");
            assert_eq!(
                stack.gaps[0].resource_id.as_deref(),
                Some("000000000000000000000002")
            );
            assert_eq!(stack.gaps[0].error.kind, ErrorKind::InvalidResponse);
        }
    }
}

#[tokio::test]
async fn projected_name_aliases_cannot_prove_distinct_service_coverage() {
    let prefix = "x".repeat(300);
    for (expected_name, observed_name) in [
        (
            "worker-health-test-key".to_owned(),
            "worker-health-test-secret".to_owned(),
        ),
        (format!("{prefix}a"), format!("{prefix}b")),
    ] {
        let core = Core::start(move |operation, _| match operation {
            "ListServers" | "ListDeployments" => ok(json!([])),
            "ListStacks" => {
                let mut stack = base("stacks")[0].clone();
                let mut expected = stack["info"]["services"][0].clone();
                expected["service"] = json!(expected_name);
                stack["info"]["services"] = json!([expected]);
                stack["info"]["state"] = json!("running");
                ok(json!([stack]))
            }
            "ListAllStackServices" => ok(json!([service(&observed_name, "Running", None)])),
            _ => panic!("unexpected read {operation}"),
        })
        .await;
        let report = health::fleet_health(&core.client(), FleetHealthInput::default())
            .await
            .unwrap();
        assert_eq!(
            report.stacks[0].health,
            HealthStatus::Unknown,
            "different raw names can collapse after redaction or clipping"
        );
        assert_eq!(
            report.stacks[0].services.len(),
            1,
            "available evidence must survive"
        );
        assert_eq!(report.stacks[0].gaps[0].source, "service_inventory");
        let output = serde_json::to_string(&report).unwrap();
        assert!(!output.contains("health-test-key") && !output.contains("health-test-secret"));
    }
}

#[tokio::test]
async fn source_failure_preserves_other_inventory_and_slow_server_times_out() {
    let core = Core::start(|operation, _| match operation {
        "ListServers" => ok(base("servers")),
        "ListStacks" => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"failed"}),
            Duration::ZERO,
        ),
        "ListDeployments" => ok(base("deployments")),
        "GetServerState" => (
            StatusCode::OK,
            json!({"status":"Ok"}),
            Duration::from_millis(2200),
        ),
        "GetSystemStats" => ok(stats()),
        _ => panic!("unexpected {operation}"),
    })
    .await;
    let started = Instant::now();
    let report = health::fleet_health(&core.client(), FleetHealthInput::default())
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(report.gaps[0].source, "stacks");
    assert!(report.stacks.is_empty());
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(report.servers[0].health, HealthStatus::Unavailable);
    assert!(report.servers[0].utilization.is_some());
    assert_eq!(report.servers[0].gaps[0].error.kind, ErrorKind::Timeout);
}

#[tokio::test]
async fn services_are_bounded_redacted_and_unknown_is_explicit() {
    let core = Core::start(|operation, _| {
        assert_eq!(operation, "ListAllStackServices");
        let services: Vec<_> = (0..101)
            .map(|i| service(&format!("{i:03}-health-test-key"), "Unknown", None))
            .collect();
        ok(json!(services))
    })
    .await;
    let report = health::stack_services(&core.client(), "000000000000000000000002")
        .await
        .unwrap();
    assert!(report.truncated);
    assert_eq!(report.services.len(), 100);
    assert!(
        report
            .services
            .iter()
            .all(|s| s.health == HealthStatus::Unknown)
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("health-test-key")
    );
    assert_eq!(core.calls().len(), 1);
}

#[tokio::test]
async fn invalid_arguments_make_no_reads_and_oversized_or_mismatched_pages_fail() {
    let core = Core::start(|_, _| ok(json!([]))).await;
    assert_eq!(
        health::fleet_health(
            &core.client(),
            FleetHealthInput {
                page: 0,
                page_size: 0
            }
        )
        .await
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArguments
    );
    assert_eq!(
        health::fleet_health(
            &core.client(),
            FleetHealthInput {
                page: u64::MAX,
                page_size: 1
            }
        )
        .await
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArguments
    );
    assert_eq!(
        health::stack_services(&core.client(), "not-an-id")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArguments
    );
    assert!(core.calls().is_empty());
    let wrong = Core::start(|_, _| {
        let mut service = service("web", "Healthy", None);
        service["stack_id"] = json!("000000000000000000000004");
        ok(json!([service]))
    })
    .await;
    assert_eq!(
        health::stack_services(&wrong.client(), "000000000000000000000002")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidResponse
    );
    let big = Core::start(|_, _| ok(json!(vec![service("web", "Healthy", None); 102]))).await;
    assert_eq!(
        health::stack_services(&big.client(), "000000000000000000000002")
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidResponse
    );
}

#[tokio::test]
async fn overall_deadline_labels_unstarted_resources_and_keeps_completed_inventory() {
    let core = Core::start(|operation, params| match operation {
        "ListServers" => {
            assert_eq!(params["limit"], 100);
            let items: Vec<_> = (0..100)
                .map(|i| {
                    let mut s = base("servers")[0].clone();
                    s["id"] = json!(format!("{i:024x}"));
                    s
                })
                .collect();
            ok(json!(items))
        }
        "ListStacks" | "ListDeployments" => ok(json!([])),
        "GetServerState" => (
            StatusCode::OK,
            json!({"status":"Ok"}),
            Duration::from_millis(2200),
        ),
        "GetSystemStats" => (StatusCode::OK, stats(), Duration::from_millis(2200)),
        _ => panic!("unexpected {operation}"),
    })
    .await;
    let started = Instant::now();
    let report = health::fleet_health(
        &core.client(),
        FleetHealthInput {
            page: 0,
            page_size: 100,
        },
    )
    .await
    .unwrap();
    assert!(started.elapsed() < health::FLEET_TIMEOUT + Duration::from_secs(1));
    assert!(started.elapsed() >= health::FLEET_TIMEOUT);
    assert_eq!(report.servers.len(), 100);
    assert!(report.servers.iter().all(|s| matches!(
        s.health,
        HealthStatus::Unavailable | HealthStatus::Unknown
    ) && s.gaps.len() == 2));
    assert!(
        report
            .servers
            .iter()
            .any(|s| s.health == HealthStatus::Unknown)
    );
    assert!(core.calls().len() < 30);
    assert!(report.has_more);
    assert_eq!(report.next_page, Some(1));
}
