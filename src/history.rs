//! Bounded, explicit projections of Core's fixed-size incident-history pages.

use komodo_client::{
    api::read::{GetUpdate, ListAlerts, ListUpdates},
    entities::{
        MongoDocument, ResourceTarget, ResourceTargetVariant,
        alert::{Alert, AlertDataVariant},
        update::{Log, Update, UpdateListItem, UpdateStatus as CoreUpdateStatus},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
};

/// Core 2.3.2 fixes both history page sizes at 100; no unlimited query is used.
pub const HISTORY_PAGE_SIZE: usize = 100;
pub const MAX_UPDATE_STAGES: usize = 20;
pub const MAX_STAGE_NAME_BYTES: usize = 128;
pub const MAX_STAGE_STDERR_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    System,
    Swarm,
    Server,
    Stack,
    Deployment,
    Build,
    Repo,
    Procedure,
    Action,
    Builder,
    Alerter,
    ResourceSync,
}

impl ResourceKind {
    fn core_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Swarm => "Swarm",
            Self::Server => "Server",
            Self::Stack => "Stack",
            Self::Deployment => "Deployment",
            Self::Build => "Build",
            Self::Repo => "Repo",
            Self::Procedure => "Procedure",
            Self::Action => "Action",
            Self::Builder => "Builder",
            Self::Alerter => "Alerter",
            Self::ResourceSync => "ResourceSync",
        }
    }
}

impl From<ResourceTargetVariant> for ResourceKind {
    fn from(kind: ResourceTargetVariant) -> Self {
        match kind {
            ResourceTargetVariant::System => Self::System,
            ResourceTargetVariant::Swarm => Self::Swarm,
            ResourceTargetVariant::Server => Self::Server,
            ResourceTargetVariant::Stack => Self::Stack,
            ResourceTargetVariant::Deployment => Self::Deployment,
            ResourceTargetVariant::Build => Self::Build,
            ResourceTargetVariant::Repo => Self::Repo,
            ResourceTargetVariant::Procedure => Self::Procedure,
            ResourceTargetVariant::Action => Self::Action,
            ResourceTargetVariant::Builder => Self::Builder,
            ResourceTargetVariant::Alerter => Self::Alerter,
            ResourceTargetVariant::ResourceSync => Self::ResourceSync,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryTarget {
    pub kind: ResourceKind,
    /// Stable resource ID; System uses its literal system identifier.
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    /// Omit to inspect all visible resources; no arbitrary Mongo queries are accepted.
    #[serde(default)]
    pub target: Option<HistoryTarget>,
    /// Zero-based Core page. Pass next_page unchanged with the same target.
    #[serde(default)]
    #[schemars(range(max = 4294967294_u64))]
    pub page: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
}

impl UpdateInput {
    pub(crate) fn validate(self, operation: &'static str) -> Result<String, AppError> {
        if !object_id(&self.id) {
            return Err(AppError::arguments(
                operation,
                "Provide one 24-character hexadecimal update id",
            ));
        }
        Ok(self.id.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryPage<T> {
    pub items: Vec<T>,
    pub page: u32,
    /// Core's fixed maximum records per page.
    pub page_size: usize,
    pub next_page: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Queued,
    Running,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOutcome {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSummary {
    pub id: String,
    pub operation: String,
    /// Unix milliseconds.
    pub start_ts: i64,
    pub target: HistoryTarget,
    pub status: UpdateStatus,
    pub outcome: UpdateOutcome,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateStage {
    /// Original zero-based index, even when failed stages are prioritized.
    pub index: usize,
    pub stage: String,
    pub stage_truncated: bool,
    pub success: bool,
    pub start_ts: i64,
    pub end_ts: i64,
    /// Bounded redacted stderr only; command and stdout are never returned.
    pub stderr: String,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDetail {
    #[serde(flatten)]
    pub summary: UpdateSummary,
    pub end_ts: Option<i64>,
    /// At most 20 stages, failures first, preserving original index/order within each group.
    pub stages: Vec<UpdateStage>,
    pub stages_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_revision: Option<RevisionObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_build_result: Option<crate::build_observation::BuildObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_procedure_result: Option<crate::procedure_observation::ProcedureObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_action_result: Option<crate::action_observation::ActionObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_sync_result: Option<crate::sync_observation::SyncObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_alerter_test: Option<crate::alerters::TestObservation>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RevisionObservation {
    pub revision: String,
    /// update_commit_hash or the pinned Repo Latest Commit stage. Never inferred
    /// from a configured branch or the resource's mutable current cache.
    pub evidence: &'static str,
}

fn observed_revision(client: &CoreClient, update: &Update) -> Option<RevisionObservation> {
    // Pinned RunSync does not bind a revision to its update. Pending/last-sync
    // hashes can describe a later refresh and cannot be attributed to this run.
    if update.operation == komodo_client::entities::Operation::RunSync
        || crate::variables::private_history(update.operation)
        || crate::alerters::private_history(update.operation)
    {
        return None;
    }
    if let Some(revision) = crate::repositories::revision(client, &update.commit_hash) {
        return Some(RevisionObservation {
            revision,
            evidence: "update_commit_hash",
        });
    }
    if !matches!(
        update.operation,
        komodo_client::entities::Operation::CloneRepo
            | komodo_client::entities::Operation::PullRepo
    ) || !matches!(update.target, ResourceTarget::Repo(_))
    {
        return None;
    }
    // Core 2.3.2 drops the clone response's structured hash, but keeps this
    // specific git-library stage. Decode only its bounded first line/hex value,
    // never return arbitrary stdout, commit messages, commands or other_data.
    let mut logs = update.logs.iter().filter(|l| l.stage == "Latest Commit");
    let log = logs.next()?;
    if logs.next().is_some() || !log.success || log.stdout.len() > 4096 {
        return None;
    }
    let first = log.stdout.lines().next()?;
    // Pinned Core's formatting::{muted,bold} emits literal HTML, not ANSI.
    // Recognize only its complete hash-line wrapper, never arbitrary HTML or
    // the following private commit message. Keep the narrow plain/SGR form too.
    let plain = if let Some(hash) = first.strip_prefix(
        "<span style=\"color: var(--mantine-color-dimmed)\">hash:</span> <span style=\"font-weight: bolder\">",
    ) {
        format!("hash: {}", hash.strip_suffix("</span>")?)
    } else {
        crate::build_observation::plain(first)?
    };
    let revision = crate::repositories::revision(client, plain.strip_prefix("hash: ")?.trim())?;
    Some(RevisionObservation {
        revision,
        evidence: "repo_latest_commit_stage",
    })
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlertSummary {
    pub id: String,
    /// Unix milliseconds.
    pub ts: i64,
    pub resolved: bool,
    pub resolved_ts: Option<i64>,
    pub level: String,
    pub target: HistoryTarget,
    /// Alert variant only; raw variant payloads, messages and error chains are omitted.
    pub variant: String,
}

fn invalid(operation: &'static str) -> AppError {
    AppError::new(ErrorKind::InvalidResponse, operation, None)
}

fn object_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_target(target: &HistoryTarget) -> bool {
    if target.kind == ResourceKind::System {
        !target.id.is_empty()
            && target.id.len() <= 64
            && target
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    } else {
        object_id(&target.id)
    }
}

impl HistoryInput {
    fn validate(mut self, operation: &'static str) -> Result<Self, AppError> {
        // Both Core resolvers compute the next page with +1. Exclude overflow
        // before making a request, even though ListAlerts accepts a u64.
        if self.page == u32::MAX
            || self
                .target
                .as_ref()
                .is_some_and(|target| !valid_target(target))
        {
            return Err(AppError::arguments(
                operation,
                "Use a page below 4294967295 and a valid stable target ID",
            ));
        }
        if let Some(target) = &mut self.target
            && target.kind != ResourceKind::System
        {
            target.id.make_ascii_lowercase();
        }
        Ok(self)
    }

    fn query(&self) -> Option<MongoDocument> {
        self.target.as_ref().map(|target| {
            serde_json::from_value(
                json!({"target.type": target.kind.core_name(), "target.id": target.id}),
            )
            .expect("literal target filter is a Mongo document")
        })
    }
}

fn project_target(
    target: &ResourceTarget,
    operation: &'static str,
) -> Result<HistoryTarget, AppError> {
    let (kind, id) = target.extract_variant_id();
    let target = HistoryTarget {
        kind: kind.into(),
        id: if kind == ResourceTargetVariant::System {
            id.clone()
        } else {
            id.to_ascii_lowercase()
        },
    };
    if !valid_target(&target) {
        return Err(invalid(operation));
    }
    Ok(target)
}

fn matches_target(
    actual: &HistoryTarget,
    expected: &Option<HistoryTarget>,
    operation: &'static str,
) -> Result<(), AppError> {
    if expected.as_ref().is_some_and(|expected| expected != actual) {
        return Err(invalid(operation));
    }
    Ok(())
}

fn project_status(status: CoreUpdateStatus, success: bool) -> (UpdateStatus, UpdateOutcome) {
    match status {
        CoreUpdateStatus::Queued => (UpdateStatus::Queued, UpdateOutcome::Queued),
        CoreUpdateStatus::InProgress => (UpdateStatus::Running, UpdateOutcome::Running),
        CoreUpdateStatus::Complete => (
            UpdateStatus::Complete,
            if success {
                UpdateOutcome::Succeeded
            } else {
                UpdateOutcome::Failed
            },
        ),
    }
}

fn project_update(update: &UpdateListItem) -> Result<UpdateSummary, AppError> {
    if !object_id(&update.id) {
        return Err(invalid("ListUpdates"));
    }
    let (status, outcome) = project_status(update.status, update.success);
    Ok(UpdateSummary {
        id: update.id.to_ascii_lowercase(),
        operation: update.operation.to_string(),
        start_ts: update.start_ts,
        target: project_target(&update.target, "ListUpdates")?,
        status,
        outcome,
    })
}

fn project_alert(alert: &Alert) -> Result<AlertSummary, AppError> {
    if !object_id(&alert.id) {
        return Err(invalid("ListAlerts"));
    }
    let variant = serde_json::to_value(AlertDataVariant::from(&alert.data))
        .expect("alert variant is serializable")
        .as_str()
        .expect("alert variant is a string")
        .to_owned();
    Ok(AlertSummary {
        id: alert.id.to_ascii_lowercase(),
        ts: alert.ts,
        resolved: alert.resolved,
        resolved_ts: alert.resolved_ts,
        level: alert.level.to_string(),
        target: project_target(&alert.target, "ListAlerts")?,
        variant,
    })
}

fn check_page(
    page: u32,
    count: usize,
    next: Option<u32>,
    operation: &'static str,
) -> Result<(), AppError> {
    if count > HISTORY_PAGE_SIZE
        || (count == HISTORY_PAGE_SIZE) != next.is_some()
        || next.is_some_and(|next| next == u32::MAX || next != page + 1)
    {
        return Err(invalid(operation));
    }
    Ok(())
}

pub async fn list_alerts(
    client: &CoreClient,
    input: HistoryInput,
) -> Result<HistoryPage<AlertSummary>, AppError> {
    let input = input.validate("ListAlerts")?;
    let response = client
        .read(ListAlerts {
            query: input.query(),
            page: u64::from(input.page),
        })
        .await?;
    let next = response
        .next_page
        .map(u32::try_from)
        .transpose()
        .map_err(|_| invalid("ListAlerts"))?;
    check_page(input.page, response.alerts.len(), next, "ListAlerts")?;
    let items = response
        .alerts
        .iter()
        .map(|alert| {
            let projected = project_alert(alert)?;
            matches_target(&projected.target, &input.target, "ListAlerts")?;
            Ok(projected)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(HistoryPage {
        items,
        page: input.page,
        page_size: HISTORY_PAGE_SIZE,
        next_page: next,
    })
}

pub async fn list_updates(
    client: &CoreClient,
    input: HistoryInput,
) -> Result<HistoryPage<UpdateSummary>, AppError> {
    let input = input.validate("ListUpdates")?;
    let response = client
        .read(ListUpdates {
            query: input.query(),
            page: input.page,
        })
        .await?;
    check_page(
        input.page,
        response.updates.len(),
        response.next_page,
        "ListUpdates",
    )?;
    let items = response
        .updates
        .iter()
        .map(|update| {
            let projected = project_update(update)?;
            matches_target(&projected.target, &input.target, "ListUpdates")?;
            Ok(projected)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(HistoryPage {
        items,
        page: input.page,
        page_size: HISTORY_PAGE_SIZE,
        next_page: response.next_page,
    })
}

/// Fixed internal cancellation filter, never an arbitrary caller-supplied query.
/// This avoids mistaking another operator's later, busy/failed run for the
/// currently active run. Truncation or multiple active IDs prevents cancellation.
pub(crate) async fn active_runs(
    client: &CoreClient,
    target: HistoryTarget,
    operation: komodo_client::entities::Operation,
) -> Result<HistoryPage<UpdateSummary>, AppError> {
    if !valid_target(&target) {
        return Err(invalid("ListUpdates"));
    }
    let response=client.read(ListUpdates {query:Some(serde_json::from_value(json!({"target.type":target.kind.core_name(),"target.id":target.id,"operation":operation,"status":{"$in":["Queued","InProgress"]}})).expect("fixed filter")),page:0}).await?;
    check_page(0, response.updates.len(), response.next_page, "ListUpdates")?;
    let items = response
        .updates
        .iter()
        .map(|u| {
            let item = project_update(u)?;
            if item.target != target
                || u.operation != operation
                || matches!(item.status, UpdateStatus::Complete)
            {
                return Err(invalid("ListUpdates"));
            }
            Ok(item)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(HistoryPage {
        items,
        page: 0,
        page_size: HISTORY_PAGE_SIZE,
        next_page: response.next_page,
    })
}

fn bounded(client: &CoreClient, text: &str, max: usize) -> (String, bool) {
    let safe = crate::logs::redact_log(client, text);
    // Git failures can echo URL-embedded credentials without assignment labels.
    // History never needs a raw source URL: retain surrounding stage evidence
    // and omit each URL-bearing token, including any query/fragment secrets.
    let safe = safe
        .split_inclusive(char::is_whitespace)
        .map(|part| {
            if part.contains("://") {
                let end = part.trim_end_matches(char::is_whitespace).len();
                format!("{}{}", client.redaction_marker(), &part[end..])
            } else {
                part.into()
            }
        })
        .collect::<String>();
    let truncated = text.len() > crate::logs::MAX_INPUT_BYTES || safe.len() > max;
    let mut end = safe.len().min(max);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    (safe[..end].to_owned(), truncated)
}

fn project_stage(client: &CoreClient, index: usize, log: &Log) -> UpdateStage {
    let (stage, stage_truncated) = bounded(client, &log.stage, MAX_STAGE_NAME_BYTES);
    let (stderr, stderr_truncated) = bounded(client, &log.stderr, MAX_STAGE_STDERR_BYTES);
    UpdateStage {
        index,
        stage,
        stage_truncated,
        success: log.success,
        start_ts: log.start_ts,
        end_ts: log.end_ts,
        stderr,
        stderr_truncated,
    }
}

pub async fn get_update(client: &CoreClient, input: UpdateInput) -> Result<UpdateDetail, AppError> {
    let id = input.validate("GetUpdate")?;
    let update = client.read(GetUpdate { id: id.clone() }).await?;
    if update.id != id {
        return Err(invalid("GetUpdate"));
    }
    project_update_detail(client, &update)
}

pub(crate) fn project_update_detail(
    client: &CoreClient,
    update: &Update,
) -> Result<UpdateDetail, AppError> {
    if !object_id(&update.id) {
        return Err(invalid("GetUpdate"));
    }
    let (status, outcome) = project_status(update.status, update.success);
    let summary = UpdateSummary {
        id: update.id.to_ascii_lowercase(),
        operation: update.operation.to_string(),
        start_ts: update.start_ts,
        target: project_target(&update.target, "GetUpdate")?,
        status,
        outcome,
    };
    let stages = update
        .logs
        .iter()
        .enumerate()
        .filter(|(_, log)| !log.success)
        .chain(
            update
                .logs
                .iter()
                .enumerate()
                .filter(|(_, log)| log.success),
        )
        .take(MAX_UPDATE_STAGES)
        .map(|(index, log)| {
            // Sync and trusted automation output can disclose raw source,
            // arguments or administrator credentials, including after restart.
            if crate::alerters::private_history(update.operation) {
                let mut safe = log.clone();
                safe.stage = "Alerter operation".into();
                safe.stderr = if log.stderr.is_empty() { String::new() } else {
                    "Alerter output omitted because Core history may contain endpoint credentials, resolved references, recipient details or provider response bodies. Inspect status and independent receiver evidence separately.".into()
                };
                let mut stage = project_stage(client, index, &safe);
                stage.stderr_truncated = !log.stderr.is_empty();
                stage
            } else if crate::variables::private_history(update.operation) {
                let mut safe = log.clone();
                safe.stage = "Variable operation".into();
                safe.stderr = if log.stderr.is_empty() { String::new() } else {
                    "Variable output omitted because pinned Core history can contain raw values, including external secret writes. Inspect status and variable metadata separately.".into()
                };
                let mut stage = project_stage(client, index, &safe);
                stage.stderr_truncated = !log.stderr.is_empty();
                stage
            } else if update.operation == komodo_client::entities::Operation::RunSync {
                let mut safe=log.clone();
                safe.stage=crate::sync_observation::stage(&log.stage).0.into();
                safe.stderr=if log.stderr.is_empty() {String::new()} else {
                    "Sync error details omitted because they may contain desired/current TOML, source contents, values or credentials. Inspect safe batch reports and independent resource/health observations.".into()
                };
                let mut stage=project_stage(client,index,&safe);
                stage.stderr_truncated = !log.stderr.is_empty();
                stage
            } else if update.operation == komodo_client::entities::Operation::RunAction {
                let mut safe = log.clone();
                if safe.stage != "Execute Action" {
                    safe.stage = "Action preparation".into();
                }
                safe.stderr = if log.stderr.is_empty() {
                    String::new()
                } else {
                    "Action output omitted because it may contain source, arguments or temporary administrator credentials. Inspect structured status and observed_action_result.".into()
                };
                let mut stage = project_stage(client, index, &safe);
                stage.stderr_truncated = !log.stderr.is_empty();
                stage
            } else if update.operation == komodo_client::entities::Operation::RunProcedure
                && !log.stderr.is_empty()
            {
                // Procedure error chains embed secret-bearing execution Debug.
                let mut safe = log.clone();
                safe.stderr = "Procedure error details omitted because they may contain configured execution arguments. Inspect the safe parent stage reports and reported child update IDs separately.".into();
                let mut stage = project_stage(client, index, &safe);
                stage.stderr_truncated = true;
                stage
            } else {
                project_stage(client, index, log)
            }
        })
        .collect();
    Ok(UpdateDetail {
        observed_alerter_test: crate::alerters::observe_test(update),
        summary,
        end_ts: update.end_ts,
        stages,
        stages_truncated: update.logs.len() > MAX_UPDATE_STAGES,
        observed_revision: observed_revision(client, update),
        observed_build_result: crate::build_observation::project(client, update),
        observed_procedure_result: crate::procedure_observation::project(client, update),
        observed_action_result: crate::action_observation::project(update),
        observed_sync_result: crate::sync_observation::project(update),
    })
}
