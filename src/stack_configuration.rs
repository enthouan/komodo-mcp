//! Declared Stack source/attachment edits. These change definitions, not host
//! files or Git commits. Raw references stay in the private preview proposal.
use komodo_client::entities::stack::{PartialStackConfig, StackConfig};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    client::CoreClient,
    configuration::{StackPatch, present},
    error::AppError,
    inventory::{self, DetailInput},
};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Attachment {
    Server { target: DetailInput },
    Swarm { target: DetailInput },
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// Replace managed inline Compose content; never writes a host/Git file.
    Inline {
        contents: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        run_directory: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "Vec<String>")]
        file_paths: Option<Vec<String>>,
    },
    /// Set the Git repo; omitted Git options preserve existing values.
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
        #[schemars(with = "String")]
        run_directory: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "Vec<String>")]
        file_paths: Option<Vec<String>>,
    },
    LinkedRepo {
        target: DetailInput,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        run_directory: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "Vec<String>")]
        file_paths: Option<Vec<String>>,
    },
    /// Select existing host files. This never writes their contents.
    Host {
        run_directory: String,
        /// [] selects Core's default compose.yaml.
        file_paths: Vec<String>,
    },
}

#[derive(Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Inline,
    Git,
    LinkedRepo,
    Host,
    Unconfigured,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct SourceSummary {
    pub kind: SourceKind,
    pub repo: Option<String>,
    pub provider: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub linked_repo_id: Option<String>,
    pub run_directory: String,
    pub file_paths: Vec<String>,
    pub inline_content_present: bool,
    pub truncated: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct AttachmentSummary {
    pub kind: &'static str,
    pub id: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub source: SourceSummary,
    pub attachment: Option<AttachmentSummary>,
    pub project_name: String,
    pub auto_pull: bool,
    pub auto_update: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ReferenceSummary {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub resolved_references: Vec<ReferenceSummary>,
    /// Observation at preview time, never inferred from a successful save.
    pub observed_runtime_state: Option<String>,
    pub core_auto_deploy_possible: bool,
    pub target_changed: bool,
    pub old_target_cleanup: &'static str,
}

pub(crate) struct Prepared {
    pub config: PartialStackConfig,
    pub references: Value,
    pub reference_summaries: Vec<ReferenceSummary>,
}

fn invalid() -> AppError {
    AppError::arguments(
        "configuration_edit",
        "Provide a nonempty inline Compose definition, a namespace/repo Git locator, or existing host file paths; use documented bounded source/target fields and explicit empty values only for supported clears",
    )
}

fn locator(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
}

fn provider_valid(value: &str) -> bool {
    if value.len() > 256
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
    {
        return false;
    }
    reqwest::Url::parse(&format!("https://{value}/")).is_ok_and(|url| {
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

impl Source {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        match self {
            Self::Inline { contents, .. } if contents.trim().is_empty() => return Err(invalid()),
            Self::Git {
                repo,
                provider,
                branch,
                commit,
                ..
            } if !locator(repo)
                || !repo.contains('/')
                || repo
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || provider.as_ref().is_some_and(|v| !provider_valid(v))
                || branch.as_ref().is_some_and(|v| {
                    !locator(v)
                        || v.starts_with('-')
                        || v.split('/')
                            .any(|part| part.is_empty() || part == "." || part == "..")
                })
                || commit.as_ref().is_some_and(|v| {
                    !v.is_empty() && (v.len() > 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                }) =>
            {
                return Err(invalid());
            }
            _ => {}
        }
        let (directory, paths) = match self {
            Self::Host {
                run_directory,
                file_paths,
            } => (Some(run_directory), Some(file_paths)),
            Self::Inline {
                run_directory,
                file_paths,
                ..
            }
            | Self::Git {
                run_directory,
                file_paths,
                ..
            }
            | Self::LinkedRepo {
                run_directory,
                file_paths,
                ..
            } => (run_directory.as_ref(), file_paths.as_ref()),
        };
        if directory.is_some_and(|v| v.len() > 1024 || v.contains(['\n', '\r', '\0']))
            || paths.is_some_and(|v| {
                v.len() > 32
                    || v.iter()
                        .any(|p| p.is_empty() || p.len() > 512 || p.contains(['\n', '\r', '\0']))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(crate) async fn prepare(
    client: &CoreClient,
    patch: &StackPatch,
    current: Option<&StackConfig>,
) -> Result<Prepared, AppError> {
    let mut config = PartialStackConfig {
        auto_pull: patch.auto_pull,
        ignore_services: patch.ignore_services.clone(),
        environment: patch.environment.clone(),
        project_name: patch.project_name.clone(),
        ..Default::default()
    };
    let default = StackConfig::default();
    let old = current.unwrap_or(&default);
    let mut references = Vec::new();
    let mut summaries = Vec::new();
    if let Some(attachment) = &patch.attachment {
        let (kind, id, name, snapshot) = match attachment {
            Attachment::Server { target } => {
                let (_, v) = inventory::resolve_server(client, target.clone()).await?;
                (
                    "server",
                    v.id.clone(),
                    v.name.clone(),
                    json!({"id":v.id,"name":v.name,"template":v.template,"config":v.config}),
                )
            }
            Attachment::Swarm { target } => {
                let (_, v) = inventory::resolve_swarm(client, target.clone()).await?;
                (
                    "swarm",
                    v.id.clone(),
                    v.name.clone(),
                    json!({"id":v.id,"name":v.name,"template":v.template,"config":v.config}),
                )
            }
        };
        if kind == "server" {
            config.server_id = Some(id.clone());
            if !old.swarm_id.is_empty() {
                config.swarm_id = Some(String::new());
            }
        } else {
            config.swarm_id = Some(id.clone());
            if !old.server_id.is_empty() {
                config.server_id = Some(String::new());
            }
        }
        references.push(json!({"kind":kind,"resource":snapshot}));
        summaries.push(ReferenceSummary {
            kind,
            id,
            name: name.chars().take(256).collect(),
        });
    }
    if let Some(source) = &patch.source {
        match source {
            Source::Inline {
                contents,
                run_directory,
                file_paths,
            } => {
                config.file_contents = Some(contents.clone());
                config.run_directory = run_directory.clone();
                config.file_paths = file_paths.clone();
                if old.files_on_host {
                    config.files_on_host = Some(false);
                }
                if !old.repo.is_empty() {
                    config.repo = Some(String::new());
                }
                if !old.linked_repo.is_empty() {
                    config.linked_repo = Some(String::new());
                }
            }
            Source::Git {
                repo,
                branch,
                provider,
                https,
                account,
                commit,
                run_directory,
                file_paths,
            } => {
                config.repo = Some(repo.clone());
                config.branch = branch.clone();
                config.git_provider = provider.clone();
                config.git_https = *https;
                config.git_account = account.clone();
                config.commit = commit.clone();
                config.run_directory = run_directory.clone();
                config.file_paths = file_paths.clone();
                if old.files_on_host {
                    config.files_on_host = Some(false);
                }
                if !old.linked_repo.is_empty() {
                    config.linked_repo = Some(String::new());
                }
                if !old.file_contents.is_empty() {
                    config.file_contents = Some(String::new());
                }
            }
            Source::LinkedRepo {
                target,
                run_directory,
                file_paths,
            } => {
                let (_, v) = inventory::resolve_repo(client, target.clone()).await?;
                config.linked_repo = Some(v.id.clone());
                config.run_directory = run_directory.clone();
                config.file_paths = file_paths.clone();
                summaries.push(ReferenceSummary {
                    kind: "repo",
                    id: v.id.clone(),
                    name: v.name.chars().take(256).collect(),
                });
                references.push(json!({"kind":"repo","resource":{"id":v.id,"name":v.name,"template":v.template,"config":v.config}}));
                if old.files_on_host {
                    config.files_on_host = Some(false);
                }
                if !old.repo.is_empty() {
                    config.repo = Some(String::new());
                }
                if !old.file_contents.is_empty() {
                    config.file_contents = Some(String::new());
                }
            }
            Source::Host {
                run_directory,
                file_paths,
            } => {
                config.files_on_host = Some(true);
                config.run_directory = Some(run_directory.clone());
                config.file_paths = Some(file_paths.clone());
                // Inactive Git/inline settings stay preserved on host selection.
            }
        }
    }
    // Use the same environment/file-content normalization as Core without
    // reconstructing or serializing a full definition into an update request.
    let config = serde_json::from_value(serde_json::to_value(config).expect("typed partial"))
        .map_err(|_| invalid())?;
    Ok(Prepared {
        config,
        references: Value::Array(references),
        reference_summaries: summaries,
    })
}

pub(crate) fn proposed(
    original: &StackConfig,
    patch: &PartialStackConfig,
) -> Result<StackConfig, AppError> {
    let mut value = serde_json::to_value(original).expect("typed config");
    value.as_object_mut().expect("config object").extend(
        serde_json::to_value(patch)
            .expect("typed partial")
            .as_object()
            .expect("partial object")
            .clone(),
    );
    serde_json::from_value(value).map_err(|_| invalid())
}

fn visible(value: &str) -> String {
    if value.contains(['@', '?', '#']) || value.contains("://") {
        "[redacted]".into()
    } else {
        value.chars().take(256).collect()
    }
}

pub(crate) fn summary(config: &StackConfig) -> DefinitionSummary {
    let (kind, repo, linked) = if config.files_on_host {
        (SourceKind::Host, None, None)
    } else if !config.linked_repo.is_empty() {
        (
            SourceKind::LinkedRepo,
            None,
            Some(visible(&config.linked_repo)),
        )
    } else if !config.repo.is_empty() {
        (SourceKind::Git, Some(visible(&config.repo)), None)
    } else if !config.file_contents.is_empty() {
        (SourceKind::Inline, None, None)
    } else {
        (SourceKind::Unconfigured, None, None)
    };
    let git = matches!(kind, SourceKind::Git);
    let source = SourceSummary {
        kind,
        repo,
        linked_repo_id: linked,
        provider: git.then(|| visible(&config.git_provider)),
        branch: git.then(|| visible(&config.branch)),
        commit: (git && !config.commit.is_empty()).then(|| visible(&config.commit)),
        run_directory: visible(&config.run_directory),
        file_paths: config
            .file_paths
            .iter()
            .take(16)
            .map(|v| visible(v))
            .collect(),
        inline_content_present: !config.file_contents.is_empty(),
        truncated: config.file_paths.len() > 16
            || [
                &config.repo,
                &config.git_provider,
                &config.branch,
                &config.commit,
                &config.run_directory,
            ]
            .iter()
            .any(|v| v.chars().count() > 256)
            || config.file_paths.iter().any(|v| v.chars().count() > 256),
    };
    let attachment = if !config.swarm_id.is_empty() {
        Some(AttachmentSummary {
            kind: "swarm",
            id: visible(&config.swarm_id),
        })
    } else if !config.server_id.is_empty() {
        Some(AttachmentSummary {
            kind: "server",
            id: visible(&config.server_id),
        })
    } else {
        None
    };
    DefinitionSummary {
        source,
        attachment,
        project_name: visible(&config.project_name),
        auto_pull: config.auto_pull,
        auto_update: config.auto_update,
    }
}
