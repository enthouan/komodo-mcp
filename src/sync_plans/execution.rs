//! One consumed review authorization, bounded fresh preflight, one RunSync.
use super::*;
use crate::{
    history::UpdateStatus,
    operations::{
        self, ConfiguredEffects, OperationResource, RequestedScope, SubmissionReceipt,
        SubmissionState, WorkloadKind, WorkloadOperation,
    },
};
use komodo_client::{
    api::execute::RunSync,
    entities::{Operation, ResourceTargetVariant},
};

const APPLY: &str = "ApplyResourceSync";
pub const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(45);

fn default_wait() -> u8 {
    3
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    #[schemars(length(min = 1, max = 128))]
    pub review_reference: String,
    pub binding: ApplyBinding,
    #[serde(default = "default_wait")]
    #[schemars(range(min = 1, max = 10))]
    pub wait_timeout_seconds: u8,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct PreflightSummary {
    pub refresh_requests_submitted: u8,
    pub refresh_outcome: &'static str,
    pub error: Option<AppError>,
}
#[derive(Serialize, JsonSchema)]
pub struct ApplyResult {
    pub binding: ApplyBinding,
    pub outcome: &'static str,
    pub review_consumed: bool,
    pub preflight: PreflightSummary,
    pub run_sync_requests_submitted: u8,
    pub reviewed_plan: PendingOutput,
    pub rejection: Option<AppError>,
    pub receipt: Option<SubmissionReceipt>,
    pub observation: Option<super::follow::FollowResult>,
    pub post_sync_pending: Option<PostPending>,
    pub post_sync_read_error: Option<AppError>,
    pub observed_resources: Vec<super::observe::ResourceObservation>,
    pub resource_observation_incomplete: bool,
    pub workload_health_verified: bool,
    pub automatic_resubmission: bool,
    pub warnings: Vec<&'static str>,
}
#[derive(Serialize, JsonSchema)]
pub struct PostPending {
    pub observation: &'static str,
    pub cached_status: &'static str,
    pub resource_changes: Option<usize>,
    pub variable_changes: Option<usize>,
    pub user_group_changes: Option<usize>,
    pub deployments: Option<usize>,
    pub errors: PlanErrors,
    pub source: SourceEvidence,
}

// No Clone/Debug/Serialize and no public constructor. Only the successful
// private fresh-plan comparison below can produce this transport authority.
pub(crate) struct AuthorizedSync {
    request: RunSync,
}
impl AuthorizedSync {
    pub(crate) fn into_request(self) -> RunSync {
        self.request
    }
}
fn core_kind(kind: Kind) -> ResourceTargetVariant {
    match kind {
        Kind::Server => ResourceTargetVariant::Server,
        Kind::Swarm => ResourceTargetVariant::Swarm,
        Kind::Stack => ResourceTargetVariant::Stack,
        Kind::Deployment => ResourceTargetVariant::Deployment,
        Kind::Build => ResourceTargetVariant::Build,
        Kind::Repo => ResourceTargetVariant::Repo,
        Kind::Builder => ResourceTargetVariant::Builder,
        Kind::Alerter => ResourceTargetVariant::Alerter,
        Kind::Procedure => ResourceTargetVariant::Procedure,
        Kind::Action => ResourceTargetVariant::Action,
        Kind::ResourceSync => ResourceTargetVariant::ResourceSync,
    }
}
pub(crate) fn validate_scope(scope: &ExecutionScope) -> Result<(), AppError> {
    if let ExecutionScope::Resources { targets, .. } = scope {
        if targets.is_empty() || targets.len() > MAX_SELECTORS {
            return Err(AppError::arguments(
                APPLY,
                "Retain the exact reviewed 1–16 typed targets; an empty selection never becomes whole-sync",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for t in targets {
            match t {
                ResolvedTarget::Existing { id, name } => {
                    if !valid_id(id)
                        || !name_ok(name)
                        || !ids.insert(id.to_ascii_lowercase())
                        || !names.insert(name)
                    {
                        return Err(AppError::arguments(
                            APPLY,
                            "Retain unique exact existing IDs and reviewed names",
                        ));
                    }
                }
                ResolvedTarget::Create { name } => {
                    if !name_ok(name) || valid_id(name) || !names.insert(name) {
                        return Err(AppError::arguments(
                            APPLY,
                            "New targets require unique exact reviewed names, never invented or ObjectId-shaped IDs",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
fn request(binding: &ApplyBinding) -> Result<RunSync, AppError> {
    if !valid_id(&binding.sync_id) {
        return Err(AppError::arguments(
            APPLY,
            "Use the exact reviewed stable sync ID",
        ));
    }
    validate_scope(&binding.selection)?;
    let (resource_type, resources) = match &binding.selection {
        ExecutionScope::WholeSync {} => (None, None),
        ExecutionScope::ResourceType { resource_type } => (Some(core_kind(*resource_type)), None),
        ExecutionScope::Resources {
            resource_type,
            targets,
        } => (
            Some(core_kind(*resource_type)),
            Some(
                targets
                    .iter()
                    .map(|t| match t {
                        ResolvedTarget::Existing { id, .. } => id.clone(),
                        ResolvedTarget::Create { name } => name.clone(),
                    })
                    .collect(),
            ),
        ),
    };
    Ok(RunSync {
        sync: binding.sync_id.clone(),
        resource_type,
        resources,
    })
}
pub(crate) fn validate_request(r: &RunSync) -> Result<(), AppError> {
    if !valid_id(&r.sync) || r.resource_type == Some(ResourceTargetVariant::System) {
        return Err(AppError::arguments(
            "RunSync",
            "Use one reviewed stable sync and supported explicit selector form",
        ));
    }
    if let Some(resources) = &r.resources {
        let mut unique = BTreeSet::new();
        if r.resource_type.is_none()
            || resources.is_empty()
            || resources.len() > MAX_SELECTORS
            || resources.iter().any(|s| !name_ok(s) || !unique.insert(s))
        {
            return Err(AppError::arguments(
                "RunSync",
                "Never drop invalid selectors or convert an empty/untyped selection into whole-sync",
            ));
        }
    }
    Ok(())
}
fn selection(scope: &ExecutionScope) -> Selection {
    match scope {
        ExecutionScope::WholeSync {} => Selection::WholeSync {},
        ExecutionScope::ResourceType { resource_type } => Selection::ResourceType {
            resource_type: *resource_type,
        },
        ExecutionScope::Resources {
            resource_type,
            targets,
        } => Selection::Resources {
            resource_type: *resource_type,
            targets: targets
                .iter()
                .map(|t| match t {
                    ResolvedTarget::Existing { id, .. } => DetailInput {
                        id: Some(id.clone()),
                        name: None,
                    },
                    ResolvedTarget::Create { name } => DetailInput {
                        id: None,
                        name: Some(name.clone()),
                    },
                })
                .collect(),
        },
    }
}
fn stale() -> AppError {
    AppError {
        correction: Some(
            "Observed definition, references, source or scoped plan changed or coverage is incomplete. No RunSync was submitted; inspect and obtain a new review. A consumed reference is never reusable.",
        ),
        ..AppError::new(ErrorKind::StalePreview, APPLY, None)
    }
}
async fn observed(
    client: &CoreClient,
    binding: &ApplyBinding,
) -> Result<(PendingOutput, Value), AppError> {
    let snap = load(
        client,
        DetailInput {
            id: Some(binding.sync_id.clone()),
            name: None,
        },
    )
    .await?;
    let (plan, fresh_binding, baseline) =
        audit::prepare(client, snap, selection(&binding.selection)).await?;
    if !plan.apply_coverage_complete
        || fresh_binding
            .as_ref()
            .and_then(|b| serde_json::to_value(b).ok())
            != Some(serde_json::to_value(binding).expect("binding"))
    {
        return Err(stale());
    }
    Ok((plan, baseline))
}
fn configured() -> ConfiguredEffects {
    ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: false,
        saved_command_wrapper: false,
        broader_effects_possible: true,
        compose_dependencies: None,
        repository: None,
        build: None,
        procedure: None,
        action: None,
    }
}
pub async fn apply(
    client: &CoreClient,
    input: ApplyInput,
    cancelled: impl std::future::Future<Output = ()>,
) -> Result<ApplyResult, AppError> {
    common::require_edit(client)?;
    operations::require_operate(client, "RunSync")?;
    if input.review_reference.is_empty()
        || input.review_reference.len() > 128
        || input.review_reference.chars().any(char::is_control)
    {
        return Err(AppError::new(ErrorKind::InvalidPreview, APPLY, None));
    }
    if !(1..=10).contains(&input.wait_timeout_seconds) {
        return Err(AppError::arguments(
            APPLY,
            "Use wait_timeout_seconds from 1 through 10",
        ));
    }
    let entry: Preview<PendingOutput> = client.previews().take_for(
        &input.review_reference,
        &common::bound_input("resource_sync_apply", &input.binding),
    )?;
    let run = request(&input.binding)?;
    if !entry.result.apply_coverage_complete
        || entry.prepared != serde_json::to_value(&input.binding).expect("binding")
    {
        return Err(AppError::new(ErrorKind::InvalidPreview, APPLY, None));
    }
    let mut result = ApplyResult {
        binding: input.binding.clone(),
        outcome: "not_submitted",
        review_consumed: true,
        preflight: PreflightSummary {
            refresh_requests_submitted: 0,
            refresh_outcome: "not_submitted",
            error: None,
        },
        run_sync_requests_submitted: 0,
        reviewed_plan: entry.result,
        rejection: None,
        receipt: None,
        observation: None,
        post_sync_pending: None,
        post_sync_read_error: None,
        observed_resources: vec![],
        resource_observation_incomplete: true,
        workload_health_verified: false,
        automatic_resubmission: false,
        warnings: vec![
            "RunSync applies desired state through Core's built-in administrator sync identity. Direct caller permissions on each affected resource do not isolate this delegated authority. Managed mode OR delete enables deletion; empty resource tags are unrestricted, and variables/groups are only included in whole-sync scope.",
            "One explicit preflight refresh can fetch/clone Core Git caches, read sources and store/alert on pending state. RunSync can perform its own internal refresh and configured automatic effects. MCP submits at most one refresh and one RunSync for this authorization; it never writes authoritative Git/host sources.",
            "Fresh private equality does not freeze Core or external sources: RunSync rereads sources without an expected-revision parameter. A final-read/submission race remains; no atomic compare-and-swap, exactly-once execution, rollback or remote cancellation is promised.",
            "The review reference is consumed before preflight. Lost acknowledgements stay unknown; retain target/scope/window and explicitly known IDs. Read-only recovery never chooses an unconfirmed candidate or resubmits execution.",
            "A successful parent sync or Sync Deploy batch does not verify child deployment success or workload health. Independent resource observations and cached pending state are separate evidence; verify workloads with health/diagnosis tools.",
        ],
    };
    let bounded_client = client.bounded_evidence();
    let mut before = Vec::new();
    let preflight = tokio::time::timeout(PREFLIGHT_TIMEOUT, async {
        if bounded_client.detect_version().await?.to_string() != "2.3.2" {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, APPLY, None));
        }
        let (initial, baseline) = observed(&bounded_client, &input.binding).await?;
        if baseline != entry.baseline {
            return Err(stale());
        }
        before = super::observe::before(&bounded_client, &initial).await;
        result.preflight.refresh_requests_submitted = 1;
        result.preflight.refresh_outcome = "submission_unknown";
        let raw = bounded_client
            .refresh_sync_pending(&input.binding.sync_id)
            .await
            .inspect_err(|error| {
                if matches!(
                    error.kind,
                    ErrorKind::PermissionDenied | ErrorKind::Authentication
                ) {
                    result.preflight.refresh_outcome = "rejected";
                }
            })?;
        let identity = SyncIdentity {
            id: input.binding.sync_id.clone(),
            name: entry.baseline["definition"]["name"]
                .as_str()
                .ok_or_else(invalid)?
                .into(),
        };
        let refreshed = snapshot(&bounded_client, raw, &identity, "apply_preflight_refresh")?;
        result.preflight.refresh_outcome = if refreshed.output.cached_status == "errors" {
            "refreshed_with_errors"
        } else if !refreshed.complete {
            "refreshed_incomplete"
        } else {
            "refreshed"
        };
        if !refreshed.complete
            || canonical(
                &refreshed,
                entry.baseline["references"].clone(),
                entry.baseline["metadata"].clone(),
            ) != entry.baseline
        {
            return Err(stale());
        }
        let (_, fresh) = observed(&bounded_client, &input.binding).await?;
        if fresh != entry.baseline {
            return Err(stale());
        }
        Ok::<_, AppError>(())
    })
    .await;
    let rejected = match preflight {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(_) => Some(AppError::new(ErrorKind::Timeout, APPLY, None)),
    };
    if let Some(error) = rejected {
        result.preflight.error = Some(error.clone());
        result.rejection = Some(error);
        return Ok(bound_result(client, result));
    }
    let authorized = AuthorizedSync { request: run };
    let receipt = operations::submit_future(
        client,
        OperationResource {
            kind: WorkloadKind::ResourceSync,
            id: input.binding.sync_id.clone(),
            name: result.reviewed_plan.sync.name.clone(),
        },
        RequestedScope::ResourceSync {
            selection: input.binding.selection.clone(),
        },
        WorkloadOperation::Run,
        Operation::RunSync,
        configured(),
        client.execute_reviewed_sync(authorized),
    )
    .await;
    let receipt = match receipt {
        Ok(r) => r,
        Err(error) => {
            result.rejection = Some(error);
            return Ok(bound_result(client, result));
        }
    };
    result.run_sync_requests_submitted = 1;
    result.outcome = match receipt.submission {
        SubmissionState::Accepted => "accepted",
        SubmissionState::Rejected => "rejected",
        SubmissionState::Unknown => "submission_unknown",
    };
    if let Some(id) = receipt.update_id.clone() {
        let observation = super::follow::follow(
            client,
            super::follow::FollowInput {
                binding: input.binding.clone(),
                update_id: id,
                timeout_seconds: input.wait_timeout_seconds,
            },
            cancelled,
        )
        .await;
        match observation {
            Ok(o) => result.observation = Some(o),
            Err(e) => result.post_sync_read_error = Some(e),
        }
    }
    let terminal = result
        .observation
        .as_ref()
        .and_then(|o| o.wait.latest_update.as_ref())
        .or(receipt.latest_update.as_ref())
        .is_some_and(|u| u.summary.status == UpdateStatus::Complete);
    result.receipt = Some(receipt);
    if terminal {
        let (observations, incomplete) = super::observe::after(&bounded_client, before).await;
        result.observed_resources = observations;
        result.resource_observation_incomplete = incomplete;
        match tokio::time::timeout(
            Duration::from_secs(2),
            load(
                &bounded_client,
                DetailInput {
                    id: Some(input.binding.sync_id),
                    name: None,
                },
            ),
        )
        .await
        {
            Ok(Ok(s)) => {
                let p = s.output;
                result.post_sync_pending = Some(PostPending {
                    observation: "cached_after_terminal_update_not_execution_proof",
                    cached_status: p.cached_status,
                    resource_changes: p.resource_changes.reported_total,
                    variable_changes: p.variable_changes.reported_total,
                    user_group_changes: p.user_group_changes.reported_total,
                    deployments: p.pending_deployments.reported_total,
                    errors: p.errors,
                    source: p.source,
                });
            }
            Ok(Err(e)) => result.post_sync_read_error = Some(e),
            Err(_) => {
                result.post_sync_read_error =
                    Some(AppError::new(ErrorKind::Timeout, "GetResourceSync", None))
            }
        }
    }
    Ok(bound_result(client, result))
}
fn bound_result(client: &CoreClient, mut result: ApplyResult) -> ApplyResult {
    // The reviewed public plan was bounded before its reference was minted;
    // receipts retain their own 64KiB bound and observations are capped at eight.
    // Preserve identity/counts/known IDs even if a future optional projection grows.
    while inventory::projected_value(client, &result)
        .to_string()
        .len()
        > 512 * 1024
    {
        if result.observed_resources.pop().is_some() {
            result.resource_observation_incomplete = true;
            continue;
        }
        if result.post_sync_pending.take().is_some() {
            continue;
        }
        if let Some(update) = result
            .observation
            .as_mut()
            .and_then(|o| o.wait.latest_update.as_mut())
            && update.stages.pop().is_some()
        {
            update.stages_truncated = true;
            continue;
        }
        break;
    }
    result
}
