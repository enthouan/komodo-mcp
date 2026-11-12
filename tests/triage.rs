mod support;

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use anyhow::Result;
use komodo_client::{
    api::read::{GetSystemStatsResponse, ListAllStackServicesResponse},
    entities::{alert::Alert, stack::Stack, update::Update},
};
use komodo_mcp::{
    health::HealthStatus,
    triage::{self, DiagnosisOutcome, StackDiagnoseInput},
};
use serde_json::Value;
use support::{
    assert_safe,
    incident::{IncidentCore, Mode, Options, fixture, wire},
};

fn input() -> StackDiagnoseInput {
    StackDiagnoseInput {
        id: fixture()["stack"]["id"].as_str().unwrap().into(),
    }
}

fn safe(value: &Value) {
    let text = value.to_string();
    assert_safe(&text);
    let f = fixture();
    for key in [
        "credential_marker",
        "raw_config_marker",
        "other_data_marker",
    ] {
        assert!(!text.contains(f[key].as_str().unwrap()), "leaked {key}");
    }
}

#[test]
fn shared_incident_wire_deserializes_into_pinned_sdk_types() -> Result<()> {
    let w = wire();
    let _: GetSystemStatsResponse = serde_json::from_value(w["stats"].clone())?;
    let _: ListAllStackServicesResponse = serde_json::from_value(w["services"].clone())?;
    let _: Stack = serde_json::from_value(w["stack_detail"].clone())?;
    let _: Update = serde_json::from_value(w["update"].clone())?;
    let _: Alert = serde_json::from_value(w["alerts"][0].clone())?;
    Ok(())
}

#[tokio::test]
async fn broken_stack_correlates_services_failed_stage_alert_and_filtered_logs() -> Result<()> {
    let core = IncidentCore::start(Options::default()).await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    let f = fixture();
    assert!(
        report
            .summary
            .contains(f["affected_service"].as_str().unwrap())
    );
    assert!(
        report
            .summary
            .contains(f["update"]["failed_stage"].as_str().unwrap())
    );
    assert!(
        report
            .summary
            .contains(f["expected_log_excerpt"].as_str().unwrap())
    );
    assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy);
    assert_eq!(
        report.affected_services,
        vec![f["affected_service"].as_str().unwrap()]
    );
    let update = report.failed_update.as_ref().unwrap();
    assert_eq!(update.summary.id, f["update"]["id"]);
    assert_eq!(update.stages[0].stage, f["update"]["failed_stage"]);
    assert!(!update.stages[0].success);
    assert_eq!(report.recent_alerts[0].id, f["alert"]["id"]);
    assert!(
        report
            .logs
            .as_ref()
            .unwrap()
            .stdout
            .contains(f["expected_log_excerpt"].as_str().unwrap())
    );
    assert!(report.gaps.is_empty(), "{:?}", report.gaps);
    let value = serde_json::to_value(&report)?;
    safe(&value);
    assert!(serde_json::to_vec(&report)?.len() <= triage::MAX_OUTPUT_BYTES);
    let requests = core.requests();
    assert_eq!(requests.len(), 7);
    assert!(core.peak() <= 4 && core.peak() > 1);
    let log = requests
        .iter()
        .find(|r| r.operation == "GetStackLog")
        .unwrap();
    assert_eq!(
        log.params["services"],
        serde_json::json!([f["affected_service"]])
    );
    assert_eq!(log.params["tail"], 40);
    let reads: BTreeSet<_> = [
        "ListStacks",
        "GetStack",
        "ListAllStackServices",
        "ListUpdates",
        "ListAlerts",
        "GetUpdate",
        "GetStackLog",
    ]
    .into();
    assert!(
        requests
            .iter()
            .all(|r| reads.contains(r.operation.as_str()))
    );
    Ok(())
}

#[tokio::test]
async fn healthy_and_inconclusive_observations_are_explicit() -> Result<()> {
    for (mode, expected) in [
        (Mode::Healthy, DiagnosisOutcome::Healthy),
        (Mode::Inconclusive, DiagnosisOutcome::Inconclusive),
    ] {
        let core = IncidentCore::start(Options {
            mode,
            ..Default::default()
        })
        .await;
        let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
        assert_eq!(report.outcome, expected);
        assert!(report.affected_services.is_empty());
        if expected == DiagnosisOutcome::Healthy {
            assert!(report.gaps.is_empty());
            assert!(
                report
                    .services
                    .unwrap()
                    .services
                    .iter()
                    .all(|s| s.health == HealthStatus::Healthy)
            );
        } else {
            assert_eq!(
                report
                    .gaps
                    .iter()
                    .map(|g| g.source.as_str())
                    .collect::<BTreeSet<_>>(),
                ["updates", "alerts", "logs"].into()
            );
            assert!(report.services.is_some());
        }
        assert_eq!(core.requests().len(), 6);
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_evidence_source_preserves_independent_incident_evidence() -> Result<()> {
    for failed in [
        "ListStacks",
        "GetStack",
        "ListAllStackServices",
        "ListUpdates",
        "ListAlerts",
        "GetUpdate",
        "GetStackLog",
    ] {
        let core = IncidentCore::start(Options {
            failures: [failed.into()].into(),
            ..Default::default()
        })
        .await;
        let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
        assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy, "{failed}");
        assert!(!report.gaps.is_empty(), "{failed}");
        if failed != "GetStackLog" {
            assert!(report.logs.is_some(), "{failed}");
        }
        if failed != "ListUpdates" && failed != "GetUpdate" {
            assert!(report.failed_update.is_some(), "{failed}");
        }
        safe(&serde_json::to_value(&report)?);
        assert!(core.requests().len() <= 7 && core.peak() <= 4);
    }
    Ok(())
}

#[tokio::test]
async fn sources_run_in_parallel_and_timeouts_remain_bounded() -> Result<()> {
    let core = IncidentCore::start(Options {
        delay: Duration::from_millis(50),
        ..Default::default()
    })
    .await;
    let start = Instant::now();
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "parallel two-wave wait: {:?}",
        start.elapsed()
    );
    assert!(core.peak() <= 4);
    let stalled = IncidentCore::start(Options {
        delay: Duration::from_secs(3),
        ..Default::default()
    })
    .await;
    let start = Instant::now();
    let report =
        triage::stack_diagnose(&stalled.client(Duration::from_millis(80)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Inconclusive);
    assert_eq!(report.gaps.len(), 5);
    assert!(
        report
            .gaps
            .iter()
            .all(|g| g.error.kind == komodo_mcp::error::ErrorKind::Timeout)
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(stalled.requests().len() <= 5);
    Ok(())
}

#[tokio::test]
async fn invalid_id_fails_before_any_core_request() {
    let core = IncidentCore::start(Options::default()).await;
    let error = triage::stack_diagnose(
        &core.client(Duration::from_secs(1)),
        StackDiagnoseInput {
            id: "invalid".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, komodo_mcp::error::ErrorKind::InvalidArguments);
    assert!(core.requests().is_empty());
}

#[tokio::test]
async fn healthy_runtime_is_distinct_from_historical_failed_rollouts() -> Result<()> {
    let core = IncidentCore::start(Options {
        mode: Mode::Healthy,
        overrides: [(
            "ListUpdates".into(),
            serde_json::json!({"updates":wire()["updates"],"next_page":null}),
        )]
        .into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Healthy);
    assert!(
        report.failed_update.is_some(),
        "history retained without changing current health"
    );
    Ok(())
}

#[tokio::test]
async fn changed_update_and_missing_service_are_labeled_as_evidence_gaps() -> Result<()> {
    let mut changed = wire()["update"].clone();
    changed["success"] = serde_json::json!(true);
    let core = IncidentCore::start(Options {
        overrides: [("GetUpdate".into(), changed)].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert!(report.failed_update.is_none());
    assert!(report.gaps.iter().any(|g| g.source == "update_detail"));
    assert!(report.logs.is_some());
    let core = IncidentCore::start(Options {
        mode: Mode::Healthy,
        overrides: [(
            "ListAllStackServices".into(),
            serde_json::json!([wire()["services"][1]]),
        )]
        .into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Inconclusive);
    assert!(report.gaps.iter().any(|g| g.source == "service_inventory"));
    let mut substituted = wire()["services"].clone();
    substituted[0]["service"] = serde_json::json!("worker");
    substituted[0]["state"] = serde_json::json!("Running");
    let core = IncidentCore::start(Options {
        mode: Mode::Healthy,
        overrides: [("ListAllStackServices".into(), substituted)].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(
        report.outcome,
        DiagnosisOutcome::Inconclusive,
        "equal count with different service membership"
    );
    assert!(report.gaps.iter().any(|g| g.source == "service_inventory"));
    Ok(())
}

#[tokio::test]
async fn escaped_large_evidence_obeys_the_serialized_report_byte_limit() -> Result<()> {
    let services: Vec<_> = (0..100).map(|i| {
        let mut value = wire()["services"][0].clone();
        value["service"] = serde_json::json!(format!("{i:03}{}", "\0".repeat(300)));
        value["container"] = serde_json::json!({"name":"\0".repeat(300),"state":"running","status":"\0".repeat(300)});
        value
    }).collect();
    let core = IncidentCore::start(Options {
        overrides: [("ListAllStackServices".into(), serde_json::json!(services))].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy);
    assert!(report.limits.evidence_truncated);
    assert!(serde_json::to_vec(&report)?.len() <= triage::MAX_OUTPUT_BYTES);
    assert!(report.failed_update.is_some());
    assert!(report.services.unwrap().truncated);
    Ok(())
}

#[tokio::test]
async fn unhealthy_replicas_share_one_filter_per_distinct_service() -> Result<()> {
    let w = wire();
    let mut services = vec![w["services"][0].clone(); 6];
    let mut db = w["services"][1].clone();
    db["state"] = serde_json::json!("Unhealthy");
    services.push(db);
    let core = IncidentCore::start(Options {
        overrides: [("ListAllStackServices".into(), serde_json::json!(services))].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy);
    assert_eq!(report.affected_services, vec!["db", "web"]);
    assert!(
        report.logs.is_some(),
        "duplicate replicas must not invalidate service-filtered logs"
    );
    assert!(report.gaps.is_empty(), "{:?}", report.gaps);
    let requests = core.requests();
    let log = requests
        .iter()
        .find(|r| r.operation == "GetStackLog")
        .unwrap();
    assert_eq!(log.params["services"], serde_json::json!(["db", "web"]));
    assert_eq!(requests.len(), 7);
    Ok(())
}

#[tokio::test]
async fn projected_service_names_cannot_establish_healthy_coverage() -> Result<()> {
    for (expected, observed) in [
        ("a".repeat(128), "a".repeat(300)),
        ("[redacted]".into(), support::KEY.into()),
    ] {
        let mut w = wire();
        w["stacks"][0]["info"]["state"] = serde_json::json!("running");
        w["stacks"][0]["info"]["services"][0]["service"] = serde_json::json!(expected);
        w["services"][0]["service"] = serde_json::json!(observed);
        w["services"][0]["state"] = serde_json::json!("Running");
        let core = IncidentCore::start(Options {
            mode: Mode::Healthy,
            overrides: [
                ("ListStacks".into(), w["stacks"].clone()),
                ("ListAllStackServices".into(), w["services"].clone()),
            ]
            .into(),
            ..Default::default()
        })
        .await;
        let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
        assert_eq!(report.outcome, DiagnosisOutcome::Inconclusive);
        assert!(report.gaps.iter().any(|g| g.source == "service_inventory"));
        assert!(report.limits.evidence_truncated);
        safe(&serde_json::to_value(&report)?);
    }
    Ok(())
}

#[tokio::test]
async fn swarm_logs_select_one_known_service_or_report_an_explicit_gap() -> Result<()> {
    for mode in [Mode::Broken, Mode::Healthy] {
        let core = IncidentCore::start(Options {
            mode,
            swarm: true,
            ..Default::default()
        })
        .await;
        let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
        assert!(report.logs.is_some(), "{:?}", report.gaps);
        let requests = core.requests();
        let log = requests
            .iter()
            .find(|r| r.operation == "GetStackLog")
            .unwrap();
        assert_eq!(log.params["services"].as_array().unwrap().len(), 1);
        assert!(
            report.limits.evidence_truncated,
            "Swarm log evidence covers one service"
        );
        assert!(requests.len() <= 7);
    }
    let mut services = wire()["services"].clone();
    services[1]["state"] = serde_json::json!("Unhealthy");
    let core = IncidentCore::start(Options {
        swarm: true,
        overrides: [("ListAllStackServices".into(), services)].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert_eq!(report.affected_services, vec!["db", "web"]);
    assert_eq!(report.logs.unwrap().target.services.len(), 1);
    let core = IncidentCore::start(Options {
        swarm: true,
        failures: ["ListAllStackServices".into()].into(),
        ..Default::default()
    })
    .await;
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(1)), input()).await?;
    assert!(report.logs.is_none());
    assert!(report.gaps.iter().any(|g| g.source == "logs"));
    assert!(core.requests().iter().all(|r| r.operation != "GetStackLog"));
    Ok(())
}

#[tokio::test]
async fn oversized_diagnosis_sources_preserve_evidence_and_existing_inventory_calls() -> Result<()>
{
    let large = "x".repeat(komodo_mcp::client::MAX_EVIDENCE_RESPONSE_BYTES);
    for chunked in [false, true] {
        for operation in ["ListStacks", "GetStack", "ListAllStackServices"] {
            let mut w = wire();
            let body = match operation {
                "ListStacks" => {
                    w["stacks"][0]["info"]["status"] = serde_json::json!(large);
                    w["stacks"].clone()
                }
                "GetStack" => {
                    w["stack_detail"]["config"]["file_contents"] = serde_json::json!(large);
                    w["stack_detail"].clone()
                }
                _ => {
                    w["services"][0]["container"]["status"] = serde_json::json!(large);
                    w["services"].clone()
                }
            };
            let core = IncidentCore::start(Options {
                chunked,
                overrides: [(operation.into(), body.clone())].into(),
                ..Default::default()
            })
            .await;
            let report =
                triage::stack_diagnose(&core.client(Duration::from_secs(2)), input()).await?;
            assert_eq!(report.outcome, DiagnosisOutcome::Unhealthy);
            assert!(
                report
                    .gaps
                    .iter()
                    .any(|g| g.error.kind == komodo_mcp::error::ErrorKind::ResponseTooLarge),
                "{operation}"
            );
            assert!(report.failed_update.is_some() && report.logs.is_some());
            assert!(core.requests().len() <= 7);
            if operation == "GetStack" {
                let core = IncidentCore::start(Options {
                    overrides: [(operation.into(), body)].into(),
                    ..Default::default()
                })
                .await;
                let detail = komodo_mcp::inventory::get_stack(
                    &core.client(Duration::from_secs(2)),
                    komodo_mcp::inventory::DetailInput {
                        id: Some(input().id),
                        name: None,
                    },
                )
                .await?;
                assert_eq!(detail.summary.id, fixture()["stack"]["id"]);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn production_source_deadlines_bound_both_waves() -> Result<()> {
    let core = IncidentCore::start(Options {
        delay: Duration::from_secs(3),
        ..Default::default()
    })
    .await;
    let start = Instant::now();
    let report = triage::stack_diagnose(&core.client(Duration::from_secs(10)), input()).await?;
    assert_eq!(report.outcome, DiagnosisOutcome::Inconclusive);
    assert!(
        report
            .gaps
            .iter()
            .all(|g| g.error.kind == komodo_mcp::error::ErrorKind::Timeout)
    );
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert!(core.requests().len() <= 5);
    Ok(())
}

#[tokio::test]
async fn stdio_reports_all_outcomes_and_bounds_the_final_redacted_json() -> Result<()> {
    use rmcp::{
        ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, Implementation},
        service::ClientLifecycleMode,
    };
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    let large_services:Vec<_>=(0..100).map(|index| {
        let mut service=wire()["services"][0].clone();
        service["service"]=serde_json::json!(format!("{index:03}{}","q\0".repeat(150)));
        service["container"]=serde_json::json!({"name":"q\0".repeat(150),"state":"running","status":"q\0".repeat(150)});
        service
    }).collect();
    let cases = [
        (Options::default(), "unhealthy"),
        (
            Options {
                mode: Mode::Healthy,
                ..Default::default()
            },
            "healthy",
        ),
        (
            Options {
                mode: Mode::Inconclusive,
                ..Default::default()
            },
            "inconclusive",
        ),
        (
            Options {
                api_key: Some("q".into()),
                overrides: [(
                    "ListAllStackServices".into(),
                    serde_json::json!(large_services),
                )]
                .into(),
                ..Default::default()
            },
            "unhealthy",
        ),
    ];
    for (index, (options, outcome)) in cases.into_iter().enumerate() {
        timeout(Duration::from_secs(15), async {
            let core = IncidentCore::start(options).await;
            let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
                .env_clear()
                .env("KOMODO_ADDRESS", &core.address)
                .env("KOMODO_API_KEY", core.api_key())
                .env("KOMODO_API_SECRET", support::SECRET)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
            let mut session = ClientConfig::new(
                Default::default(),
                Implementation::new("diagnosis-regression", "1"),
            )
            .serve_with_lifecycle(
                (child.stdout.take().unwrap(), child.stdin.take().unwrap()),
                ClientLifecycleMode::Initialize,
            )
            .await?;
            let result = session
                .call_tool(
                    CallToolRequestParams::new("stack_diagnose").with_arguments(
                        serde_json::json!({"id":input().id})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await?;
            assert_eq!(result.is_error, Some(false));
            let value = serde_json::to_value(result)?;
            let report = &value["structuredContent"];
            let text: Value = serde_json::from_str(value["content"][0]["text"].as_str().unwrap())?;
            assert_eq!(report, &text);
            assert_eq!(report["outcome"], outcome);
            assert!(serde_json::to_vec(report)?.len() <= triage::MAX_OUTPUT_BYTES);
            safe(&value);
            if index == 0 {
                assert_eq!(
                    report["affected_services"],
                    serde_json::json!([fixture()["affected_service"]])
                );
                assert_eq!(report["failed_update"]["id"], fixture()["update"]["id"]);
                assert_eq!(report["recent_alerts"][0]["id"], fixture()["alert"]["id"]);
                assert!(
                    report["summary"]
                        .as_str()
                        .unwrap()
                        .contains(fixture()["expected_log_excerpt"].as_str().unwrap())
                );
            }
            if index == 3 {
                assert_eq!(report["limits"]["evidence_truncated"], true);
            }
            session.close().await?;
            assert!(child.wait().await?.success());
            assert!(
                core.requests().len() <= 8,
                "startup probe plus at most seven diagnosis reads"
            );
            assert!(core.requests().iter().all(|r| {
                [
                    "GetVersion",
                    "ListStacks",
                    "GetStack",
                    "ListAllStackServices",
                    "ListUpdates",
                    "ListAlerts",
                    "GetUpdate",
                    "GetStackLog",
                ]
                .contains(&r.operation.as_str())
            }));
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
    Ok(())
}
