//! Cached Core observations, explicit refresh, and private scoped review records.
//! This module cannot submit RunSync or write authoritative source files.
mod audit;

use crate::{
    capabilities::Capability,
    client::{CoreClient, CoreVersion},
    configuration::{self as common, Preview},
    error::{AppError, ErrorKind},
    export::{self, Kind},
    inventory::{self, DetailInput},
    resource_syncs,
};
use komodo_client::{
    api::read::GetResourceSync,
    entities::{
        ResourceTarget,
        sync::{DiffData, ResourceSync},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const OP: &str = "resource_sync_pending";
const MAX_DISPLAY: usize = 32;
const MAX_CACHED: usize = 256;
const MAX_SOURCE: usize = 128 * 1024;
const MAX_OUTPUT: usize = 256 * 1024;
const MAX_PRIVATE: usize = 2 * 1024 * 1024;
const MAX_SELECTORS: usize = 16;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    WholeSync {},
    ResourceType {
        resource_type: Kind,
    },
    Resources {
        resource_type: Kind,
        targets: Vec<DetailInput>,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolvedTarget {
    Existing { id: String, name: String },
    Create { name: String },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionScope {
    WholeSync {},
    ResourceType {
        resource_type: Kind,
    },
    Resources {
        resource_type: Kind,
        targets: Vec<ResolvedTarget>,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyBinding {
    pub sync_id: String,
    pub selection: ExecutionScope,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub target: DetailInput,
    pub selection: Selection,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct SyncIdentity {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Create,
    Update,
    Delete,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct Change {
    pub resource_type: Option<Kind>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub action: ChangeKind,
    pub origin: &'static str,
    pub template_before: Option<bool>,
    pub template_after: Option<bool>,
    pub secret_flag_before: Option<bool>,
    pub secret_flag_after: Option<bool>,
    pub details_omitted: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct Changes {
    pub available: bool,
    pub reported_total: Option<usize>,
    pub excluded_by_selection: usize,
    pub items: Vec<Change>,
    pub truncated: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DeploymentIntent {
    pub resource_type: Kind,
    pub name: String,
    pub reason: &'static str,
    pub dependencies: Vec<DeploymentDependency>,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DeploymentDependency {
    pub resource_type: Kind,
    pub name: String,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct Deployments {
    pub available: bool,
    pub reported_total: Option<usize>,
    pub items: Vec<DeploymentIntent>,
    pub truncated: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct PlanErrors {
    pub pending_error: Option<bool>,
    pub pending_deploy_error: Option<bool>,
    pub source_file_errors: Option<usize>,
    pub details_omitted: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct SourceEvidence {
    pub kind: &'static str,
    pub pending_git_revision: Option<String>,
    pub files_observed: Option<usize>,
    pub contents_omitted: bool,
    pub observation: &'static str,
    pub proves_current_external_revision: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct PendingOutput {
    pub sync: SyncIdentity,
    pub observation: &'static str,
    pub cached_status: &'static str,
    pub reviewed_status: Option<&'static str>,
    pub variables_in_review_scope: Option<bool>,
    pub user_groups_in_review_scope: Option<bool>,
    pub settings: resource_syncs::DefinitionSummary,
    pub source: SourceEvidence,
    pub resource_changes: Changes,
    pub variable_changes: Changes,
    pub user_group_changes: Changes,
    pub pending_deployments: Deployments,
    pub errors: PlanErrors,
    pub selection: Option<ExecutionScope>,
    pub apply_coverage_complete: bool,
    pub coverage_gaps: Vec<&'static str>,
    pub observation_errors: Vec<AppError>,
    pub core_mutation_requests: u8,
    pub run_sync_requests: u8,
    pub warnings: Vec<&'static str>,
}
#[derive(Serialize, JsonSchema)]
pub struct ReviewOutput {
    pub plan: PendingOutput,
    pub review_reference: Option<String>,
    pub apply_binding: Option<ApplyBinding>,
    pub expires_after_seconds: u32,
    pub observed_core_version: String,
}
#[derive(Serialize, JsonSchema)]
pub struct RefreshOutput {
    pub sync: SyncIdentity,
    pub outcome: &'static str,
    pub refresh_requests_submitted: u8,
    pub run_sync_requests: u8,
    pub pending: Option<PendingOutput>,
    pub error: Option<AppError>,
    pub effects: Vec<&'static str>,
}

// Never Debug/Serialize: these include raw secret-bearing Core observations.
struct Snapshot {
    sync: ResourceSync,
    raw: Value,
    output: PendingOutput,
    complete: bool,
}

fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, "GetResourceSync", None)
}
fn valid_id(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn name_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn gap(gaps: &mut Vec<&'static str>, message: &'static str) {
    if !gaps.contains(&message) && gaps.len() < 32 {
        gaps.push(message);
    }
}
fn target(t: &ResourceTarget) -> Option<(Kind, &str)> {
    Some(match t {
        ResourceTarget::Server(v) => (Kind::Server, v),
        ResourceTarget::Swarm(v) => (Kind::Swarm, v),
        ResourceTarget::Stack(v) => (Kind::Stack, v),
        ResourceTarget::Deployment(v) => (Kind::Deployment, v),
        ResourceTarget::Build(v) => (Kind::Build, v),
        ResourceTarget::Repo(v) => (Kind::Repo, v),
        ResourceTarget::Builder(v) => (Kind::Builder, v),
        ResourceTarget::Alerter(v) => (Kind::Alerter, v),
        ResourceTarget::Procedure(v) => (Kind::Procedure, v),
        ResourceTarget::Action(v) => (Kind::Action, v),
        ResourceTarget::ResourceSync(v) => (Kind::ResourceSync, v),
        ResourceTarget::System(_) => return None,
    })
}
fn delta(data: &DiffData) -> (ChangeKind, &str, Option<&str>, Option<&str>) {
    match data {
        DiffData::Create { name, proposed } => (ChangeKind::Create, proposed, None, Some(name)),
        DiffData::Update { proposed, current } => {
            (ChangeKind::Update, proposed, Some(current), None)
        }
        DiffData::Delete { current } => (ChangeKind::Delete, current, None, None),
    }
}
fn root_metadata(text: &str, header: &str) -> Option<(String, bool, Option<bool>)> {
    if text.len() > MAX_SOURCE {
        return None;
    }
    let doc: toml::Value = toml::from_str(text).ok()?;
    let table = doc.as_table()?;
    if table.len() != 1 {
        return None;
    }
    let entries = table
        .get(header)
        .or_else(|| table.get(&format!("{header}s")))?
        .as_array()?;
    if entries.len() != 1 {
        return None;
    }
    let row = entries[0].as_table()?;
    let name = row.get("name")?.as_str()?;
    if !name_ok(name) {
        return None;
    }
    let template = match row.get("template") {
        Some(v) => v.as_bool()?,
        None => false,
    };
    let secret = if header == "variable" {
        Some(match row.get("is_secret") {
            Some(v) => v.as_bool()?,
            None => false,
        })
    } else {
        None
    };
    Some((name.into(), template, secret))
}
fn change(kind: Option<Kind>, id: Option<&str>, data: &DiffData, header: &str) -> (Change, bool) {
    let (action, proposed, current, declared) = delta(data);
    let after = root_metadata(proposed, header);
    let before = current.and_then(|v| root_metadata(v, header));
    let valid = after.is_some()
        && (current.is_none() || before.is_some())
        && declared.is_none_or(|n| after.as_ref().is_some_and(|v| v.0 == n))
        && id.is_none_or(|v| {
            if action == ChangeKind::Create {
                v.is_empty()
            } else {
                valid_id(v)
            }
        });
    let name = after.as_ref().map(|v| v.0.clone());
    let (before, after) = if action == ChangeKind::Delete {
        (after, None)
    } else {
        (before, after)
    };
    let row = Change {
        resource_type: kind,
        id: id.filter(|s| valid_id(s)).map(str::to_owned),
        name,
        action,
        origin: "core_cache",
        template_before: kind.and(before.as_ref().map(|v| v.1)),
        template_after: kind.and(after.as_ref().map(|v| v.1)),
        secret_flag_before: before.as_ref().and_then(|v| v.2),
        secret_flag_after: after.as_ref().and_then(|v| v.2),
        details_omitted: true,
    };
    (row, valid)
}
fn section(items: Vec<Change>, available: bool, total: usize) -> Changes {
    Changes {
        available,
        reported_total: available.then_some(total),
        excluded_by_selection: 0,
        truncated: total > MAX_DISPLAY,
        items: items.into_iter().take(MAX_DISPLAY).collect(),
    }
}
fn present_error(raw: &Value, key: &str) -> Option<bool> {
    raw.get(key).and_then(|v| {
        if v.is_null() {
            Some(false)
        } else {
            v.as_str().map(|_| true)
        }
    })
}
fn warnings() -> Vec<&'static str> {
    vec![
        "Raw proposed/current TOML, source contents, variable values, memberships, permissions, commit messages and error text are omitted. All authoritative comparison data stays private; sanitized display is never an apply baseline.",
        "This is a Core cache observation, not a frozen execution plan or proof that an external source is current. A later authorized apply must refresh/recheck source, definition, references and plan; Core rereads sources and has no expected-revision parameter.",
        "Desired-state TOML omission can reset defaults. Managed mode enables deletion even when delete=false; empty resource-tag filters are unrestricted. Variables/groups are included only in an unfiltered whole-sync run and are not isolated by resource tags.",
        "RunSync uses Core's built-in administrator sync identity. Resource updates can trigger configured automatic effects; deployment intent does not establish workload health. External Git/host write-back is not exposed.",
    ]
}
fn snapshot(
    client: &CoreClient,
    raw: Value,
    identity: &SyncIdentity,
    observation: &'static str,
) -> Result<Snapshot, AppError> {
    if raw["_id"]["$oid"].as_str() != Some(&identity.id)
        || raw["name"].as_str() != Some(&identity.name)
        || !raw["config"].is_object()
        || !raw["info"].is_object()
        || !raw["template"].is_boolean()
    {
        return Err(invalid());
    }
    let sync: ResourceSync = serde_json::from_value(raw.clone()).map_err(|_| invalid())?;
    let info = &raw["info"];
    let mut gaps = vec![];
    let mut complete = true;
    let expected =
        serde_json::to_value(komodo_client::entities::sync::ResourceSyncConfig::default())
            .expect("config");
    if expected
        .as_object()
        .expect("object")
        .iter()
        .any(|(key, value)| {
            raw["config"].get(key).is_none_or(|v| {
                v.is_boolean() != value.is_boolean()
                    || v.is_string() != value.is_string()
                    || v.is_array() != value.is_array()
            })
        })
        || raw["config"]
            .as_object()
            .expect("checked")
            .keys()
            .any(|key| expected.get(key).is_none())
    {
        complete = false;
        gap(&mut gaps, "missing_or_unsupported_definition_fields");
    }
    for key in [
        "resource_updates",
        "variable_updates",
        "user_group_updates",
        "pending_deploys",
        "remote_contents",
        "remote_errors",
    ] {
        if !info.get(key).is_some_and(Value::is_array) {
            complete = false;
            gap(&mut gaps, "missing_cache_fields");
        }
        if info[key].as_array().is_some_and(|v| v.len() > MAX_CACHED) {
            complete = false;
            gap(&mut gaps, "cached_plan_entry_budget_exceeded");
        }
    }
    for key in ["pending_error", "pending_deploy_error", "pending_hash"] {
        if !info.get(key).is_some_and(|v| v.is_null() || v.is_string()) {
            complete = false;
            gap(&mut gaps, "missing_cache_fields");
        }
    }
    let mut resources = Vec::new();
    for item in sync.info.resource_updates.iter().take(MAX_CACHED) {
        if let Some((kind, id)) = target(&item.target) {
            let (row, valid) = change(Some(kind), Some(id), &item.data, kind.header());
            if !valid {
                complete = false;
                gap(&mut gaps, "unparseable_cached_change");
            }
            resources.push(row);
        } else {
            complete = false;
            gap(&mut gaps, "unsupported_cached_resource_type");
        }
    }
    let mut variables = Vec::new();
    let mut groups = Vec::new();
    for (deltas, out, header) in [
        (&sync.info.variable_updates, &mut variables, "variable"),
        (&sync.info.user_group_updates, &mut groups, "user_group"),
    ] {
        for data in deltas.iter().take(MAX_CACHED) {
            let (row, valid) = change(None, None, data, header);
            if !valid {
                complete = false;
                gap(&mut gaps, "unparseable_cached_change");
            }
            out.push(row);
        }
    }
    let mut deployments = Vec::new();
    for deploy in sync.info.pending_deploys.iter().take(MAX_DISPLAY) {
        let Some((kind, name)) = target(&deploy.target) else {
            complete = false;
            gap(&mut gaps, "unsupported_deployment_target");
            continue;
        };
        if !matches!(kind, Kind::Stack | Kind::Deployment)
            || !name_ok(name)
            || deploy.after.len() > MAX_DISPLAY
        {
            complete = false;
            gap(&mut gaps, "incomplete_deployment_metadata");
            continue;
        }
        let mut dependencies = Vec::new();
        for dep in &deploy.after {
            if let Some((kind, name)) = target(dep)
                && matches!(kind, Kind::Stack | Kind::Deployment)
                && name_ok(name)
            {
                dependencies.push(DeploymentDependency {
                    resource_type: kind,
                    name: name.into(),
                });
            } else {
                complete = false;
                gap(&mut gaps, "incomplete_deployment_metadata");
            }
        }
        let reason = if deploy.reason == "deploy on creation" {
            "deploy_on_creation"
        } else if deploy.reason.ends_with("config has changed") {
            "configuration_changed"
        } else {
            "core_reported_deployment_intent"
        };
        deployments.push(DeploymentIntent {
            resource_type: kind,
            name: name.into(),
            reason,
            dependencies,
        });
    }
    let errors = PlanErrors {
        pending_error: present_error(info, "pending_error"),
        pending_deploy_error: present_error(info, "pending_deploy_error"),
        source_file_errors: info["remote_errors"].as_array().map(Vec::len),
        details_omitted: true,
    };
    let has_errors = errors.pending_error == Some(true)
        || errors.pending_deploy_error == Some(true)
        || errors.source_file_errors.is_some_and(|n| n > 0);
    if has_errors {
        complete = false;
        gap(&mut gaps, "core_pending_or_source_errors");
    }
    let settings = resource_syncs::summary(client, &sync.config);
    let git = matches!(settings.source.kind, "git" | "linked_repo");
    let hash = if git {
        sync.info
            .pending_hash
            .as_deref()
            .and_then(|s| crate::repositories::revision(client, s))
    } else {
        None
    };
    let pending = !resources.is_empty()
        || !variables.is_empty()
        || !groups.is_empty()
        || !deployments.is_empty();
    let output = PendingOutput {
        sync: SyncIdentity {
            id: sync.id.clone(),
            name: common::bounded_text(client, &sync.name, 128),
        },
        observation,
        cached_status: if has_errors {
            "errors"
        } else if !complete {
            "incomplete"
        } else if pending {
            "pending"
        } else {
            "clean"
        },
        reviewed_status: None,
        variables_in_review_scope: None,
        user_groups_in_review_scope: None,
        source: SourceEvidence {
            kind: settings.source.kind,
            pending_git_revision: hash,
            files_observed: info["remote_contents"].as_array().map(Vec::len),
            contents_omitted: true,
            observation,
            proves_current_external_revision: false,
        },
        settings,
        resource_changes: section(
            resources,
            info["resource_updates"].is_array(),
            sync.info.resource_updates.len(),
        ),
        variable_changes: section(
            variables,
            info["variable_updates"].is_array(),
            sync.info.variable_updates.len(),
        ),
        user_group_changes: section(
            groups,
            info["user_group_updates"].is_array(),
            sync.info.user_group_updates.len(),
        ),
        pending_deployments: Deployments {
            available: info["pending_deploys"].is_array(),
            reported_total: info["pending_deploys"].as_array().map(Vec::len),
            truncated: sync.info.pending_deploys.len() > MAX_DISPLAY,
            items: deployments,
        },
        errors,
        selection: None,
        apply_coverage_complete: false,
        coverage_gaps: gaps,
        observation_errors: vec![],
        core_mutation_requests: 0,
        run_sync_requests: 0,
        warnings: warnings(),
    };
    Ok(Snapshot {
        sync,
        raw,
        output,
        complete,
    })
}
async fn load(client: &CoreClient, input: DetailInput) -> Result<Snapshot, AppError> {
    export::validate_selector(&input)?;
    let bounded = client.bounded_evidence();
    let (_, sync) = inventory::resolve_resource_sync(&bounded, input).await?;
    let identity = SyncIdentity {
        id: sync.id.clone(),
        name: sync.name,
    };
    let raw = bounded
        .read_references(GetResourceSync { sync: sync.id })
        .await?;
    snapshot(client, raw, &identity, "cached")
}
fn bounded<T: Serialize>(client: &CoreClient, result: T) -> Result<T, AppError> {
    if inventory::projected_value(client, &result)
        .to_string()
        .len()
        > MAX_OUTPUT
    {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    Ok(result)
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<PendingOutput, AppError> {
    let mut snap = load(client, input).await?;
    gap(
        &mut snap.output.coverage_gaps,
        "explicit_scoped_review_and_metadata_audit_required",
    );
    bounded(client, snap.output)
}
pub async fn refresh(client: &CoreClient, input: DetailInput) -> Result<RefreshOutput, AppError> {
    if !client.capabilities().allows(Capability::Operate) {
        return Err(AppError::new(
            ErrorKind::CapabilityDisabled,
            "RefreshResourceSyncPending",
            None,
        ));
    }
    export::validate_selector(&input)?;
    let bounded_client = client.bounded_evidence();
    let (_, sync) = inventory::resolve_resource_sync(&bounded_client, input).await?;
    let identity = SyncIdentity {
        id: sync.id,
        name: sync.name,
    };
    let result = bounded_client.refresh_sync_pending(&identity.id).await;
    let (outcome, pending, error) = match result
        .and_then(|raw| snapshot(client, raw, &identity, "refresh_response"))
    {
        Ok(mut snap) => {
            snap.output.core_mutation_requests = 1;
            gap(
                &mut snap.output.coverage_gaps,
                "explicit_scoped_review_and_metadata_audit_required",
            );
            (
                if snap.output.cached_status == "errors" {
                    "refreshed_with_errors"
                } else {
                    "refreshed"
                },
                Some(snap.output),
                None,
            )
        }
        Err(error) => (
            if error.kind == ErrorKind::PermissionDenied || error.kind == ErrorKind::Authentication
            {
                "denied"
            } else {
                "submission_unknown"
            },
            None,
            Some(error),
        ),
    };
    bounded(
        client,
        RefreshOutput {
            sync: SyncIdentity {
                id: identity.id,
                name: common::bounded_text(client, &identity.name, 128),
            },
            outcome,
            refresh_requests_submitted: 1,
            run_sync_requests: 0,
            pending,
            error,
            effects: vec![
                "One explicit RefreshResourceSyncPending checks Core Execute, reads sources, can clone/fetch Core Git caches, stores pending state and can create/resolve/send pending alerts. It submits no RunSync or authoritative source write-back.",
                "A refresh reply does not freeze sources/configuration. Core can skip an unconfigured sync. A lost or invalid acknowledgement may follow a cache mutation; inspect cached state without automatically retrying.",
            ],
        },
    )
}
pub async fn review(
    client: &CoreClient,
    version: &CoreVersion,
    input: ReviewInput,
) -> Result<ReviewOutput, AppError> {
    audit::validate_selection(&input.selection)?;
    let snap = load(client, input.target).await?;
    let original = snap.output.clone();
    let prepared = tokio::time::timeout(
        Duration::from_secs(30),
        audit::prepare(client, snap, input.selection),
    )
    .await;
    let (mut plan, binding, baseline) = match prepared {
        Ok(result) => result?,
        Err(_) => {
            let mut plan = original;
            gap(&mut plan.coverage_gaps, "metadata_audit_deadline");
            (plan, None, Value::Null)
        }
    };
    if version.to_string() != "2.3.2" {
        gap(
            &mut plan.coverage_gaps,
            "review_requires_inspected_core_2_3_2",
        );
        plan.apply_coverage_complete = false;
    }
    let mut reference = None;
    let mut apply_binding = None;
    if inventory::projected_value(client, &plan).to_string().len() > MAX_OUTPUT - 16 * 1024 {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    if plan.apply_coverage_complete
        && let Some(binding) = binding
    {
        let input = common::bound_input("resource_sync_apply", &binding);
        if baseline.to_string().len() > MAX_PRIVATE {
            plan.apply_coverage_complete = false;
            gap(&mut plan.coverage_gaps, "private_plan_budget_exceeded");
        } else if inventory::projected_value(client, &binding)
            != serde_json::to_value(&binding).expect("binding")
        {
            plan.apply_coverage_complete = false;
            gap(
                &mut plan.coverage_gaps,
                "redaction_prevents_exact_selection_binding",
            );
        } else {
            let token = client.previews().insert_entry(Preview {
                created: Instant::now(),
                input: input.clone(),
                baseline,
                prepared: serde_json::to_value(&binding).expect("binding"),
                result: plan.clone(),
            })?;
            if client.redact(&token) != token {
                let _: Preview<PendingOutput> = client.previews().take_for(&token, &input)?;
                plan.apply_coverage_complete = false;
                gap(
                    &mut plan.coverage_gaps,
                    "redaction_prevents_exact_review_reference",
                );
            } else {
                reference = Some(token);
                apply_binding = Some(binding);
            }
        }
    }
    if !plan.apply_coverage_complete {
        plan.reviewed_status = Some(if plan.cached_status == "errors" {
            "errors"
        } else {
            "incomplete"
        });
    }
    bounded(
        client,
        ReviewOutput {
            plan,
            review_reference: reference,
            apply_binding,
            expires_after_seconds: 600,
            observed_core_version: common::bounded_text(client, &version.to_string(), 64),
        },
    )
}

fn canonical(snapshot: &Snapshot, references: Value, metadata: Value) -> Value {
    let mut info = snapshot.raw["info"].clone();
    if let Some(fields) = info.as_object_mut() {
        fields.retain(|key, _| {
            [
                "resource_updates",
                "variable_updates",
                "user_group_updates",
                "pending_deploys",
                "pending_error",
                "pending_deploy_error",
                "pending_hash",
                "remote_contents",
                "remote_errors",
            ]
            .contains(&key.as_str())
        });
        for value in fields.values_mut() {
            if let Some(items) = value.as_array_mut() {
                items.sort_by_cached_key(Value::to_string);
            }
        }
    }
    json!({"definition":common::resource_baseline("resource_sync",&snapshot.sync),"pending":info,"references":references,"metadata":metadata})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_metadata_describes_only_the_before_state() {
        let data = DiffData::Delete {
            current: "[[variable]]\nname='owned'\nis_secret=true\nvalue='private-value'\n".into(),
        };
        let (deleted, valid) = change(None, None, &data, "variable");
        assert!(valid);
        assert_eq!(deleted.secret_flag_before, Some(true));
        assert_eq!(deleted.secret_flag_after, None);
        let data = DiffData::Delete {
            current: "[[server]]\nname='owned'\ntemplate=true\n".into(),
        };
        let (deleted, valid) = change(
            Some(Kind::Server),
            Some("000000000000000000000001"),
            &data,
            "server",
        );
        assert!(valid);
        assert_eq!(deleted.template_before, Some(true));
        assert_eq!(deleted.template_after, None);
    }
    #[test]
    fn private_baseline_ignores_hashmap_order_but_keeps_secret_only_source_and_reference_changes() {
        let client = CoreClient::new(
            crate::config::Config::parse("http://127.0.0.1:1", "fixture-key", "fixture-secret")
                .unwrap(),
            crate::client::RequestPolicy::default(),
        )
        .unwrap();
        let sync = ResourceSync {
            id: "00000000000000000000000b".into(),
            name: "owned".into(),
            ..Default::default()
        };
        let identity = SyncIdentity {
            id: sync.id.clone(),
            name: sync.name.clone(),
        };
        let mut raw = serde_json::to_value(sync).unwrap();
        raw["info"]["remote_contents"] = json!([{"resource_path":"a","path":"a","contents":"private-one"},{"resource_path":"b","path":"b","contents":"private-two"}]);
        let first = snapshot(&client, raw.clone(), &identity, "cached").unwrap();
        let baseline = canonical(&first, json!({"secret":"reference-one"}), json!([]));
        raw["info"]["remote_contents"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let reordered = snapshot(&client, raw.clone(), &identity, "cached").unwrap();
        assert_eq!(
            baseline,
            canonical(&reordered, json!({"secret":"reference-one"}), json!([]))
        );
        assert_ne!(
            baseline,
            canonical(&reordered, json!({"secret":"reference-two"}), json!([]))
        );
        raw["info"]["remote_contents"][0]["contents"] = json!("private-three");
        let changed = snapshot(&client, raw, &identity, "cached").unwrap();
        assert_ne!(
            baseline,
            canonical(&changed, json!({"secret":"reference-one"}), json!([]))
        );
        assert!(
            !serde_json::to_string(&first.output)
                .unwrap()
                .contains("private-one")
        );
    }
}
