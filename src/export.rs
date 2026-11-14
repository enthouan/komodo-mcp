//! Explicitly scoped, conservative inspection exports; never an apply baseline.
use std::collections::BTreeSet;

use komodo_client::{
    api::read::{ExportResourcesToToml, ListUserGroups},
    entities::ResourceTarget,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
};

const OP: &str = "ExportResourcesToToml";
pub const MAX_TARGETS: usize = 16;
pub const MAX_SOURCE_BYTES: usize = 128 * 1024;
pub const MAX_TOML_BYTES: usize = 64 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 128;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Server,
    Swarm,
    Stack,
    Deployment,
    Build,
    Repo,
    Builder,
    Alerter,
    Procedure,
    Action,
    ResourceSync,
}
impl Kind {
    fn header(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Swarm => "swarm",
            Self::Stack => "stack",
            Self::Deployment => "deployment",
            Self::Build => "build",
            Self::Repo => "repo",
            Self::Builder => "builder",
            Self::Alerter => "alerter",
            Self::Procedure => "procedure",
            Self::Action => "action",
            Self::ResourceSync => "resource_sync",
        }
    }
    fn core_target(self, id: String) -> ResourceTarget {
        match self {
            Self::Server => ResourceTarget::Server(id),
            Self::Swarm => ResourceTarget::Swarm(id),
            Self::Stack => ResourceTarget::Stack(id),
            Self::Deployment => ResourceTarget::Deployment(id),
            Self::Build => ResourceTarget::Build(id),
            Self::Repo => ResourceTarget::Repo(id),
            Self::Builder => ResourceTarget::Builder(id),
            Self::Alerter => ResourceTarget::Alerter(id),
            Self::Procedure => ResourceTarget::Procedure(id),
            Self::Action => ResourceTarget::Action(id),
            Self::ResourceSync => ResourceTarget::ResourceSync(id),
        }
    }
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: Kind,
    pub target: DetailInput,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Explicit targets, at most 16. Use [] only with explicit groups/variables.
    #[schemars(length(max = 16))]
    pub targets: Vec<Target>,
    /// Exact visible group selectors, at most 16; not a blanket inclusion flag.
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub user_groups: Vec<DetailInput>,
    /// Core exports ALL variables, not individually selected variables.
    #[serde(default)]
    pub include_variables: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct Selected {
    pub kind: Kind,
    pub id: String,
    pub name: String,
    pub returned_by_core: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub returned_by_core: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct Output {
    pub toml: String,
    pub targets: Vec<Selected>,
    pub user_groups: Vec<Group>,
    pub variable_scope: &'static str,
    pub variables_returned: usize,
    /// True only when Core returned every explicitly selected resource/group.
    pub selection_complete: bool,
    /// Always false: omission/default resets make this inspection-only output.
    pub authoritative_apply_input: bool,
    pub omitted_fields: usize,
    pub mutation_requests: u8,
    pub warnings: Vec<&'static str>,
}

fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
fn too_large() -> AppError {
    AppError {
        message: "The export exceeded its bounded inspection budget; no partial TOML is returned.",
        correction: Some(
            "Select fewer resources/groups or exclude the all-variables scope. No mutation was submitted.",
        ),
        ..AppError::new(ErrorKind::ResponseTooLarge, OP, None)
    }
}
fn validate_selector(input: &DetailInput) -> Result<(), AppError> {
    match (&input.id, &input.name) {
        (Some(id), None) if crate::deletion::object_id(id) => Ok(()),
        (None, Some(name))
            if !name.trim().is_empty()
                && name.len() <= 128
                && !name.chars().any(char::is_control) =>
        {
            Ok(())
        }
        _ => Err(AppError::arguments(
            OP,
            "Use exactly one stable hexadecimal ID or a nonempty exact name of at most 128 bytes per selector",
        )),
    }
}

async fn select(client: &CoreClient, target: Target) -> Result<Selected, AppError> {
    macro_rules! resolve {
        ($f:ident) => {{
            let (summary, _) = inventory::$f(client, target.target).await?;
            (summary.id, summary.name)
        }};
    }
    let (id, name) = match target.kind {
        Kind::Server => resolve!(resolve_server),
        Kind::Swarm => resolve!(resolve_swarm),
        Kind::Stack => resolve!(resolve_stack),
        Kind::Deployment => resolve!(resolve_deployment),
        Kind::Build => resolve!(resolve_build),
        Kind::Repo => resolve!(resolve_repo),
        Kind::Builder => resolve!(resolve_builder),
        Kind::Alerter => resolve!(resolve_alerter),
        Kind::Procedure => resolve!(resolve_procedure),
        Kind::Action => resolve!(resolve_action),
        Kind::ResourceSync => resolve!(resolve_resource_sync),
    };
    if !safe_name(client, &name) {
        return Err(invalid());
    }
    Ok(Selected {
        kind: target.kind,
        id,
        name,
        returned_by_core: false,
    })
}

pub async fn export(client: &CoreClient, input: Input) -> Result<Output, AppError> {
    export_scoped(client, input).await.map_err(|error| {
        if error.kind == ErrorKind::ResponseTooLarge {
            AppError {
                status: error.status,
                ..too_large()
            }
        } else {
            error
        }
    })
}

async fn export_scoped(client: &CoreClient, input: Input) -> Result<Output, AppError> {
    if input.targets.len() > MAX_TARGETS
        || input.user_groups.len() > MAX_TARGETS
        || (input.targets.is_empty() && input.user_groups.is_empty() && !input.include_variables)
    {
        return Err(AppError::arguments(
            OP,
            "Explicitly select 1–16 resources, 1–16 user groups, or include_variables=true (all Core variables); there is no export-all fallback",
        ));
    }
    // Validate the entire selection before performing any Core I/O.
    for target in &input.targets {
        validate_selector(&target.target)?;
    }
    for group in &input.user_groups {
        validate_selector(group)?;
    }
    let client = client.bounded_evidence();
    let mut targets = Vec::new();
    let mut unique = BTreeSet::new();
    for target in input.targets {
        let selected = select(&client, target).await?;
        if !unique.insert((selected.kind, selected.id.clone())) {
            return Err(AppError::arguments(
                OP,
                "Select each resource only once, including aliases resolving to the same stable ID",
            ));
        }
        targets.push(selected);
    }
    let mut groups = Vec::new();
    if !input.user_groups.is_empty() {
        let visible = client.read(ListUserGroups {}).await?;
        let mut unique = BTreeSet::new();
        for selector in input.user_groups {
            let matches: Vec<_> = visible
                .iter()
                .filter(|g| match (&selector.id, &selector.name) {
                    (Some(id), None) => g.id.eq_ignore_ascii_case(id),
                    (None, Some(name)) => g.name == *name,
                    _ => false,
                })
                .collect();
            if matches.len() != 1 {
                return Err(AppError::new(
                    if matches.is_empty() {
                        ErrorKind::NotFound
                    } else {
                        ErrorKind::AmbiguousTarget
                    },
                    "ListUserGroups",
                    None,
                ));
            }
            let group = matches[0];
            if !crate::deletion::object_id(&group.id) || !safe_name(&client, &group.name) {
                return Err(invalid());
            }
            if !unique.insert(group.id.clone()) {
                return Err(AppError::arguments(
                    OP,
                    "Select each visible user group only once",
                ));
            }
            groups.push(Group {
                id: group.id.clone(),
                name: group.name.clone(),
                returned_by_core: false,
            });
        }
    }
    // Deliberately no ExportAllResourcesToToml or 'existing' input surface.
    let response = client
        .read(ExportResourcesToToml {
            targets: targets
                .iter()
                .map(|s| s.kind.core_target(s.id.clone()))
                .collect(),
            user_groups: groups.iter().map(|g| g.id.clone()).collect(),
            include_variables: input.include_variables,
            existing: None,
        })
        .await?;
    sanitize(
        &client,
        &response.toml,
        targets,
        groups,
        input.include_variables,
    )
}

fn safe_name(client: &CoreClient, value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 128
        && !value.chars().any(char::is_control)
        && client.redact(value) == value
}
fn reference(client: &CoreClient, value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() <= 128
            && client.redact(s) == s
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
    })
}
fn references(client: &CoreClient, value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|a| a.len() <= 32 && a.iter().all(|v| reference(client, v)))
}

/// Positive projection: unknown fields, arbitrary text, paths/URLs, inline
/// documents, commands, arguments, credentials and ALL variable values disappear.
/// Secret-looking-key blacklists cannot cover embedded Compose/JSON/scripts.
fn config(client: &CoreClient, kind: Kind, raw: &Table, omitted: &mut usize) -> Table {
    let mut safe = Table::new();
    for (key, value) in raw {
        let bool_field = match kind {
            Kind::Server => matches!(
                key.as_str(),
                "enabled"
                    | "insecure_tls"
                    | "auto_prune"
                    | "send_unreachable_alerts"
                    | "send_cpu_alerts"
                    | "send_mem_alerts"
                    | "send_disk_alerts"
            ),
            Kind::Stack => matches!(
                key.as_str(),
                "auto_pull"
                    | "auto_update"
                    | "destroy_before_deploy"
                    | "files_on_host"
                    | "git_https"
                    | "webhook_enabled"
                    | "webhook_force_deploy"
                    | "send_alerts"
            ),
            Kind::Deployment => matches!(
                key.as_str(),
                "auto_update" | "send_alerts" | "redeploy_on_build" | "skip_secret_interp"
            ),
            Kind::Repo => matches!(key.as_str(), "git_https" | "webhook_enabled" | "reclone"),
            Kind::Build => matches!(
                key.as_str(),
                "git_https"
                    | "webhook_enabled"
                    | "files_on_host"
                    | "auto_increment_version"
                    | "push_latest"
                    | "push_version"
                    | "push_commit"
            ),
            Kind::ResourceSync => matches!(
                key.as_str(),
                "git_https"
                    | "webhook_enabled"
                    | "files_on_host"
                    | "managed"
                    | "delete"
                    | "include_resources"
                    | "include_variables"
                    | "include_user_groups"
            ),
            Kind::Action => matches!(key.as_str(), "webhook_enabled" | "run_at_startup"),
            Kind::Procedure => matches!(key.as_str(), "webhook_enabled"),
            Kind::Alerter => key == "enabled",
            _ => false,
        };
        let ref_field = match kind {
            Kind::Stack | Kind::Deployment => {
                matches!(key.as_str(), "server" | "swarm" | "project_name")
            }
            Kind::Repo => matches!(key.as_str(), "server" | "builder" | "repo" | "branch"),
            Kind::Build => matches!(key.as_str(), "builder" | "linked_repo" | "repo" | "branch"),
            Kind::ResourceSync => matches!(key.as_str(), "linked_repo" | "repo" | "branch"),
            _ => false,
        };
        let refs_field = match kind {
            Kind::Swarm => key == "servers",
            Kind::Stack => key == "ignore_services",
            Kind::ResourceSync => key == "resource_tags",
            _ => false,
        };
        if (bool_field && value.is_bool())
            || (ref_field && reference(client, value))
            || (refs_field && references(client, value))
            || (kind == Kind::Builder
                && key == "type"
                && value
                    .as_str()
                    .is_some_and(|v| ["Server", "Url", "Aws"].contains(&v)))
        {
            safe.insert(key.clone(), value.clone());
        } else if kind == Kind::Builder && key == "params" {
            let mut params = Table::new();
            if let Some(fields) = value.as_table() {
                for (k, v) in fields {
                    if raw.get("type").and_then(Value::as_str) == Some("Server")
                        && k == "servers"
                        && references(client, v)
                    {
                        params.insert(k.clone(), v.clone());
                    } else {
                        *omitted += 1;
                    }
                }
            } else {
                *omitted += 1;
            }
            safe.insert(key.clone(), Value::Table(params));
        } else {
            *omitted += 1;
        }
    }
    safe
}

fn sanitize(
    client: &CoreClient,
    source: &str,
    mut targets: Vec<Selected>,
    mut groups: Vec<Group>,
    variables: bool,
) -> Result<Output, AppError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(too_large());
    }
    // Never include parser errors, snippets, comments, unknown keys or values.
    let document = source.parse::<Table>().map_err(|_| invalid())?;
    let mut safe = Table::new();
    let mut seen = BTreeSet::new();
    let mut count = 0;
    let mut omitted = 0;
    let mut variables_returned = 0;
    for (header, entries) in document {
        if !targets.iter().any(|t| t.kind.header() == header)
            && !(header == "user_group" && !groups.is_empty())
            && !(header == "variable" && variables)
        {
            return Err(invalid());
        }
        let entries = entries.as_array().ok_or_else(invalid)?;
        count += entries.len();
        if count > MAX_ENTRIES {
            return Err(too_large());
        }
        let mut output = Vec::new();
        for entry in entries {
            let entry = entry.as_table().ok_or_else(invalid)?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            if !safe_name(client, name) || !seen.insert((header.clone(), name.to_owned())) {
                return Err(invalid());
            }
            let kind = if header == "variable" && variables {
                variables_returned += 1;
                None
            } else if header == "user_group" {
                let g = groups
                    .iter_mut()
                    .find(|g| g.name == name)
                    .ok_or_else(invalid)?;
                g.returned_by_core = true;
                None
            } else {
                let t = targets
                    .iter_mut()
                    .find(|t| t.kind.header() == header && t.name == name)
                    .ok_or_else(invalid)?;
                t.returned_by_core = true;
                Some(t.kind)
            };
            let mut item = Table::new();
            item.insert("name".into(), Value::String(name.into()));
            for (key, value) in entry {
                if key == "name" {
                    continue;
                }
                if let Some(kind) = kind.filter(|_| key == "config") {
                    let projected = config(
                        client,
                        kind,
                        value.as_table().ok_or_else(invalid)?,
                        &mut omitted,
                    );
                    item.insert(key.clone(), Value::Table(projected));
                } else if (kind.is_some()
                    && matches!(key.as_str(), "template" | "deploy")
                    && value.is_bool())
                    || (kind.is_some()
                        && matches!(key.as_str(), "tags" | "after")
                        && references(client, value))
                    || (header == "variable" && key == "is_secret" && value.is_bool())
                    || (header == "user_group" && key == "everyone" && value.is_bool())
                {
                    item.insert(key.clone(), value.clone());
                } else {
                    omitted += 1;
                }
            }
            output.push(Value::Table(item));
        }
        if !output.is_empty() {
            safe.insert(header, Value::Array(output));
        }
    }
    let toml = format!(
        "# Sanitized inspection export; NOT authoritative desired state.\n# Restore omitted fields manually: applying omissions can reset configuration to defaults.\n{}",
        toml::to_string_pretty(&safe).map_err(|_| invalid())?
    );
    if toml.len() > MAX_TOML_BYTES {
        return Err(too_large());
    }
    // The shared final redactor must not change serialized TOML syntax or keys.
    if client.redact(&toml) != toml || toml.parse::<Table>().is_err() {
        return Err(invalid());
    }
    let selection_complete =
        targets.iter().all(|t| t.returned_by_core) && groups.iter().all(|g| g.returned_by_core);
    let mut warnings = vec![
        "Inspection only: arbitrary text, scripts/documents, paths/URLs, credentials, all variable values and unknown fields are omitted by a positive allowlist, including for administrators. Do not apply this export as authoritative desired state.",
        "Desired-state TOML omissions can reset existing configuration to defaults; restore and review the complete intended configuration separately.",
        "Selected definitions can reference resources outside this selection. Core defaults/minimization and metadata conversion do not establish a lossless round trip.",
    ];
    if !selection_complete {
        warnings.push("Core omitted one or more requested definitions. Core 2.3.2 does not export inline or unconfigured ResourceSyncs; other missing definitions are not represented as successful exports.");
    }
    if variables {
        warnings.push("include_variables selects all variables returned by Core, with no per-variable selector or independent completeness attestation. Every variable value is omitted, even when not marked secret.");
    }
    let output = Output {
        toml,
        targets,
        user_groups: groups,
        variable_scope: if variables {
            "all_core_variables"
        } else {
            "excluded"
        },
        variables_returned,
        selection_complete,
        authoritative_apply_input: false,
        omitted_fields: omitted,
        mutation_requests: 0,
        warnings,
    };
    if serde_json::to_vec(&inventory::projected_value(client, &output))
        .map_err(|_| invalid())?
        .len()
        > MAX_OUTPUT_BYTES
    {
        return Err(too_large());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::RequestPolicy, config::Config};

    #[test]
    fn credential_redaction_cannot_corrupt_serialized_toml_syntax() {
        for credential in ["false", "enabled", "server"] {
            let client = CoreClient::new(
                Config::parse("http://127.0.0.1:1", credential, "unique-test-secret").unwrap(),
                RequestPolicy::default(),
            )
            .unwrap();
            let selected = Selected {
                kind: Kind::Server,
                id: "000000000000000000000001".into(),
                name: "selected-resource".into(),
                returned_by_core: false,
            };
            let result = sanitize(
                &client,
                "[[server]]\nname='selected-resource'\n[server.config]\nenabled=false\n",
                vec![selected],
                vec![],
                false,
            );
            assert!(matches!(
                result,
                Err(AppError {
                    kind: ErrorKind::InvalidResponse,
                    ..
                })
            ));
        }
    }
}
