//! Build definitions over shared preview/apply; execution is a separate tool family.
use std::time::Instant;

use crate::{
    builders,
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget, Preview,
        present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
    repositories::{self as repo, Hook},
};
use komodo_client::{
    api::{
        read::{
            GetBuild, ListDeployments, ListGitProviderAccounts, ListGitProvidersFromConfig,
            ListImageRegistriesFromConfig, ListImageRegistryAccounts,
        },
        write::{CreateBuild, UpdateBuild},
    },
    entities::{
        ResourceTarget, Version,
        build::{Build, BuildConfig, BuildListItem, ImageRegistryConfig, PartialBuildConfig},
        deployment::DeploymentImage,
        to_docker_compatible_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const OP: &str = "build_configuration";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Git {
        repo: String,
        branch: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        git_provider: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "bool")]
        git_https: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        commit: Option<String>,
    },
    LinkedRepo {
        target: DetailInput,
    },
    Inline {
        dockerfile: String,
    },
    Host {},
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub domain: String,
    pub account: String,
    pub organization: String,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "DetailInput")]
    pub builder: Option<DetailInput>,
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
    #[schemars(with = "String")]
    pub image_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub image_tag: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<Registry>")]
    pub image_registry: Option<Vec<Registry>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub git_account: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub auto_increment_version: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_latest_tag: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_version_tags: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub include_commit_tag: Option<bool>,
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
    #[schemars(with = "crate::repositories::Hook")]
    pub pre_build: Option<crate::repositories::Hook>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub build_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub dockerfile_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub build_args: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub secret_args: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub labels: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<String>")]
    pub extra_args: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub use_buildx: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub skip_secret_interp: Option<bool>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: BuildPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: BuildPatch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    #[schemars(schema_with = "create_schema")]
    pub config: BuildPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_schema")]
    pub config: BuildPatch,
    pub preview_id: String,
}
fn create_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[g.subschema_for::<BuildPatch>()],"required":["builder","source","image_name","image_registry"]})
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct SourceSummary {
    pub kind: &'static str,
    pub selector_evidence: &'static str,
    pub linked_repo_id: Option<String>,
    pub repo: String,
    pub provider: String,
    pub configured_branch: String,
    pub configured_https: bool,
    pub configured_commit: Option<String>,
    pub credential_reference_configured: bool,
    pub custom_build_path_configured: bool,
    pub custom_dockerfile_path_configured: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct RegistrySummary {
    pub domain: String,
    pub organization: String,
    pub credential_reference_configured: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub builder_id: Option<String>,
    pub source: SourceSummary,
    pub configured_version: String,
    pub auto_increment_version: bool,
    pub image_name: String,
    pub image_tag: String,
    pub image_registries: Vec<RegistrySummary>,
    pub registry_count: usize,
    pub registries_complete: bool,
    pub include_latest_tag: bool,
    pub include_version_tags: bool,
    pub include_commit_tag: bool,
    pub pre_build_configured: bool,
    pub build_args_configured: bool,
    pub secret_args_configured: bool,
    pub webhook_enabled: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct BuildSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub state: String,
    pub builder_id: Option<String>,
    pub configured_version: String,
    pub cached_built_revision: Option<String>,
    pub cached_latest_revision: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct BuildDetail {
    #[serde(flatten)]
    pub summary: BuildSummary,
    pub definition: DefinitionSummary,
    pub builder: Option<builders::DefinitionSummary>,
    pub builder_observation_error: Option<AppError>,
    pub evidence_limit: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct CredentialObservation {
    pub kind: &'static str,
    pub registry_index: Option<usize>,
    pub visible_reference_found: bool,
    pub sources_complete: bool,
    pub evidence_limit: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DependentDeployment {
    pub id: String,
    pub name: String,
    pub redeploy_on_build: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DependentEffects {
    pub visible: Vec<DependentDeployment>,
    pub has_more: bool,
    pub observation_error: Option<AppError>,
    pub evidence_limit: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub builder: builders::DefinitionSummary,
    pub credentials: Vec<CredentialObservation>,
    pub dependent_deployments: DependentEffects,
    pub requires_operate_to_apply: bool,
    pub reference_validation: &'static str,
}
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use typed bounded Build fields, an explicit existing-Server Builder, a supported source, safe image/registry selectors, complete trusted hooks and documented explicit clears",
    )
}
fn id(s: &str) -> Option<String> {
    crate::deletion::object_id(s).then(|| s.into())
}
fn account(s: &str) -> bool {
    s.len() <= 256 && !s.contains("://") && !s.chars().any(char::is_control)
}
fn registry_account(s: &str) -> bool {
    !s.is_empty()
        && account(s)
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b))
}
fn image_name(s: &str) -> bool {
    s.len() <= 256
        && (s.is_empty()
            || (repo::locator(s)
                && s.bytes().all(|b| !b.is_ascii_uppercase())
                && s.split('/')
                    .all(|p| p.as_bytes()[0].is_ascii_alphanumeric())))
}
fn image_tag(s: &str) -> bool {
    s.len() <= 128
        && (s.is_empty()
            || (!s.starts_with(['.', '-'])
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))))
}
fn version(s: &str) -> Result<Version, AppError> {
    let p = s.split('.').collect::<Vec<_>>();
    if p.len() != 3
        || p.iter().any(|s| {
            s.is_empty()
                || s.len() > 10
                || !s.bytes().all(|b| b.is_ascii_digit())
                || (s.len() > 1 && s.starts_with('0'))
        })
    {
        return Err(invalid());
    }
    let parse = |s: &str| s.parse::<i32>().map_err(|_| invalid());
    Ok(Version {
        major: parse(p[0])?,
        minor: parse(p[1])?,
        patch: parse(p[2])?,
    })
}
pub(crate) fn summary(client: &CoreClient, c: &BuildConfig) -> DefinitionSummary {
    let source = if c.files_on_host {
        "host"
    } else if !c.linked_repo.is_empty() {
        "linked_repo"
    } else if !c.repo.is_empty() {
        "git"
    } else {
        "inline"
    };
    DefinitionSummary {
        builder_id: id(&c.builder_id),
        source: SourceSummary {
            kind: source,
            selector_evidence: "Git selector fields belong to this saved Build. When kind is linked_repo, the linked Repo ID controls the effective Git source instead; inspect get_repo for its current source. Host/inline modes do not use these Git selectors.",
            linked_repo_id: id(&c.linked_repo),
            repo: repo::safe_source(client, &c.repo, repo::locator(&c.repo)),
            provider: repo::safe_source(client, &c.git_provider, repo::provider(&c.git_provider)),
            configured_branch: repo::safe_source(client, &c.branch, repo::locator(&c.branch)),
            configured_https: c.git_https,
            configured_commit: repo::revision(client, &c.commit),
            credential_reference_configured: !c.git_account.is_empty(),
            custom_build_path_configured: c.build_path != ".",
            custom_dockerfile_path_configured: c.dockerfile_path != "Dockerfile",
        },
        configured_version: c.version.to_string(),
        auto_increment_version: c.auto_increment_version,
        image_name: repo::safe_source(client, &c.image_name, image_name(&c.image_name)),
        image_tag: repo::safe_source(client, &c.image_tag, image_tag(&c.image_tag)),
        image_registries: c
            .image_registry
            .iter()
            .take(8)
            .map(|r| RegistrySummary {
                domain: repo::safe_source(client, &r.domain, repo::provider(&r.domain)),
                organization: repo::safe_source(
                    client,
                    &r.organization,
                    image_name(&r.organization),
                ),
                credential_reference_configured: !r.account.is_empty(),
            })
            .collect(),
        registry_count: c.image_registry.len(),
        registries_complete: c.image_registry.len() <= 8,
        include_latest_tag: c.include_latest_tag,
        include_version_tags: c.include_version_tags,
        include_commit_tag: c.include_commit_tag,
        pre_build_configured: !c.pre_build.is_none(),
        build_args_configured: !c.build_args.is_empty(),
        secret_args_configured: !c.secret_args.is_empty(),
        webhook_enabled: c.webhook_enabled,
    }
}
fn item(client: &CoreClient, b: BuildListItem) -> BuildSummary {
    BuildSummary {
        id: b.id,
        name: common::bounded_text(client, &b.name, 256),
        template: b.template,
        state: b.info.state.to_string(),
        builder_id: id(&b.info.builder_id),
        configured_version: b.info.version.to_string(),
        cached_built_revision: b
            .info
            .built_hash
            .as_deref()
            .and_then(|s| repo::revision(client, s)),
        cached_latest_revision: b
            .info
            .latest_hash
            .as_deref()
            .and_then(|s| repo::revision(client, s)),
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<BuildSummary>, AppError> {
    let p = inventory::list_build_records(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: p.items.into_iter().map(|b| item(client, b)).collect(),
        page: p.page,
        page_size: p.page_size,
        has_more: p.has_more,
        next_page: p.next_page,
    })
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<BuildDetail, AppError> {
    let bounded = client.bounded_evidence();
    let (listed, b) = inventory::resolve_build(&bounded, input).await?;
    let observed = if b.config.builder_id.is_empty() {
        Err(builders::unsupported())
    } else {
        inventory::resolve_builder(
            &bounded,
            DetailInput {
                id: Some(b.config.builder_id.clone()),
                name: None,
            },
        )
        .await
        .map(|(_, b)| builders::summary(&b.config))
    };
    let (builder, builder_observation_error) = match observed {
        Ok(b) => (Some(b), None),
        Err(e) => (None, Some(e)),
    };
    Ok(BuildDetail {
        summary: item(client, listed),
        definition: summary(client, &b.config),
        builder,
        builder_observation_error,
        evidence_limit: "Configured selectors/version/image are desired state. Built/latest revisions are Core caches and do not prove a particular run or deployment health. AWS/URL builders can be inspected but are unsupported for direct writes/execution. Scripts, arguments, accounts, file contents and private cache errors are omitted.",
    })
}
impl BuildPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        if self.image_name.as_deref().is_some_and(|s| !image_name(s))
            || self.image_tag.as_deref().is_some_and(|s| !image_tag(s))
            || self.git_account.as_deref().is_some_and(|s| !account(s))
            || self.image_registry.as_ref().is_some_and(|rs| {
                rs.len() > 8
                    || rs.iter().any(|r| {
                        !repo::provider(&r.domain)
                            || !registry_account(&r.account)
                            || (r.organization.is_empty() && !image_name(&r.account))
                            || !image_name(&r.organization)
                    })
            })
            || [&self.build_path, &self.dockerfile_path].iter().any(|v| {
                v.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control)
                })
            })
            || self.pre_build.as_ref().is_some_and(|h| {
                h.path.len() > 1024
                    || h.path.chars().any(char::is_control)
                    || h.command.len() > 8192
                    || h.command.contains('\0')
            })
            || [&self.build_args, &self.secret_args, &self.labels]
                .iter()
                .any(|v| {
                    v.as_ref()
                        .is_some_and(|s| s.len() > 8192 || s.contains('\0'))
                })
            || self.extra_args.as_ref().is_some_and(|v| {
                v.len() > 32
                    || v.iter()
                        .any(|s| s.is_empty() || s.len() > 1024 || s.contains('\0'))
            })
        {
            return Err(invalid());
        }
        if let Some(v) = &self.version {
            version(v)?;
        }
        if let Some(source) = &self.source {
            match source {
                Source::Git {
                    repo: r,
                    branch,
                    git_provider,
                    commit,
                    ..
                } if !repo::locator(r)
                    || !repo::locator(branch)
                    || git_provider.as_deref().is_some_and(|s| !repo::provider(s))
                    || commit.as_deref().is_some_and(|s| {
                        s.len() > 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())
                    }) =>
                {
                    return Err(invalid());
                }
                Source::Inline { dockerfile }
                    if dockerfile.is_empty()
                        || dockerfile.len() > 8192
                        || dockerfile.contains('\0') =>
                {
                    return Err(invalid());
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn partial(&self) -> Result<PartialBuildConfig, AppError> {
        let hook = |h: &Hook| komodo_client::entities::SystemCommand {
            path: h.path.clone(),
            command: h.command.clone(),
            shell_mode: h.shell_mode,
        };
        Ok(PartialBuildConfig {
            image_name: self.image_name.clone(),
            image_tag: self.image_tag.clone(),
            image_registry: self.image_registry.as_ref().map(|rs| {
                rs.iter()
                    .map(|r| ImageRegistryConfig {
                        domain: r.domain.clone(),
                        account: r.account.clone(),
                        organization: r.organization.clone(),
                    })
                    .collect()
            }),
            git_account: self.git_account.clone(),
            version: self.version.as_deref().map(version).transpose()?,
            auto_increment_version: self.auto_increment_version,
            include_latest_tag: self.include_latest_tag,
            include_version_tags: self.include_version_tags,
            include_commit_tag: self.include_commit_tag,
            webhook_enabled: self.webhook_enabled,
            pre_build: self.pre_build.as_ref().map(hook),
            build_path: self.build_path.clone(),
            dockerfile_path: self.dockerfile_path.clone(),
            build_args: self.build_args.clone(),
            secret_args: self.secret_args.clone(),
            labels: self.labels.clone(),
            extra_args: self
                .extra_args
                .as_ref()
                .map(|a| a.iter().filter(|s| !s.trim().is_empty()).cloned().collect()),
            use_buildx: self.use_buildx,
            skip_secret_interp: self.skip_secret_interp,
            ..Default::default()
        })
    }
}
fn proposed(
    current: Option<&BuildConfig>,
    p: &PartialBuildConfig,
) -> Result<BuildConfig, AppError> {
    let mut v = serde_json::to_value(current.cloned().unwrap_or_default()).expect("config");
    v.as_object_mut().expect("object").extend(
        common::partial_value(p)?
            .as_object()
            .expect("patch")
            .clone(),
    );
    serde_json::from_value(v).map_err(|_| invalid())
}
fn validate_cache_paths(c: &BuildConfig) -> Result<(), AppError> {
    if !c.files_on_host && (!c.repo.is_empty() || !c.linked_repo.is_empty()) {
        for path in [&c.build_path, &c.dockerfile_path] {
            if path.is_empty()
                || path.len() > 1024
                || path.starts_with('/')
                || !path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
                || path.split('/').any(|p| p.is_empty() || p == "..")
            {
                return Err(AppError::arguments(
                    OP,
                    "Git/linked-Repo build and Dockerfile paths must remain relative to the source cache, without parent traversal. Explicit host-source paths use the selected Server instead.",
                ));
            }
        }
    }
    Ok(())
}
struct Prepared {
    config: PartialBuildConfig,
    references: Value,
    builder: builders::DefinitionSummary,
    credentials: Vec<CredentialObservation>,
    dependents: DependentEffects,
}
fn private(p: &Prepared) -> Result<Value, AppError> {
    Ok(
        json!({"config":common::partial_value(&p.config)?,"references":p.references,"dependent_policy":p.dependents}),
    )
}

async fn credential(
    client: &CoreClient,
    kind: &'static str,
    index: Option<usize>,
    domain: &str,
    username: &str,
    servers: &[String],
) -> Result<(CredentialObservation, Value), AppError> {
    let db = if kind == "git" {
        client
            .read_references(ListGitProviderAccounts {
                domain: Some(domain.into()),
                username: Some(username.into()),
            })
            .await
    } else {
        client
            .read_references(ListImageRegistryAccounts {
                domain: Some(domain.into()),
                username: Some(username.into()),
            })
            .await
    };
    let mut found = false;
    let mut complete = true;
    let mut evidence = Vec::new();
    match db {
        Ok(v)
            if v.as_array().is_some_and(|a| {
                a.len() <= 32
                    && a.iter()
                        .all(|r| r["domain"] == domain && r["username"] == username)
            }) =>
        {
            found = v.as_array().is_some_and(|a| !a.is_empty());
            evidence.push(json!({"database":v}));
        }
        Ok(_) => return Err(AppError::new(ErrorKind::InvalidResponse, OP, None)),
        Err(e) => {
            complete = false;
            evidence.push(json!({"database_unavailable":e.kind}));
        }
    }
    // Core's config listing deliberately omits tokens. Check every fallback
    // Server rather than Builder-target listing (which only checks the first).
    for server in servers {
        let target = Some(ResourceTarget::Server(server.clone()));
        let response = if kind == "git" {
            client
                .read_references(ListGitProvidersFromConfig { target })
                .await
        } else {
            client
                .read_references(ListImageRegistriesFromConfig { target })
                .await
        };
        match response {
            Ok(v) if v.as_array().is_some_and(|a| a.len() <= 128) => {
                let matched=v.as_array().expect("array").iter().filter(|p|p["domain"]==domain).map(|p|json!({"domain":p["domain"],"https":p["https"],"account_present":p["accounts"].as_array().is_some_and(|a|a.iter().any(|a|a["username"]==username))})).collect::<Vec<_>>();
                found |= matched.iter().any(|p| p["account_present"] == true);
                evidence.push(json!({"server":server,"config":matched}));
            }
            Ok(_) => return Err(AppError::new(ErrorKind::InvalidResponse, OP, None)),
            Err(e) => {
                complete = false;
                evidence.push(json!({"server":server,"config_unavailable":e.kind}));
            }
        }
    }
    if !found && complete {
        return Err(AppError::arguments(
            OP,
            "The selected credential reference was absent from all readable Core database/config and selected fallback Server config sources. Inspect the reference; no write was submitted.",
        ));
    }
    Ok((
        CredentialObservation {
            kind,
            registry_index: index,
            visible_reference_found: found,
            sources_complete: complete,
            evidence_limit: "Presence of a visible account reference does not prove token validity, registry/repository access, successful push or deployment health. Denied/unavailable sources remain unverified; no privileged fallback is used.",
        },
        json!(evidence),
    ))
}
async fn dependents(client: &CoreClient, build: Option<&str>) -> DependentEffects {
    let limit = "Only visible linked Deployments are inspected, at most 16; hidden/new/concurrent dependents may exist. redeploy_on_build is configured future behavior, not a submission or proof of deployment/health. Core's first registry is used for attached Deployments.";
    let mut result = DependentEffects {
        visible: vec![],
        has_more: false,
        observation_error: None,
        evidence_limit: limit,
    };
    let Some(build) = build else {
        return result;
    };
    let mut request = ListDeployments {
        limit: Some(17),
        sort_desc: false,
        ..Default::default()
    };
    request.query.specific.build_ids = vec![build.into()];
    let items = match client.read(request).await {
        Ok(items) => items,
        Err(e) => {
            result.observation_error = Some(e);
            return result;
        }
    };
    if items.len() > 17 {
        result.observation_error = Some(AppError::new(
            ErrorKind::InvalidResponse,
            "ListDeployments",
            None,
        ));
        return result;
    }
    result.has_more = items.len() > 16;
    for listed in items.into_iter().take(16) {
        if !crate::deletion::object_id(&listed.id) {
            result.observation_error = Some(AppError::new(
                ErrorKind::InvalidResponse,
                "ListDeployments",
                None,
            ));
            break;
        }
        let full = inventory::resolve_deployment(
            client,
            DetailInput {
                id: Some(listed.id),
                name: None,
            },
        )
        .await;
        match full {
            Ok((_, d)) if matches!(&d.config.image,DeploymentImage::Build {build_id,..} if build_id==build) => {
                result.visible.push(DependentDeployment {
                    id: d.id,
                    name: common::bounded_text(client, &d.name, 256),
                    redeploy_on_build: d.config.redeploy_on_build,
                })
            }
            Ok(_) => {
                result.observation_error = Some(AppError::new(
                    ErrorKind::InvalidResponse,
                    "GetDeployment",
                    None,
                ));
                break;
            }
            Err(e) => {
                result.observation_error = Some(e);
                break;
            }
        }
    }
    result
}
async fn prepare(
    client: &CoreClient,
    p: &BuildPatch,
    current: Option<&Build>,
    expected: Option<&Value>,
) -> Result<Prepared, AppError> {
    p.validate(client)?;
    let mut partial = p.partial()?;
    let builder_target = p
        .builder
        .clone()
        .or_else(|| {
            current.map(|b| DetailInput {
                id: Some(b.config.builder_id.clone()),
                name: None,
            })
        })
        .ok_or_else(invalid)?;
    let (builder, builder_refs) = builders::resolve_supported(client, builder_target).await?;
    if expected.is_some_and(|v| v["references"]["builder"] != builder_refs) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let servers = &builders::server_config(&builder)?.server_ids;
    if servers.is_empty() {
        return Err(builders::unsupported());
    }
    if p.builder.is_some() {
        partial.builder_id = Some(builder.id.clone());
    }
    let mut linked_refs = Value::Null;
    if let Some(source) = &p.source {
        partial.files_on_host = Some(matches!(source, Source::Host {}));
        partial.repo = Some(String::new());
        partial.linked_repo = Some(String::new());
        partial.dockerfile = Some(String::new());
        match source {
            Source::Git {
                repo,
                branch,
                git_provider,
                git_https,
                commit,
            } => {
                partial.repo = Some(repo.clone());
                partial.branch = Some(branch.clone());
                partial.git_provider = git_provider.clone();
                partial.git_https = *git_https;
                partial.commit = commit.clone();
            }
            Source::LinkedRepo { target } => {
                let (_, r) = inventory::resolve_repo(client, target.clone()).await?;
                if r.template {
                    return Err(builders::unsupported());
                }
                partial.linked_repo = Some(r.id.clone());
                linked_refs = common::resource_baseline("repo", &r);
            }
            Source::Inline { dockerfile } => partial.dockerfile = Some(dockerfile.clone()),
            Source::Host {} => {}
        }
    }
    if current.is_none() && partial.webhook_enabled.is_none() {
        partial.webhook_enabled = Some(false);
    }
    let partial: PartialBuildConfig =
        serde_json::from_value(common::partial_value(&partial)?).map_err(|_| invalid())?;
    let after = proposed(current.map(|b| &b.config), &partial)?;
    validate_cache_paths(&after)?;
    if !after.files_on_host && !after.linked_repo.is_empty() && linked_refs.is_null() {
        let (_, r) = inventory::resolve_repo(
            client,
            DetailInput {
                id: Some(after.linked_repo.clone()),
                name: None,
            },
        )
        .await?;
        if r.template {
            return Err(builders::unsupported());
        }
        linked_refs = common::resource_baseline("repo", &r);
    }
    if expected.is_some_and(|v| v["references"]["linked_repo"] != linked_refs) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    // Saving refreshes the effective saved source, even for a one-field edit.
    // Do not feed unsafe inherited selectors into Core's Git cache commands.
    let active_git = (!after.files_on_host && !after.linked_repo.is_empty()).then_some((
        &linked_refs["config"]["repo"],
        &linked_refs["config"]["branch"],
        &linked_refs["config"]["git_provider"],
        &linked_refs["config"]["commit"],
    ));
    if let Some((r, b, p, c)) = active_git {
        if !r.as_str().is_some_and(repo::locator)
            || !b.as_str().is_some_and(repo::locator)
            || !p.as_str().is_some_and(repo::provider)
            || !c
                .as_str()
                .is_some_and(|s| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
    } else if !after.files_on_host
        && !after.repo.is_empty()
        && (!repo::locator(&after.repo)
            || !repo::locator(&after.branch)
            || !repo::provider(&after.git_provider)
            || after.commit.len() > 64
            || !after.commit.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(invalid());
    }
    if !image_name(&after.image_name)
        || !image_tag(&after.image_tag)
        || after.image_registry.iter().any(|r| {
            !r.domain.is_empty()
                && (!repo::provider(&r.domain)
                    || !registry_account(&r.account)
                    || (r.organization.is_empty() && !image_name(&r.account))
                    || !image_name(&r.organization))
        })
    {
        return Err(invalid());
    }
    let mut observations = Vec::new();
    let mut credential_refs = Vec::new();
    let (git_domain, git_account) = if !after.files_on_host && !after.linked_repo.is_empty() {
        (
            linked_refs["config"]["git_provider"]
                .as_str()
                .unwrap_or_default(),
            linked_refs["config"]["git_account"]
                .as_str()
                .unwrap_or_default(),
        )
    } else {
        (&after.git_provider[..], &after.git_account[..])
    };
    if (!after.repo.is_empty() || !after.linked_repo.is_empty())
        && !after.files_on_host
        && !git_account.is_empty()
    {
        let (o, e) = credential(client, "git", None, git_domain, git_account, servers).await?;
        observations.push(o);
        credential_refs.push(e);
    }
    if after.image_registry.len() > 8 {
        return Err(invalid());
    }
    for (index, r) in after.image_registry.iter().enumerate() {
        if !r.domain.is_empty() && !r.account.is_empty() {
            let (o, e) = credential(
                client,
                "registry",
                Some(index),
                &r.domain,
                &r.account,
                servers,
            )
            .await?;
            observations.push(o);
            credential_refs.push(e);
        }
    }
    let dependent_deployments = dependents(client, current.map(|b| b.id.as_str())).await;
    Ok(Prepared {
        config: partial,
        references: json!({"builder":builder_refs,"linked_repo":linked_refs,"credentials":credential_refs}),
        builder: builders::summary(&builder.config),
        credentials: observations,
        dependents: dependent_deployments,
    })
}
pub(crate) fn create_requires_operate(c: &Value) -> bool {
    c["webhook_enabled"] != false
        || c["files_on_host"] == true
        || c["repo"].as_str().is_some_and(|s| !s.is_empty())
        || c["linked_repo"].as_str().is_some_and(|s| !s.is_empty())
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&BuildConfig>,
    p: &Prepared,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(&p.config)?;
    let after = proposed(current, &p.config)?;
    let requires = if current.is_some() {
        patch.as_object().is_none_or(|p| !p.is_empty())
    } else {
        create_requires_operate(&serde_json::to_value(&after).expect("config"))
    };
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "build",
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
            "Build scripts, Dockerfiles, arguments and paths are trusted code/configuration. Future builds can execute pre-build commands, push tagged images and redeploy configured dependent Deployments under Core authority. The first registry supplies attached Deployments. Account names, tokens, script source, secret/default arguments and private Dockerfile/cache contents are not displayed.",
            "MCP submits zero RunBuild or other execution requests. Nevertheless Core 2.3.2 CreateBuild/UpdateBuild refresh caches as Core's built-in build identity: Git sources can clone/pull into Core's cache and host sources can contact an existing Server's Periphery to read Dockerfile contents. This path does not run Repo hooks or pre-build commands and does not provision AWS. A cache-refresh error can be logged after the definition was saved.",
            "Every nonempty Build update requires both local grants, regardless of source or webhook state, because cache refresh and automatic-execution policy can race. New inline-only builds with webhooks explicitly off need edit alone; Git/linked-Repo/host creation or webhook enablement needs both. Empty updates remain zero-write no-ops. Core retains creation policy, Write and Builder/Repo Read-Attach authority.",
            "Private baseline, effective Server Builder and ordered Server/trust, linked Repo and visible credential-reference evidence are revalidated on apply. Hidden/unavailable reference sources remain explicit gaps. A visible account is not proof of usable credentials. No elevated fallback, build execution, automatic retry or replay is used.",
            "Shared previews expire after ten minutes and are single-use. Apply writes once, checks the echo and independently reads the complete saved config, including omitted fields/secrets. Core has no conditional-write guarantee; source/Builder/dependent changes after final reads still race. Saved config/cache observations do not prove a completed build, registry push, deployment or healthy workload.",
        ],
        definition: Some(common::DefinitionPreview::Build(Box::new(
            DefinitionPreview {
                before: current.map(|c| summary(client, c)),
                after: summary(client, &after),
                builder: p.builder.clone(),
                credentials: p.credentials.clone(),
                dependent_deployments: p.dependents.clone(),
                requires_operate_to_apply: requires,
                reference_validation: "Visible effective Builder/ordered Server definitions and trust, linked Repo and bounded credential sources. Omitted config and secrets are preserved. Core remains authoritative for permissions and credential usability.",
            },
        ))),
    })
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Build, AppError>,
    patch: &Value,
    expected: (&BuildConfig, &str, Option<&str>),
    op: &'static str,
) -> ConfigurationResult {
    let (config, name, id) = expected;
    let mut r = common::finish_save(r, response, patch, name, id, op, |r, c| {
        if let Some(common::DefinitionPreview::Build(d)) = &mut r.definition {
            d.after = summary(client, c);
        }
    });
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let raw = client
            .read_references(GetBuild {
                build: r.resource.id.clone().expect("saved"),
            })
            .await;
        r = common::verify_readback(
            r,
            raw,
            &serde_json::to_value(config).expect("config"),
            name,
            "GetBuild",
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
    let (_, b) = inventory::resolve_build(&bounded, input.target.clone()).await?;
    if b.template {
        return Err(builders::unsupported());
    }
    let p = prepare(&bounded, &input.patch, Some(&b), None).await?;
    let r = result(client, Some(b.id.clone()), &b.name, Some(&b.config), &p)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("build_update", &input),
        baseline: common::resource_baseline("build", &b),
        prepared: private(&p)?,
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    builders::require_operate(
        client,
        !serde_json::to_value(&input.patch)
            .expect("patch")
            .as_object()
            .expect("object")
            .is_empty(),
        "UpdateBuild",
    )?;
    input.patch.validate(client)?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("build_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, b) = inventory::resolve_build(&bounded, input.target).await?;
    if entry.baseline != common::resource_baseline("build", &b) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let p = prepare(&bounded, &input.patch, Some(&b), Some(&entry.prepared)).await?;
    if entry.prepared != private(&p)? {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let patch = common::partial_value(&p.config)?;
    let config = proposed(Some(&b.config), &p.config)?;
    let response = client
        .write_checked(UpdateBuild {
            id: b.id.clone(),
            config: p.config,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&config, &b.name, Some(&b.id)),
        "UpdateBuild",
    )
    .await)
}
fn name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    input.config.validate(client)?;
    let name = to_docker_compatible_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || crate::deletion::object_id(&name)
        || input.config.builder.is_none()
        || input.config.source.is_none()
        || input.config.image_name.is_none()
        || input.config.image_registry.is_none()
    {
        return Err(invalid());
    }
    Ok(name)
}
async fn absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_build(
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
    let p = prepare(&bounded, &input.config, None, None).await?;
    let r = result(client, None, &name, None, &p)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("build_create", &input),
        baseline: json!({"kind":"build_create","name":name}),
        prepared: private(&p)?,
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let needs = input.config.webhook_enabled == Some(true)
        || !matches!(input.config.source, Some(Source::Inline { .. }));
    builders::require_operate(client, needs, "CreateBuild")?;
    let submitted = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = name(client, &submitted)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("build_create", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let p = prepare(&bounded, &submitted.config, None, Some(&entry.prepared)).await?;
    let patch = common::partial_value(&p.config)?;
    if entry.baseline != json!({"kind":"build_create","name":name})
        || entry.prepared != private(&p)?
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let config = proposed(None, &p.config)?;
    let response = client
        .write_checked(CreateBuild {
            name: name.clone(),
            config: p.config,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&config, &name, None),
        "CreateBuild",
    )
    .await)
}
/// The shared typed mutation path checks both effects and the actual effective
/// Builder; omitting builder_id must not bypass the Server-only restriction.
pub(crate) async fn guard_write(
    client: &CoreClient,
    op: &'static str,
    params: &Value,
) -> Result<(), AppError> {
    if !matches!(op, "CreateBuild" | "UpdateBuild") {
        return Ok(());
    }
    let c = &params["config"];
    builders::require_operate(
        client,
        if op == "CreateBuild" {
            create_requires_operate(c)
        } else {
            c.as_object().is_none_or(|c| !c.is_empty())
        },
        op,
    )?;
    let bounded = client.bounded_evidence();
    let current = if op == "UpdateBuild" {
        let target = params["id"]
            .as_str()
            .filter(|b| crate::deletion::object_id(b))
            .ok_or_else(builders::unsupported)?;
        let (_, b) = inventory::resolve_build(
            &bounded,
            DetailInput {
                id: Some(target.into()),
                name: None,
            },
        )
        .await?;
        if b.template {
            return Err(builders::unsupported());
        }
        Some(b)
    } else {
        None
    };
    let partial: PartialBuildConfig = serde_json::from_value(c.clone()).map_err(|_| invalid())?;
    validate_cache_paths(&proposed(current.as_ref().map(|b| &b.config), &partial)?)?;
    let selected = if let Some(b) = c.get("builder_id") {
        b.as_str()
            .filter(|b| crate::deletion::object_id(b))
            .ok_or_else(builders::unsupported)?
            .to_string()
    } else if let Some(b) = current {
        b.config.builder_id
    } else {
        return Err(builders::unsupported());
    };
    let (_, b) = inventory::resolve_builder(
        &bounded,
        DetailInput {
            id: Some(selected),
            name: None,
        },
    )
    .await?;
    if builders::server_config(&b)?.server_ids.is_empty() {
        return Err(builders::unsupported());
    }
    Ok(())
}
