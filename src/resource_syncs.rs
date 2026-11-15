//! ResourceSync definitions. Source reads/cache effects are distinct from RunSync.
use std::time::Instant;

use komodo_client::{
    api::{
        read::GetResourceSync,
        write::{CreateResourceSync, UpdateResourceSync},
    },
    entities::{
        sync::{PartialResourceSyncConfig, ResourceSync, ResourceSyncConfig, ResourceSyncListItem},
        to_general_name,
        toml::ResourcesToml,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget, Preview,
        present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
    repositories,
};

const OP: &str = "resource_sync_configuration";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// Complete trusted desired-state TOML. Omitted desired fields can reset defaults.
    Inline { contents: String },
    /// Omitted options preserve their saved values; creation requires resource_paths.
    Git {
        repo: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        branch: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        provider: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "bool")]
        https: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        account: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        commit: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "Vec<String>")]
        resource_paths: Option<Vec<String>>,
    },
    LinkedRepo {
        target: DetailInput,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "Vec<String>")]
        resource_paths: Option<Vec<String>>,
    },
    /// Core-host paths under sync_directory / normalized sync name, never MCP-host paths.
    Host { resource_paths: Vec<String> },
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SyncPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Source")]
    pub source: Option<Source>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub managed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub delete: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_resources: Option<bool>,
    /// [] deliberately removes the resource-tag restriction. Variables/groups are separate.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<String>")]
    pub match_tags: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_variables: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_user_groups: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub pending_alert: Option<bool>,
    /// Creation explicitly defaults false. Enabling requires both startup grants.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub webhook_enabled: Option<bool>,
    /// Original secret only; "" restores Core's default secret. Never returned.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub webhook_secret: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: SyncPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: SyncPatch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: SyncPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: SyncPatch,
    pub preview_id: String,
}
fn create_config_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[g.subschema_for::<SyncPatch>()],"required":["source"]})
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct SourceSummary {
    pub kind: &'static str,
    pub linked_repo_id: Option<String>,
    pub git: Option<repositories::SourceSummary>,
    pub resource_paths: Vec<String>,
    pub resource_path_count: usize,
    pub inline_content_configured: bool,
    pub truncated: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub source: SourceSummary,
    pub managed: bool,
    pub delete: bool,
    pub effective_delete: bool,
    pub include_resources: bool,
    pub match_tags: Vec<String>,
    pub resource_tag_filter_unrestricted: bool,
    pub include_variables: bool,
    pub include_user_groups: bool,
    pub pending_alert: bool,
    pub webhook_enabled: bool,
    pub webhook_secret_configured: bool,
    pub truncated: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
    pub reference_validation: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct SyncSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub source_kind: &'static str,
    pub managed: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct SyncDetail {
    pub summary: SyncSummary,
    pub definition: DefinitionSummary,
    pub effects: Vec<&'static str>,
}

fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use bounded typed source/settings, original secret values or references, relative Core paths without traversal, at most 32 paths/tags, and explicit supported clears; omission preserves definition fields",
    )
}
fn id(s: &str) -> Option<String> {
    (s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.into())
}
fn check_target(t: &DetailInput) -> Result<(), AppError> {
    if t.name
        .as_ref()
        .is_some_and(|s| s.len() > 128 || s.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    Ok(())
}
fn paths_valid(paths: &[String]) -> bool {
    paths.len() <= 32
        && paths.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 512
                && !p.starts_with('/')
                && !p.contains('\\')
                && !p.chars().any(char::is_control)
                && !p.contains("://")
                && p.split('/').all(|s| !s.is_empty() && s != "..")
        })
}
/// Match pinned Core's triple-quoted-string backslash adaptation before typed parsing.
fn validate_inline(contents: &str) -> Result<(), AppError> {
    parse_toml(contents).map(|_| ())
}

pub(crate) fn parse_toml(contents: &str) -> Result<ResourcesToml, AppError> {
    let adapted = contents
        .split("\"\"\"")
        .enumerate()
        .map(|(i, s)| {
            if i % 2 == 1 {
                s.replace('\\', "\\\\")
            } else {
                s.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\"\"\"");
    let value: toml::Value = toml::from_str(&adapted).map_err(|_| AppError {
        field: Some("source.contents"),
        ..AppError::arguments(OP, "Inline content must be valid TOML; check table headers, quoting and value syntax. Parser snippets and values are withheld because desired state may contain secrets")
    })?;
    let allowed = [
        "server",
        "servers",
        "swarm",
        "swarms",
        "stack",
        "stacks",
        "deployment",
        "deployments",
        "build",
        "builds",
        "repo",
        "repos",
        "builder",
        "builders",
        "alerter",
        "alerters",
        "procedure",
        "procedures",
        "action",
        "actions",
        "resource_sync",
        "resource_syncs",
        "variable",
        "variables",
        "user_group",
        "user_groups",
    ];
    if value
        .as_table()
        .is_none_or(|t| t.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(AppError::arguments(
            OP,
            "Inline TOML has an unsupported top-level section; use Core resource, variable or user_group arrays. Unknown section names are withheld",
        ));
    }
    toml::from_str::<ResourcesToml>(&adapted).map_err(|_| AppError {
        field: Some("source.contents"),
        ..AppError::arguments(OP, "Inline TOML does not match Core 2.3.2 resource types; check required names, array tables and typed config values. No desired-state snippet is returned")
    })
}
impl SyncPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        if self.match_tags.as_ref().is_some_and(|v| {
            v.len() > 32
                || v.iter().any(|s| {
                    s.trim().is_empty() || s.len() > 128 || s.chars().any(char::is_control)
                })
        }) || self
            .webhook_secret
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.chars().any(char::is_control))
        {
            return Err(invalid());
        }
        match &self.source {
            Some(Source::Inline { contents }) => validate_inline(contents)?,
            Some(Source::Git {
                repo,
                branch,
                provider,
                account,
                commit,
                resource_paths,
                ..
            }) => {
                if !repositories::locator(repo)
                    || branch.as_deref().is_some_and(|s| !repositories::locator(s))
                    || provider
                        .as_deref()
                        .is_some_and(|s| !repositories::provider(s))
                    || account.as_ref().is_some_and(|s| {
                        s.len() > 256 || s.chars().any(char::is_control) || s.contains("://")
                    })
                    || commit
                        .as_ref()
                        .is_some_and(|s| s.len() > 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
                    || resource_paths.as_ref().is_some_and(|p| !paths_valid(p))
                {
                    return Err(invalid());
                }
            }
            Some(Source::LinkedRepo {
                target,
                resource_paths,
            }) => {
                check_target(target)?;
                if resource_paths.as_ref().is_some_and(|p| !paths_valid(p)) {
                    return Err(invalid());
                }
            }
            Some(Source::Host { resource_paths }) if !paths_valid(resource_paths) => {
                return Err(invalid());
            }
            _ => {}
        }
        Ok(())
    }
    fn partial(&self) -> PartialResourceSyncConfig {
        PartialResourceSyncConfig {
            managed: self.managed,
            delete: self.delete,
            include_resources: self.include_resources,
            match_tags: self.match_tags.clone(),
            include_variables: self.include_variables,
            include_user_groups: self.include_user_groups,
            pending_alert: self.pending_alert,
            webhook_enabled: self.webhook_enabled,
            webhook_secret: self.webhook_secret.clone(),
            ..Default::default()
        }
    }
}
pub(crate) fn update_requires_operate(config: &Value) -> bool {
    config.as_object().is_none_or(|fields| {
        fields.iter().any(|(key, value)| match key.as_str() {
            "pending_alert" => false,
            "webhook_enabled" => value != false,
            _ => true,
        })
    })
}
pub(crate) fn require_operate(
    client: &CoreClient,
    required: bool,
    operation: &'static str,
) -> Result<(), AppError> {
    if required && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "Execution-bearing ResourceSync definition updates and enabling webhooks require both local grants, even if webhooks currently appear disabled. Automatic execution policy can race; no Core request was submitted",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, operation, None)
        });
    }
    Ok(())
}
pub(crate) fn guard_write(
    client: &CoreClient,
    op: &'static str,
    params: &Value,
) -> Result<(), AppError> {
    require_operate(
        client,
        (op == "CreateResourceSync" && params["config"]["webhook_enabled"] != false)
            || (op == "UpdateResourceSync" && update_requires_operate(&params["config"])),
        op,
    )
}
fn proposed(
    current: Option<&ResourceSyncConfig>,
    p: &PartialResourceSyncConfig,
) -> Result<ResourceSyncConfig, AppError> {
    let mut v = serde_json::to_value(current.cloned().unwrap_or_default()).expect("config");
    v.as_object_mut().expect("object").extend(
        common::partial_value(p)?
            .as_object()
            .expect("partial")
            .clone(),
    );
    serde_json::from_value(v).map_err(|_| invalid())
}
async fn prepare(
    client: &CoreClient,
    p: &SyncPatch,
    current: Option<&ResourceSyncConfig>,
) -> Result<(PartialResourceSyncConfig, Value), AppError> {
    let mut partial = p.partial();
    if let Some(source) = &p.source {
        // Clear Core's higher-precedence selectors deliberately, keeping unrelated
        // inactive options and omitted secret references intact.
        partial.files_on_host = Some(matches!(source, Source::Host { .. }));
        partial.file_contents = Some(String::new());
        partial.linked_repo = Some(String::new());
        partial.repo = Some(String::new());
        match source {
            Source::Inline { contents } => partial.file_contents = Some(contents.clone()),
            Source::Host { resource_paths } => partial.resource_path = Some(resource_paths.clone()),
            Source::Git {
                repo,
                branch,
                provider,
                https,
                account,
                commit,
                resource_paths,
            } => {
                partial.repo = Some(repo.clone());
                partial.branch = branch.clone();
                partial.git_provider = provider.clone();
                partial.git_https = *https;
                partial.git_account = account.clone();
                partial.commit = commit.clone();
                partial.resource_path = resource_paths.clone();
            }
            Source::LinkedRepo {
                target,
                resource_paths,
            } => {
                let (_, repo) = inventory::resolve_repo(client, target.clone()).await?;
                if repo.template {
                    return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
                }
                partial.linked_repo = Some(repo.id);
                partial.resource_path = resource_paths.clone();
            }
        }
    }
    if current.is_none() && partial.webhook_enabled.is_none() {
        partial.webhook_enabled = Some(false);
    }
    let partial: PartialResourceSyncConfig =
        serde_json::from_value(common::partial_value(&partial)?).map_err(|_| invalid())?;
    let after = proposed(current, &partial)?;
    let references = if !after.files_on_host && !after.linked_repo.is_empty() {
        let target = if let Some(id) = id(&after.linked_repo) {
            DetailInput {
                id: Some(id),
                name: None,
            }
        } else {
            DetailInput {
                id: None,
                name: Some(after.linked_repo.clone()),
            }
        };
        check_target(&target)?;
        let (_, repo) = inventory::resolve_repo(client, target).await?;
        if repo.template {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        common::resource_baseline("repo", &repo)
    } else {
        Value::Null
    };
    Ok((partial, references))
}
fn source_kind(c: &ResourceSyncConfig) -> &'static str {
    if c.files_on_host {
        "host"
    } else if !c.linked_repo.is_empty() {
        "linked_repo"
    } else if !c.repo.is_empty() {
        "git"
    } else if !c.file_contents.is_empty() || c.managed {
        "inline"
    } else {
        "unconfigured"
    }
}
pub(crate) fn summary_kind(c: &ResourceSyncConfig) -> &'static str {
    source_kind(c)
}
pub(crate) fn summary(client: &CoreClient, c: &ResourceSyncConfig) -> DefinitionSummary {
    let source = SourceSummary {
        kind: source_kind(c),
        linked_repo_id: id(&c.linked_repo),
        git: (source_kind(c) == "git").then(|| repositories::SourceSummary {
            repo: repositories::safe_source(client, &c.repo, repositories::locator(&c.repo)),
            provider: repositories::safe_source(
                client,
                &c.git_provider,
                repositories::provider(&c.git_provider),
            ),
            branch: repositories::safe_source(client, &c.branch, repositories::locator(&c.branch)),
            configured_commit: repositories::revision(client, &c.commit),
            https: c.git_https,
            credential_reference_configured: !c.git_account.is_empty(),
        }),
        resource_paths: c
            .resource_path
            .iter()
            .take(32)
            .map(|s| {
                if paths_valid(std::slice::from_ref(s)) {
                    common::bounded_text(client, s, 256)
                } else {
                    client.redaction_marker().into()
                }
            })
            .collect(),
        resource_path_count: c.resource_path.len(),
        inline_content_configured: !c.file_contents.is_empty(),
        truncated: c.resource_path.len() > 32
            || c.resource_path.iter().any(|s| s.chars().count() > 256),
    };
    DefinitionSummary {
        source,
        managed: c.managed,
        delete: c.delete,
        effective_delete: c.managed || c.delete,
        include_resources: c.include_resources,
        match_tags: c
            .match_tags
            .iter()
            .take(32)
            .map(|s| common::bounded_text(client, s, 128))
            .collect(),
        resource_tag_filter_unrestricted: c.match_tags.is_empty(),
        include_variables: c.include_variables,
        include_user_groups: c.include_user_groups,
        pending_alert: c.pending_alert,
        webhook_enabled: c.webhook_enabled,
        webhook_secret_configured: !c.webhook_secret.is_empty(),
        truncated: c.match_tags.len() > 32 || c.match_tags.iter().any(|s| s.chars().count() > 128),
    }
}
fn project(client: &CoreClient, r: ResourceSyncListItem) -> SyncSummary {
    let kind = if r.info.files_on_host {
        "host"
    } else if !r.info.linked_repo.is_empty() {
        "linked_repo"
    } else if !r.info.repo.is_empty() {
        "git"
    } else if r.info.file_contents || r.info.managed {
        "inline"
    } else {
        "unconfigured"
    };
    SyncSummary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 128),
        template: r.template,
        state: r.info.state.to_string(),
        source_kind: kind,
        managed: r.info.managed,
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<SyncSummary>, AppError> {
    let page = inventory::list_resource_sync_records(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: page.items.into_iter().map(|r| project(client, r)).collect(),
        page: page.page,
        page_size: page.page_size,
        has_more: page.has_more,
        next_page: page.next_page,
    })
}
fn effects() -> Vec<&'static str> {
    vec![
        "Saving uses one typed definition write and submits no RunSync, CommitSync, WriteSyncFileContents or deployment. Core create/update hooks refresh pending state through its built-in sync administrator, can fetch/clone Git into Core caches, read Core-host files and change pending alerts. A saved definition does not establish successful source parsing or application.",
        "Core requires an administrator to create a ResourceSync, Write to update it and Read/Attach for a supplied linked Repo. No credential fallback is used. RunSync is separate privileged desired-state automation through Core's built-in administrator sync identity.",
        "Omitted definition-patch fields preserve saved values and secret references. Inline TOML is complete desired state: omitted desired resource fields can reset existing configuration to defaults. Sanitized export is not a lossless apply input. Source switching clears conflicting selectors but never edits authoritative Git or host files.",
        "Managed mode enables deletion even with delete=false. An empty resource-tag filter removes that restriction; variables and groups have independent inclusion and selection behavior. Empty resource_paths reads no external files, not a default file or directory. Review desired-state effects separately before RunSync.",
        "Core source precedence is host files, linked Repo, direct Git, then inline. Host paths are relative to Core sync_directory / normalized sync name, not the MCP machine. Paths are validated lexically; Core still controls filesystem access and symlinks.",
        "New definitions explicitly default webhooks off. Execution-bearing updates and enabling webhooks require both local grants independently of the observed webhook state; disabling webhooks or changing pending_alert alone needs edit. Automatic triggers and concurrent Core changes can still race.",
        "Previews are bounded, process-local, expire after ten minutes and are consumed once before apply I/O. Raw definition and relevant active linked Repo baselines are rechecked privately, including secret-only changes. No-op/stale proposals write zero times. One partial write is followed by full authoritative read-back; no conditional write or automatic retry is available.",
    ]
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<SyncDetail, AppError> {
    check_target(&input)?;
    let (listed, sync) =
        inventory::resolve_resource_sync(&client.bounded_evidence(), input).await?;
    Ok(SyncDetail {
        summary: project(client, listed),
        definition: summary(client, &sync.config),
        effects: effects(),
    })
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&ResourceSyncConfig>,
    p: &PartialResourceSyncConfig,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(p)?;
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "resource_sync",
            id,
            name: common::bounded_text(client, name, 128),
        },
        preview_id: None,
        changes: common::changes(
            client,
            &current.map_or(json!({}), |c| serde_json::to_value(c).expect("config")),
            &patch,
        ),
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: effects(),
        definition: Some(common::DefinitionPreview::ResourceSync(Box::new(
            DefinitionPreview {
                before: current.map(|c| summary(client, c)),
                after: summary(client, &proposed(current, p)?),
                requires_operate_to_apply: if current.is_some() {
                    update_requires_operate(&patch)
                } else {
                    p.webhook_enabled == Some(true)
                },
                reference_validation: "The active linked Repo definition is bound and revalidated even when the source field is omitted. Core retains authoritative Attach and secret/token availability decisions. External file contents are not frozen by a definition preview.",
            },
        ))),
    })
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<ResourceSync, AppError>,
    patch: &Value,
    expected: (&ResourceSyncConfig, &str, Option<&str>),
    op: &'static str,
) -> ConfigurationResult {
    let (config, name, id) = expected;
    let mut r = common::finish_save(r, response, patch, name, id, op, |r, c| {
        if let Some(common::DefinitionPreview::ResourceSync(d)) = &mut r.definition {
            d.after = summary(client, c);
        }
    });
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let read = client
            .read_references(GetResourceSync {
                sync: r.resource.id.clone().expect("saved identity"),
            })
            .await;
        r = common::verify_readback(
            r,
            read,
            &serde_json::to_value(config).expect("config"),
            name,
            "GetResourceSync",
        );
    }
    r
}
pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    input.patch.validate(client)?;
    check_target(&input.target)?;
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_resource_sync(&bounded, input.target.clone()).await?;
    if current.template {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
    }
    let (partial, references) = prepare(&bounded, &input.patch, Some(&current.config)).await?;
    let r = result(
        client,
        Some(current.id.clone()),
        &current.name,
        Some(&current.config),
        &partial,
    )?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("resource_sync_update", &input),
        baseline: common::resource_baseline("resource_sync", &current),
        prepared: json!({"config":common::partial_value(&partial)?,"references":references}),
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_operate(
        client,
        update_requires_operate(&serde_json::to_value(&input.patch).expect("typed patch")),
        "UpdateResourceSync",
    )?;
    input.patch.validate(client)?;
    check_target(&input.target)?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("resource_sync_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_resource_sync(&bounded, input.target).await?;
    if entry.baseline != common::resource_baseline("resource_sync", &current) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let (partial, references) = prepare(&bounded, &input.patch, Some(&current.config)).await?;
    let patch = common::partial_value(&partial)?;
    if entry.prepared != json!({"config":patch,"references":references}) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let expected = proposed(Some(&current.config), &partial)?;
    let response = client
        .write_checked(UpdateResourceSync {
            id: current.id.clone(),
            config: partial,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&expected, &current.name, Some(&current.id)),
        "UpdateResourceSync",
    )
    .await)
}
fn create_name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    input.config.validate(client)?;
    let name = to_general_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || name.contains(['/', '\\'])
        || name == "."
        || name == ".."
        || id(&name).is_some()
    {
        return Err(invalid());
    }
    match input.config.source.as_ref() {
        None
        | Some(Source::Git {
            resource_paths: None,
            ..
        })
        | Some(Source::LinkedRepo {
            resource_paths: None,
            ..
        }) => {
            return Err(AppError::arguments(
                OP,
                "Creation requires an explicit source and explicit resource_paths for external sources; [] deliberately reads no files",
            ));
        }
        _ => {}
    }
    Ok(name)
}
async fn require_absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_resource_sync(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(
            ErrorKind::StalePreview,
            "CreateResourceSync",
            None,
        )),
        Err(e) => Err(e),
    }
}
pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let name = create_name(client, &input)?;
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let (partial, references) = prepare(&bounded, &input.config, None).await?;
    let r = result(client, None, &name, None, &partial)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("resource_sync_create", &input),
        baseline: json!({"kind":"resource_sync_create","name":name}),
        prepared: json!({"config":common::partial_value(&partial)?,"references":references}),
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_operate(
        client,
        input.config.webhook_enabled == Some(true),
        "CreateResourceSync",
    )?;
    let submitted = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = create_name(client, &submitted)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("resource_sync_create", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let (partial, references) = prepare(&bounded, &submitted.config, None).await?;
    let patch = common::partial_value(&partial)?;
    if entry.baseline != json!({"kind":"resource_sync_create","name":name})
        || entry.prepared != json!({"config":patch,"references":references})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let expected = proposed(None, &partial)?;
    let response = client
        .write_checked(CreateResourceSync {
            name: name.clone(),
            config: partial,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&expected, &name, None),
        "CreateResourceSync",
    )
    .await)
}
