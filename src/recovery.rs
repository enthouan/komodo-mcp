//! Bounded evidence search, never a replacement execution or proof of attribution.

use std::{collections::BTreeSet, future::Future, pin::Pin, time::Duration};

use komodo_client::{
    api::read::GetUpdate,
    entities::{ResourceTarget, update::Update},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep_until};

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    history::{self, HistoryInput, HistoryTarget, ResourceKind, UpdateSummary},
    inventory,
    operations::{
        RequestedScope, StackScope, SubmissionState, SubmissionWindow, WorkloadKind,
        WorkloadOperation,
    },
};

pub const MAX_PAGES: u32 = 3;
pub const MAX_CANDIDATES: usize = 5;
pub const MAX_DETAIL_READS: usize = 5;
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_OUTPUT_BYTES: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTarget {
    pub kind: WorkloadKind,
    /// Original resolved stable ID, never a clipped/redacted display name.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInput {
    pub target: RecoveryTarget,
    pub requested_scope: RequestedScope,
    pub operation: WorkloadOperation,
    /// Local UTC Unix milliseconds when available. Omit unknown bounds. Core's
    /// clock/history visibility may differ; no time match proves ownership.
    #[serde(default)]
    pub submission_window: Option<SubmissionWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeEvidence {
    /// Core's whole-workload operation variant matches; no attribution proof.
    WholeWorkloadVariant,
    /// A bounded requested-service log matches; no invocation or isolation proof.
    NamedServiceMatches,
    /// Missing/ambiguous Core service log, inaccessible detail or lookup cap.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchEnd {
    HistoryEnd,
    PageLimit,
    CandidateLimit,
    Deadline,
    ReadFailed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCandidate {
    pub update: UpdateSummary,
    pub scope_evidence: ScopeEvidence,
    pub lookup_error: Option<AppError>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryResult {
    pub target: RecoveryTarget,
    pub requested_scope: RequestedScope,
    pub operation: WorkloadOperation,
    pub submission_window: Option<SubmissionWindow>,
    pub core_operation: String,
    /// History without a known acknowledgement cannot identify this invocation,
    /// even when there is only one matching update or the same Core actor.
    pub original_submission: SubmissionState,
    pub attribution: Attribution,
    pub candidates: Vec<RecoveryCandidate>,
    pub end: SearchEnd,
    pub history_requests: u32,
    pub detail_requests: usize,
    pub excluded_by_scope: usize,
    pub evidence_incomplete: bool,
    pub candidates_truncated: bool,
    pub history_error: Option<AppError>,
    pub automatic_resubmission: bool,
}

impl RecoveryInput {
    fn validate(&mut self) -> Result<(), AppError> {
        let invalid = || {
            AppError::arguments(
                "reconcile_operation",
                "Use the original stable resource ID, compatible operation/scope and nonnegative ordered submission-window bounds when available",
            )
        };
        if self.target.id.len() != 24 || !self.target.id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        self.target.id.make_ascii_lowercase();
        let compatible = match self.target.kind {
            WorkloadKind::Repo => matches!(
                self.operation,
                WorkloadOperation::Clone | WorkloadOperation::Pull
            ),
            WorkloadKind::Build | WorkloadKind::Procedure | WorkloadKind::Action => matches!(
                self.operation,
                WorkloadOperation::Run | WorkloadOperation::Cancel
            ),
            WorkloadKind::ResourceSync => matches!(self.operation, WorkloadOperation::Run),
            _ => matches!(
                self.operation,
                WorkloadOperation::Deploy
                    | WorkloadOperation::Start
                    | WorkloadOperation::Stop
                    | WorkloadOperation::Restart
            ),
        };
        if !compatible {
            return Err(invalid());
        }
        match (&self.target.kind, &self.requested_scope) {
            (WorkloadKind::Stack, RequestedScope::WholeStack {})
            | (WorkloadKind::Deployment, RequestedScope::WholeDeployment {}) => (),
            (WorkloadKind::Repo, RequestedScope::WholeRepo {}) => (),
            (WorkloadKind::Build, RequestedScope::WholeBuild {}) => (),
            (WorkloadKind::Procedure, RequestedScope::WholeProcedure {}) => (),
            (WorkloadKind::Action, RequestedScope::WholeAction {}) => (),
            (WorkloadKind::ResourceSync, RequestedScope::ResourceSync { selection }) => {
                crate::sync_plans::validate_recovery_scope(selection)?
            }
            (WorkloadKind::Stack, RequestedScope::Service { service }) => StackScope::Service {
                service: service.clone(),
            }
            .validate("reconcile_operation")?,
            _ => return Err(invalid()),
        }
        if let Some(window) = &self.submission_window
            && (window.started_ts.is_some_and(|x| x < 0)
                || window.ended_ts.is_some_and(|x| x < 0)
                || window
                    .started_ts
                    .zip(window.ended_ts)
                    .is_some_and(|(a, b)| a > b))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl RecoveryResult {
    fn finish(mut self, client: &CoreClient, end: SearchEnd) -> Self {
        self.end = end;
        self.evidence_incomplete |= end != SearchEnd::HistoryEnd;
        while serde_json::to_vec(&inventory::projected_value(client, &self))
            .expect("serializable recovery")
            .len()
            > MAX_OUTPUT_BYTES
        {
            assert!(
                self.candidates.pop().is_some(),
                "fixed recovery fields fit budget"
            );
            self.candidates_truncated = true;
            self.evidence_incomplete = true;
        }
        self
    }
}

enum ReadStop {
    Cancelled,
    Deadline,
    Error(AppError),
}
async fn read<T, C: Future<Output = ()>>(
    future: impl Future<Output = Result<T, AppError>>,
    operation: &'static str,
    deadline: Instant,
    cancelled: Pin<&mut C>,
) -> Result<T, ReadStop> {
    let stop = deadline.min(Instant::now() + READ_TIMEOUT);
    tokio::select! {
        biased;
        _ = cancelled => Err(ReadStop::Cancelled),
        _ = sleep_until(stop) => if Instant::now() >= deadline { Err(ReadStop::Deadline) }
            else { Err(ReadStop::Error(AppError::new(ErrorKind::Timeout, operation, None))) },
        reply = future => reply.map_err(ReadStop::Error),
    }
}

fn in_window(ts: i64, window: &Option<SubmissionWindow>) -> bool {
    window
        .as_ref()
        .is_none_or(|w| w.started_ts.is_none_or(|x| ts >= x) && w.ended_ts.is_none_or(|x| ts <= x))
}

/// Core 2.3.2 records requested service names in this specific stage, separate
/// from dependency effects. Never infer scope from arbitrary stdout/other_data.
fn service_evidence(update: &Update, service: &str) -> Option<bool> {
    if update.logs.len() > 128 {
        return None;
    }
    let mut stages = update.logs.iter().filter(|log| log.stage == "Service/s");
    let log = stages.next()?;
    if stages.next().is_some() || !log.success || log.stdout.len() > 512 {
        return None;
    }
    let names = log
        .stdout
        .strip_prefix("Execution requested for Stack service/s ")?;
    let selectors: Vec<_> = names.split(", ").collect();
    if selectors.is_empty()
        || selectors.iter().any(|s| {
            StackScope::Service {
                service: (*s).into(),
            }
            .validate("scope_evidence")
            .is_err()
        })
    {
        return None;
    }
    Some(selectors.len() == 1 && selectors[0] == service)
}

pub async fn reconcile_operation(
    client: &CoreClient,
    mut input: RecoveryInput,
    cancelled: impl Future<Output = ()>,
) -> Result<RecoveryResult, AppError> {
    input.validate()?;
    let target = HistoryTarget {
        kind: match input.target.kind {
            WorkloadKind::Stack => ResourceKind::Stack,
            WorkloadKind::Deployment => ResourceKind::Deployment,
            WorkloadKind::Repo => ResourceKind::Repo,
            WorkloadKind::Build => ResourceKind::Build,
            WorkloadKind::Procedure => ResourceKind::Procedure,
            WorkloadKind::Action => ResourceKind::Action,
            WorkloadKind::ResourceSync => ResourceKind::ResourceSync,
        },
        id: input.target.id.clone(),
    };
    let expected_target = match input.target.kind {
        WorkloadKind::Stack => ResourceTarget::Stack(input.target.id.clone()),
        WorkloadKind::Deployment => ResourceTarget::Deployment(input.target.id.clone()),
        WorkloadKind::Repo => ResourceTarget::Repo(input.target.id.clone()),
        WorkloadKind::Build => ResourceTarget::Build(input.target.id.clone()),
        WorkloadKind::Procedure => ResourceTarget::Procedure(input.target.id.clone()),
        WorkloadKind::Action => ResourceTarget::Action(input.target.id.clone()),
        WorkloadKind::ResourceSync => ResourceTarget::ResourceSync(input.target.id.clone()),
    };
    let operation = match input.target.kind {
        WorkloadKind::ResourceSync => komodo_client::entities::Operation::RunSync,
        WorkloadKind::Stack => {
            let (_, whole, service) = input.operation.stack_variants()?;
            if matches!(input.requested_scope, RequestedScope::Service { .. }) {
                service
            } else {
                whole
            }
        }
        WorkloadKind::Deployment => input.operation.deployment_variant()?.1,
        WorkloadKind::Procedure => match input.operation {
            WorkloadOperation::Run => komodo_client::entities::Operation::RunProcedure,
            WorkloadOperation::Cancel => komodo_client::entities::Operation::CancelProcedure,
            _ => {
                return Err(AppError::arguments(
                    "reconcile_operation",
                    "Procedure recovery requires run or cancel",
                ));
            }
        },
        WorkloadKind::Action => match input.operation {
            WorkloadOperation::Run => komodo_client::entities::Operation::RunAction,
            WorkloadOperation::Cancel => komodo_client::entities::Operation::CancelAction,
            _ => {
                return Err(AppError::arguments(
                    "reconcile_operation",
                    "Action recovery requires run or cancel",
                ));
            }
        },
        WorkloadKind::Build => match input.operation {
            WorkloadOperation::Run => komodo_client::entities::Operation::RunBuild,
            WorkloadOperation::Cancel => komodo_client::entities::Operation::CancelBuild,
            _ => {
                return Err(AppError::arguments(
                    "reconcile_operation",
                    "Build recovery requires run or cancel",
                ));
            }
        },
        WorkloadKind::Repo => match input.operation {
            WorkloadOperation::Clone => komodo_client::entities::Operation::CloneRepo,
            WorkloadOperation::Pull => komodo_client::entities::Operation::PullRepo,
            _ => {
                return Err(AppError::arguments(
                    "reconcile_operation",
                    "Repo recovery requires clone or pull",
                ));
            }
        },
    };
    let mut result = RecoveryResult {
        target: input.target,
        requested_scope: input.requested_scope,
        operation: input.operation,
        submission_window: input.submission_window,
        core_operation: operation.to_string(),
        original_submission: SubmissionState::Unknown,
        attribution: Attribution::Unconfirmed,
        candidates: vec![],
        end: SearchEnd::HistoryEnd,
        history_requests: 0,
        detail_requests: 0,
        excluded_by_scope: 0,
        evidence_incomplete: matches!(target.kind, ResourceKind::ResourceSync),
        candidates_truncated: false,
        history_error: None,
        automatic_resubmission: false,
    };
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    let mut seen = BTreeSet::new();
    tokio::pin!(cancelled);
    for page in 0..MAX_PAGES {
        let query = async {
            result.history_requests += 1;
            history::list_updates(
                client,
                HistoryInput {
                    target: Some(target.clone()),
                    page,
                },
            )
            .await
        };
        let history = match read(query, "ListUpdates", deadline, cancelled.as_mut()).await {
            Ok(history) => history,
            Err(ReadStop::Cancelled) => return Ok(result.finish(client, SearchEnd::Cancelled)),
            Err(ReadStop::Deadline) => return Ok(result.finish(client, SearchEnd::Deadline)),
            Err(ReadStop::Error(error)) => {
                result.history_error = Some(error);
                return Ok(result.finish(client, SearchEnd::ReadFailed));
            }
        };
        for summary in history.items {
            if !seen.insert(summary.id.clone()) {
                result.evidence_incomplete = true;
                continue;
            }
            if summary.operation != result.core_operation
                || !in_window(summary.start_ts, &result.submission_window)
            {
                continue;
            }
            let mut candidate = RecoveryCandidate {
                update: summary,
                scope_evidence: ScopeEvidence::WholeWorkloadVariant,
                lookup_error: None,
            };
            if matches!(target.kind, ResourceKind::ResourceSync) {
                // No reliable selector or invocation identity, even for a
                // claimed whole-sync run. Never inspect raw logs for it.
                candidate.scope_evidence = ScopeEvidence::Unavailable;
            }
            if let RequestedScope::Service { service } = &result.requested_scope {
                candidate.scope_evidence = ScopeEvidence::Unavailable;
                if result.detail_requests < MAX_DETAIL_READS {
                    let lookup = async {
                        result.detail_requests += 1;
                        client
                            .read(GetUpdate {
                                id: candidate.update.id.clone(),
                            })
                            .await
                    };
                    match read(lookup, "GetUpdate", deadline, cancelled.as_mut()).await {
                        Ok(update) => {
                            if update.id != candidate.update.id
                                || update.target != expected_target
                                || update.operation != operation
                                || update.start_ts != candidate.update.start_ts
                            {
                                result.evidence_incomplete = true;
                                result.history_error.get_or_insert(AppError::new(
                                    ErrorKind::InvalidResponse,
                                    "GetUpdate",
                                    None,
                                ));
                                continue;
                            }
                            match service_evidence(&update, service) {
                                Some(false) => {
                                    result.excluded_by_scope += 1;
                                    continue;
                                }
                                Some(true) => {
                                    candidate.scope_evidence = ScopeEvidence::NamedServiceMatches
                                }
                                None => (),
                            }
                            match history::project_update_detail(client, &update) {
                                Ok(detail) => candidate.update = detail.summary,
                                Err(error) => candidate.lookup_error = Some(error),
                            }
                        }
                        Err(ReadStop::Error(error)) => candidate.lookup_error = Some(error),
                        Err(stop) => {
                            result.candidates.push(candidate);
                            return Ok(result.finish(
                                client,
                                match stop {
                                    ReadStop::Cancelled => SearchEnd::Cancelled,
                                    _ => SearchEnd::Deadline,
                                },
                            ));
                        }
                    }
                }
                result.evidence_incomplete |= candidate.scope_evidence
                    == ScopeEvidence::Unavailable
                    || candidate.lookup_error.is_some();
            }
            result.candidates.push(candidate);
            if result.candidates.len() == MAX_CANDIDATES {
                result.candidates_truncated = true;
                return Ok(result.finish(client, SearchEnd::CandidateLimit));
            }
        }
        if history.next_page.is_none() {
            return Ok(result.finish(client, SearchEnd::HistoryEnd));
        }
    }
    Ok(result.finish(client, SearchEnd::PageLimit))
}
