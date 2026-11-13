//! Typed configuration previews retain raw baselines privately, never display
//! projections. Tokens are process-local, bounded, expiring and single-use.

use std::{
    collections::BTreeMap,
    hash::BuildHasher,
    sync::Mutex,
    time::{Duration, Instant},
};

use komodo_client::{
    api::write::{CreateStack, UpdateStack},
    entities::{
        resource::Resource,
        stack::{Stack, StackConfig},
        to_docker_compatible_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
};

const OP: &str = "configuration_edit";
const TTL: Duration = Duration::from_secs(600);
const MAX_PREVIEWS: usize = 16;
const MAX_PATCH_BYTES: usize = 16 * 1024;

pub(crate) fn present<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Attachment {
    Server { target: DetailInput },
    Swarm { target: DetailInput },
}

pub(crate) fn bound_input(operation: &'static str, input: &impl Serialize) -> Value {
    json!({"operation":operation,"input":input})
}

pub(crate) fn bounded_text(client: &CoreClient, value: &str, limit: usize) -> String {
    let redacted = client.redact(value);
    if redacted != value {
        client.redaction_marker().into()
    } else {
        value.chars().take(limit).collect()
    }
}

fn contains_configured_secret(client: &CoreClient, value: &Value) -> bool {
    match value {
        Value::String(s) => client.redact(s) != *s,
        Value::Array(v) => v.iter().any(|v| contains_configured_secret(client, v)),
        Value::Object(v) => v.values().any(|v| contains_configured_secret(client, v)),
        _ => false,
    }
}

pub(crate) fn resource_baseline<C: Default + Serialize, I: Default>(
    kind: &'static str,
    resource: &Resource<C, I>,
) -> Value {
    json!({"kind":kind,"id":resource.id,"name":resource.name,"template":resource.template,"config":resource.config})
}

pub(crate) fn validate_payload(
    client: &CoreClient,
    input: &impl Serialize,
) -> Result<(), AppError> {
    let encoded = serde_json::to_string(input).expect("typed configuration input");
    if encoded.len() > MAX_PATCH_BYTES
        || encoded.contains(client.redaction_marker())
        || encoded.contains("[redacted]")
        || encoded.contains("[hidden]")
    {
        return Err(AppError::arguments(
            OP,
            "Use at most 16384 JSON bytes and original values or secret references rather than display redaction markers",
        ));
    }
    Ok(())
}

pub(crate) fn partial_value(partial: &impl Serialize) -> Result<Value, AppError> {
    let value = serde_json::to_value(partial).expect("typed partial configuration");
    if value.to_string().len() > MAX_PATCH_BYTES {
        return Err(AppError::arguments(
            OP,
            "The serialized partial configuration must fit within 16384 JSON bytes",
        ));
    }
    Ok(value)
}

/// Omission preserves a field; null is invalid. Empty environment and an empty
/// ignore list explicitly clear those fields. Boolean values cannot be cleared.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackPatch {
    /// Applying a supplied source needs both local operate and configuration-edit
    /// grants, regardless of Core's current auto_update policy. Creation is separate.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "crate::stack_configuration::Source")]
    pub source: Option<crate::stack_configuration::Source>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "crate::stack_configuration::Attachment")]
    pub attachment: Option<crate::stack_configuration::Attachment>,
    /// Empty string restores Core's default Stack-name project selection.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub project_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<String>")]
    pub ignore_services: Option<Vec<String>>,
    /// Secret-bearing Compose environment. Returned only as a redacted change.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub environment: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackPreviewInput {
    pub target: DetailInput,
    pub patch: StackPatch,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackApplyInput {
    pub target: DetailInput,
    pub patch: StackPatch,
    pub preview_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackCreateInput {
    pub name: String,
    /// Creation requires source and attachment; other fields are optional.
    #[schemars(schema_with = "create_config_schema")]
    pub config: StackPatch,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackCreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: StackPatch,
    pub preview_id: String,
}

fn create_config_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[generator.subschema_for::<StackPatch>()],"required":["source","attachment"]})
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ConfigurationTarget {
    pub kind: &'static str,
    pub id: Option<String>,
    pub name: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
    pub redacted: bool,
    pub before_truncated: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub after_truncated: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationOutcome {
    Preview,
    NoOp,
    Saved,
    SubmissionUnknown,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ConfigurationResult {
    pub outcome: ConfigurationOutcome,
    pub resource: ConfigurationTarget,
    pub preview_id: Option<String>,
    pub changes: Vec<FieldChange>,
    pub writes_submitted: u8,
    pub saved_configuration_verified: bool,
    pub observation_error: Option<AppError>,
    pub effects: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<DefinitionPreview>,
}

#[derive(Clone, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum DefinitionPreview {
    Stack(crate::stack_configuration::DefinitionPreview),
    Deployment(crate::deployment_configuration::DefinitionPreview),
}

// No Debug: these values can contain raw secrets. Nothing here is serialized.
pub(crate) struct Preview {
    pub created: Instant,
    pub input: Value,
    pub baseline: Value,
    pub prepared: Value,
    pub result: ConfigurationResult,
}
struct Entries {
    nonce: String,
    next: u64,
    values: BTreeMap<String, Preview>,
}
impl Default for Entries {
    fn default() -> Self {
        // Independently seeded standard-library hash states keep token spaces
        // distinct across process restarts without publishing baseline hashes.
        let nonce = format!(
            "{:016x}{:016x}",
            std::collections::hash_map::RandomState::new().hash_one(0_u8),
            std::collections::hash_map::RandomState::new().hash_one(1_u8)
        );
        Self {
            nonce,
            next: 0,
            values: BTreeMap::new(),
        }
    }
}
#[derive(Default)]
pub(crate) struct PreviewStore(Mutex<Entries>);

impl PreviewStore {
    pub(crate) fn insert(&self, mut entry: Preview) -> Result<ConfigurationResult, AppError> {
        let mut entries = self.0.lock().map_err(|_| invalid_preview())?;
        entries.values.retain(|_, p| p.created.elapsed() < TTL);
        if entries.values.len() >= MAX_PREVIEWS {
            return Err(AppError::arguments(
                OP,
                "At most 16 previews may be retained per process; apply an existing preview or wait ten minutes for expiry",
            ));
        }
        entries.next = entries.next.checked_add(1).ok_or_else(invalid_preview)?;
        let token = format!("configuration-{}-{}", entries.nonce, entries.next);
        entry.result.preview_id = Some(token.clone());
        // Result is an allowlisted projection; only the private entry has secrets.
        let mut result = entry.result.clone();
        result.outcome = if entry.result.changes.is_empty() {
            ConfigurationOutcome::NoOp
        } else {
            ConfigurationOutcome::Preview
        };
        entries.values.insert(token, entry);
        Ok(result)
    }

    pub(crate) fn take(&self, token: &str, input: &Value) -> Result<Preview, AppError> {
        let entry = self
            .0
            .lock()
            .map_err(|_| invalid_preview())?
            .values
            .remove(token)
            .ok_or_else(invalid_preview)?;
        if entry.created.elapsed() >= TTL || &entry.input != input {
            return Err(invalid_preview());
        }
        Ok(entry)
    }
}

fn invalid_preview() -> AppError {
    AppError::new(ErrorKind::InvalidPreview, OP, None)
}

pub(crate) fn require_edit(client: &CoreClient) -> Result<(), AppError> {
    if !client.capabilities().allows(Capability::ConfigurationEdit) {
        return Err(AppError::new(ErrorKind::CapabilityDisabled, OP, None));
    }
    Ok(())
}

pub(crate) fn require_source_operate(
    client: &CoreClient,
    source_submitted: bool,
) -> Result<(), AppError> {
    if source_submitted && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "Stack source updates require both operate and configuration-edit grants because Core can auto-deploy. The saved auto_update value can race; no request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, "UpdateStack", None)
        });
    }
    Ok(())
}

impl StackPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        let value = serde_json::to_value(self).expect("typed patch");
        let encoded = value.to_string();
        if encoded.len() > MAX_PATCH_BYTES
            || encoded.contains(client.redaction_marker())
            || encoded.contains("[redacted]")
            || encoded.contains("[hidden]")
            || self.project_name.as_ref().is_some_and(|s| {
                s.len() > 128 || (!s.is_empty() && to_docker_compatible_name(s) != *s)
            })
            || self
                .ignore_services
                .as_ref()
                .is_some_and(|v| v.len() > 32 || v.iter().any(|s| s.is_empty() || s.len() > 128))
        {
            return Err(AppError::arguments(
                OP,
                "Use a patch of at most 16384 JSON bytes, at most 32 nonempty ignore_services of at most 128 bytes each, and original values or secret references rather than display redaction markers",
            ));
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        Ok(())
    }
}

fn baseline(stack: &Stack) -> Value {
    resource_baseline("stack", stack)
}

fn sensitive_locator(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains(['@', '?', '#']) || s.contains("://"),
        Value::Array(items) => items.iter().any(sensitive_locator),
        Value::Object(fields) => fields.values().any(sensitive_locator),
        _ => false,
    }
}

pub(crate) fn changes(client: &CoreClient, config: &Value, patch: &Value) -> Vec<FieldChange> {
    patch
        .as_object()
        .expect("typed patch object")
        .iter()
        .filter(|(key, value)| config[*key] != **value)
        .map(|(key, after)| {
            let sensitive = matches!(
                key.as_str(),
                "environment"
                    | "file_contents"
                    | "git_account"
                    | "image"
                    | "image_registry_account"
                    | "command"
                    | "volumes"
                    | "labels"
                    | "extra_args"
            ) || sensitive_locator(&config[key])
                || sensitive_locator(after)
                || contains_configured_secret(client, &config[key])
                || contains_configured_secret(client, after);
            let display = |v: &Value| {
                let empty = v.is_null()
                    || v.as_str() == Some("")
                    || v.as_array().is_some_and(Vec::is_empty);
                if sensitive && !empty {
                    Value::String(client.redaction_marker().into())
                } else if let Some(items) = v.as_array() {
                    Value::Array(
                        items
                            .iter()
                            .take(32)
                            .map(|item| {
                                Value::String(
                                    item.as_str()
                                        .unwrap_or_default()
                                        .chars()
                                        .take(128)
                                        .collect(),
                                )
                            })
                            .collect(),
                    )
                } else {
                    v.as_str().map_or_else(
                        || v.clone(),
                        |s| Value::String(s.chars().take(256).collect()),
                    )
                }
            };
            FieldChange {
                field: key.clone(),
                before: display(&config[key]),
                after: display(after),
                redacted: sensitive,
                before_truncated: !sensitive && display(&config[key]) != config[key],
                after_truncated: !sensitive && display(after) != *after,
            }
        })
        .collect()
}

fn prepared_value(prepared: &crate::stack_configuration::Prepared) -> Value {
    json!({"config":prepared.config,"references":prepared.references})
}

fn prepared_patch(prepared: &crate::stack_configuration::Prepared) -> Result<Value, AppError> {
    partial_value(&prepared.config)
}

fn result_for_stack(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&StackConfig>,
    prepared: &crate::stack_configuration::Prepared,
    runtime: Option<String>,
) -> Result<ConfigurationResult, AppError> {
    use crate::stack_configuration as sc;
    let patch = prepared_patch(prepared)?;
    let default = StackConfig::default();
    let after = sc::proposed(current.unwrap_or(&default), &prepared.config)?;
    let fields = changes(
        client,
        &current.map_or(Value::Object(Default::default()), |v| {
            serde_json::to_value(v).expect("typed config")
        }),
        &patch,
    );
    let source_check = ["repo", "linked_repo", "file_contents", "files_on_host"]
        .iter()
        .any(|k| patch.get(k).is_some());
    let auto_deploy = current.is_some_and(|v| v.auto_update) && source_check && !fields.is_empty();
    let before = current.map(|v| sc::summary(client, v));
    let after = sc::summary(client, &after);
    let target_changed = before.as_ref().is_some_and(|v| {
        serde_json::to_value(&v.attachment).expect("attachment")
            != serde_json::to_value(&after.attachment).expect("attachment")
    });
    let mut effects = vec![
        "MCP submits only a typed definition write, never deployment, lifecycle, host-file writes or Git commits. Core refreshes Stack/target caches; saving does not establish deployment or health.",
        "Apply consumes the ten-minute process-local preview, rechecks authoritative configuration and references, and submits at most once. Core has no conditional write or invocation idempotency: another writer can race after the final reads.",
    ];
    if source_check && current.is_some() {
        effects.push("Applying a supplied source requires both local configuration-edit and operate grants, even when auto_update is currently disabled or the source is unchanged, because Core's policy can race. A configuration-only caller can preview but cannot submit it.");
        effects.push(if fields.is_empty() {"This proposal is unchanged: apply submits no write and triggers no Core source-update check. The saved auto_update policy is shown in the definition separately."} else if current.is_some_and(|v|v.auto_update) {"Submitted source fields trigger Core CheckStackForUpdate. The saved auto_update policy is enabled: Core itself can automatically deploy. Applying this source requires both local operate and configuration-edit grants."} else {"Submitted source fields trigger Core CheckStackForUpdate. The previewed saved auto_update policy is disabled; policy changes after our final read can still race."});
    }
    if target_changed {
        effects.push("The pinned SDK warns about old-target cleanup. The inspected Core 2.3.2 UpdateStack implementation does not tear down the old target; old workloads may remain. It refreshes caches using the new target. Inspect both targets separately.");
    }
    if current.is_some_and(|v| {
        patch
            .get("project_name")
            .is_some_and(|p| p != &v.project_name)
    }) {
        effects.push("Changing project_name changes the Compose project selected by future operations; an already running project can remain under its old name. Inspect it separately.");
    }
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "stack",
            id,
            name: bounded_text(client, name, 256),
        },
        preview_id: None,
        changes: fields,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects,
        definition: Some(DefinitionPreview::Stack(sc::DefinitionPreview {
            before,
            after,
            resolved_references: prepared.reference_summaries.clone(),
            observed_runtime_state: runtime,
            core_auto_deploy_possible: auto_deploy,
            requires_operate_to_apply: source_check && current.is_some(),
            target_changed,
            old_target_cleanup: "not_performed_by_inspected_core_2_3_2_update_path",
        })),
    })
}

pub(crate) fn finish_save<C: Default + Serialize, I: Default>(
    mut result: ConfigurationResult,
    response: Result<Resource<C, I>, AppError>,
    patch: &Value,
    expected_name: &str,
    expected_id: Option<&str>,
    operation: &'static str,
    project: impl FnOnce(&mut ConfigurationResult, &C),
) -> ConfigurationResult {
    result.preview_id = None;
    result.writes_submitted = 1;
    match response {
        Ok(saved)
            if saved.name == expected_name
                && !saved.template
                && saved.id.len() == 24
                && saved.id.bytes().all(|b| b.is_ascii_hexdigit())
                && expected_id.is_none_or(|id| id == saved.id) =>
        {
            let saved_config = serde_json::to_value(&saved.config).expect("typed config");
            if patch
                .as_object()
                .expect("partial object")
                .iter()
                .all(|(k, v)| saved_config[k] == *v)
            {
                result.outcome = ConfigurationOutcome::Saved;
                result.saved_configuration_verified = true;
                result.resource.id = Some(saved.id);
                project(&mut result, &saved.config);
            } else {
                result.outcome = ConfigurationOutcome::SubmissionUnknown;
                result.observation_error =
                    Some(AppError::new(ErrorKind::InvalidResponse, operation, None));
            }
        }
        other => {
            result.outcome = ConfigurationOutcome::SubmissionUnknown;
            result.observation_error = Some(
                other
                    .err()
                    .unwrap_or_else(|| AppError::new(ErrorKind::InvalidResponse, operation, None)),
            );
        }
    }
    result
}

fn saved_stack_projection(
    client: &CoreClient,
    result: &mut ConfigurationResult,
    config: &StackConfig,
) {
    if let Some(DefinitionPreview::Stack(definition)) = &mut result.definition {
        definition.after = crate::stack_configuration::summary(client, config);
    }
}

pub async fn preview_stack_update(
    client: &CoreClient,
    input: StackPreviewInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    input.patch.validate(client)?;
    let bounded = client.bounded_evidence();
    let (listed, stack) = inventory::resolve_stack(&bounded, input.target.clone()).await?;
    let prepared =
        crate::stack_configuration::prepare(&bounded, &input.patch, Some(&stack.config)).await?;
    let result = result_for_stack(
        client,
        Some(stack.id.clone()),
        &stack.name,
        Some(&stack.config),
        &prepared,
        Some(
            serde_json::to_value(listed.info.state)
                .expect("state")
                .as_str()
                .unwrap_or("unknown")
                .into(),
        ),
    )?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: bound_input("stack_update", &input),
        baseline: baseline(&stack),
        prepared: prepared_value(&prepared),
        result,
    })
}

pub async fn apply_stack_update(
    client: &CoreClient,
    input: StackApplyInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    require_source_operate(client, input.patch.source.is_some())?;
    input.patch.validate(client)?;
    let submitted = StackPreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client
        .previews()
        .take(&input.preview_id, &bound_input("stack_update", &submitted))?;
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_stack(&bounded, input.target).await?;
    if baseline(&current) != entry.baseline {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let prepared =
        crate::stack_configuration::prepare(&bounded, &input.patch, Some(&current.config)).await?;
    let patch = prepared_patch(&prepared)?;
    if prepared_value(&prepared) != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let response = client
        .write(UpdateStack {
            id: current.id.clone(),
            config: prepared.config,
        })
        .await;
    Ok(finish_save(
        entry.result,
        response,
        &patch,
        &current.name,
        Some(&current.id),
        "UpdateStack",
        |result, config| saved_stack_projection(client, result, config),
    ))
}

fn create_name(input: &StackCreateInput, client: &CoreClient) -> Result<String, AppError> {
    input.config.validate(client)?;
    validate_payload(client, &input.name)?;
    let name = to_docker_compatible_name(&input.name);
    if input.name.len() > 256
        || name.is_empty()
        || name.len() > 128
        || (name.len() == 24 && name.bytes().all(|b| b.is_ascii_hexdigit()))
        || input.config.source.is_none()
        || input.config.attachment.is_none()
    {
        return Err(AppError::arguments(
            "CreateStack",
            "Provide a nonempty name (at most 128 normalized characters; no ObjectId name), an explicit source, and an existing Server or Swarm attachment",
        ));
    }
    Ok(name)
}

async fn require_absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_stack(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, "CreateStack", None)),
        Err(e) => Err(e),
    }
}

pub async fn preview_stack_create(
    client: &CoreClient,
    input: StackCreateInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    let name = create_name(&input, client)?;
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let prepared = crate::stack_configuration::prepare(&bounded, &input.config, None).await?;
    let mut result = result_for_stack(client, None, &name, None, &prepared, None)?;
    result.effects.push("Creation checks only visible name absence. Core enforces global uniqueness and its creation/Read-Attach permissions; visibility gaps and a concurrent creator can still reject the write. Core can normalize the supplied name, shown in this preview. New definitions are not deployed.");
    client.previews().insert(Preview {
        created: Instant::now(),
        input: bound_input("stack_create", &input),
        baseline: json!({"kind":"stack_create","name":name}),
        prepared: prepared_value(&prepared),
        result,
    })
}

pub async fn apply_stack_create(
    client: &CoreClient,
    input: StackCreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    let submitted = StackCreateInput {
        name: input.name,
        config: input.config,
    };
    let name = create_name(&submitted, client)?;
    let entry = client
        .previews()
        .take(&input.preview_id, &bound_input("stack_create", &submitted))?;
    if entry.baseline != json!({"kind":"stack_create","name":name}) {
        return Err(invalid_preview());
    }
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let prepared = crate::stack_configuration::prepare(&bounded, &submitted.config, None).await?;
    let patch = prepared_patch(&prepared)?;
    if prepared_value(&prepared) != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, "CreateStack", None));
    }
    let response = client
        .write(CreateStack {
            name: name.clone(),
            config: prepared.config,
        })
        .await;
    Ok(finish_save(
        entry.result,
        response,
        &patch,
        &name,
        None,
        "CreateStack",
        |result, config| saved_stack_projection(client, result, config),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Preview {
        Preview {
            created: Instant::now(),
            input: json!({}),
            baseline: json!({"secret":"private"}),
            prepared: json!({}),
            result: ConfigurationResult {
                outcome: ConfigurationOutcome::Preview,
                resource: ConfigurationTarget {
                    kind: "stack",
                    id: Some("fixture".into()),
                    name: "fixture".into(),
                },
                preview_id: None,
                changes: vec![],
                writes_submitted: 0,
                saved_configuration_verified: false,
                observation_error: None,
                effects: vec![],
                definition: None,
            },
        }
    }

    #[test]
    fn previews_expire_and_capacity_is_bounded_without_evicting_live_previews() {
        let store = PreviewStore::default();
        let first = store.insert(entry()).unwrap().preview_id.unwrap();
        store
            .0
            .lock()
            .unwrap()
            .values
            .get_mut(&first)
            .unwrap()
            .created = Instant::now() - TTL;
        assert!(
            matches!(store.take(&first,&json!({})),Err(e) if e.kind==ErrorKind::InvalidPreview)
        );
        for _ in 0..MAX_PREVIEWS {
            assert!(store.insert(entry()).is_ok());
        }
        assert!(store.insert(entry()).is_err());
        for p in store.0.lock().unwrap().values.values_mut() {
            p.created = Instant::now() - TTL;
        }
        assert!(store.insert(entry()).is_ok());
        assert_eq!(store.0.lock().unwrap().values.len(), 1);
    }
}
