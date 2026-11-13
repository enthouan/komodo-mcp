//! Deliberate single-resource deletion; definition and runtime evidence are separate.
use komodo_client::api::{
    read::{GetDeployment, GetServer, GetStack},
    write::{DeleteDeployment, DeleteServer, DeleteStack},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

mod references;
mod runtime;
mod updates;

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{self as common, Preview},
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
};

pub(crate) fn require_apply(client: &CoreClient) -> Result<(), AppError> {
    if !client.capabilities().allows(Capability::Destructive) {
        return Err(AppError {
            correction: Some(
                "Deliberate deletion requires both KOMODO_ALLOW_CONFIGURATION_EDIT=true and KOMODO_ALLOW_OPERATE=true because Core can tear down live workloads. No Core request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, "resource_delete", None)
        });
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Stack,
    Deployment,
    Server,
}
impl ResourceKind {
    fn core_name(self) -> &'static str {
        match self {
            Self::Stack => "Stack",
            Self::Deployment => "Deployment",
            Self::Server => "Server",
        }
    }
    fn delete_operation(self) -> &'static str {
        match self {
            Self::Stack => "DeleteStack",
            Self::Deployment => "DeleteDeployment",
            Self::Server => "DeleteServer",
        }
    }
}

fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, "resource_delete", None)
}
fn object_id(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn raw_id(v: &Value) -> Result<String, AppError> {
    let a = v.get("id").and_then(Value::as_str);
    let b = v
        .get("_id")
        .and_then(|v| v.as_str().or_else(|| v.get("$oid").and_then(Value::as_str)));
    if a.zip(b).is_some_and(|(a, b)| !a.eq_ignore_ascii_case(b)) {
        return Err(invalid());
    }
    let id = a.or(b).filter(|s| object_id(s)).ok_or_else(invalid)?;
    Ok(id.to_ascii_lowercase())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: ResourceKind,
    pub id: String,
}

#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Delete,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: Target,
    pub action: Action,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: Target,
    pub action: Action,
    pub preview_id: String,
}

#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Preview,
    Acknowledged,
    SubmissionUnknown,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ResourceSummary {
    pub kind: ResourceKind,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Reference {
    pub kind: String,
    pub id: String,
    pub name: String,
    /// The exact structured reference Core may detach, or may leave dangling.
    pub field: String,
    pub expected_effect: String,
    pub after: Option<String>,
    pub error: Option<AppError>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Coverage {
    pub family: String,
    pub visible_records_examined: usize,
    pub visible_scan_complete: bool,
    pub hidden_records_possible: bool,
    pub error: Option<AppError>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Workload {
    pub server_id: String,
    pub container_id: String,
    pub name: String,
    pub state_before: String,
    pub observation: String,
    pub after_state: Option<String>,
    pub error: Option<AppError>,
}

struct Selected {
    target: Target,
    name: String,
    definition: Value,
    state: String,
    runtime_name: String,
    server_id: Option<String>,
    is_swarm: bool,
    attachment: Value,
    errors: Vec<AppError>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionRemoval {
    pub reported_by_core: bool,
    pub observation: String,
    pub error: Option<AppError>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct CleanupUpdate {
    pub id: String,
    pub attribution: &'static str,
    pub reported_cleanup: &'static str,
    pub failed_or_skipped_stages: Vec<String>,
    pub error: Option<AppError>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct DeletionResult {
    pub outcome: Outcome,
    pub resource: ResourceSummary,
    pub preview_id: Option<String>,
    pub writes_submitted: u8,
    pub definition_removal: DefinitionRemoval,
    pub known_workloads: Vec<Workload>,
    pub runtime_cleanup_fully_verified: bool,
    pub visible_references: Vec<Reference>,
    pub coverage: Vec<Coverage>,
    pub cleanup_updates: Vec<CleanupUpdate>,
    pub effects: Vec<&'static str>,
    pub evidence_gaps: Vec<&'static str>,
    pub observation_errors: Vec<AppError>,
}

impl Target {
    fn validate(mut self) -> Result<Self, AppError> {
        if !object_id(&self.id) {
            return Err(AppError::arguments(
                "resource_delete",
                "Use one exact supported resource kind and a stable 24-character hexadecimal ID from inventory; no names, bulk selection or wildcards",
            ));
        }
        self.id.make_ascii_lowercase();
        Ok(self)
    }
}
fn signature(mut raw: Value, kind: ResourceKind) -> Result<Value, AppError> {
    let id = raw_id(&raw)?;
    if !raw["name"].as_str().is_some_and(|s| !s.trim().is_empty())
        || raw["template"] != false
        || !raw["config"].is_object()
        || !raw["info"].is_object()
    {
        return Err(invalid());
    }
    let info = match kind {
        ResourceKind::Stack => {
            json!({"deployed_project_name":raw["info"].get("deployed_project_name").cloned().unwrap_or(Value::Null)})
        }
        ResourceKind::Deployment => {
            json!({"deployed_name":raw["info"]["deployed_name"].as_str().ok_or_else(invalid)?})
        }
        ResourceKind::Server => {
            json!({"public_key":raw["info"]["public_key"].as_str().ok_or_else(invalid)?})
        }
    };
    let map = raw.as_object_mut().ok_or_else(invalid)?;
    map.remove("_id");
    map.remove("updated_at");
    map.insert("id".into(), json!(id));
    map.insert("info".into(), info);
    Ok(raw)
}
async fn raw_definition(client: &CoreClient, t: &Target) -> Result<Value, AppError> {
    match t.kind {
        ResourceKind::Stack => {
            client
                .read_references(GetStack {
                    stack: t.id.clone(),
                })
                .await
        }
        ResourceKind::Deployment => {
            client
                .read_references(GetDeployment {
                    deployment: t.id.clone(),
                })
                .await
        }
        ResourceKind::Server => {
            client
                .read_references(GetServer {
                    server: t.id.clone(),
                })
                .await
        }
    }
}
async fn select(client: &CoreClient, t: &Target) -> Result<Selected, AppError> {
    let input = DetailInput {
        id: Some(t.id.clone()),
        name: None,
    };
    let (name, definition, state, runtime_name, server, swarm) = match t.kind {
        ResourceKind::Stack => {
            let (list, r) = inventory::resolve_stack(client, input).await?;
            (
                r.name.clone(),
                signature(serde_json::to_value(&r).expect("resource"), t.kind)?,
                serde_json::to_value(list.info.state).expect("state"),
                r.project_name(false),
                r.config.server_id,
                r.config.swarm_id,
            )
        }
        ResourceKind::Deployment => {
            let (list, r) = inventory::resolve_deployment(client, input).await?;
            (
                r.name.clone(),
                signature(serde_json::to_value(&r).expect("resource"), t.kind)?,
                serde_json::to_value(list.info.state).expect("state"),
                r.deployed_name().to_string(),
                r.config.server_id,
                r.config.swarm_id,
            )
        }
        ResourceKind::Server => {
            let (list, r) = inventory::resolve_server(client, input).await?;
            (
                r.name.clone(),
                signature(serde_json::to_value(&r).expect("resource"), t.kind)?,
                serde_json::to_value(list.info.state).expect("state"),
                String::new(),
                r.id,
                String::new(),
            )
        }
    };
    // Preserve complete raw configuration rather than filling omitted echoed
    // fields from SDK defaults during preview or acknowledgement verification.
    if signature(raw_definition(client, t).await?, t.kind)? != definition {
        return Err(AppError {
            field: Some("definition"),
            ..AppError::new(ErrorKind::StalePreview, "resource_delete", None)
        });
    }
    if t.kind != ResourceKind::Server && (runtime_name.is_empty() || runtime_name.len() > 256) {
        return Err(AppError::arguments(
            "resource_delete",
            "The deployed cleanup name/project must be nonempty and at most 256 bytes; inspect this definition directly in Core",
        ));
    }
    let is_swarm = !swarm.is_empty();
    let mut errors = vec![];
    let attachment = if is_swarm {
        match inventory::resolve_swarm(
            client,
            DetailInput {
                id: Some(swarm.clone()),
                name: None,
            },
        )
        .await
        {
            Ok((_, r)) => common::resource_baseline("swarm", &r),
            Err(e) if e.kind == ErrorKind::InvalidResponse => return Err(e),
            Err(e) => {
                let v = json!({"kind":"swarm","id":swarm,"error":{"kind":e.kind,"operation":e.operation,"status":e.status}});
                errors.push(e);
                v
            }
        }
    } else if t.kind == ResourceKind::Server || server.is_empty() {
        Value::Null
    } else {
        match inventory::resolve_server(
            client,
            DetailInput {
                id: Some(server.clone()),
                name: None,
            },
        )
        .await
        {
            Ok((_, r)) => {
                json!({"resource":common::resource_baseline("server",&r),"public_key":r.info.public_key})
            }
            Err(e) if e.kind == ErrorKind::InvalidResponse => return Err(e),
            Err(e) => {
                let v = json!({"kind":"server","id":server,"error":{"kind":e.kind,"operation":e.operation,"status":e.status}});
                errors.push(e);
                v
            }
        }
    };
    Ok(Selected {
        target: t.clone(),
        name,
        definition,
        state: state.as_str().ok_or_else(invalid)?.into(),
        runtime_name,
        server_id: (!is_swarm && !server.is_empty()).then_some(server),
        is_swarm,
        attachment,
        errors,
    })
}
struct Prepared {
    selected: Selected,
    references: references::Scan,
    runtime: runtime::Evidence,
    history: updates::History,
    baseline: Value,
}
async fn prepare(client: &CoreClient, t: &Target) -> Result<Prepared, AppError> {
    let selected = select(client, t).await?;
    let references = references::scan(client, t).await?;
    let runtime = runtime::before(client, &selected).await?;
    let history = updates::before(client, t).await?;
    let baseline = json!({"definition":selected.definition,"attachment":selected.attachment,"references":references.snapshot,"runtime":runtime.snapshot,"history":history.snapshot});
    if baseline.to_string().len() > 256 * 1024 {
        return Err(AppError::arguments(
            "resource_delete",
            "The complete private deletion-effect snapshot exceeds 256 KiB; inspect and delete this larger resource directly in Core",
        ));
    }
    Ok(Prepared {
        selected,
        references,
        runtime,
        history,
        baseline,
    })
}
async fn bounded_prepare(client: &CoreClient, t: &Target) -> Result<Prepared, AppError> {
    tokio::time::timeout(Duration::from_secs(15),prepare(client,t)).await.unwrap_or_else(|_|Err(AppError {correction:Some("Deletion effects inspection exceeded its budget. No deletion was submitted; prepare a new preview."),..AppError::new(ErrorKind::Timeout,"DeletePreflight",None)}))
}
fn result(client: &CoreClient, p: &Prepared) -> DeletionResult {
    let mut gaps = p.runtime.gaps.clone();
    gaps.extend([
        "Visible bounded searches do not prove no dependencies. Hidden resources, additional pages, arbitrary Procedure/Action/ResourceSync scripts, onboarding-key records and alert details are not exhaustively enumerated.",
        "Core deletes resource permissions before its definition. A non-admin can lose deletion-update history/detail access. Missing or invisible history is not proof of successful cleanup; no alternate credentials will be used.",
        "Core returns the old resource, not a deletion update ID. Matching new update records are corroborating candidates only; invocation attribution is not guaranteed.",
        "No conditional delete or invocation idempotency exists. Another writer, workload, trust change or reference can race after final revalidation. New containers and Compose orphans can be outside observed IDs.",
    ]);
    if p.references.references.len() > references::DISPLAY_LIMIT {
        gaps.push("More visible reference effects exist than the displayed bound; additional effects remain in the private revalidation snapshot.");
    }
    let mut errors = p.selected.errors.clone();
    errors.extend(p.runtime.errors.clone());
    if let Some(e) = &p.history.error {
        errors.push(e.clone());
    }
    let mut effects = vec![
        "Applying this deliberate delete requires BOTH local configuration-edit and operate grants, even when the previewed workload is down/unknown. Core independently requires Write on the exact selected definition. Operate-only access cannot delete.",
        "Only one typed DeleteStack/DeleteDeployment/DeleteServer write is submitted. Core can perform lifecycle cleanup inside that write; MCP submits no separate execute, shell, host-provisioning or fallback request.",
        "The shared bounded ten-minute process-local preview is single-use, bound to action/type/stable ID and raw definition, cleanup identity, target, runtime and visible reference effects. Changed relevant state rejects with zero writes. Never automatically repeat an uncertain submission.",
        "Core archives the deleted full definition in its update history. That archive can retain secrets; MCP never returns Deleted Toml/stdout. Core history permissions and retention remain authoritative.",
        "Core removes the definition, permissions and recent-view entries, and clears Alerter whitelist/blacklist references, including hidden references. Alerter cleanup is best-effort and can fail outside the acknowledgement.",
        "Definition acknowledgement/removal and runtime cleanup are separate. A missing definition does not prove containers/services were removed. Cache data and successful command/update flags are not complete runtime proof. Only validated fresh observations of known container IDs are reported as such.",
    ];
    match p.selected.target.kind {
        ResourceKind::Stack=>effects.push("Core can run Compose down --remove-orphans for the deployed project, or remove a Swarm stack with detached/asynchronous cleanup. Unknown/down state, missing targets, disabled Servers or cleanup failures can leave workloads while the definition is removed. Stack cleanup errors can be logged with success=true; overall update success is not proof."),
        ResourceKind::Deployment=>effects.push("Core can remove the deployed container or Swarm service using its saved termination signal/timeout. Unknown/not-deployed state, missing targets, disabled Servers and cleanup errors can leave workloads while the definition is removed."),
        ResourceKind::Server=>effects.push("Core does not stop host workloads when deleting a Server. It detaches Stack/Deployment/Repo server_id references, targets a legacy scalar Builder server_id field, resolves Server alerts, clears onboarding links and drops the cached connection. Current Builder server_ids arrays can remain dangling; verify each visible reference independently. All host workloads can remain running and become unreachable through the removed Server record."),
    }
    DeletionResult {
        outcome: Outcome::Preview,
        resource: ResourceSummary {
            kind: p.selected.target.kind,
            id: p.selected.target.id.clone(),
            name: common::bounded_text(client, &p.selected.name, 256),
        },
        preview_id: None,
        writes_submitted: 0,
        definition_removal: DefinitionRemoval {
            reported_by_core: false,
            observation: "not_submitted".into(),
            error: None,
        },
        known_workloads: p.runtime.workloads.clone(),
        runtime_cleanup_fully_verified: false,
        visible_references: p
            .references
            .references
            .iter()
            .take(references::DISPLAY_LIMIT)
            .cloned()
            .collect(),
        coverage: p.references.coverage.clone(),
        cleanup_updates: vec![],
        effects,
        evidence_gaps: gaps,
        observation_errors: errors,
    }
}
pub async fn preview(client: &CoreClient, input: PreviewInput) -> Result<DeletionResult, AppError> {
    common::require_edit(client)?;
    let input = PreviewInput {
        target: input.target.validate()?,
        action: input.action,
    };
    let bounded = client.bounded_evidence();
    let p = bounded_prepare(&bounded, &input.target).await?;
    let r = result(client, &p);
    client.previews().insert_destructive(Preview {
        created: Instant::now(),
        input: common::bound_input("resource_delete", &input),
        baseline: p.baseline,
        prepared: json!({"operation":input.target.kind.delete_operation(),"id":input.target.id}),
        result: r,
    })
}
async fn observe(
    client: &CoreClient,
    t: &Target,
    before: &updates::History,
    r: &mut DeletionResult,
) {
    match raw_definition(client, t).await {
        Ok(v)
            if raw_id(&v).is_ok_and(|id| id == t.id)
                && v["config"].is_object()
                && v["name"]
                    .as_str()
                    .is_some_and(|name| !name.trim().is_empty()) =>
        {
            r.definition_removal.observation = "definition_still_present_at_read".into()
        }
        Ok(_) => {
            r.definition_removal.observation = "observation_unavailable".into();
            r.definition_removal.error = Some(invalid());
        }
        Err(e) if e.kind == ErrorKind::NotFound => {
            r.definition_removal.observation = "definition_absent_at_direct_read".into()
        }
        Err(e) => {
            r.definition_removal.observation = "observation_unavailable".into();
            r.definition_removal.error = Some(e);
        }
    }
    match updates::after(client, t, before).await {
        Ok((items, incomplete)) => {
            r.cleanup_updates = items;
            if incomplete {
                r.evidence_gaps.push("Deletion history is absent, truncated, ambiguous or newly inaccessible; matching updates cannot prove invocation attribution or cleanup success.");
            }
        }
        Err(e) => {
            r.observation_errors.push(e);
            r.evidence_gaps
                .push("Deletion-update evidence could not be read; cleanup errors may be hidden.");
        }
    }
    if t.kind == ResourceKind::Server
        && r.definition_removal.observation == "definition_absent_at_direct_read"
    {
        for w in &mut r.known_workloads {
            w.observation = "server_record_removed_runtime_cleanup_not_requested".into();
        }
        r.evidence_gaps.push("The removed Server cannot provide fresh post-delete container inspection. Server deletion requests no host cleanup; use an independent host observer for remaining workloads.");
    } else {
        runtime::after(client, &mut r.known_workloads).await;
    }
    references::observe(client, t, &mut r.visible_references).await;
}
pub async fn apply(client: &CoreClient, input: ApplyInput) -> Result<DeletionResult, AppError> {
    require_apply(client)?;
    let target = input.target.validate()?;
    if input.preview_id.len() > 128 {
        return Err(AppError::new(
            ErrorKind::InvalidPreview,
            "resource_delete",
            None,
        ));
    }
    let canonical = PreviewInput {
        target: target.clone(),
        action: input.action,
    };
    let entry = client.previews().take_destructive(
        &input.preview_id,
        &common::bound_input("resource_delete", &canonical),
    )?;
    let bounded = client.bounded_evidence();
    let p = bounded_prepare(&bounded, &target).await?;
    if p.baseline != entry.baseline
        || entry.prepared != json!({"operation":target.kind.delete_operation(),"id":target.id})
    {
        let field = [
            "definition",
            "attachment",
            "references",
            "runtime",
            "history",
        ]
        .into_iter()
        .find(|key| p.baseline[*key] != entry.baseline[*key]);
        return Err(AppError {
            field,
            correction: Some(
                "The previewed deletion effects changed. Inspect the changed evidence category and prepare a new preview; no deletion was submitted.",
            ),
            ..AppError::new(ErrorKind::StalePreview, "resource_delete", None)
        });
    }
    let mut r = entry.result;
    r.preview_id = None;
    r.writes_submitted = 1;
    let response = match target.kind {
        ResourceKind::Stack => {
            client
                .delete(DeleteStack {
                    id: target.id.clone(),
                })
                .await
        }
        ResourceKind::Deployment => {
            client
                .delete(DeleteDeployment {
                    id: target.id.clone(),
                })
                .await
        }
        ResourceKind::Server => {
            client
                .delete(DeleteServer {
                    id: target.id.clone(),
                })
                .await
        }
    };
    match response.and_then(|raw| {
        if signature(raw, target.kind)? == p.selected.definition {
            Ok(())
        } else {
            Err(invalid())
        }
    }) {
        Ok(()) => {
            r.outcome = Outcome::Acknowledged;
            r.definition_removal.reported_by_core = true;
        }
        Err(e) => {
            r.outcome = Outcome::SubmissionUnknown;
            r.observation_errors.push(e);
        }
    }
    r.definition_removal.observation = "not_checked_after_attempt".into();
    if tokio::time::timeout(
        Duration::from_secs(10),
        observe(&bounded, &target, &p.history, &mut r),
    )
    .await
    .is_err()
    {
        r.observation_errors
            .push(AppError::new(ErrorKind::Timeout, "DeleteObservation", None));
        for w in &mut r.known_workloads {
            if w.observation == "fresh_inspection_before_delete" {
                w.observation = "post_attempt_not_checked_within_budget".into();
            }
        }
        for reference in &mut r.visible_references {
            if reference.after.is_none() {
                reference.after = Some("post_attempt_not_checked_within_budget".into());
            }
        }
        r.evidence_gaps.push("Post-attempt observation reached its budget; unchecked cleanup/reference effects remain unknown. No further write was submitted.");
    }
    Ok(r)
}
