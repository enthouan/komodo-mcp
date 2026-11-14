//! Shared resource-level cancellation checks. Core cannot enforce a run-ID CAS.
use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    history::{self, HistoryTarget, ResourceKind, UpdateDetail, UpdateInput, UpdateStatus},
    operations::{
        self, CancellationOutcome, CancellationResult, CancellationStop, CancellationTargeting,
        ConfiguredEffects, OperationResource, RequestedScope, SubmissionState, WorkloadKind,
        WorkloadOperation,
    },
};
use komodo_client::{
    api::execute::KomodoExecuteRequest,
    entities::{Operation, update::Update},
};
use serde::Serialize;
use std::future::Future;

pub(crate) struct Context {
    pub resource: OperationResource,
    pub original_update_id: String,
    pub scope: RequestedScope,
    pub run_operation: Operation,
    pub cancel_operation: Operation,
    pub effects: ConfiguredEffects,
    pub limit: &'static str,
}
struct Active {
    id: Option<String>,
    complete: bool,
    running: bool,
    ambiguous: bool,
}
fn operation(c: &Context) -> &'static str {
    match c.cancel_operation {
        Operation::CancelBuild => "CancelBuild",
        Operation::CancelProcedure => "CancelProcedure",
        Operation::CancelAction => "CancelAction",
        _ => "cancel_operation",
    }
}
fn history_target(r: &OperationResource) -> HistoryTarget {
    HistoryTarget {
        kind: match r.kind {
            WorkloadKind::Build => ResourceKind::Build,
            WorkloadKind::Procedure => ResourceKind::Procedure,
            WorkloadKind::Action => ResourceKind::Action,
            WorkloadKind::Stack => ResourceKind::Stack,
            WorkloadKind::Deployment => ResourceKind::Deployment,
            WorkloadKind::Repo => ResourceKind::Repo,
        },
        id: r.id.clone(),
    }
}
async fn active<F, Fut>(client: &CoreClient, c: &Context, running: &F) -> Result<Active, AppError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<bool, AppError>>,
{
    let page = history::active_runs(client, history_target(&c.resource), c.run_operation).await?;
    let running = running().await?;
    let ids = page
        .items
        .iter()
        .map(|u| u.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    Ok(Active {
        id: if ids.len() == 1 {
            ids.into_iter().next()
        } else {
            None
        },
        complete: page.next_page.is_none(),
        running,
        ambiguous: page.items.len() > 1,
    })
}
fn validate(u: &UpdateDetail, c: &Context) -> Result<(), AppError> {
    if u.summary.target != history_target(&c.resource)
        || u.summary.operation != c.run_operation.to_string()
    {
        return Err(AppError::arguments(
            operation(c),
            "The original update must belong to this exact selected resource and run operation. No cancellation was submitted.",
        ));
    }
    Ok(())
}
fn stop(a: &Active, u: &UpdateDetail) -> Option<CancellationStop> {
    if !a.complete {
        Some(CancellationStop::IncompleteActiveEvidence)
    } else if a.ambiguous {
        Some(CancellationStop::AmbiguousActiveRuns)
    } else if a.id.as_ref().is_some_and(|id| id != &u.summary.id) {
        Some(CancellationStop::ReplacementRun)
    } else if u.summary.status == UpdateStatus::Complete {
        Some(CancellationStop::OriginalComplete)
    } else if !a.running {
        Some(CancellationStop::NotActive)
    } else if a.id.is_none() {
        Some(CancellationStop::IncompleteActiveEvidence)
    } else {
        None
    }
}
pub(crate) async fn resource<T, F, Fut>(
    client: &CoreClient,
    mut c: Context,
    request: T,
    running: F,
) -> Result<CancellationResult, AppError>
where
    T: Serialize + KomodoExecuteRequest<Response = Update>,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<bool, AppError>>,
{
    operations::require_operate(client, operation(&c))?;
    let op = operation(&c);
    c.original_update_id = UpdateInput {
        id: c.original_update_id,
    }
    .validate(op)?;
    let bounded = client.bounded_evidence();
    let original = history::get_update(
        &bounded,
        UpdateInput {
            id: c.original_update_id.clone(),
        },
    )
    .await?;
    validate(&original, &c)?;
    let first = active(&bounded, &c, &running).await?;
    let mut result = CancellationResult {
        resource: c.resource.clone(),
        original_update_id: c.original_update_id.clone(),
        targeting: CancellationTargeting::Resource,
        outcome: CancellationOutcome::NotSubmitted,
        not_submitted_reason: stop(&first, &original),
        active_update_id: first.id.clone(),
        active_evidence_complete: first.complete,
        cancellation_receipt: None,
        original_update: Some(original),
        original_observation_error: None,
        original_terminal_confirmed: false,
        replacement_run_observed: first
            .id
            .as_ref()
            .is_some_and(|id| id != &c.original_update_id),
        remote_cancellation_requested: false,
        automatic_resubmission: false,
        targeting_limit: c.limit,
    };
    if result.not_submitted_reason.is_some() {
        return Ok(result.bound(client));
    }
    let original = history::get_update(
        &bounded,
        UpdateInput {
            id: c.original_update_id.clone(),
        },
    )
    .await?;
    validate(&original, &c)?;
    let second = active(&bounded, &c, &running).await?;
    result.not_submitted_reason = stop(&second, &original);
    result.active_update_id = second.id.clone();
    result.active_evidence_complete = second.complete;
    result.replacement_run_observed |= second
        .id
        .as_ref()
        .is_some_and(|id| id != &c.original_update_id);
    result.original_update = Some(original);
    if result.not_submitted_reason.is_some() {
        return Ok(result.bound(client));
    }
    let receipt = operations::submit(
        client,
        c.resource.clone(),
        c.scope.clone(),
        WorkloadOperation::Cancel,
        c.cancel_operation,
        c.effects.clone(),
        request,
    )
    .await?;
    result.remote_cancellation_requested = true;
    result.outcome = match receipt.submission {
        SubmissionState::Accepted => CancellationOutcome::Acknowledged,
        SubmissionState::Rejected => CancellationOutcome::Rejected,
        SubmissionState::Unknown => CancellationOutcome::Unknown,
    };
    result.cancellation_receipt = Some(Box::new(receipt));
    match tokio::time::timeout(
        operations::OBSERVATION_TIMEOUT,
        history::get_update(
            &bounded,
            UpdateInput {
                id: c.original_update_id.clone(),
            },
        ),
    )
    .await
    {
        Ok(Ok(u)) => match validate(&u, &c) {
            Ok(()) => result.original_update = Some(u),
            Err(e) => result.original_observation_error = Some(e),
        },
        Ok(Err(e)) => result.original_observation_error = Some(e),
        Err(_) => {
            result.original_observation_error =
                Some(AppError::new(ErrorKind::Timeout, "GetUpdate", None))
        }
    }
    match tokio::time::timeout(
        operations::OBSERVATION_TIMEOUT,
        active(&bounded, &c, &running),
    )
    .await
    {
        Ok(Ok(a)) => {
            result.replacement_run_observed |=
                a.id.as_ref().is_some_and(|id| id != &c.original_update_id);
            result.active_update_id = a.id;
            result.active_evidence_complete = a.complete;
        }
        Ok(Err(e)) => {
            result.active_evidence_complete = false;
            if result.original_observation_error.is_none() {
                result.original_observation_error = Some(e);
            }
        }
        Err(_) => {
            result.active_evidence_complete = false;
            if result.original_observation_error.is_none() {
                result.original_observation_error =
                    Some(AppError::new(ErrorKind::Timeout, "ListUpdates", None));
            }
        }
    }
    Ok(result.bound(client))
}

/// Action cancellation can target an exact update while other runs overlap.
/// Core does not check this association itself, so the shared execute transport
/// also revalidates it immediately before submission. No list/first-run fallback.
pub(crate) async fn update<T>(
    client: &CoreClient,
    c: Context,
    request: T,
) -> Result<CancellationResult, AppError>
where
    T: Serialize + KomodoExecuteRequest<Response = Update>,
{
    let op = operation(&c);
    operations::require_operate(client, op)?;
    let bounded = client.bounded_evidence();
    let original = history::get_update(
        &bounded,
        UpdateInput {
            id: c.original_update_id.clone(),
        },
    )
    .await?;
    validate(&original, &c)?;
    let complete = original.summary.status == UpdateStatus::Complete;
    let mut result = CancellationResult {
        resource: c.resource.clone(),
        original_update_id: c.original_update_id.clone(),
        targeting: CancellationTargeting::Update,
        outcome: CancellationOutcome::NotSubmitted,
        not_submitted_reason: complete.then_some(CancellationStop::OriginalComplete),
        active_update_id: (!complete).then(|| c.original_update_id.clone()),
        active_evidence_complete: true,
        cancellation_receipt: None,
        original_update: Some(original),
        original_observation_error: None,
        original_terminal_confirmed: complete,
        replacement_run_observed: false,
        remote_cancellation_requested: false,
        automatic_resubmission: false,
        targeting_limit: c.limit,
    };
    if complete {
        return Ok(result.bound(client));
    }
    // The transport repeats the exact target/operation/terminal checks. An outer
    // submit error means no POST, and must retain the already known original ID.
    let receipt = match operations::submit(
        client,
        c.resource.clone(),
        c.scope.clone(),
        WorkloadOperation::Cancel,
        c.cancel_operation,
        c.effects.clone(),
        request,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            result.original_observation_error = Some(e);
            result.active_evidence_complete = false;
            return Ok(result.bound(client));
        }
    };
    result.remote_cancellation_requested = true;
    result.outcome = match receipt.submission {
        SubmissionState::Accepted => CancellationOutcome::Acknowledged,
        SubmissionState::Rejected => CancellationOutcome::Rejected,
        SubmissionState::Unknown => CancellationOutcome::Unknown,
    };
    result.cancellation_receipt = Some(Box::new(receipt));
    match tokio::time::timeout(
        operations::OBSERVATION_TIMEOUT,
        history::get_update(
            &bounded,
            UpdateInput {
                id: c.original_update_id.clone(),
            },
        ),
    )
    .await
    {
        Ok(Ok(u)) => match validate(&u, &c) {
            Ok(()) => {
                result.active_update_id = (u.summary.status != UpdateStatus::Complete)
                    .then(|| c.original_update_id.clone());
                result.original_update = Some(u);
            }
            Err(e) => {
                result.original_observation_error = Some(e);
                result.active_evidence_complete = false;
            }
        },
        Ok(Err(e)) => {
            result.original_observation_error = Some(e);
            result.active_evidence_complete = false;
        }
        Err(_) => {
            result.original_observation_error =
                Some(AppError::new(ErrorKind::Timeout, "GetUpdate", None));
            result.active_evidence_complete = false;
        }
    }
    Ok(result.bound(client))
}
