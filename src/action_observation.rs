//! Safe metadata from the pinned Action command log. Arbitrary output can
//! contain unknown argument values or Core's temporary admin API credentials;
//! suppress it consistently, including read-only observation after a restart.
use komodo_client::entities::{Operation, update::Update};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ActionObservation {
    pub command_stage_found: bool,
    pub command_success: Option<bool>,
    pub stdout_present: bool,
    pub stderr_present: bool,
    pub cancellation_reported: bool,
    pub output_omitted: bool,
    pub evidence_limit: &'static str,
}
pub(crate) fn project(u: &Update) -> Option<ActionObservation> {
    if u.operation != Operation::RunAction {
        return None;
    }
    let mut logs = u.logs.iter().filter(|l| l.stage == "Execute Action");
    let log = logs.next().filter(|_| logs.next().is_none());
    Some(ActionObservation {
        command_stage_found: log.is_some(),
        command_success: log.map(|l| l.success),
        stdout_present: u.logs.iter().any(|l| !l.stdout.is_empty()),
        stderr_present: u.logs.iter().any(|l| !l.stderr.is_empty()),
        cancellation_reported: log.is_some_and(|l| {
            !l.success && l.stderr.trim() == "Command cancelled (process group killed)"
        }),
        output_omitted: u
            .logs
            .iter()
            .any(|l| !l.stdout.is_empty() || !l.stderr.is_empty()),
        evidence_limit: "Arbitrary Action stdout/stderr, including source/default/per-run arguments and temporary administrator credentials, is omitted rather than treated as safe text. Structured Core update status/success and command-stage metadata remain available. The exact Core process-group cancellation marker is reported, but scripts can print the same text; it is not independent cancellation proof. A successful Action does not establish child-operation results or workload health. Missing/duplicate command stages provide no command result.",
    })
}
