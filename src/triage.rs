//! A bounded composition of the inventory and incident readers.

use std::{future::Future, time::Duration};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    health::{self, HealthStatus, StackServices},
    history::{
        self, AlertSummary, HistoryInput, HistoryTarget, ResourceKind, UpdateDetail, UpdateInput,
        UpdateOutcome, UpdateSummary,
    },
    inventory::{self, DetailInput, StackDetail, StackState},
    logs::{self, StackLogsInput, WorkloadLogs},
};

pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_OUTPUT_BYTES: usize = 65_536;
const MAX_SERVICES: usize = 20;
const MAX_RECORDS: usize = 5;
const MAX_STAGES: usize = 5;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackDiagnoseInput {
    /// Stable Stack ID returned by inventory or fleet_health.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisOutcome {
    Unhealthy,
    Healthy,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceGap {
    pub source: String,
    pub error: AppError,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosisLimits {
    pub max_core_calls: usize,
    pub max_parallel_core_calls: usize,
    pub source_timeout_ms: u64,
    /// Two bounded waves; includes inventory resolution in its first deadline.
    pub max_wait_ms: u64,
    /// Serialized structured-report JSON bytes; text content and MCP framing add overhead.
    pub max_output_bytes: usize,
    pub evidence_truncated: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackDiagnosis {
    pub stack_id: String,
    pub stack: Option<StackDetail>,
    pub outcome: DiagnosisOutcome,
    pub summary: String,
    /// At most four affected service names; services retains further evidence.
    pub affected_services: Vec<String>,
    pub services: Option<StackServices>,
    pub recent_updates: Vec<UpdateSummary>,
    pub recent_alerts: Vec<AlertSummary>,
    pub failed_update: Option<UpdateDetail>,
    pub logs: Option<WorkloadLogs>,
    pub gaps: Vec<EvidenceGap>,
    pub limits: DiagnosisLimits,
}

async fn source<T>(
    operation: &'static str,
    future: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    timeout(SOURCE_TIMEOUT, future)
        .await
        .map_err(|_| AppError::new(ErrorKind::Timeout, operation, None))?
}

fn retain<T>(
    name: &'static str,
    result: Result<T, AppError>,
    gaps: &mut Vec<EvidenceGap>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            gaps.push(EvidenceGap {
                source: name.into(),
                error,
            });
            None
        }
    }
}

fn clip(client: &CoreClient, text: &str, bytes: usize) -> String {
    let mut value = client.redact(text);
    if value.len() > bytes {
        let mut end = bytes;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

pub async fn stack_diagnose(
    client: &CoreClient,
    input: StackDiagnoseInput,
) -> Result<StackDiagnosis, AppError> {
    if input.id.len() != 24 || !input.id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::arguments(
            "stack_diagnose",
            "Provide a 24-character hexadecimal stack ID",
        ));
    }
    let id = input.id.to_ascii_lowercase();
    let history_input = HistoryInput {
        target: Some(HistoryTarget {
            kind: ResourceKind::Stack,
            id: id.clone(),
        }),
        page: 0,
    };
    // Four independent sources. Inventory's two reads are sequential inside its
    // own deadline; a failing source cannot cancel the other evidence futures.
    let (stack, services, updates, alerts) = tokio::join!(
        source(
            "GetStack",
            inventory::get_stack_observation(
                client,
                DetailInput {
                    id: Some(id.clone()),
                    name: None
                },
                health::MAX_SERVICES
            )
        ),
        source("ListAllStackServices", health::stack_services(client, &id)),
        source(
            "ListUpdates",
            history::list_updates(client, history_input.clone())
        ),
        source("ListAlerts", history::list_alerts(client, history_input)),
    );
    let mut gaps = vec![];
    let stack_observation = retain("inventory", stack, &mut gaps);
    let (mut stack, expected_services) = match stack_observation {
        Some((stack, expected)) => (Some(stack), expected),
        None => (None, None),
    };
    let mut services = retain("services", services, &mut gaps);
    let updates = retain("updates", updates, &mut gaps);
    let alerts = retain("alerts", alerts, &mut gaps);
    let service_failure = services.as_ref().is_some_and(|s| {
        s.services
            .iter()
            .any(|s| s.health == HealthStatus::Unhealthy)
    });
    let stack_failure = stack.as_ref().is_some_and(|s| {
        !s.summary.template
            && !matches!(
                s.summary.state,
                StackState::Running | StackState::Unknown | StackState::Deploying
            )
    });
    let service_coverage_matches =
        stack
            .as_ref()
            .zip(services.as_ref())
            .is_some_and(|(stack, services)| {
                let names: std::collections::BTreeSet<_> = services
                    .services
                    .iter()
                    .map(|s| s.service.as_str())
                    .collect();
                names.len() <= stack.summary.service_count
                    && expected_services.as_ref().is_some_and(|expected| {
                        names == expected.iter().map(String::as_str).collect()
                    })
            });
    if stack.is_some() && services.is_some() && !service_coverage_matches {
        gaps.push(EvidenceGap {
            source: "service_inventory".into(),
            error: AppError::new(ErrorKind::InvalidResponse, "ListAllStackServices", None),
        });
    }
    let current_healthy = service_coverage_matches
        && stack
            .as_ref()
            .is_some_and(|s| !s.summary.template && matches!(s.summary.state, StackState::Running))
        && services.as_ref().is_some_and(|s| {
            !s.truncated
                && !s.services.is_empty()
                && s.services.iter().all(|s| s.health == HealthStatus::Healthy)
        });
    // Old failed rollouts are historical evidence, not proof that a currently
    // healthy service remains unhealthy. Missing history is still an explicit gap.
    let outcome = if service_failure || stack_failure {
        DiagnosisOutcome::Unhealthy
    } else if current_healthy {
        DiagnosisOutcome::Healthy
    } else {
        DiagnosisOutcome::Inconclusive
    };
    let affected_services: Vec<String> = services
        .as_ref()
        .map(|s| {
            s.services
                .iter()
                .filter(|s| s.health == HealthStatus::Unhealthy)
                .map(|s| s.service.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    let selected_failed = updates
        .as_ref()
        .and_then(|p| p.items.iter().find(|u| u.outcome == UpdateOutcome::Failed))
        .cloned();
    let failed_id = selected_failed.as_ref().map(|u| u.id.clone());
    // At most one update detail and one bounded log tail; never retry/fallback.
    let (failed_update, log) = tokio::join!(
        async {
            match failed_id {
                Some(id) => {
                    Some(source("GetUpdate", history::get_update(client, UpdateInput { id })).await)
                }
                None => None,
            }
        },
        source(
            "GetStackLog",
            logs::stack_logs(
                client,
                StackLogsInput {
                    id: id.clone(),
                    services: affected_services.clone(),
                    tail: 40,
                    max_bytes: 4096,
                    timestamps: false
                }
            )
        ),
    );
    let mut failed_update = failed_update.and_then(|r| retain("update_detail", r, &mut gaps));
    // A resource could be changed between list and lookup. Never correlate a
    // different resource's valid update to this stack.
    if failed_update.as_ref().is_some_and(|u| {
        u.summary.target.kind != ResourceKind::Stack
            || u.summary.target.id != id
            || u.summary.outcome != UpdateOutcome::Failed
            || selected_failed.as_ref().is_none_or(|selected| {
                selected.operation != u.summary.operation || selected.start_ts != u.summary.start_ts
            })
    }) {
        failed_update = None;
        gaps.push(EvidenceGap {
            source: "update_detail".into(),
            error: AppError::new(ErrorKind::InvalidResponse, "GetUpdate", None),
        });
    }
    let logs = retain("logs", log, &mut gaps);
    let mut truncated = logs
        .as_ref()
        .is_some_and(|log| log.truncation.lines || log.truncation.bytes)
        || updates
            .as_ref()
            .is_some_and(|p| p.items.len() > MAX_RECORDS || p.next_page.is_some())
        || alerts
            .as_ref()
            .is_some_and(|p| p.items.len() > MAX_RECORDS || p.next_page.is_some());
    if let Some(stack) = &mut stack {
        stack.summary.name = clip(client, &stack.summary.name, 256);
        stack.project_name = stack.project_name.as_deref().map(|s| clip(client, s, 256));
        if let Some(target) = &mut stack.summary.target {
            target.id = clip(client, &target.id, 64);
            target.name = target.name.as_deref().map(|s| clip(client, s, 256));
        }
    }
    if let Some(services) = &mut services {
        // Keep unhealthy services visible even if their lexical names sort last.
        services
            .services
            .sort_by_key(|s| s.health != HealthStatus::Unhealthy);
        if services.services.len() > MAX_SERVICES {
            services.services.truncate(MAX_SERVICES);
            services.truncated = true;
        }
        truncated |= services.truncated;
    }
    if let Some(update) = &mut failed_update {
        if update.stages.len() > MAX_STAGES {
            update.stages.truncate(MAX_STAGES);
            update.stages_truncated = true;
        }
        truncated |= update.stages_truncated;
    }
    let mut summary = match outcome {
        DiagnosisOutcome::Healthy => format!(
            "The stack and {} distinct observed services are healthy.",
            expected_services.as_ref().map_or(0, |names| names.len())
        ),
        DiagnosisOutcome::Unhealthy if !affected_services.is_empty() => {
            format!("Unhealthy services: {}.", affected_services.join(", "))
        }
        DiagnosisOutcome::Unhealthy => "The stack has a confirmed unhealthy workload state.".into(),
        DiagnosisOutcome::Inconclusive => "Current stack health is inconclusive.".into(),
    };
    if outcome != DiagnosisOutcome::Healthy {
        if let Some(update) = &failed_update {
            summary.push_str(&format!(" Recent update {} failed", update.summary.id));
            if let Some(stage) = update.stages.iter().find(|stage| !stage.success) {
                summary.push_str(&format!(" at stage {}", stage.stage));
            }
            summary.push('.');
        }
        if let Some(logs) = &logs {
            let lines: Vec<_> = logs
                .stdout
                .lines()
                .chain(logs.stderr.lines())
                .filter(|line| !line.trim().is_empty())
                .collect();
            if let Some(line) = lines
                .iter()
                .find(|line| line.to_ascii_lowercase().contains("error"))
                .or_else(|| lines.first())
            {
                summary.push_str(" Runtime log excerpt: ");
                summary.push_str(&clip(client, line, 512));
            }
        }
    }
    if !gaps.is_empty() {
        summary.push_str(&format!(
            " Evidence gaps: {}.",
            gaps.iter()
                .map(|gap| gap.source.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let summary = clip(client, &summary, 2048);
    let mut report = StackDiagnosis {
        stack_id: id,
        stack,
        outcome,
        summary,
        affected_services,
        services,
        recent_updates: updates
            .map(|p| p.items.into_iter().take(MAX_RECORDS).collect())
            .unwrap_or_default(),
        recent_alerts: alerts
            .map(|p| p.items.into_iter().take(MAX_RECORDS).collect())
            .unwrap_or_default(),
        failed_update,
        logs,
        gaps,
        limits: DiagnosisLimits {
            max_core_calls: 7,
            max_parallel_core_calls: 4,
            source_timeout_ms: 2000,
            max_wait_ms: 4000,
            max_output_bytes: MAX_OUTPUT_BYTES,
            evidence_truncated: truncated,
        },
    };
    // Account for JSON escaping as well as raw UTF-8. Preserve core identifiers
    // and gaps while shrinking optional projected evidence if needed.
    while serde_json::to_vec(&inventory::projected_value(client, &report))
        .expect("diagnosis serializes")
        .len()
        > MAX_OUTPUT_BYTES
    {
        report.limits.evidence_truncated = true;
        if let Some(services) = report.services.as_mut().filter(|s| !s.services.is_empty()) {
            services.services.pop();
            services.truncated = true;
        } else if let Some(update) = report
            .failed_update
            .as_mut()
            .filter(|u| !u.stages.is_empty())
        {
            update.stages.pop();
            update.stages_truncated = true;
        } else if !report.recent_alerts.is_empty() {
            report.recent_alerts.pop();
        } else if !report.recent_updates.is_empty() {
            report.recent_updates.pop();
        } else {
            return Err(AppError::new(
                ErrorKind::InvalidResponse,
                "stack_diagnose",
                None,
            ));
        }
    }
    Ok(report)
}
