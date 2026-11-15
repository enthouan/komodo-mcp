//! Safe RunSync batch reports, never raw TOML, errors or inferred child success.
use komodo_client::entities::{Operation, ResourceTarget, update::Update};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SyncBatchReport {
    pub stage: &'static str,
    pub phase: &'static str,
    pub success_reported: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SyncObservation {
    pub batches: Vec<SyncBatchReport>,
    pub resource_batches_failed: usize,
    pub deployment_batches_failed: usize,
    pub no_changes_reported: bool,
    pub evidence_incomplete: bool,
    pub raw_output_omitted: bool,
    pub partial_changes_possible: bool,
    pub child_deployment_success_verified: bool,
    pub workload_health_verified: bool,
    pub evidence_limit: &'static str,
}

pub(crate) fn stage(name: &str) -> (&'static str, &'static str) {
    match name {
        "Update Servers" => ("Update Servers", "resources"),
        "Update Swarms" => ("Update Swarms", "resources"),
        "Update Stacks" => ("Update Stacks", "resources"),
        "Update Deployments" => ("Update Deployments", "resources"),
        "Update Builds" => ("Update Builds", "resources"),
        "Update Repos" => ("Update Repos", "resources"),
        "Update Builders" => ("Update Builders", "resources"),
        "Update Alerters" => ("Update Alerters", "resources"),
        "Update Procedures" => ("Update Procedures", "resources"),
        "Update Actions" => ("Update Actions", "resources"),
        "Update ResourceSyncs" => ("Update ResourceSyncs", "resources"),
        "Update Variables" => ("Update Variables", "variables"),
        "Update UserGroups" => ("Update UserGroups", "user_groups"),
        "Sync Deploy" => ("Sync Deploy", "deployments"),
        "No Changes" => ("No Changes", "preparation"),
        "Task Error" => ("Task Error", "preparation"),
        "Spawn Error" => ("Spawn Error", "preparation"),
        "refresh sync" => ("Post-sync refresh", "cache"),
        _ => ("Sync preparation", "preparation"),
    }
}
pub(crate) fn project(update: &Update) -> Option<SyncObservation> {
    if update.operation != Operation::RunSync
        || !matches!(update.target, ResourceTarget::ResourceSync(_))
    {
        return None;
    }
    let mut result = SyncObservation {
        batches: vec![],
        resource_batches_failed: 0,
        deployment_batches_failed: 0,
        no_changes_reported: false,
        evidence_incomplete: update.logs.len() > 256,
        raw_output_omitted: true,
        partial_changes_possible: true,
        child_deployment_success_verified: false,
        workload_health_verified: false,
        evidence_limit: "These are bounded parent-update batch reports, not per-resource success or caller attribution. A failed batch can contain successful changes. Core 2.3.2's Sync Deploy checks the child Result but not an Ok update's success flag, so parent success does not verify child deployment success or health. Core records no reliable RunSync selector identity or child-update association. Inspect resources, workload health and explicitly known child IDs independently; do not select or replay recovery candidates automatically.",
    };
    for log in update.logs.iter().take(256) {
        let (name, phase) = stage(&log.stage);
        result.no_changes_reported |= log.stage == "No Changes" && log.success;
        if !log.success {
            if matches!(phase, "resources" | "variables" | "user_groups") {
                result.resource_batches_failed += 1;
            }
            if phase == "deployments" {
                result.deployment_batches_failed += 1;
            }
        }
        if result.batches.len() < 32 {
            result.batches.push(SyncBatchReport {
                stage: name,
                phase,
                success_reported: log.success,
            });
        } else {
            result.evidence_incomplete = true;
        }
    }
    Some(result)
}
