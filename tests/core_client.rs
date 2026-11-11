mod support;

use std::time::Duration;

use anyhow::Result;
use komodo_client::api::read::GetServer;
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    error::{AppError, ErrorKind},
};
use serde_json::json;
use support::*;

const DEADLINE: Duration = Duration::from_millis(300);

fn check_error(error: &AppError, kind: ErrorKind, status: Option<u16>) {
    assert_eq!(error.kind, kind);
    assert_eq!(error.status, status);
    assert_safe(&serde_json::to_string(error).unwrap());
    assert_safe(&format!("{error} {error:?} {error:#} {error:#?}"));
    let nested = anyhow::Error::new(error.clone()).context("safe diagnostic context");
    assert_safe(&format!("{nested:#} {nested:#?}"));
    let result = error.clone().into_tool_result();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.as_ref().unwrap()["kind"],
        kind.id()
    );
    assert_safe(&serde_json::to_string(&result).unwrap());
}

#[tokio::test]
async fn non_admin_reads_and_clones_reuse_transport_with_exact_request_counts() -> Result<()> {
    let core = MockCore::start(vec![
        Step::version(),
        Step::new("ListServers", Reply::new(200, SERVERS)),
        Step::new("ListServers", Reply::new(200, "[]")),
    ])
    .await?;
    let client = core.client(TEST_TIMEOUT);
    assert_eq!(client.detect_version().await?.to_string(), "2.3.2");
    let servers = client.clone().list_servers().await?;
    assert_eq!(servers[0].name, "fixture-server");
    assert_eq!(servers[0].info.region, "fixture");
    assert!(client.list_servers().await?.is_empty());
    assert_eq!(
        core.connections(),
        1,
        "clones must reuse a keep-alive connection"
    );
    assert_safe(&format!("{client:#?}"));
    core.assert_finished(&["GetVersion", "ListServers", "ListServers"]);
    Ok(())
}

#[tokio::test]
async fn typed_failures_and_recovery_drop_all_upstream_error_text() -> Result<()> {
    let failures = [
        (
            Reply::echoed(401, AUTHENTICATION),
            ErrorKind::Authentication,
            "ListServers",
        ),
        (
            Reply::echoed(500, PERMISSION),
            ErrorKind::PermissionDenied,
            "GetServer",
        ),
        (
            Reply::new(403, format!("{KEY} {SECRET}")),
            ErrorKind::PermissionDenied,
            "ListServers",
        ),
        (
            Reply::echoed(
                500,
                &json!({"error":format!("permission database failed {KEY}"),"trace":[SECRET]})
                    .to_string(),
            ),
            ErrorKind::Upstream,
            "ListServers",
        ),
        (
            Reply::new(502, format!("proxy {KEY} {SECRET}")),
            ErrorKind::Upstream,
            "ListServers",
        ),
        (
            Reply::new(200, format!("{{broken: {KEY} {SECRET}")),
            ErrorKind::InvalidResponse,
            "ListServers",
        ),
        (
            Reply::new(200, "{\"wrong_shape\":true}"),
            ErrorKind::InvalidResponse,
            "ListServers",
        ),
    ];
    for (reply, kind, operation) in failures {
        let status = reply.status;
        let core = MockCore::start(vec![
            Step::version(),
            Step::new(operation, reply),
            Step::new("ListServers", Reply::new(200, "[]")),
        ])
        .await?;
        let client = core.client(TEST_TIMEOUT);
        client.detect_version().await?;
        let error = if operation == "GetServer" {
            client
                .read(GetServer {
                    server: "restricted-server".into(),
                })
                .await
                .unwrap_err()
        } else {
            client.list_servers().await.unwrap_err()
        };
        check_error(&error, kind, Some(status));
        assert_eq!(error.operation, operation);
        assert!(client.list_servers().await?.is_empty());
        core.assert_finished(&["GetVersion", operation, "ListServers"]);
    }
    Ok(())
}

#[tokio::test]
async fn deadlines_cover_delayed_headers_and_stalled_success_and_error_bodies() -> Result<()> {
    for status in [200, 500] {
        for body_stall in [false, true] {
            let mut reply = Reply::new(status, "[]");
            if body_stall {
                reply.body_delay = Duration::from_secs(3);
            } else {
                reply.header_delay = Duration::from_secs(3);
            }
            let core = MockCore::start(vec![
                Step::version(),
                Step::new("ListServers", reply),
                Step::new("ListServers", Reply::new(200, "[]")),
            ])
            .await?;
            let client = core.client(DEADLINE);
            client.detect_version().await?;
            let error = timeout_request(client.list_servers()).await.unwrap_err();
            check_error(&error, ErrorKind::Timeout, None);
            assert!(client.list_servers().await?.is_empty());
            core.assert_finished(&["GetVersion", "ListServers", "ListServers"]);
        }
    }
    Ok(())
}

async fn timeout_request<T>(
    future: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::time::timeout(TEST_TIMEOUT, future)
        .await
        .expect("outer test deadline")
}

#[tokio::test]
async fn version_probes_have_the_same_deadline_and_validate_actual_response() -> Result<()> {
    let mut stalled = Reply::new(200, VERSION);
    stalled.body_delay = Duration::from_secs(3);
    let replies = [
        (stalled, ErrorKind::Timeout),
        (
            Reply::new(
                200,
                json!({"version":format!("invalid {KEY} {SECRET}")}).to_string(),
            ),
            ErrorKind::InvalidResponse,
        ),
        (
            Reply::echoed(401, AUTHENTICATION),
            ErrorKind::Authentication,
        ),
    ];
    for (reply, kind) in replies {
        let core = MockCore::start(vec![Step::new("GetVersion", reply)]).await?;
        let error = timeout_request(core.client(DEADLINE).detect_version())
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_safe(&format!("{error:#?}"));
        core.assert_finished(&["GetVersion"]);
    }
    let core = MockCore::start(vec![Step::new(
        "GetVersion",
        Reply::new(200, "{\"version\":\"2.3.3\"}"),
    )])
    .await?;
    assert_eq!(
        core.client(TEST_TIMEOUT)
            .detect_version()
            .await?
            .to_string(),
        "2.3.3"
    );
    core.assert_finished(&["GetVersion"]);
    let core = MockCore::start(vec![Step::new(
        "GetVersion",
        Reply::new(
            200,
            json!({"version": format!("2.3.2+{KEY}.{SECRET}")}).to_string(),
        ),
    )])
    .await?;
    let version = core.client(TEST_TIMEOUT).detect_version().await?;
    assert_safe(&format!(
        "{version} {version:?} {}",
        serde_json::to_string(&version)?
    ));
    assert!(version.to_string().contains("[redacted]"));
    core.assert_finished(&["GetVersion"]);
    Ok(())
}

#[tokio::test]
async fn refused_connections_are_unreachable() -> Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = format!("http://{}", listener.local_addr()?);
    drop(listener);
    let client = CoreClient::new(
        Config::parse(&address, KEY, SECRET)?,
        RequestPolicy::default(),
    )?;
    let error = timeout_request(client.detect_version()).await.unwrap_err();
    check_error(&error, ErrorKind::Unreachable, None);
    Ok(())
}

#[tokio::test]
async fn tls_connection_timeouts_precede_connectivity_classification() -> Result<()> {
    // A local TCP peer accepts but never completes TLS. This exercises reqwest's
    // typed connect-timeout path without relying on an external unroutable host.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = format!("https://{}", listener.local_addr()?);
    let holding = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(TEST_TIMEOUT).await;
    });
    let client = CoreClient::new(
        Config::parse(&address, KEY, SECRET)?,
        RequestPolicy {
            connect_timeout: Duration::from_millis(100),
            operation_timeout: Duration::from_secs(2),
        },
    )?;
    let outcome = timeout_request(client.detect_version()).await;
    holding.abort();
    check_error(&outcome.unwrap_err(), ErrorKind::Timeout, None);
    Ok(())
}

#[tokio::test]
async fn concurrent_reads_keep_their_own_deadlines_and_results() -> Result<()> {
    let mut slow = Reply::new(200, "[]");
    slow.body_delay = Duration::from_secs(3);
    let core = MockCore::start(vec![
        Step::version(),
        Step::new("ListServers", slow),
        Step::new("GetServer", Reply::echoed(500, PERMISSION)),
        Step::new("ListServers", Reply::new(200, SERVERS)),
    ])
    .await?;
    let client = core.client(DEADLINE);
    client.detect_version().await?;
    let slow_client = client.clone();
    let slow = tokio::spawn(async move { slow_client.list_servers().await });
    core.wait_requests(2).await?;
    let permission = client
        .read(GetServer {
            server: "restricted-server".into(),
        })
        .await
        .unwrap_err();
    check_error(&permission, ErrorKind::PermissionDenied, Some(500));
    assert_eq!(client.list_servers().await?[0].name, "fixture-server");
    let error = tokio::time::timeout(TEST_TIMEOUT, slow)
        .await??
        .unwrap_err();
    check_error(&error, ErrorKind::Timeout, None);
    core.assert_finished(&["GetVersion", "ListServers", "GetServer", "ListServers"]);
    Ok(())
}

#[tokio::test]
async fn redirects_never_forward_credentials() -> Result<()> {
    let destination = MockCore::start(vec![]).await?;
    let mut redirect = Reply::new(307, "");
    redirect.location = Some(format!("{}/read", destination.address));
    let core = MockCore::start(vec![Step::new("GetVersion", redirect)]).await?;
    let error = core
        .client(TEST_TIMEOUT)
        .detect_version()
        .await
        .unwrap_err();
    check_error(&error, ErrorKind::Upstream, Some(307));
    assert_eq!(destination.connections(), 0);
    core.assert_finished(&["GetVersion"]);
    destination.assert_finished(&[]);
    Ok(())
}
