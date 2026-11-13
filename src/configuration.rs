//! Typed configuration previews retain raw baselines privately, never display
//! projections. Tokens are process-local, bounded, expiring and single-use.

use std::{
    collections::BTreeMap,
    hash::BuildHasher,
    sync::Mutex,
    time::{Duration, Instant},
};

use komodo_client::{
    api::write::UpdateStack,
    entities::stack::{PartialStackConfig, Stack},
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

fn present<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// Omission preserves a field; null is invalid. Empty environment and an empty
/// ignore list explicitly clear those fields. Boolean values cannot be cleared.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackPatch {
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

#[derive(Clone, Serialize, JsonSchema)]
pub struct ConfigurationTarget {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
    pub redacted: bool,
    pub before_truncated: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationOutcome {
    Preview,
    NoOp,
    Saved,
    SubmissionUnknown,
}

#[derive(Serialize, JsonSchema)]
pub struct ConfigurationResult {
    pub outcome: ConfigurationOutcome,
    pub resource: ConfigurationTarget,
    pub preview_id: Option<String>,
    pub changes: Vec<FieldChange>,
    pub writes_submitted: u8,
    pub saved_configuration_verified: bool,
    pub observation_error: Option<AppError>,
    pub effects: Vec<&'static str>,
}

// No Debug: these values can contain raw secrets. Nothing here is serialized.
struct Preview {
    created: Instant,
    input: Value,
    baseline: Value,
    result: ConfigurationResult,
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
    fn insert(&self, mut entry: Preview) -> Result<ConfigurationResult, AppError> {
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
        let result = ConfigurationResult {
            resource: entry.result.resource.clone(),
            changes: entry.result.changes.clone(),
            outcome: if entry.result.changes.is_empty() {
                ConfigurationOutcome::NoOp
            } else {
                ConfigurationOutcome::Preview
            },
            preview_id: Some(token.clone()),
            writes_submitted: 0,
            saved_configuration_verified: false,
            observation_error: None,
            effects: entry.result.effects.clone(),
        };
        entries.values.insert(token, entry);
        Ok(result)
    }

    fn take(&self, token: &str, input: &Value) -> Result<Preview, AppError> {
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

impl StackPatch {
    fn validate(&self, client: &CoreClient) -> Result<Value, AppError> {
        let value = serde_json::to_value(self).expect("typed patch");
        let encoded = value.to_string();
        if encoded.len() > MAX_PATCH_BYTES
            || encoded.contains(client.redaction_marker())
            || encoded.contains("[redacted]")
            || encoded.contains("[hidden]")
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
        // Core deserializes environment strings with the pinned SDK helper
        // (including its trailing newline normalization). Preview and compare
        // the same canonical typed value that will actually be stored.
        Ok(serde_json::to_value(self.typed()?).expect("typed partial config"))
    }
    fn typed(&self) -> Result<PartialStackConfig, AppError> {
        serde_json::from_value(serde_json::to_value(self).expect("typed patch")).map_err(|_| {
            AppError::arguments(
                OP,
                "Use a supported typed configuration value; check the environment variable syntax",
            )
        })
    }
}

fn baseline(stack: &Stack) -> Value {
    json!({"kind":"stack","id":stack.id,"name":stack.name,"template":stack.template,"config":stack.config})
}

fn changes(client: &CoreClient, config: &Value, patch: &Value) -> Vec<FieldChange> {
    patch
        .as_object()
        .expect("typed patch object")
        .iter()
        .filter(|(key, value)| config[*key] != **value)
        .map(|(key, after)| {
            let sensitive = key == "environment";
            let display = |v: &Value| {
                if sensitive && v.as_str() != Some("") {
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
                    v.clone()
                }
            };
            FieldChange {
                field: key.clone(),
                before: display(&config[key]),
                after: display(after),
                redacted: sensitive,
                before_truncated: !sensitive && display(&config[key]) != config[key],
            }
        })
        .collect()
}

pub async fn preview_stack_update(
    client: &CoreClient,
    input: StackPreviewInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    let patch = input.patch.validate(client)?;
    let (_, stack) =
        inventory::resolve_stack(&client.bounded_evidence(), input.target.clone()).await?;
    let baseline = baseline(&stack);
    let result = ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "stack",
            id: stack.id,
            name: stack.name.chars().take(256).collect(),
        },
        changes: changes(client, &baseline["config"], &patch),
        preview_id: None,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            "Saves only supplied fields. MCP submits no deployment, file-write or lifecycle command; Core refreshes Stack and target caches. Saved configuration does not establish deployment or health.",
            "Apply rechecks the private authoritative configuration, then submits once. Core has no conditional-write or invocation-idempotency parameter: another writer can race after the final read. The single-use preview expires in ten minutes and cannot survive process restart.",
        ],
    };
    client.previews().insert(Preview {
        created: Instant::now(),
        input: serde_json::to_value(&input).expect("typed input"),
        baseline,
        result,
    })
}

pub async fn apply_stack_update(
    client: &CoreClient,
    input: StackApplyInput,
) -> Result<ConfigurationResult, AppError> {
    require_edit(client)?;
    let patch = input.patch.validate(client)?;
    let submitted = StackPreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &serde_json::to_value(submitted).expect("typed input"),
    )?;
    let (_, current) = inventory::resolve_stack(&client.bounded_evidence(), input.target).await?;
    if baseline(&current) != entry.baseline {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    // Consumed before I/O: concurrent callers/replayed tokens cannot resubmit,
    // including when cancellation or transport loss leaves acceptance unknown.
    entry.result.writes_submitted = 1;
    let result = client
        .write(UpdateStack {
            id: current.id.clone(),
            config: input.patch.typed()?,
        })
        .await;
    match result {
        Ok(saved) if saved.id == current.id && saved.name == current.name => {
            let saved_config = serde_json::to_value(&saved.config).expect("typed configuration");
            let verified = patch
                .as_object()
                .expect("patch object")
                .iter()
                .all(|(k, v)| saved_config[k] == *v);
            if verified {
                entry.result.outcome = ConfigurationOutcome::Saved;
                entry.result.saved_configuration_verified = true;
            } else {
                entry.result.outcome = ConfigurationOutcome::SubmissionUnknown;
                entry.result.observation_error = Some(AppError::new(
                    ErrorKind::InvalidResponse,
                    "UpdateStack",
                    None,
                ));
            }
        }
        other => {
            entry.result.outcome = ConfigurationOutcome::SubmissionUnknown;
            entry.result.observation_error =
                Some(other.err().unwrap_or_else(|| {
                    AppError::new(ErrorKind::InvalidResponse, "UpdateStack", None)
                }));
        }
    }
    Ok(entry.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Preview {
        Preview {
            created: Instant::now(),
            input: json!({}),
            baseline: json!({"secret":"private"}),
            result: ConfigurationResult {
                outcome: ConfigurationOutcome::Preview,
                resource: ConfigurationTarget {
                    kind: "stack",
                    id: "fixture".into(),
                    name: "fixture".into(),
                },
                preview_id: None,
                changes: vec![],
                writes_submitted: 0,
                saved_configuration_verified: false,
                observation_error: None,
                effects: vec![],
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
