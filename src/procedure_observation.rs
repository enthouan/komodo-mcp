//! Narrow parent-log reports. Never expose Execution Debug/argument payloads.
use crate::{client::CoreClient, configuration::bounded_text};
use komodo_client::entities::{Operation, ResourceTarget, update::Update};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StageEvent {
    pub name: String,
    pub event: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExecutionEvent {
    pub operation: String,
    pub event: &'static str,
    pub stage_event_index: Option<usize>,
    pub terminal_success: Option<bool>,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProcedureObservation {
    pub stages: Vec<StageEvent>,
    pub executions: Vec<ExecutionEvent>,
    pub reported_child_update_ids: Vec<String>,
    pub evidence_incomplete: bool,
    pub evidence_ambiguous: bool,
    pub cancellation_reported: bool,
    pub permission_denial_reported: bool,
    pub failed_stage_reported: Option<String>,
    pub child_results_complete: bool,
    pub evidence_limit: &'static str,
}
fn operation(s: &str) -> Option<String> {
    let (kind, _) = s.split_once('(')?;
    // Only SDK variant names, never Debug parameters or free-form stdout.
    if kind.is_empty() || kind.len() > 40 || !kind.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    // Recognize only pinned execution variant names; parameters remain private.
    let known = [
        "None",
        "Sleep",
        "RunBuild",
        "CancelBuild",
        "CloneRepo",
        "PullRepo",
        "BuildRepo",
        "CancelRepoBuild",
        "RunProcedure",
        "CancelProcedure",
        "RunAction",
        "CancelAction",
        "Deploy",
        "PullDeployment",
        "StartDeployment",
        "RestartDeployment",
        "PauseDeployment",
        "UnpauseDeployment",
        "StopDeployment",
        "DestroyDeployment",
        "DeployStack",
        "DeployStackIfChanged",
        "PullStack",
        "StartStack",
        "RestartStack",
        "PauseStack",
        "UnpauseStack",
        "StopStack",
        "DestroyStack",
        "RunStackService",
        "RunSync",
        "CommitSync",
        "SendAlert",
        "TestAlerter",
        "GlobalAutoUpdate",
        "ClearRepoCache",
        "BackupCoreDatabase",
        "RotateAllServerKeys",
        "RotateCoreKeys",
        "StartContainer",
        "RestartContainer",
        "PauseContainer",
        "UnpauseContainer",
        "StopContainer",
        "DestroyContainer",
        "StartAllContainers",
        "RestartAllContainers",
        "PauseAllContainers",
        "UnpauseAllContainers",
        "StopAllContainers",
        "PruneContainers",
        "DeleteNetwork",
        "PruneNetworks",
        "DeleteImage",
        "PruneImages",
        "DeleteVolume",
        "PruneVolumes",
        "PruneDockerBuilders",
        "PruneBuildx",
        "PruneSystem",
        "RemoveSwarmNodes",
        "UpdateSwarmNode",
        "RemoveSwarmStacks",
        "RemoveSwarmServices",
        "CreateSwarmConfig",
        "RotateSwarmConfig",
        "RemoveSwarmConfigs",
        "CreateSwarmSecret",
        "RotateSwarmSecret",
        "RemoveSwarmSecrets",
    ];
    known.contains(&kind).then(|| kind.into())
}
fn tail(text: &str) -> (&str, bool) {
    let mut at = text.len().saturating_sub(65536);
    while !text.is_char_boundary(at) {
        at += 1;
    }
    (&text[at..], at > 0)
}
fn core_text(line: &str) -> Option<String> {
    let mut text = crate::build_observation::plain(line)?;
    // Core's Procedure formatter uses these literal HTML spans, unlike Docker
    // and Git terminal output. Strip only the pinned formatter's known tags.
    for tag in [
        "<span style=\"color: var(--mantine-color-dimmed)\">",
        "<span style=\"font-weight: bolder\">",
        "<span style=\"color: var(--mantine-color-red-6)\">",
        "<span style=\"color: var(--mantine-color-green-6)\">",
        "<span style=\"color: var(--mantine-color-blue-6)\">",
        "</span>",
    ] {
        text = text.replace(tag, "");
    }
    Some(text)
}
fn core_message(line: &str) -> Option<String> {
    let text = core_text(line)?;
    let text = text.trim();
    if let Some(message) = text.strip_prefix("ERROR: ") {
        return Some(message.into());
    }
    let (number, message) = text.split_once(": ")?;
    if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
        Some(message.strip_prefix("ERROR: ").unwrap_or(message).into())
    } else {
        None
    }
}
pub(crate) fn project(client: &CoreClient, u: &Update) -> Option<ProcedureObservation> {
    if u.operation != Operation::RunProcedure || !matches!(u.target, ResourceTarget::Procedure(_)) {
        return None;
    }
    let mut result = ProcedureObservation {
        stages: vec![],
        executions: vec![],
        reported_child_update_ids: vec![],
        evidence_incomplete: false,
        evidence_ambiguous: false,
        cancellation_reported: false,
        permission_denial_reported: false,
        failed_stage_reported: None,
        child_results_complete: false,
        evidence_limit: "These are bounded parent-update log reports, not a snapshot of the current saved definition. Executions in a stage are parallel. Core prints Finished execution even when that child fails, so terminal_success remains unknown. Core waits for all children but reports only the first collected failure; missing child results are not success. Child IDs are explicit parent-error URL reports, with unverified association; inspect each known ID separately. No caller attribution, rollback or workload-health claim follows these reports.",
    };
    let mut logs = u.logs.iter().filter(|l| l.stage == "Execute procedure");
    if let Some(log) = logs.next() {
        if logs.next().is_some() {
            result.evidence_ambiguous = true;
        } else {
            let (stdout, truncated) = tail(&log.stdout);
            result.evidence_incomplete |= truncated;
            let mut current_stage = None;
            for line in stdout.lines() {
                let Some(line) = core_text(line) else {
                    result.evidence_incomplete = true;
                    current_stage = None;
                    continue;
                };
                let line = line.trim();
                let stage = if let Some(s) = line
                    .strip_prefix("INFO: Executing stage: '")
                    .and_then(|s| s.strip_suffix('\''))
                {
                    Some((s, "started"))
                } else if let Some(s) = line
                    .strip_prefix("INFO: Finished stage '")
                    .and_then(|s| s.rsplit_once("' execution in ").map(|(s, _)| s))
                {
                    Some((s, "finished_reported"))
                } else if let Some(s) = line
                    .strip_prefix("ERROR: Cancelled before stage: '")
                    .and_then(|s| s.strip_suffix('\''))
                {
                    result.cancellation_reported = true;
                    Some((s, "cancelled_before"))
                } else {
                    None
                };
                if let Some((name, event)) = stage {
                    if result.stages.len() >= 24 {
                        result.evidence_incomplete = true;
                        current_stage = None;
                        continue;
                    }
                    result.stages.push(StageEvent {
                        name: bounded_text(client, &crate::logs::redact_log(client, name), 128),
                        event,
                    });
                    current_stage = if event == "started" {
                        Some(result.stages.len() - 1)
                    } else {
                        None
                    };
                    continue;
                }
                let execution = if let Some(s) = line.strip_prefix("INFO: Executing: ") {
                    Some((s, "started"))
                } else {
                    line.strip_prefix("INFO: Finished execution in ")
                        .and_then(|s| s.split_once(": ").map(|(_, s)| (s, "finished_reported")))
                };
                if let Some((text, event)) = execution
                    && let Some(operation) = operation(text)
                {
                    if result.executions.len() < 32 {
                        result.executions.push(ExecutionEvent {
                            operation,
                            event,
                            stage_event_index: current_stage,
                            terminal_success: None,
                        });
                    } else {
                        result.evidence_incomplete = true;
                    }
                }
            }
        }
    }
    for log in u
        .logs
        .iter()
        .filter(|l| matches!(l.stage.as_str(), "Execution error" | "Task Error"))
    {
        let (stderr, truncated) = tail(&log.stderr);
        result.evidence_incomplete |= truncated;
        for line in stderr.lines() {
            let Some(message) = core_message(line) else {
                continue;
            };
            result.cancellation_reported |= message == "Procedure cancelled before completion";
            result.permission_denial_reported |=
                message.starts_with("User does not have required permissions");
            if let Some(name) = message
                .strip_prefix("Failed stage '")
                .and_then(|s| s.rsplit_once("' execution after ").map(|(name, _)| name))
            {
                result.failed_stage_reported = Some(bounded_text(
                    client,
                    &crate::logs::redact_log(client, name),
                    128,
                ));
            }
            let Some(suffix) = message.strip_prefix("Execution not successful, see update: '")
            else {
                continue;
            };
            let Some((url, _)) = suffix.split_once('\'') else {
                continue;
            };
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let Some((_, id)) = url.rsplit_once("/updates/") else {
                continue;
            };
            if !crate::deletion::object_id(id) || id.eq_ignore_ascii_case(&u.id) {
                continue;
            }
            let id = id.to_ascii_lowercase();
            if result.reported_child_update_ids.contains(&id) {
                continue;
            }
            if result.reported_child_update_ids.len() < 8 {
                result.reported_child_update_ids.push(id);
            } else {
                result.evidence_incomplete = true;
            }
        }
    }
    Some(result)
}
