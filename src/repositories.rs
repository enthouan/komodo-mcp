//! Repo definitions and explicit clone/pull over the shared mutation contracts.

use std::time::Instant;

use komodo_client::{
    api::{
        execute::{CloneRepo, PullRepo},
        read::GetRepo,
        write::{CreateRepo, UpdateRepo},
    },
    entities::{
        Operation, SystemCommand,
        repo::{PartialRepoConfig, Repo, RepoConfig, RepoListItem},
        to_path_compatible_name,
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
    operations::{
        self, ConfiguredEffects, OperationResource, RequestedScope, SubmissionReceipt,
        WorkloadKind, WorkloadOperation,
    },
};

const OP: &str = "repo_configuration_edit";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Server {
        target: DetailInput,
    },
    /// Explicitly clear the Server association. Clone/pull then remain unavailable.
    Unassigned {},
}

/// Replacing a hook requires its complete trusted command. An empty command
/// explicitly disables that hook; omission of the hook preserves it.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub path: String,
    pub command: String,
    pub shell_mode: bool,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepoPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Target")]
    pub target: Option<Target>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub repo: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub branch: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub git_provider: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub git_https: Option<bool>,
    /// Account name/reference only; tokens remain in Core/Periphery. "" clears.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub git_account: Option<String>,
    /// "" clears a pinned commit; the configured branch is not an observed revision.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub commit: Option<String>,
    /// "" restores Core's name-derived destination. Paths may select trusted host locations.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub path: Option<String>,
    /// Enabling future webhook execution requires both local grants, including creation.
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
    #[schemars(with = "Hook")]
    pub on_clone: Option<Hook>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Hook")]
    pub on_pull: Option<Hook>,
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
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: RepoPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: RepoPatch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: RepoPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: RepoPatch,
    pub preview_id: String,
}
fn create_config_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[g.subschema_for::<RepoPatch>()],"required":["repo","branch","target"]})
}
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectedHooks {
    pub on_clone: bool,
    pub on_pull: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationInput {
    pub target: DetailInput,
    /// Copy both presence flags from get_repo.definition after inspecting the
    /// trusted execution effects. A mismatch stops submission for reinspection.
    pub expected_hooks: ExpectedHooks,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SourceSummary {
    pub repo: String,
    pub provider: String,
    pub branch: String,
    pub configured_commit: Option<String>,
    pub https: bool,
    pub credential_reference_configured: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub source: SourceSummary,
    pub server_id: Option<String>,
    pub custom_path_configured: bool,
    pub on_clone_configured: bool,
    pub on_pull_configured: bool,
    pub environment_configured: bool,
    pub webhook_enabled: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
    pub reference_validation: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct RepoSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub server_id: Option<String>,
    pub repo: String,
    pub provider: String,
    pub configured_branch: String,
    /// Current Core/Periphery cache, not proof of a particular execution's result.
    pub observed_cloned_revision: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct RepoDetail {
    #[serde(flatten)]
    pub summary: RepoSummary,
    pub definition: DefinitionSummary,
    pub revision_evidence: &'static str,
}

fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use documented typed fields, safe bounded source selectors without URL credentials, explicit supported clears, and complete trusted hook objects",
    )
}
pub(crate) fn locator(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
        && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
pub(crate) fn provider(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
        && reqwest::Url::parse(&format!("https://{s}/")).is_ok_and(|u| {
            u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.path() == "/"
                && u.query().is_none()
                && u.fragment().is_none()
        })
}
pub(crate) fn safe_source(client: &CoreClient, s: &str, valid: bool) -> String {
    if valid || s.is_empty() {
        common::bounded_text(client, s, 256)
    } else {
        client.redaction_marker().into()
    }
}
pub(crate) fn revision(client: &CoreClient, s: &str) -> Option<String> {
    ((4..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_hexdigit())
        && client.redact(s) == s)
        .then(|| s.into())
}
fn stable_id(id: &str) -> Option<String> {
    (id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.into())
}
pub(crate) fn summary(client: &CoreClient, c: &RepoConfig) -> DefinitionSummary {
    DefinitionSummary {
        source: SourceSummary {
            repo: safe_source(client, &c.repo, locator(&c.repo)),
            provider: safe_source(client, &c.git_provider, provider(&c.git_provider)),
            branch: safe_source(client, &c.branch, locator(&c.branch)),
            configured_commit: revision(client, &c.commit),
            https: c.git_https,
            credential_reference_configured: !c.git_account.is_empty(),
        },
        server_id: stable_id(&c.server_id),
        custom_path_configured: !c.path.is_empty(),
        on_clone_configured: !c.on_clone.is_none(),
        on_pull_configured: !c.on_pull.is_none(),
        environment_configured: !c.environment.is_empty(),
        webhook_enabled: c.webhook_enabled,
    }
}
fn project(client: &CoreClient, r: RepoListItem) -> RepoSummary {
    RepoSummary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 256),
        template: r.template,
        state: r.info.state.to_string(),
        server_id: stable_id(&r.info.server_id),
        repo: safe_source(client, &r.info.repo, locator(&r.info.repo)),
        provider: safe_source(client, &r.info.git_provider, provider(&r.info.git_provider)),
        configured_branch: safe_source(client, &r.info.branch, locator(&r.info.branch)),
        observed_cloned_revision: r
            .info
            .cloned_hash
            .as_deref()
            .and_then(|s| revision(client, s)),
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<RepoSummary>, AppError> {
    let page = inventory::list_repo_records(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: page.items.into_iter().map(|r| project(client, r)).collect(),
        page: page.page,
        page_size: page.page_size,
        has_more: page.has_more,
        next_page: page.next_page,
    })
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<RepoDetail, AppError> {
    let (listed, repo) = inventory::resolve_repo(&client.bounded_evidence(), input).await?;
    Ok(RepoDetail {
        summary: project(client, listed),
        definition: summary(client, &repo.config),
        revision_evidence: "Core-reported current cloned revision is cached and may lag or belong to another run. Configured branch/commit and an individual update's observed revision remain separate.",
    })
}
impl RepoPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        if self.repo.as_deref().is_some_and(|s| !locator(s))
            || self.branch.as_deref().is_some_and(|s| !locator(s))
            || self.git_provider.as_deref().is_some_and(|s| !provider(s))
            || self
                .commit
                .as_ref()
                .is_some_and(|s| s.len() > 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            || self.git_account.as_ref().is_some_and(|s| {
                s.len() > 256 || s.contains(['\n', '\r', '\0']) || s.contains("://")
            })
            || self
                .path
                .as_ref()
                .is_some_and(|s| s.len() > 1024 || s.chars().any(char::is_control))
            || [&self.on_clone, &self.on_pull].iter().any(|h| {
                h.as_ref().is_some_and(|h| {
                    h.path.len() > 1024
                        || h.path.chars().any(char::is_control)
                        || h.command.len() > 8192
                        || h.command.contains('\0')
                })
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn partial(&self) -> PartialRepoConfig {
        let hook = |h: &Hook| SystemCommand {
            path: h.path.clone(),
            command: h.command.clone(),
            shell_mode: h.shell_mode,
        };
        PartialRepoConfig {
            repo: self.repo.clone(),
            branch: self.branch.clone(),
            git_provider: self.git_provider.clone(),
            git_https: self.git_https,
            git_account: self.git_account.clone(),
            commit: self.commit.clone(),
            path: self.path.clone(),
            webhook_enabled: self.webhook_enabled,
            on_clone: self.on_clone.as_ref().map(hook),
            on_pull: self.on_pull.as_ref().map(hook),
            environment: self.environment.clone(),
            ..Default::default()
        }
    }
}
/// Fail closed for execution-bearing edits even when a preview saw webhooks
/// disabled: another operator can enable them after the final read. Only empty
/// patches, ordinary links and explicit webhook disabling avoid enabling or
/// replacing executable inputs.
pub(crate) fn update_requires_operate(config: &Value) -> bool {
    config.as_object().is_none_or(|fields| {
        fields.iter().any(|(key, value)| match key.as_str() {
            "links" => false,
            "webhook_enabled" => value != false,
            _ => true,
        })
    })
}
pub(crate) fn require_execution_effect_operate(
    client: &CoreClient,
    required: bool,
    operation: &'static str,
) -> Result<(), AppError> {
    if required && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "Execution-bearing Repo updates and enabling webhooks require both local grants. A webhook can already be active or be enabled concurrently; source, target, credentials, hooks, environment and paths can affect future execution. No Core request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, operation, None)
        });
    }
    Ok(())
}
async fn prepare(
    client: &CoreClient,
    p: &RepoPatch,
    current: Option<&RepoConfig>,
) -> Result<(PartialRepoConfig, Value), AppError> {
    let mut partial = p.partial();
    let mut references = Value::Null;
    if let Some(target) = &p.target {
        partial.server_id = Some(match target {
            Target::Unassigned {} => String::new(),
            Target::Server { target } => {
                let (_, s) = inventory::resolve_server(client, target.clone()).await?;
                if s.template {
                    return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
                }
                references = json!({"server":common::resource_baseline("server",&s),"public_key":s.info.public_key});
                s.id
            }
        });
    }
    if current.is_none() && partial.webhook_enabled.is_none() {
        partial.webhook_enabled = Some(false);
    }
    // Match Core's environment and hook-command newline normalization before
    // comparing, displaying or submitting a partial payload.
    let partial =
        serde_json::from_value(common::partial_value(&partial)?).map_err(|_| invalid())?;
    Ok((partial, references))
}
fn proposed(current: Option<&RepoConfig>, p: &PartialRepoConfig) -> Result<RepoConfig, AppError> {
    let mut v = serde_json::to_value(current.cloned().unwrap_or_default()).expect("config");
    v.as_object_mut().expect("object").extend(
        common::partial_value(p)?
            .as_object()
            .expect("partial")
            .clone(),
    );
    serde_json::from_value(v).map_err(|_| invalid())
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&RepoConfig>,
    p: &PartialRepoConfig,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(p)?;
    let after = proposed(current, p)?;
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "repo",
            id,
            name: common::bounded_text(client, name, 256),
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
        effects: vec![
            "Repo hooks, environment interpolation and destination paths are trusted executable configuration on the attached Server. Clone runs on-clone then on-pull; pull runs on-pull. Saving does not prove any hook or clone completed.",
            "The inspected Core 2.3.2 CreateRepo/UpdateRepo hooks refresh state caches without cloning, pulling or running hooks. MCP submits no execute or Git-file/commit request. The SDK warns about old-Server cleanup, but the inspected update path leaves old files in place; inspect both Servers separately.",
            "Creation defaults webhooks off explicitly. Enabling webhooks or applying any execution-bearing Repo update requires both local grants, including generic writes, independently of the observed webhook state. Only empty patches and explicit webhook disabling are exempt on this typed surface. This prevents changing executable configuration under already-active or concurrently enabled webhooks.",
            "Core enforces creation policy, Write on an existing Repo and Read/Attach on a supplied Server. Visible Server definitions/trust are bound privately and revalidated. Credential references are preserved, never disclosed or resolved with elevated credentials; token availability in Core/Periphery is not verified by this preview.",
            "Shared previews expire after ten minutes and are consumed on apply before I/O. No-op and stale previews submit zero writes. At most one typed partial write is followed by an independent authoritative read-back. Core has no compare-and-swap: a concurrent final-read/write race remains, and an ambiguous write is never retried.",
        ],
        definition: Some(common::DefinitionPreview::Repo(DefinitionPreview {
            before: current.map(|c| summary(client, c)),
            after: summary(client, &after),
            requires_operate_to_apply: if current.is_some() {
                update_requires_operate(&patch)
            } else {
                p.webhook_enabled == Some(true)
            },
            reference_validation: "Selected Server definition and pinned trust where readable; Core decides Attach and credential availability. Omitted references are preserved, not claimed as revalidated.",
        })),
    })
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Repo, AppError>,
    patch: &Value,
    expected: (&RepoConfig, &str, Option<&str>),
    op: &'static str,
) -> ConfigurationResult {
    let (expected, name, id) = expected;
    let mut r = common::finish_save(r, response, patch, name, id, op, |r, c| {
        if let Some(common::DefinitionPreview::Repo(d)) = &mut r.definition {
            d.after = summary(client, c);
        }
    });
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let read = client
            .read_references(GetRepo {
                repo: r.resource.id.clone().expect("saved identity"),
            })
            .await;
        r = common::verify_readback(
            r,
            read,
            &serde_json::to_value(expected).expect("config"),
            name,
            "GetRepo",
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
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_repo(&bounded, input.target.clone()).await?;
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
        input: common::bound_input("repo_update", &input),
        baseline: common::resource_baseline("repo", &current),
        prepared: json!({"config":common::partial_value(&partial)?,"references":references}),
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_execution_effect_operate(
        client,
        update_requires_operate(&serde_json::to_value(&input.patch).expect("typed patch")),
        "UpdateRepo",
    )?;
    input.patch.validate(client)?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("repo_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_repo(&bounded, input.target).await?;
    if entry.baseline != common::resource_baseline("repo", &current) {
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
        .write(UpdateRepo {
            id: current.id.clone(),
            config: partial,
        })
        .await;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&expected, &current.name, Some(&current.id)),
        "UpdateRepo",
    )
    .await)
}
fn create_name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    input.config.validate(client)?;
    let name = to_path_compatible_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || stable_id(&name).is_some()
        || input.config.repo.is_none()
        || input.config.branch.is_none()
        || input.config.target.is_none()
    {
        return Err(invalid());
    }
    Ok(name)
}
async fn require_absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_repo(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, "CreateRepo", None)),
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
        input: common::bound_input("repo_create", &input),
        baseline: json!({"kind":"repo_create","name":name}),
        prepared: json!({"config":common::partial_value(&partial)?,"references":references}),
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_execution_effect_operate(
        client,
        input.config.webhook_enabled == Some(true),
        "CreateRepo",
    )?;
    let input_config = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = create_name(client, &input_config)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("repo_create", &input_config),
    )?;
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let (partial, references) = prepare(&bounded, &input_config.config, None).await?;
    let patch = common::partial_value(&partial)?;
    if entry.baseline != json!({"kind":"repo_create","name":name})
        || entry.prepared != json!({"config":patch,"references":references})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let expected = proposed(None, &partial)?;
    let response = client
        .write(CreateRepo {
            name: name.clone(),
            config: partial,
        })
        .await;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&expected, &name, None),
        "CreateRepo",
    )
    .await)
}

/// The receipt discloses which saved hooks can execute, never their source.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RepoEffects {
    pub definition: DefinitionSummary,
    pub runs_on_clone: bool,
    pub runs_on_pull: bool,
    pub can_replace_working_directory: bool,
    pub environment_file_write_possible: bool,
    pub configuration_race: &'static str,
}
pub async fn execute(
    client: &CoreClient,
    input: OperationInput,
    action: WorkloadOperation,
) -> Result<SubmissionReceipt, AppError> {
    operations::require_operate(client, "RepoExecution")?;
    let (op, core_op) = match action {
        WorkloadOperation::Clone => ("CloneRepo", Operation::CloneRepo),
        WorkloadOperation::Pull => ("PullRepo", Operation::PullRepo),
        _ => return Err(invalid()),
    };
    let (_, repo) = inventory::resolve_repo(&client.bounded_evidence(), input.target).await?;
    if repo.template || stable_id(&repo.config.server_id).is_none() {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, op, None));
    }
    let definition = summary(client, &repo.config);
    if definition.on_clone_configured != input.expected_hooks.on_clone
        || definition.on_pull_configured != input.expected_hooks.on_pull
    {
        return Err(AppError::arguments(
            op,
            "Inspect get_repo first and supply the current on_clone_configured/on_pull_configured flags as expected_hooks.on_clone/on_pull. Clone can run both hooks; pull can run on-pull. Configured hooks changed or were not acknowledged; no execution submitted.",
        ));
    }
    let effects = ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: false,
        saved_command_wrapper: false,
        broader_effects_possible: true,
        compose_dependencies: None,
        repository: Some(RepoEffects {
            runs_on_clone: action == WorkloadOperation::Clone && definition.on_clone_configured,
            runs_on_pull: definition.on_pull_configured,
            can_replace_working_directory: action == WorkloadOperation::Clone,
            environment_file_write_possible: definition.environment_configured,
            definition,
            configuration_race: "Saved definition inspected before submission; Core reads it again asynchronously. Hooks, source and target can change after inspection. No definition rewrite, execution replay or hook isolation is promised.",
        }),
    };
    let target = OperationResource {
        kind: WorkloadKind::Repo,
        id: repo.id.clone(),
        name: common::bounded_text(client, &repo.name, 256),
    };
    match action {
        WorkloadOperation::Clone => {
            operations::submit(
                client,
                target,
                RequestedScope::WholeRepo {},
                action,
                core_op,
                effects,
                CloneRepo { repo: repo.id },
            )
            .await
        }
        _ => {
            operations::submit(
                client,
                target,
                RequestedScope::WholeRepo {},
                action,
                core_op,
                effects,
                PullRepo { repo: repo.id },
            )
            .await
        }
    }
}
