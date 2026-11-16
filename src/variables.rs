//! Ordinary-variable lifecycle and secret metadata. Raw values are private;
//! Core's administrator responses and history are not sanitized for this purpose.
use std::{collections::BTreeSet, time::Instant};

use komodo_client::api::{
    read::{ListSecrets, ListVariables},
    write::{CreateVariable, DeleteVariable, UpdateVariableDescription, UpdateVariableValue},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{self as common, Preview},
    error::{AppError, ErrorKind},
};

const OP: &str = "variables";
const MAX_SCAN: usize = 512;
const MAX_PRIVATE: usize = 65_536;
const EFFECTS: &[&str] = &[
    "Values are interpolated by future consumers, including automatic execution and notification delivery. Creating, changing or deleting an ordinary variable requires both local operate and configuration-edit grants. Description edits require configuration-edit only. Core requires an enabled administrator for every write.",
    "One typed write at most; no explicit execution, delivery or retry. A lost response can follow a saved change. Read back the name separately; matching state does not attribute an uncertain write. Core returns no execution update ID for these writes.",
    "Names are Core's unique natural keys, without an immutable ID or revision. Complete private entries and caller authority are checked again immediately before submission, but Core has no compare-and-swap: a final race or identical same-name replacement cannot be excluded.",
    "Secret values, including their masked text and length, are omitted. Secret entries support description edits only. Existing upstream database/log/history exposure is not erased or changed by these tools.",
];

fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
fn args(message: &'static str) -> AppError {
    AppError::arguments(OP, message)
}
fn stale() -> AppError {
    AppError::new(ErrorKind::StalePreview, OP, None)
}
pub(crate) fn name_valid(name: &str) -> bool {
    (1..=500).contains(&name.len())
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
}
fn validate_name(name: &str) -> Result<(), AppError> {
    if !name_valid(name) {
        return Err(args(
            "Use one exact case-sensitive variable name: 1–500 ASCII bytes matching [A-Za-z_][A-Za-z0-9_]*",
        ));
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// Always creates is_secret=false; no secret provisioning or classification field.
    CreateOrdinary {
        name: String,
        value: String,
        #[serde(default)]
        description: String,
    },
    SetOrdinaryValue {
        name: String,
        value: String,
    },
    /// The only supported secret-entry mutation. Omitted values remain private.
    SetDescription {
        name: String,
        description: String,
    },
    DeleteOrdinary {
        name: String,
    },
}
impl Change {
    fn name(&self) -> &str {
        match self {
            Self::CreateOrdinary { name, .. }
            | Self::SetOrdinaryValue { name, .. }
            | Self::SetDescription { name, .. }
            | Self::DeleteOrdinary { name } => name,
        }
    }
    fn action(&self) -> &'static str {
        match self {
            Self::CreateOrdinary { .. } => "create_ordinary",
            Self::SetOrdinaryValue { .. } => "set_ordinary_value",
            Self::SetDescription { .. } => "set_description",
            Self::DeleteOrdinary { .. } => "delete_ordinary",
        }
    }
    fn needs_operate(&self) -> bool {
        !matches!(self, Self::SetDescription { .. })
    }
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        validate_name(self.name())?;
        let (value, description) = match self {
            Self::CreateOrdinary {
                value, description, ..
            } => (Some(value), Some(description)),
            Self::SetOrdinaryValue { value, .. } => (Some(value), None),
            Self::SetDescription { description, .. } => (None, Some(description)),
            Self::DeleteOrdinary { .. } => (None, None),
        };
        if value.is_some_and(|v| v.len() > 8192 || v.contains('\0'))
            || description.is_some_and(|v| v.len() > 4096 || v.contains('\0'))
        {
            return Err(args(
                "Use at most 8192 value bytes and 4096 description bytes, without NUL; omit secret values and use original values rather than display markers",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub change: Change,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub change: Change,
    pub preview_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameInput {
    pub name: String,
}
fn first_page() -> usize {
    1
}
fn page_limit() -> usize {
    20
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageInput {
    #[serde(default = "first_page")]
    #[schemars(range(min = 1, max = 1000000))]
    pub page: usize,
    #[serde(default = "page_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: usize,
}
impl PageInput {
    fn offset(&self) -> Result<usize, AppError> {
        if !(1..=1_000_000).contains(&self.page) || !(1..=50).contains(&self.limit) {
            return Err(args(
                "Use a positive page up to 1000000 and limit from 1 through 50",
            ));
        }
        Ok((self.page - 1) * self.limit)
    }
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct VariableView {
    pub name: String,
    pub description: String,
    pub description_truncated: bool,
    pub is_secret: bool,
    /// Only get_variable and change previews include an ordinary value that passes
    /// the documented projection; secret fields are absent, never masked text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub value_status: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct VariablePage {
    pub items: Vec<VariableView>,
    pub page: usize,
    pub next_page: Option<usize>,
    pub scanned: usize,
    pub scan_truncated: bool,
    pub evidence: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct CoreSecretNames {
    pub scope: &'static str,
    pub names: Vec<String>,
    pub page: usize,
    pub next_page: Option<usize>,
    pub scan_truncated: bool,
    pub unsafe_names_omitted: usize,
    pub evidence: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct VariableResult {
    pub action: &'static str,
    pub outcome: &'static str,
    pub preview_id: Option<String>,
    pub before: Option<VariableView>,
    pub after: Option<VariableView>,
    pub writes_submitted: u8,
    pub acknowledgement_valid: bool,
    pub saved_state_matches: bool,
    pub observation_error: Option<AppError>,
    pub submission_error: Option<AppError>,
    pub effects: Vec<&'static str>,
}

/// Explicit raw classification/value checks precede SDK defaults. An omitted
/// description is legitimate: pinned Variable skips an empty description.
pub(crate) fn raw_variable(value: &Value) -> Result<Value, AppError> {
    if !value.is_object()
        || value.to_string().len() > MAX_PRIVATE
        || !value["name"].as_str().is_some_and(name_valid)
        || !value["is_secret"].is_boolean()
        || !value["value"].as_str().is_some_and(|s| s.len() <= 40_000)
        || value
            .get("description")
            .is_some_and(|v| !v.as_str().is_some_and(|s| s.len() <= 32_768))
    {
        return Err(invalid());
    }
    let mut value = value.clone();
    if value.get("description").is_none() {
        value["description"] = json!("");
    }
    Ok(value)
}
async fn entries(client: &CoreClient, complete: bool) -> Result<(Vec<Value>, bool), AppError> {
    let raw = client.read_references(ListVariables {}).await?;
    let values = raw.as_array().ok_or_else(invalid)?;
    let truncated = values.len() > MAX_SCAN;
    if complete && truncated {
        return Err(AppError::new(
            ErrorKind::IncompleteResolution,
            "ListVariables",
            None,
        ));
    }
    let mut names = BTreeSet::new();
    let mut result = Vec::new();
    for value in values.iter().take(MAX_SCAN) {
        let value = raw_variable(value)?;
        if !names.insert(value["name"].as_str().expect("checked").to_owned()) {
            return Err(invalid());
        }
        result.push(value);
    }
    result.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok((result, truncated))
}
async fn lookup(client: &CoreClient, name: &str) -> Result<Option<Value>, AppError> {
    let (values, _) = entries(client, true).await?;
    Ok(values.into_iter().find(|v| v["name"] == name))
}
fn require_policy(client: &CoreClient, operate: bool) -> Result<(), AppError> {
    common::require_edit(client)?;
    if operate && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            correction: Some(
                "Ordinary variable creation, value updates and deletion can affect future automatic consumers. Both operate and configuration-edit grants are required; no request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, OP, None)
        });
    }
    Ok(())
}
async fn snapshot(client: &CoreClient, name: &str) -> Result<Value, AppError> {
    let caller = client.current_caller().await?;
    if caller["enabled"] != true || caller["admin"] != true {
        return Err(AppError {
            correction: Some(
                "Core variable writes require an enabled administrator. Metadata inspection remains available without switching credentials.",
            ),
            ..AppError::new(ErrorKind::PermissionDenied, OP, None)
        });
    }
    let value = lookup(client, name).await?;
    let result = json!({"caller":caller,"variable":value});
    if result.to_string().len() > MAX_PRIVATE {
        return Err(invalid());
    }
    Ok(result)
}
fn prepared(change: &Change, current: &Value) -> Result<Value, AppError> {
    if let Change::CreateOrdinary {
        name,
        value,
        description,
    } = change
    {
        if !current.is_null() {
            return Err(args(
                "This exact name already exists; inspect it and prepare an update instead",
            ));
        }
        return Ok(json!({"name":name,"value":value,"description":description,"is_secret":false}));
    }
    if current.is_null() {
        return Err(AppError::new(ErrorKind::NotFound, OP, None));
    }
    if change.needs_operate() && current["is_secret"] != false {
        return Err(args(
            "Secret entries support metadata inspection and description edits only; ordinary-value updates, deletion and reclassification are forbidden",
        ));
    }
    let mut next = current.clone();
    match change {
        Change::SetOrdinaryValue { value, .. } => next["value"] = json!(value),
        Change::SetDescription { description, .. } => next["description"] = json!(description),
        Change::DeleteOrdinary { .. } => next = Value::Null,
        Change::CreateOrdinary { .. } => unreachable!(),
    }
    Ok(next)
}
fn text(client: &CoreClient, input: &str, max: usize) -> (String, bool) {
    let safe = common::bounded_text(client, input, usize::MAX);
    let safe: String = safe
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let mut end = safe.len().min(max);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    (safe[..end].to_owned(), end < safe.len())
}
fn ordinary_value(client: &CoreClient, value: &str) -> (Option<String>, &'static str) {
    // Omit whole unsafe values before clipping; this is not a secret scanner or
    // a claim that an administrator's ordinary classification is correct.
    if value.len() > 1024 {
        return (None, "too_long_omitted");
    }
    let lower = value.to_ascii_lowercase();
    if client.redact(value) != value
        || value.chars().any(char::is_control)
        || value.contains("://")
        || [
            "password",
            "secret",
            "token",
            "api_key",
            "private_key",
            "authorization",
            "-----begin",
        ]
        .iter()
        .any(|s| lower.contains(s))
    {
        return (None, "unsafe_omitted");
    }
    (Some(value.to_owned()), "available")
}
fn view(client: &CoreClient, value: &Value, include_value: bool) -> Option<VariableView> {
    if value.is_null() {
        return None;
    }
    let secret = value["is_secret"].as_bool().expect("validated");
    let raw_description = value["description"].as_str().expect("validated");
    let secret_value = value["value"].as_str().expect("validated");
    let (description, description_truncated) =
        if secret && !secret_value.is_empty() && raw_description.contains(secret_value) {
            ("[omitted]".into(), false)
        } else {
            text(client, raw_description, 512)
        };
    let (value_text, value_status) = if secret {
        (None, "secret_omitted")
    } else if !include_value {
        (None, "not_requested")
    } else {
        ordinary_value(client, value["value"].as_str().expect("validated"))
    };
    Some(VariableView {
        name: common::bounded_text(client, value["name"].as_str().expect("validated"), 500),
        description,
        description_truncated,
        is_secret: secret,
        value: value_text,
        value_status,
    })
}
pub async fn list(client: &CoreClient, input: PageInput) -> Result<VariablePage, AppError> {
    let offset = input.offset()?;
    let (values, truncated) = entries(client, false).await?;
    let next_page = (offset.saturating_add(input.limit) < values.len()).then_some(input.page + 1);
    Ok(VariablePage {
        items: values
            .iter()
            .skip(offset)
            .take(input.limit)
            .filter_map(|v| view(client, v, false))
            .collect(),
        page: input.page,
        next_page,
        scanned: values.len(),
        scan_truncated: truncated,
        evidence: "One non-atomic, unpaginated Core response, at most 512 entries scanned and 2 MiB read. Local pages do not bound Core's internal database enumeration. Values are omitted from lists; secret classification is validated explicitly.",
    })
}
pub async fn get(client: &CoreClient, input: NameInput) -> Result<VariableView, AppError> {
    validate_name(&input.name)?;
    let value = lookup(client, &input.name)
        .await?
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, OP, None))?;
    view(client, &value, true).ok_or_else(invalid)
}
pub async fn core_secret_names(
    client: &CoreClient,
    input: PageInput,
) -> Result<CoreSecretNames, AppError> {
    let offset = input.offset()?;
    let raw = client.read_references(ListSecrets { target: None }).await?;
    let values = raw.as_array().ok_or_else(invalid)?;
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    let mut unsafe_names = 0;
    for value in values.iter().take(MAX_SCAN) {
        let name = value.as_str().ok_or_else(invalid)?;
        if !seen.insert(name) {
            return Err(invalid());
        }
        if name.is_empty()
            || name.len() > 500
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || client.redact(name) != name
        {
            unsafe_names += 1;
            continue;
        }
        names.push(name.to_owned());
    }
    names.sort();
    let next_page = (offset.saturating_add(input.limit) < names.len()).then_some(input.page + 1);
    Ok(CoreSecretNames {
        scope: "configured_core_secret_key_names",
        names: names.into_iter().skip(offset).take(input.limit).collect(),
        page: input.page,
        next_page,
        scan_truncated: values.len() > MAX_SCAN,
        unsafe_names_omitted: unsafe_names,
        evidence: "Authenticated Core ListSecrets with target=None returns configured Core key names only. These are distinct from is_secret variables. No Server/Builder expansion, configured values, host reads or file writes; no independent completeness guarantee.",
    })
}
pub async fn preview(client: &CoreClient, input: PreviewInput) -> Result<VariableResult, AppError> {
    common::require_edit(client)?;
    input.change.validate(client)?;
    let baseline = snapshot(client, input.change.name()).await?;
    let next = prepared(&input.change, &baseline["variable"])?;
    let result = VariableResult {
        action: input.change.action(),
        outcome: if next == baseline["variable"] {
            "no_op"
        } else {
            "preview"
        },
        preview_id: None,
        before: view(client, &baseline["variable"], true),
        after: view(client, &next, true),
        writes_submitted: 0,
        acknowledgement_valid: false,
        saved_state_matches: false,
        observation_error: None,
        submission_error: None,
        effects: EFFECTS.to_vec(),
    };
    let mut result_copy = result.clone();
    result_copy.preview_id = Some(client.previews().insert_entry(Preview {
        created: Instant::now(),
        input: common::bound_input("variable_change", &input.change),
        baseline,
        prepared: next,
        result,
    })?);
    Ok(result_copy)
}

/// Called by the narrowly typed shared transport, including direct Rust entry
/// points. Guards and fresh raw baselines precede the only write attempt.
pub(crate) async fn guard_write(
    client: &CoreClient,
    operation: &str,
    params: &Value,
    baseline: &Value,
) -> Result<(), AppError> {
    require_policy(client, operation != "UpdateVariableDescription")?;
    let fields: &[&str] = match operation {
        "CreateVariable" => &["name", "value", "description", "is_secret"],
        "UpdateVariableValue" => &["name", "value"],
        "UpdateVariableDescription" => &["name", "description"],
        "DeleteVariable" => &["name"],
        _ => return Err(args("Unsupported variable operation")),
    };
    if !params
        .as_object()
        .is_some_and(|p| p.len() == fields.len() && p.keys().all(|k| fields.contains(&k.as_str())))
    {
        return Err(args(
            "Use only the exact typed fields for the selected variable operation",
        ));
    }
    let change:Change=match operation {
        "CreateVariable" if params["is_secret"]==false => serde_json::from_value(json!({"kind":"create_ordinary","name":params["name"],"value":params["value"],"description":params["description"]})).map_err(|_|invalid())?,
        "UpdateVariableValue" => serde_json::from_value(json!({"kind":"set_ordinary_value","name":params["name"],"value":params["value"]})).map_err(|_|invalid())?,
        "UpdateVariableDescription" => serde_json::from_value(json!({"kind":"set_description","name":params["name"],"description":params["description"]})).map_err(|_|invalid())?,
        "DeleteVariable" => serde_json::from_value(json!({"kind":"delete_ordinary","name":params["name"]})).map_err(|_|invalid())?,
        _=>return Err(args("Only ordinary creation/value/deletion and description edits are supported; no secret writes or classification changes")),
    };
    change.validate(client)?;
    let current = snapshot(client, change.name()).await?;
    if &current != baseline {
        return Err(stale());
    }
    prepared(&change, &current["variable"])?;
    Ok(())
}
pub async fn apply(client: &CoreClient, input: ApplyInput) -> Result<VariableResult, AppError> {
    require_policy(client, input.change.needs_operate())?;
    input.change.validate(client)?;
    if input.preview_id.len() > 128 {
        return Err(args("Use the opaque preview_id returned by this process"));
    }
    let review = client.previews().take_for::<VariableResult>(
        &input.preview_id,
        &common::bound_input("variable_change", &input.change),
    )?;
    if snapshot(client, input.change.name()).await? != review.baseline {
        return Err(stale());
    }
    let mut result = review.result;
    result.preview_id = None;
    if review.prepared == review.baseline["variable"] {
        result.outcome = "no_op";
        result.saved_state_matches = true;
        return Ok(result);
    }
    let submission = match input.change.clone() {
        Change::CreateOrdinary {
            name,
            value,
            description,
        } => {
            client
                .variable_write_checked(
                    CreateVariable {
                        name,
                        value,
                        description,
                        is_secret: false,
                    },
                    &review.baseline,
                )
                .await?
        }
        Change::SetOrdinaryValue { name, value } => {
            client
                .variable_write_checked(UpdateVariableValue { name, value }, &review.baseline)
                .await?
        }
        Change::SetDescription { name, description } => {
            client
                .variable_write_checked(
                    UpdateVariableDescription { name, description },
                    &review.baseline,
                )
                .await?
        }
        Change::DeleteOrdinary { name } => {
            client
                .variable_write_checked(DeleteVariable { name }, &review.baseline)
                .await?
        }
    };
    result.writes_submitted = 1;
    match submission {
        Ok(ack) => {
            let expected = if matches!(input.change, Change::DeleteOrdinary { .. }) {
                &review.baseline["variable"]
            } else {
                &review.prepared
            };
            result.acknowledgement_valid = raw_variable(&ack).is_ok_and(|v| &v == expected);
            if !result.acknowledgement_valid {
                result.submission_error = Some(invalid());
            }
        }
        Err(mut error) => {
            if error.kind == ErrorKind::PermissionDenied {
                error.message = "Core denied the variable write.";
                error.correction = Some(
                    "Core variable writes require an enabled administrator. Inspect authority using the same credential; never automatically retry or select another credential after denial.",
                );
            }
            result.submission_error = Some(error);
        }
    }
    match lookup(client, input.change.name()).await {
        Ok(value) => result.saved_state_matches = value.unwrap_or(Value::Null) == review.prepared,
        Err(error) => result.observation_error = Some(error),
    }
    result.outcome = if result.acknowledgement_valid && result.saved_state_matches {
        if review.prepared.is_null() {
            "deleted"
        } else {
            "saved"
        }
    } else if result.submission_error.as_ref().is_some_and(|e| {
        matches!(
            e.kind,
            ErrorKind::Authentication | ErrorKind::PermissionDenied
        )
    }) {
        "rejected"
    } else if !result.acknowledgement_valid {
        "submission_unknown"
    } else {
        "verification_failed"
    };
    Ok(result)
}

/// Block value-bearing variable history even after restart and even when the
/// secret was written by another administrator rather than this MCP process.
pub(crate) fn private_history(operation: komodo_client::entities::Operation) -> bool {
    use komodo_client::entities::Operation;
    // Description/classification writes have no Operation/history variant in
    // pinned Core. These are all three recorded variable operation kinds.
    matches!(
        operation,
        Operation::CreateVariable | Operation::UpdateVariableValue | Operation::DeleteVariable
    )
}
