//! Typed trusted Procedure definitions, ordered stages and saved execution.
use crate::{
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget, Preview,
        present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
};
use komodo_client::{
    api::{
        execute::Execution,
        read::*,
        write::{CreateProcedure, UpdateProcedure},
    },
    entities::{
        procedure::{
            EnabledExecution, PartialProcedureConfig, Procedure, ProcedureConfig,
            ProcedureListItem, ProcedureStage,
        },
        to_general_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

const OP: &str = "procedure_configuration";
pub const AUTHORITY: &str = "Core runs saved Procedure children as its built-in administrator Procedure user, not the initiating caller. Saving steps checks the editor's reference Execute permissions (including disabled entries), and Batch/maintenance steps require Core admin. Running needs Execute on the Procedure and delegates trusted saved code; it is not sandboxed by MCP direct tools' Server-only Build restriction.";
pub const RACE: &str = "Core post-create/update refreshes the schedule. Existing nonempty definition updates require both local grants regardless of current automatic policy. Shared expiring single-use previews recheck the private definition and bounded readable references; hidden/dynamic/concurrent effects remain possible. Core provides no CAS and uncertain writes are never retried.";

macro_rules! steps {
    ($($variant:ident => ($core:literal,$field:literal)),* $(,)?) => {
        #[derive(Clone,Serialize,Deserialize,JsonSchema)]
        #[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
        pub enum Step {
            $($variant{#[schemars(regex(pattern="^[0-9a-fA-F]{24}$"))] resource_id:String},)*
            None{},
            Sleep{#[schemars(range(max=86400000))] duration_ms:u32},
            RunAction{#[schemars(regex(pattern="^[0-9a-fA-F]{24}$"))] resource_id:String,args:BTreeMap<String,Value>},
            BatchRunBuild{pattern:String},
            BatchRunAction{pattern:String},
            BatchRunProcedure{pattern:String},
            GlobalAutoUpdate{skip_auto_update:bool},
            ClearRepoCache{},
        }
        impl Step {
            fn execution(&self)->Result<Execution,AppError> {
                let (kind,mut params)=match self {
                    $(Self::$variant{resource_id}=>($core,json!({$field:resource_id})),)*
                    Self::None{}=>("None",json!({})),
                    Self::Sleep{duration_ms} if *duration_ms<=86400000 =>("Sleep",json!({"duration_ms":duration_ms})),
                    Self::Sleep{..}=>return Err(invalid()),
                    Self::RunAction{resource_id,args}=>{validate_args(args)?; ("RunAction",json!({"action":resource_id,"args":args}))},
                    Self::BatchRunBuild{pattern} => ("BatchRunBuild",pattern_params(pattern)?),
                    Self::BatchRunAction{pattern} => ("BatchRunAction",pattern_params(pattern)?),
                    Self::BatchRunProcedure{pattern} => ("BatchRunProcedure",pattern_params(pattern)?),
                    Self::GlobalAutoUpdate{skip_auto_update} => ("GlobalAutoUpdate",json!({"skip_auto_update":skip_auto_update})),
                    Self::ClearRepoCache{}=>("ClearRepoCache",json!({})),
                };
                for field in ["build","repo","action","procedure","deployment","stack"] {
                    if let Some(v)=params.get_mut(field) {
                        let id=v.as_str().filter(|s|crate::deletion::object_id(s)).ok_or_else(invalid)?.to_ascii_lowercase();
                        *v=json!(id);
                    }
                }
                // Stack steps deliberately operate on the whole saved Stack.
                if matches!(kind,"DeployStack"|"StartStack"|"RestartStack"|"StopStack") {params["services"]=json!([]);}
                serde_json::from_value(json!({"type":kind,"params":params})).map_err(|_|invalid())
            }
        }
    }
}
steps! {
    RunBuild=>("RunBuild","build"),CancelBuild=>("CancelBuild","build"),
    CloneRepo=>("CloneRepo","repo"),PullRepo=>("PullRepo","repo"),
    RunProcedure=>("RunProcedure","procedure"),CancelProcedure=>("CancelProcedure","procedure"),
    DeployDeployment=>("Deploy","deployment"),StartDeployment=>("StartDeployment","deployment"),RestartDeployment=>("RestartDeployment","deployment"),StopDeployment=>("StopDeployment","deployment"),
    DeployStack=>("DeployStack","stack"),StartStack=>("StartStack","stack"),RestartStack=>("RestartStack","stack"),StopStack=>("StopStack","stack"),
}
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use documented typed steps, 1–128 byte names without control characters, stable resource IDs, at most 16 stages / 64 executions / 32 distinct static references and 16384 JSON bytes. Schedule fields are managed separately; no generic execution/configuration passthrough is accepted.",
    )
}
fn pattern_params(s: &str) -> Result<Value, AppError> {
    if s.is_empty() || s.len() > 1024 || s.chars().any(|c| c.is_control() && c != '\n') {
        return Err(invalid());
    }
    Ok(json!({"pattern":s,"tags":[]}))
}
pub(crate) fn validate_args(a: &BTreeMap<String, Value>) -> Result<(), AppError> {
    fn value(v: &Value, depth: usize) -> bool {
        if depth > 8 {
            return false;
        }
        match v {
            Value::Array(a) => a.len() <= 64 && a.iter().all(|v| value(v, depth + 1)),
            Value::Object(m) => {
                m.len() <= 64
                    && m.iter().all(|(k, v)| {
                        k.len() <= 128 && !k.chars().any(char::is_control) && value(v, depth + 1)
                    })
            }
            _ => true,
        }
    }
    if a.len() > 64
        || serde_json::to_vec(a).expect("arguments").len() > 8192
        || !a
            .iter()
            .all(|(k, v)| k.len() <= 128 && !k.chars().any(char::is_control) && value(v, 0))
    {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub name: String,
    pub enabled: bool,
    pub executions: Vec<StepEntry>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepEntry {
    pub enabled: bool,
    pub execution: Step,
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<Stage>")]
    pub stages: Option<Vec<Stage>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub webhook_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub failure_alert: Option<bool>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: Patch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: Patch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    pub config: Patch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    pub config: Patch,
    pub preview_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StepSummary {
    pub index: usize,
    pub enabled: bool,
    pub operation: String,
    pub reference: Option<Reference>,
    pub arguments_configured: bool,
    pub admin_only_to_configure: bool,
    pub dynamic_targets: bool,
    pub writable: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StageSummary {
    pub index: usize,
    pub name: String,
    pub enabled: bool,
    pub executions: Vec<StepSummary>,
    pub execution_count: usize,
    pub executions_complete: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Reference {
    pub kind: String,
    pub id: String,
    pub required_editor_permission: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub stages: Vec<StageSummary>,
    pub stage_count: usize,
    pub execution_count: usize,
    pub enabled_execution_count: usize,
    pub stages_complete: bool,
    pub webhook_enabled: bool,
    pub nonempty_schedule_configured: bool,
    pub schedule_enabled: bool,
    pub failure_alert: bool,
    pub authority: &'static str,
    pub evidence_limit: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProcedureSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub stage_count: i64,
    pub state: String,
    pub last_run_at: Option<i64>,
}
#[derive(Serialize, JsonSchema)]
pub struct Detail {
    pub summary: ProcedureSummary,
    pub definition: DefinitionSummary,
    pub effects: ProcedureEffects,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
    pub reference_evidence: ReferenceEvidence,
}
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct ReferenceEvidence {
    pub readable: usize,
    pub unavailable: usize,
    pub incomplete: bool,
    pub limit: &'static str,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub core_admin_authority: bool,
    pub broader_effects_possible: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProcedureEffects {
    pub definition: DefinitionSummary,
    pub acknowledgement: Acknowledgement,
    pub cancellation_limit: &'static str,
    pub configuration_race: &'static str,
}
pub const CANCEL_LIMIT: &str = "Core 2.3.2 CancelProcedure targets only the Procedure resource and ignores its optional update_id. MCP checks/rechecks the original RunProcedure and active history/action state, but another run can replace it after reads. The cancel token is checked before each stage, so the current stage's children finish first. Cancelling acknowledges the token, not the original's termination or rollback, and does not cancel child operations.";

fn execution_value(e: &Execution) -> Value {
    serde_json::to_value(e).expect("typed execution")
}
fn reference(e: &Value) -> Option<Reference> {
    let p = &e["params"];
    for (field, kind) in [
        ("build", "build"),
        ("repo", "repo"),
        ("action", "action"),
        ("procedure", "procedure"),
        ("deployment", "deployment"),
        ("stack", "stack"),
        ("server", "server"),
        ("swarm", "swarm"),
        ("sync", "resource_sync"),
        ("alerter", "alerter"),
    ] {
        if let Some(id) = p[field].as_str().filter(|s| crate::deletion::object_id(s)) {
            return Some(Reference {
                kind: kind.into(),
                id: id.to_ascii_lowercase(),
                required_editor_permission: if e["type"] == "CommitSync" {
                    "Write"
                } else {
                    "Execute"
                },
            });
        }
    }
    None
}
fn step_summary(index: usize, e: &EnabledExecution) -> StepSummary {
    let value = execution_value(&e.execution);
    let operation = value["type"].as_str().expect("execution type").to_owned();
    let writable = matches!(
        operation.as_str(),
        "None"
            | "Sleep"
            | "RunBuild"
            | "CancelBuild"
            | "CloneRepo"
            | "PullRepo"
            | "RunProcedure"
            | "CancelProcedure"
            | "Deploy"
            | "StartDeployment"
            | "RestartDeployment"
            | "StopDeployment"
            | "DeployStack"
            | "StartStack"
            | "RestartStack"
            | "StopStack"
            | "RunAction"
            | "BatchRunBuild"
            | "BatchRunAction"
            | "BatchRunProcedure"
            | "GlobalAutoUpdate"
            | "ClearRepoCache"
    );
    StepSummary {
        index,
        enabled: e.enabled,
        reference: reference(&value),
        arguments_configured: value["params"].get("args").is_some_and(|v| !v.is_null()),
        admin_only_to_configure: operation.starts_with("Batch")
            || matches!(
                operation.as_str(),
                "ClearRepoCache"
                    | "BackupCoreDatabase"
                    | "GlobalAutoUpdate"
                    | "RotateAllServerKeys"
                    | "RotateCoreKeys"
            ),
        dynamic_targets: operation.starts_with("Batch")
            || matches!(
                operation.as_str(),
                "GlobalAutoUpdate" | "ClearRepoCache" | "RunAction"
            ),
        operation,
        writable,
    }
}
pub(crate) fn summary(client: &CoreClient, c: &ProcedureConfig) -> DefinitionSummary {
    let mut remaining = 32usize;
    DefinitionSummary {
        stages: c
            .stages
            .iter()
            .take(16)
            .enumerate()
            .map(|(i, s)| {
                let take = remaining.min(16).min(s.executions.len());
                remaining -= take;
                StageSummary {
                    index: i,
                    name: common::bounded_text(
                        client,
                        &crate::logs::redact_log(client, &s.name),
                        128,
                    ),
                    enabled: s.enabled,
                    executions: s
                        .executions
                        .iter()
                        .take(take)
                        .enumerate()
                        .map(|(i, e)| step_summary(i, e))
                        .collect(),
                    execution_count: s.executions.len(),
                    executions_complete: s.executions.len() <= take,
                }
            })
            .collect(),
        stage_count: c.stages.len(),
        execution_count: c.stages.iter().map(|s| s.executions.len()).sum(),
        enabled_execution_count: c
            .stages
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.executions.iter().filter(|e| e.enabled).count())
            .sum(),
        stages_complete: c.stages.len() <= 16,
        webhook_enabled: c.webhook_enabled,
        nonempty_schedule_configured: !c.schedule.is_empty(),
        schedule_enabled: c.schedule_enabled,
        failure_alert: c.failure_alert,
        authority: AUTHORITY,
        evidence_limit: "Stages remain ordered; enabled executions within a stage run in parallel. Parameters, script/argument values and webhook secrets are omitted. Unsupported step types are inspected by kind and preserved during unrelated edits, never converted or dropped. Definition references reflect configuration, not child-run evidence. Schedule inspection/editing is separate; hidden and dynamic effects remain possible.",
    }
}
fn item(client: &CoreClient, r: ProcedureListItem) -> ProcedureSummary {
    ProcedureSummary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 256),
        template: r.template,
        stage_count: r.info.stages,
        state: r.info.state.to_string(),
        last_run_at: r.info.last_run_at,
    }
}
pub async fn list(
    client: &CoreClient,
    input: ListInput,
) -> Result<Page<ProcedureSummary>, AppError> {
    let p = inventory::list_procedures(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: p.items.into_iter().map(|r| item(client, r)).collect(),
        page: p.page,
        page_size: p.page_size,
        next_page: p.next_page,
        has_more: p.has_more,
    })
}
pub(crate) fn effects(client: &CoreClient, p: &Procedure) -> ProcedureEffects {
    ProcedureEffects {
        definition: summary(client, &p.config),
        acknowledgement: Acknowledgement {
            core_admin_authority: true,
            broader_effects_possible: true,
        },
        cancellation_limit: CANCEL_LIMIT,
        configuration_race: "MCP executes one saved Procedure by stable ID without rewriting it. Core rereads saved steps asynchronously; steps, referenced resources and automatic policy can change after inspection. No CAS, sandbox or child-result/health guarantee is provided.",
    }
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<Detail, AppError> {
    let (i, p) = inventory::resolve_procedure(&client.bounded_evidence(), input).await?;
    Ok(Detail {
        summary: item(client, i),
        definition: summary(client, &p.config),
        effects: effects(client, &p),
    })
}

fn partial(p: &Patch, create: bool) -> Result<PartialProcedureConfig, AppError> {
    let stages = p
        .stages
        .as_ref()
        .map(|stages| {
            if stages.len() > 16 || stages.iter().map(|s| s.executions.len()).sum::<usize>() > 64 {
                return Err(invalid());
            }
            stages
                .iter()
                .map(|s| {
                    if s.name.is_empty()
                        || s.name.len() > 128
                        || s.name.chars().any(char::is_control)
                        || s.executions.len() > 32
                    {
                        return Err(invalid());
                    }
                    Ok(ProcedureStage {
                        name: s.name.clone(),
                        enabled: s.enabled,
                        executions: s
                            .executions
                            .iter()
                            .map(|e| {
                                Ok(EnabledExecution {
                                    enabled: e.enabled,
                                    execution: e.execution.execution()?,
                                })
                            })
                            .collect::<Result<_, AppError>>()?,
                    })
                })
                .collect::<Result<Vec<_>, AppError>>()
        })
        .transpose()?;
    if create && (stages.is_none() || p.webhook_enabled == Some(true)) {
        return Err(invalid());
    }
    Ok(PartialProcedureConfig {
        stages,
        webhook_enabled: if create {
            Some(false)
        } else {
            p.webhook_enabled
        },
        schedule: if create { Some(String::new()) } else { None },
        schedule_enabled: if create { Some(false) } else { None },
        failure_alert: p.failure_alert,
        ..Default::default()
    })
}
fn proposed(
    c: Option<&ProcedureConfig>,
    p: &PartialProcedureConfig,
) -> Result<ProcedureConfig, AppError> {
    let mut v = serde_json::to_value(c.cloned().unwrap_or_default()).expect("config");
    for (k, x) in common::partial_value(p)?.as_object().expect("partial") {
        v[k] = x.clone();
    }
    serde_json::from_value(v).map_err(|_| invalid())
}
async fn read_reference(client: &CoreClient, r: &Reference) -> Result<Value, AppError> {
    let id = r.id.clone();
    match r.kind.as_str() {
        "build" => client.read_references(GetBuild { build: id }).await,
        "repo" => client.read_references(GetRepo { repo: id }).await,
        "action" => client.read_references(GetAction { action: id }).await,
        "procedure" => client.read_references(GetProcedure { procedure: id }).await,
        "deployment" => {
            client
                .read_references(GetDeployment { deployment: id })
                .await
        }
        "stack" => client.read_references(GetStack { stack: id }).await,
        "server" => client.read_references(GetServer { server: id }).await,
        "swarm" => client.read_references(GetSwarm { swarm: id }).await,
        "resource_sync" => client.read_references(GetResourceSync { sync: id }).await,
        "alerter" => client.read_references(GetAlerter { alerter: id }).await,
        _ => Err(invalid()),
    }
}
async fn references(
    client: &CoreClient,
    c: &ProcedureConfig,
    id: Option<&str>,
    new_steps: bool,
) -> Result<(Value, ReferenceEvidence), AppError> {
    let mut snapshots = vec![];
    let mut seen = BTreeSet::new();
    let mut evidence = ReferenceEvidence {
        limit: "At most 32 distinct visible static references are privately bound/rechecked, including disabled steps. Read visibility does not prove editor Execute/Write/admin; Core validates those authoritatively. Dynamic batches, nested automation and unsupported/multi-target forms have explicit gaps. No elevated fallback.",
        ..Default::default()
    };
    for e in c.stages.iter().flat_map(|s| &s.executions) {
        let v = execution_value(&e.execution);
        if v["type"] == "RunProcedure" && id.is_some_and(|id| v["params"]["procedure"] == id) {
            return Err(invalid());
        }
        let Some(r) = reference(&v) else {
            evidence.incomplete |= !matches!(v["type"].as_str(), Some("None" | "Sleep"));
            continue;
        };
        if !seen.insert((r.kind.clone(), r.id.clone())) {
            continue;
        }
        if snapshots.len() >= 32 {
            if new_steps {
                return Err(invalid());
            }
            evidence.incomplete = true;
            continue;
        }
        match read_reference(client, &r).await {
            Ok(raw) => {
                if raw["_id"]["$oid"] != r.id || !raw["config"].is_object() {
                    return Err(AppError::new(ErrorKind::InvalidResponse, OP, None));
                }
                if new_steps && raw["template"] == true {
                    return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
                }
                evidence.readable += 1;
                snapshots.push(json!({"kind":r.kind,"id":r.id,"resource":raw}));
            }
            Err(error) if new_steps => return Err(error),
            Err(error) => {
                evidence.unavailable += 1;
                evidence.incomplete = true;
                snapshots.push(json!({"kind":r.kind,"id":r.id,"unavailable":error.kind}));
            }
        }
    }
    Ok((
        json!({"snapshots":snapshots,"incomplete":evidence.incomplete}),
        evidence,
    ))
}
async fn prepare(
    client: &CoreClient,
    c: Option<&Procedure>,
    patch: &Patch,
    create: bool,
) -> Result<
    (
        PartialProcedureConfig,
        Value,
        ReferenceEvidence,
        ProcedureConfig,
    ),
    AppError,
> {
    common::validate_payload(client, patch)?;
    let p = partial(patch, create)?;
    let after = proposed(c.map(|p| &p.config), &p)?;
    let (refs, evidence) = references(
        client,
        &after,
        c.map(|p| p.id.as_str()),
        patch.stages.is_some(),
    )
    .await?;
    Ok((p, refs, evidence, after))
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    before: Option<&ProcedureConfig>,
    p: &PartialProcedureConfig,
    after: &ProcedureConfig,
    evidence: ReferenceEvidence,
) -> ConfigurationResult {
    let base = serde_json::to_value(before.cloned().unwrap_or_default()).expect("config");
    let patch = common::partial_value(p).expect("bounded validated partial");
    let changes = common::changes(client, &base, &patch);
    ConfigurationResult {
        outcome: if id.is_some() && changes.is_empty() {
            ConfigurationOutcome::NoOp
        } else {
            ConfigurationOutcome::Preview
        },
        resource: ConfigurationTarget {
            kind: "procedure",
            id,
            name: common::bounded_text(client, name, 256),
        },
        preview_id: None,
        changes,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            AUTHORITY,
            RACE,
            "Definition saves submit no RunProcedure. Creation forces empty/disabled schedule and disabled webhooks; schedule fields are not accepted by these definition tools. Omitted stages/secrets/schedule and unrelated settings remain unchanged. [] deliberately clears stages. Core post-save scheduling/cache work and automatic-policy races are disclosed.",
        ],
        definition: Some(common::DefinitionPreview::Procedure(Box::new(
            DefinitionPreview {
                before: before.map(|c| summary(client, c)),
                after: summary(client, after),
                requires_operate_to_apply: before.is_some()
                    && !patch.as_object().expect("partial").is_empty(),
                reference_evidence: evidence,
            },
        ))),
    }
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Procedure, AppError>,
    p: &PartialProcedureConfig,
    expected: (&ProcedureConfig, &str, Option<&str>),
    op: &'static str,
) -> ConfigurationResult {
    let (c, name, id) = expected;
    let mut r = common::finish_save(
        r,
        response,
        &common::partial_value(p).expect("partial"),
        name,
        id,
        op,
        |r, c| {
            if let Some(common::DefinitionPreview::Procedure(d)) = &mut r.definition {
                d.after = summary(client, c);
            }
        },
    );
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let raw = client
            .read_references(GetProcedure {
                procedure: r.resource.id.clone().expect("saved"),
            })
            .await;
        r = common::verify_readback(
            r,
            raw,
            &serde_json::to_value(c).expect("config"),
            name,
            "GetProcedure",
        );
    }
    r
}
fn supported(p: &Procedure) -> Result<(), AppError> {
    if p.template {
        Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None))
    } else {
        Ok(())
    }
}
pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let bounded = client.bounded_evidence();
    let (_, c) = inventory::resolve_procedure(&bounded, input.target.clone()).await?;
    supported(&c)?;
    let (p, refs, evidence, after) = prepare(&bounded, Some(&c), &input.patch, false).await?;
    let r = result(
        client,
        Some(c.id.clone()),
        &c.name,
        Some(&c.config),
        &p,
        &after,
        evidence,
    );
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("procedure_update", &input),
        baseline: common::resource_baseline("procedure", &c),
        prepared: json!({"config":common::partial_value(&p)?,"references":refs}),
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    crate::builders::require_operate(
        client,
        !common::partial_value(&input.patch)?
            .as_object()
            .expect("patch")
            .is_empty(),
        "UpdateProcedure",
    )?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("procedure_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, c) = inventory::resolve_procedure(&bounded, input.target).await?;
    supported(&c)?;
    if entry.baseline != common::resource_baseline("procedure", &c) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let (p, refs, _, after) = prepare(&bounded, Some(&c), &input.patch, false).await?;
    if entry.prepared != json!({"config":common::partial_value(&p)?,"references":refs}) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let response = client
        .write_checked(UpdateProcedure {
            id: c.id.clone(),
            config: p.clone(),
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &p,
        (&after, &c.name, Some(&c.id)),
        "UpdateProcedure",
    )
    .await)
}
fn name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    let name = to_general_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || crate::deletion::object_id(&name)
    {
        return Err(invalid());
    }
    Ok(name)
}
async fn absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_procedure(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, OP, None)),
        Err(e) => Err(e),
    }
}
pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let name = name(client, &input)?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let (p, refs, evidence, after) = prepare(&bounded, None, &input.config, true).await?;
    let r = result(client, None, &name, None, &p, &after, evidence);
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("procedure_create", &input),
        baseline: json!({"kind":"procedure_create","name":name}),
        prepared: json!({"config":common::partial_value(&p)?,"references":refs}),
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let submitted = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = name(client, &submitted)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("procedure_create", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let (p, refs, _, after) = prepare(&bounded, None, &submitted.config, true).await?;
    if entry.prepared != json!({"config":common::partial_value(&p)?,"references":refs})
        || entry.baseline != json!({"kind":"procedure_create","name":name})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let response = client
        .write_checked(CreateProcedure {
            name: name.clone(),
            config: p.clone(),
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &p,
        (&after, &name, None),
        "CreateProcedure",
    )
    .await)
}
pub(crate) fn guard_write(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if matches!(op, "CreateProcedure" | "UpdateProcedure") {
        p["config"].as_object().ok_or_else(invalid)?;
        let inert = op == "CreateProcedure"
            && p["config"]["webhook_enabled"] == false
            && p["config"]["schedule_enabled"] == false
            && p["config"]["schedule"] == "";
        crate::builders::require_operate(client, !inert, op)?;
    }
    Ok(())
}
