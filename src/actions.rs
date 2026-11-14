//! Trusted Action definitions; private source/default arguments never enter projections.
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
        read::GetAction,
        write::{CreateAction, UpdateAction},
    },
    entities::{
        FileFormat,
        action::{Action, ActionConfig, ActionListItem, PartialActionConfig},
        to_general_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};
const OP: &str = "action_configuration";
pub const AUTHORITY: &str = "Core runs saved Action TypeScript on its host with Deno --allow-all and a temporary API key for its built-in administrator Action user. Scripts have host/filesystem/network and administrator Core authority, not the initiating caller's resource permissions. Trusted scripts can call cloud APIs; MCP's direct Server-builder restriction is not a script sandbox. Core requires admin to create Actions and Execute on the Action to run/cancel.";
pub const RACE: &str = "Core post-create/update refreshes scheduling and cached Action state. Every actual existing-definition update requires both local grants, regardless of current automatic policy. Previews bind the complete private definition, including source/arguments/secrets. Core has no CAS; definitions and automatic policy can change after the final read. Uncertain writes are never retried.";
pub const MERGE: &str = "Core parses saved KeyValue/Toml/Yaml/Json defaults, recursively merges per-run objects, appends per-run arrays to existing arrays and replaces scalars. Replacing an existing object/array with a different type (including null) fails in Core. Empty objects/arrays do not clear defaults; MCP forwards overrides without merging or persisting them.";
pub const CANCEL_LIMIT: &str = "Core CancelAction uses the supplied run update ID, but does not check that it belongs to the selected Action. MCP requires and revalidates this exact Action/RunAction association at the handler and shared transport, then sends that ID explicitly. Concurrent Action runs are allowed; no resource-only fallback is used. A finished run/token removal can race the last read. Cancellation acknowledgement is separate from termination; Core kills the Deno process group, without rollback or cancellation of already-submitted child operations.";
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use an explicit source (at most 8192 bytes) and typed fields. JSON argument objects are limited to 8192 bytes, depth 8, 64 entries per collection and 128-byte keys; total patch 16384 bytes. Omission preserves source/defaults/settings; {} deliberately clears saved defaults. Null, schedule fields and generic SDK passthrough are not accepted.",
    )
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub source: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "BTreeMap<String,Value>")]
    pub default_arguments: Option<BTreeMap<String, Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub reload_deno_deps: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub run_at_startup: Option<bool>,
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
pub struct DefinitionSummary {
    pub source_configured: bool,
    pub source_bytes: usize,
    pub default_arguments_configured: bool,
    pub arguments_format: String,
    pub reload_deno_deps: bool,
    pub run_at_startup: bool,
    pub webhook_enabled: bool,
    pub nonempty_schedule_configured: bool,
    pub schedule_enabled: bool,
    pub failure_alert: bool,
    pub authority: &'static str,
    pub argument_merge: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ActionSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub last_run_at: Option<i64>,
}
#[derive(Serialize, JsonSchema)]
pub struct Detail {
    pub summary: ActionSummary,
    pub definition: DefinitionSummary,
    pub effects: ActionEffects,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub core_admin_authority: bool,
    pub host_access: bool,
    pub broader_effects_possible: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ActionEffects {
    pub definition: DefinitionSummary,
    pub acknowledgement: Acknowledgement,
    pub cancellation_limit: &'static str,
    pub configuration_race: &'static str,
}
pub(crate) fn summary(c: &ActionConfig) -> DefinitionSummary {
    DefinitionSummary {
        source_configured: !c.file_contents.is_empty(),
        source_bytes: c.file_contents.len(),
        default_arguments_configured: !c.arguments.is_empty(),
        arguments_format: format!("{:?}", c.arguments_format),
        reload_deno_deps: c.reload_deno_deps,
        run_at_startup: c.run_at_startup,
        webhook_enabled: c.webhook_enabled,
        nonempty_schedule_configured: !c.schedule.is_empty(),
        schedule_enabled: c.schedule_enabled,
        failure_alert: c.failure_alert,
        authority: AUTHORITY,
        argument_merge: MERGE,
    }
}
fn item(client: &CoreClient, r: ActionListItem) -> ActionSummary {
    ActionSummary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 256),
        template: r.template,
        state: r.info.state.to_string(),
        last_run_at: r.info.last_run_at,
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<ActionSummary>, AppError> {
    let p = inventory::list_actions(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: p.items.into_iter().map(|r| item(client, r)).collect(),
        page: p.page,
        page_size: p.page_size,
        next_page: p.next_page,
        has_more: p.has_more,
    })
}
pub(crate) fn effects(p: &Action) -> ActionEffects {
    ActionEffects {
        definition: summary(&p.config),
        acknowledgement: Acknowledgement {
            core_admin_authority: true,
            host_access: true,
            broader_effects_possible: true,
        },
        cancellation_limit: CANCEL_LIMIT,
        configuration_race: "MCP submits one saved Action by stable ID and never rewrites source/defaults on run. Core asynchronously rereads its definition; configuration may race. Multiple runs can overlap. No sandbox, child-result correlation or workload-health guarantee is provided.",
    }
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<Detail, AppError> {
    let (i, p) = inventory::resolve_action(&client.bounded_evidence(), input).await?;
    Ok(Detail {
        summary: item(client, i),
        definition: summary(&p.config),
        effects: effects(&p),
    })
}
fn partial(p: &Patch, create: bool) -> Result<PartialActionConfig, AppError> {
    if p.source
        .as_ref()
        .is_some_and(|s| s.len() > 8192 || s.contains('\0'))
        || create
            && (p.source.is_none()
                || p.webhook_enabled == Some(true)
                || p.run_at_startup == Some(true))
    {
        return Err(invalid());
    }
    if let Some(a) = &p.default_arguments {
        crate::procedures::validate_args(a).map_err(|_| invalid())?;
    }
    let config = PartialActionConfig {
        file_contents: p.source.clone(),
        arguments: p
            .default_arguments
            .as_ref()
            .map(|a| serde_json::to_string(a).expect("arguments")),
        arguments_format: p.default_arguments.as_ref().map(|_| FileFormat::Json),
        reload_deno_deps: p.reload_deno_deps,
        run_at_startup: if create {
            Some(false)
        } else {
            p.run_at_startup
        },
        webhook_enabled: if create {
            Some(false)
        } else {
            p.webhook_enabled
        },
        schedule: if create { Some(String::new()) } else { None },
        schedule_enabled: if create { Some(false) } else { None },
        failure_alert: p.failure_alert,
        ..Default::default()
    };
    // Match Core deserialization before sending: nonempty source/default text
    // is trimmed at the end and terminated with one newline by the pinned SDK.
    let config: PartialActionConfig =
        serde_json::from_value(serde_json::to_value(config).expect("partial"))
            .map_err(|_| invalid())?;
    if config
        .file_contents
        .as_ref()
        .is_some_and(|s| s.len() > 8192)
    {
        return Err(invalid());
    }
    Ok(config)
}
fn prepare(
    client: &CoreClient,
    c: Option<&Action>,
    patch: &Patch,
    create: bool,
) -> Result<(PartialActionConfig, ActionConfig), AppError> {
    common::validate_payload(client, patch)?;
    let p = partial(patch, create)?;
    let mut v =
        serde_json::to_value(c.map(|a| a.config.clone()).unwrap_or_default()).expect("config");
    for (k, x) in common::partial_value(&p)?.as_object().expect("partial") {
        v[k] = x.clone();
    }
    Ok((p, serde_json::from_value(v).map_err(|_| invalid())?))
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    before: Option<&ActionConfig>,
    p: &PartialActionConfig,
    after: &ActionConfig,
) -> ConfigurationResult {
    let base = serde_json::to_value(before.cloned().unwrap_or_default()).expect("config");
    let patch = common::partial_value(p).expect("partial");
    let changes = common::changes(client, &base, &patch);
    ConfigurationResult {
        outcome: if id.is_some() && changes.is_empty() {
            ConfigurationOutcome::NoOp
        } else {
            ConfigurationOutcome::Preview
        },
        resource: ConfigurationTarget {
            kind: "action",
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
            MERGE,
            "Creation requires explicit trusted source and forces startup/webhooks/schedule disabled with empty schedule. Updating source/defaults/settings preserves omitted secrets and unrelated settings. Startup true runs on a future Core startup; webhook/schedule policy can trigger saved code. These tools submit no RunAction; schedule editing is separate.",
        ],
        definition: Some(common::DefinitionPreview::Action(Box::new(
            DefinitionPreview {
                before: before.map(summary),
                after: summary(after),
                requires_operate_to_apply: before.is_some()
                    && !patch.as_object().expect("partial").is_empty(),
            },
        ))),
    }
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Action, AppError>,
    p: &PartialActionConfig,
    expected: (&ActionConfig, &str, Option<&str>),
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
            if let Some(common::DefinitionPreview::Action(d)) = &mut r.definition {
                d.after = summary(c);
            }
        },
    );
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let raw = client
            .read_references(GetAction {
                action: r.resource.id.clone().expect("saved"),
            })
            .await;
        r = common::verify_readback(
            r,
            raw,
            &serde_json::to_value(c).expect("config"),
            name,
            "GetAction",
        );
    }
    r
}
fn supported(p: &Action) -> Result<(), AppError> {
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
    let (_, c) = inventory::resolve_action(&bounded, input.target.clone()).await?;
    supported(&c)?;
    let (p, after) = prepare(&bounded, Some(&c), &input.patch, false)?;
    let r = result(
        client,
        Some(c.id.clone()),
        &c.name,
        Some(&c.config),
        &p,
        &after,
    );
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("action_update", &input),
        baseline: common::resource_baseline("action", &c),
        prepared: common::partial_value(&p)?,
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
        "UpdateAction",
    )?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("action_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, c) = inventory::resolve_action(&bounded, input.target).await?;
    supported(&c)?;
    if entry.baseline != common::resource_baseline("action", &c) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let (p, after) = prepare(&bounded, Some(&c), &input.patch, false)?;
    if entry.prepared != common::partial_value(&p)? {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let response = client
        .write_checked(UpdateAction {
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
        "UpdateAction",
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
    match inventory::resolve_action(
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
    let (p, after) = prepare(&bounded, None, &input.config, true)?;
    let r = result(client, None, &name, None, &p, &after);
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("action_create", &input),
        baseline: json!({"kind":"action_create","name":name}),
        prepared: common::partial_value(&p)?,
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
        &common::bound_input("action_create", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let (p, after) = prepare(&bounded, None, &submitted.config, true)?;
    if entry.prepared != common::partial_value(&p)?
        || entry.baseline != json!({"kind":"action_create","name":name})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let response = client
        .write_checked(CreateAction {
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
        "CreateAction",
    )
    .await)
}
pub(crate) fn guard_write(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if matches!(op, "CreateAction" | "UpdateAction") {
        p["config"].as_object().ok_or_else(invalid)?;
        let inert = op == "CreateAction"
            && p["config"]["webhook_enabled"] == false
            && p["config"]["schedule_enabled"] == false
            && p["config"]["schedule"] == ""
            && p["config"]["run_at_startup"] == false;
        crate::builders::require_operate(client, !inert, op)?;
    }
    Ok(())
}
