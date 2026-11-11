#[path = "../examples/support/mod.rs"]
mod demo;

use anyhow::{Result, ensure};
use demo::{
    mock_core::{DEFAULT_SERVER, KEY, MockCore, SECRET},
    scenario::{self, Configuration, StackState},
};
use komodo_client::{
    api::read::{GetDeployment, GetServer, GetStack, ListDeployments, ListServers, ListStacks},
    entities::resource::TemplatesQueryBehavior,
};
use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    error::ErrorKind,
};
use serde_json::{Value, json};
use std::path::Path;
use tokio::{
    net::TcpStream,
    time::{Duration, timeout},
};

fn executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_komodo-mcp"))
}
async fn stopped(address: &str) -> Result<()> {
    let address = address
        .trim_start_matches("http://")
        .trim_end_matches("/komodo");
    ensure!(
        timeout(Duration::from_secs(2), TcpStream::connect(address))
            .await?
            .is_err(),
        "mock listener still active"
    );
    Ok(())
}

#[tokio::test]
async fn release_verification_checks_the_selected_executables_mcp_version() -> Result<()> {
    let evidence = scenario::verify_release(executable(), env!("CARGO_PKG_VERSION")).await?;
    assert_eq!(evidence["observed_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(evidence["child_exit_success"], true);
    assert_eq!(evidence["forced_kill"], false);
    assert_eq!(evidence["mock_stopped"], true);
    let error = scenario::verify_release(executable(), "9.9.9")
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("MCP version mismatch: expected 9.9.9")
    );
    Ok(())
}

#[tokio::test]
async fn quickstart_answers_from_initialized_stdio_and_all_inventory_pages() -> Result<()> {
    let report =
        scenario::run(executable(), DEFAULT_SERVER, Configuration::Synthetic, None).await?;
    stopped(&report.address).await?;
    assert!(report.status.success(), "{}", report.stderr);
    assert!(report.stderr.contains("MCP session closed"));
    scenario::check_safe(&report.stderr)?;
    scenario::check_safe(&serde_json::to_string(&report.transcript)?)?;
    let answer = report.result?;
    assert_eq!(answer.server.id, "000000000000000000000001");
    assert_eq!(answer.pages, 3);
    assert_eq!(answer.visible, 6);
    assert_eq!(
        answer
            .stacks
            .iter()
            .map(|s| (s.name.as_str(), s.id.as_str(), &s.state))
            .collect::<Vec<_>>(),
        vec![
            (
                "alpha-web",
                "000000000000000000000002",
                &StackState::Running
            ),
            (
                "gamma-jobs",
                "000000000000000000000005",
                &StackState::Stopped
            ),
            (
                "zeta-metrics",
                "000000000000000000000008",
                &StackState::Paused
            ),
        ]
    );
    let get = &report.transcript[2];
    assert_eq!(get["name"], "get_server");
    assert_eq!(get["arguments"], json!({"name":DEFAULT_SERVER}));
    assert_eq!(get["result"]["structuredContent"]["id"], answer.server.id);
    assert_eq!(report.transcript[0]["method"], "initialize");
    assert_eq!(report.transcript[1]["method"], "tools/list");
    for (page, entry) in report.transcript[3..].iter().enumerate() {
        assert_eq!(entry["name"], "list_stacks");
        assert_eq!(entry["arguments"], json!({"page":page,"page_size":2}));
    }
    assert_eq!(
        report
            .requests
            .iter()
            .map(|r| r.operation.as_str())
            .collect::<Vec<_>>(),
        vec![
            "GetVersion",
            "ListServers",
            "GetServer",
            "ListStacks",
            "ListStacks",
            "ListStacks",
            "ListStacks",
            "ListStacks",
            "ListStacks"
        ]
    );
    assert_eq!(
        report.requests[3..]
            .iter()
            .map(|r| (r.page, r.limit))
            .collect::<Vec<_>>(),
        vec![
            (Some(0), Some(2)),
            (Some(1), Some(2)),
            (Some(1), Some(2)),
            (Some(2), Some(2)),
            (Some(2), Some(2)),
            (Some(3), Some(2))
        ]
    );
    Ok(())
}

#[tokio::test]
async fn empty_answer_names_the_server_and_visible_inventory() -> Result<()> {
    let report =
        scenario::run(executable(), "empty-server", Configuration::Synthetic, None).await?;
    stopped(&report.address).await?;
    let answer = report.result?;
    assert!(answer.stacks.is_empty());
    assert!(
        answer
            .to_string()
            .contains("empty-server (00000000000000000000000a)")
    );
    assert!(
        answer
            .to_string()
            .contains("No matching stacks in the 6 visible stacks")
    );
    Ok(())
}

#[tokio::test]
async fn startup_and_scenario_failures_capture_safe_diagnostics_and_reap_resources() -> Result<()> {
    for (config, name, fail, expected, success, rejections) in [
        (
            Configuration::MissingSecret,
            DEFAULT_SERVER,
            None,
            "configuration (configure)",
            false,
            0,
        ),
        (
            Configuration::WrongKey,
            DEFAULT_SERVER,
            None,
            "authentication (GetVersion)",
            false,
            1,
        ),
        (
            Configuration::WrongSecret,
            DEFAULT_SERVER,
            None,
            "authentication (GetVersion)",
            false,
            1,
        ),
        (
            Configuration::Synthetic,
            "missing-server",
            None,
            "get_server failed (not_found)",
            true,
            0,
        ),
        (
            Configuration::Synthetic,
            DEFAULT_SERVER,
            Some("ListStacks"),
            "list_stacks failed (upstream)",
            true,
            0,
        ),
    ] {
        let report = scenario::run(executable(), name, config, fail).await?;
        stopped(&report.address).await?;
        assert_eq!(report.status.success(), success, "{}", report.stderr);
        assert_eq!(report.rejected_credentials, rejections);
        assert!(report.result.is_err());
        let diagnostic = format!("{:#}\n{}", report.result.unwrap_err(), report.stderr);
        assert!(diagnostic.contains(expected), "{diagnostic}");
        scenario::check_safe(&diagnostic)?;
        scenario::check_safe(&serde_json::to_string(&report.transcript)?)?;
        if matches!(config, Configuration::MissingSecret) {
            assert!(report.requests.is_empty());
            assert!(report.stderr.contains("KOMODO_API_SECRET is required"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn standalone_mock_repeats_typed_reads_filters_and_details_across_connections() -> Result<()>
{
    let core = MockCore::start("127.0.0.1:0".parse()?).await?;
    for _ in 0..2 {
        let client = CoreClient::new(
            Config::parse(&core.address, KEY, SECRET)?,
            RequestPolicy::default(),
        )?;
        let stacks = client
            .read(ListStacks {
                limit: Some(0),
                ..Default::default()
            })
            .await?;
        assert_eq!(stacks.len(), 6);
        assert_eq!(client.detect_version().await?.to_string(), "2.3.2");
        for stack in &stacks {
            let full = client
                .read(GetStack {
                    stack: stack.id.clone(),
                })
                .await?;
            assert_eq!(full.id, stack.id);
            assert_eq!(full.name, stack.name);
            assert_eq!(full.config.server_id, stack.info.server_id);
            assert_eq!(full.config.swarm_id, stack.info.swarm_id);
        }
        let servers = client
            .read(ListServers {
                limit: Some(0),
                ..Default::default()
            })
            .await?;
        assert_eq!(servers.len(), 3);
        for server in &servers {
            assert_eq!(
                client
                    .read(GetServer {
                        server: server.id.clone()
                    })
                    .await?
                    .name,
                server.name
            );
        }
        let deployments = client.read(ListDeployments::default()).await?;
        assert_eq!(deployments.len(), 1);
        let full = client
            .read(GetDeployment {
                deployment: deployments[0].id.clone(),
            })
            .await?;
        assert_eq!(full.config.server_id, deployments[0].info.server_id);
        let mut query = ListStacks {
            page: 1,
            limit: Some(2),
            sort_desc: true,
            ..Default::default()
        };
        assert_eq!(
            client
                .read(query.clone())
                .await?
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["omega-swarm", "gamma-jobs"]
        );
        query.page = 0;
        query.query.terms = vec!["WEB".into()];
        assert_eq!(client.read(query.clone()).await?[0].name, "alpha-web");
        query.query.terms.clear();
        query.query.names = vec![stacks[2].id.clone()];
        assert_eq!(client.read(query.clone()).await?[0].id, stacks[2].id);
        query.query.names = vec!["beta-other".into()];
        assert_eq!(client.read(query.clone()).await?[0].name, "beta-other");
        query.query.templates = TemplatesQueryBehavior::Only;
        assert!(client.read(query).await?.is_empty());
        let missing = client
            .read(GetServer {
                server: "ffffffffffffffffffffffff".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(missing.kind, ErrorKind::NotFound);
        let restricted = client
            .read(GetServer {
                server: "restricted-server".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(restricted.kind, ErrorKind::PermissionDenied);
    }
    core.verify_reads()?;
    core.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn mock_rejects_writes_and_unexpected_reads_observably_without_echoing_credentials()
-> Result<()> {
    let core = MockCore::start("127.0.0.1:0".parse()?).await?;
    for (endpoint, body) in [
        ("write", json!({"type":"UpdateServer","params":{}})),
        ("execute", json!({"type":"RunAction","params":{}})),
        ("read", json!({"type":"ListUsers","params":{}})),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{}/{endpoint}", core.address))
            .header("x-api-key", KEY)
            .header("x-api-secret", SECRET)
            .json(&body)
            .send()
            .await?;
        assert!(response.status().is_client_error());
        let value: Value = response.json().await?;
        scenario::check_safe(&value.to_string())?;
    }
    assert!(core.verify_reads().is_err());
    core.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn deadline_and_interruption_paths_also_close_pipes_and_stop_the_mock() -> Result<()> {
    let report = scenario::run_with_control(
        executable(),
        DEFAULT_SERVER,
        Configuration::Synthetic,
        None,
        Duration::ZERO,
        std::future::pending(),
    )
    .await?;
    stopped(&report.address).await?;
    assert!(format!("{:#}", report.result.unwrap_err()).contains("overall scenario deadline"));
    let report = scenario::run_with_control(
        executable(),
        DEFAULT_SERVER,
        Configuration::Synthetic,
        None,
        Duration::from_secs(30),
        std::future::ready(Err(anyhow::anyhow!("injected interruption"))),
    )
    .await?;
    stopped(&report.address).await?;
    assert!(format!("{:#}", report.result.unwrap_err()).contains("injected interruption"));
    scenario::check_safe(&report.stderr)?;
    Ok(())
}
