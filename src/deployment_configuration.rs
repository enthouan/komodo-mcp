//! Desired Deployment definitions over the shared private preview/apply contract.
use std::time::Instant;

use komodo_client::{
    api::write::{CreateDeployment, UpdateDeployment},
    entities::{
        Version,
        deployment::{DeploymentConfig, DeploymentImage, PartialDeploymentConfig},
        to_container_compatible_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{
        self as common, Attachment, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget,
        Preview, present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
    stack_configuration::{AttachmentSummary, ReferenceSummary},
};

const OP: &str = "deployment_configuration_edit";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageInput {
    Image {
        reference: String,
    },
    Build {
        target: DetailInput,
        /// Exactly major.minor.patch. 0.0.0 explicitly selects latest at deploy time.
        version: String,
    },
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentPatch {
    /// Image updates require both local grants, independently of current Core policy.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "ImageInput")]
    pub image: Option<ImageInput>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Attachment")]
    pub attachment: Option<Attachment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub image_registry_account: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub environment: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub ports: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub volumes: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub command: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub network: Option<String>,
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
    #[schemars(with = "String")]
    pub labels: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "i32")]
    pub replicas: Option<i32>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: DeploymentPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: DeploymentPatch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: DeploymentPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: DeploymentPatch,
    pub preview_id: String,
}
fn create_config_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[generator.subschema_for::<DeploymentPatch>()],"required":["image","attachment"]})
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ImageSummary {
    pub kind: &'static str,
    pub reference: Option<String>,
    pub build_id: Option<String>,
    pub version: Option<String>,
    pub latest_build_version_at_deploy: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub image: ImageSummary,
    pub attachment: Option<AttachmentSummary>,
    pub network: String,
    pub environment_present: bool,
    pub mounts_present: bool,
    pub command_present: bool,
    pub extra_args_count: usize,
    pub registry_account_configured: bool,
    pub auto_update: bool,
    pub redeploy_on_build: bool,
    pub replicas: i32,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct RuntimeObservation {
    pub state: String,
    /// Core can fall back to desired configuration when runtime image cache is missing.
    pub core_reported_image: Option<String>,
    pub image_may_be_desired_fallback: bool,
    pub running_configuration_verified: bool,
    pub observed_at: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub resolved_references: Vec<ReferenceSummary>,
    pub runtime_observation: Option<RuntimeObservation>,
    pub requires_operate_to_apply: bool,
    pub core_registry_check_possible: bool,
    pub core_auto_deploy_possible: bool,
    pub target_changed: bool,
    pub old_target_cleanup: &'static str,
}
struct Prepared {
    config: PartialDeploymentConfig,
    references: Value,
    summaries: Vec<ReferenceSummary>,
}
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use a bounded image reference or resolved Build with an explicit major.minor.patch version, supported partial fields, and documented empty values only for supported clears",
    )
}
fn version(value: &str) -> Result<Version, AppError> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 10
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        return Err(invalid());
    }
    let parse = |p: &str| p.parse::<i32>().map_err(|_| invalid());
    Ok(Version {
        major: parse(parts[0])?,
        minor: parse(parts[1])?,
        patch: parse(parts[2])?,
    })
}
fn valid_image(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 512
        || value.contains("://")
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:@".contains(&b))
    {
        return false;
    }
    let parts = value.split('@').collect::<Vec<_>>();
    if parts.len() > 2
        || parts[0].is_empty()
        || !parts[0].as_bytes()[0].is_ascii_alphanumeric()
        || parts[0].ends_with(['/', ':'])
        || parts[0]
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return false;
    }
    parts.len() == 1
        || parts[1]
            .strip_prefix("sha256:")
            .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn image_display(value: &str) -> String {
    if valid_image(value) {
        value.into()
    } else {
        "[redacted]".into()
    }
}
fn public_locator(client: &CoreClient, value: &str) -> String {
    if value.contains(['@', '?', '#']) || value.contains("://") {
        "[redacted]".into()
    } else {
        common::bounded_text(client, value, 256)
    }
}
fn attachment_summary(client: &CoreClient, config: &DeploymentConfig) -> Option<AttachmentSummary> {
    if !config.swarm_id.is_empty() {
        Some(AttachmentSummary {
            kind: "swarm",
            id: public_locator(client, &config.swarm_id),
        })
    } else if !config.server_id.is_empty() {
        Some(AttachmentSummary {
            kind: "server",
            id: public_locator(client, &config.server_id),
        })
    } else {
        None
    }
}
fn summary(client: &CoreClient, config: &DeploymentConfig) -> DefinitionSummary {
    let image = match &config.image {
        DeploymentImage::Image { image } => ImageSummary {
            kind: "image",
            reference: Some(image_display(image)),
            build_id: None,
            version: None,
            latest_build_version_at_deploy: false,
        },
        DeploymentImage::Build { build_id, version } => ImageSummary {
            kind: "build",
            reference: None,
            build_id: Some(public_locator(client, build_id)),
            version: Some(version.to_string()),
            latest_build_version_at_deploy: version.is_none(),
        },
    };
    DefinitionSummary {
        image,
        attachment: attachment_summary(client, config),
        network: public_locator(client, &config.network),
        environment_present: !config.environment.is_empty(),
        mounts_present: !config.volumes.is_empty(),
        command_present: !config.command.is_empty(),
        extra_args_count: config.extra_args.len(),
        registry_account_configured: !config.image_registry_account.is_empty(),
        auto_update: config.auto_update,
        redeploy_on_build: config.redeploy_on_build,
        replicas: config.replicas,
    }
}
impl DeploymentPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        match &self.image {
            Some(ImageInput::Image { reference }) if !valid_image(reference) => {
                return Err(invalid());
            }
            Some(ImageInput::Build { version: v, .. }) => {
                version(v)?;
            }
            _ => {}
        }
        if self
            .network
            .as_ref()
            .is_some_and(|v| v.trim().is_empty() || v.len() > 256 || v.contains(['\n', '\r', '\0']))
            || self.replicas.is_some_and(|v| !(0..=1000).contains(&v))
            || self.extra_args.as_ref().is_some_and(|v| {
                v.len() > 32
                    || v.iter()
                        .any(|a| a.trim().is_empty() || a.len() > 512 || a.contains('\0'))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(crate) fn require_image_operate(client: &CoreClient, supplied: bool) -> Result<(), AppError> {
    if supplied && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "Deployment image updates require both operate and configuration-edit grants because Core can inspect registries and auto-deploy under its policy. The policy can race; no request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, "UpdateDeployment", None)
        });
    }
    Ok(())
}
async fn prepare(
    client: &CoreClient,
    patch: &DeploymentPatch,
    current: Option<&DeploymentConfig>,
) -> Result<Prepared, AppError> {
    let mut config = PartialDeploymentConfig {
        image_registry_account: patch.image_registry_account.clone(),
        environment: patch.environment.clone(),
        ports: patch.ports.clone(),
        volumes: patch.volumes.clone(),
        command: patch.command.clone(),
        network: patch.network.clone(),
        extra_args: patch.extra_args.clone(),
        labels: patch.labels.clone(),
        send_alerts: patch.send_alerts,
        replicas: patch.replicas,
        ..Default::default()
    };
    let default = DeploymentConfig::default();
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
                    common::resource_baseline("server", &v),
                )
            }
            Attachment::Swarm { target } => {
                let (_, v) = inventory::resolve_swarm(client, target.clone()).await?;
                (
                    "swarm",
                    v.id.clone(),
                    v.name.clone(),
                    common::resource_baseline("swarm", &v),
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
        references.push(snapshot);
        summaries.push(ReferenceSummary {
            kind,
            id,
            name: common::bounded_text(client, &name, 256),
        });
    }
    config.image = match &patch.image {
        None => None,
        Some(ImageInput::Image { reference }) => Some(DeploymentImage::Image {
            image: reference.clone(),
        }),
        Some(ImageInput::Build { target, version: v }) => {
            let (_, build) = inventory::resolve_build(client, target.clone()).await?;
            references.push(common::resource_baseline("build", &build));
            summaries.push(ReferenceSummary {
                kind: "build",
                id: build.id.clone(),
                name: common::bounded_text(client, &build.name, 256),
            });
            Some(DeploymentImage::Build {
                build_id: build.id,
                version: version(v)?,
            })
        }
    };
    let config = serde_json::from_value(serde_json::to_value(config).expect("typed partial"))
        .map_err(|_| invalid())?;
    Ok(Prepared {
        config,
        references: Value::Array(references),
        summaries,
    })
}
fn prepared_value(p: &Prepared) -> Value {
    json!({"config":p.config,"references":p.references})
}
fn proposed(
    current: Option<&DeploymentConfig>,
    p: &Prepared,
) -> Result<DeploymentConfig, AppError> {
    let mut value = serde_json::to_value(current.cloned().unwrap_or_default()).expect("config");
    value.as_object_mut().expect("config object").extend(
        common::partial_value(&p.config)?
            .as_object()
            .expect("partial object")
            .clone(),
    );
    serde_json::from_value(value).map_err(|_| invalid())
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&DeploymentConfig>,
    p: &Prepared,
    runtime: Option<RuntimeObservation>,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(&p.config)?;
    let old = current.map_or(json!({}), |v| serde_json::to_value(v).expect("config"));
    let fields = common::changes(client, &old, &patch);
    let after = summary(client, &proposed(current, p)?);
    let before = current.map(|v| summary(client, v));
    let target_changed = before.as_ref().is_some_and(|v| {
        serde_json::to_value(&v.attachment).expect("attachment")
            != serde_json::to_value(&after.attachment).expect("attachment")
    });
    let direct_image =
        current.is_some() && matches!(p.config.image, Some(DeploymentImage::Image { .. }));
    let registry_check = !fields.is_empty()
        && current.is_some()
        && matches!(&p.config.image, Some(DeploymentImage::Image { image }) if !image.contains('@'));
    let build_policy = matches!(p.config.image, Some(DeploymentImage::Build { .. }))
        && current.is_some_and(|v| v.redeploy_on_build);
    let auto_deploy = ((registry_check && current.is_some_and(|v| v.auto_update)) || build_policy)
        && !fields.is_empty();
    let mut effects = vec![
        "MCP submits only a typed Deployment definition write, never deployment, restart, image-pull, shell or host-provisioning requests. Core refreshes target caches. Saving desired configuration does not establish deployment or health.",
        "Apply consumes the shared ten-minute process-local preview, rechecks raw configuration/references and submits at most once. Core has no conditional write or invocation idempotency; another writer can race after the final reads.",
        "Runtime observation is from preview-time Core inventory. Its reported image can fall back to desired configuration when runtime cache data is missing; it does not verify the running image or configuration.",
    ];
    if current.is_some() && p.config.image.is_some() {
        effects.push("Any supplied image update requires both local operate and configuration-edit grants, independently of the observed auto_update or redeploy_on_build policy. A configuration-only caller can preview but cannot apply it.");
    }
    if direct_image {
        effects.push("A submitted direct image triggers Core CheckDeploymentForUpdate: Periphery can log in to the registry and inspect its manifest. If policy permits and a running workload has a newer digest, Core can auto-deploy (including pulling/recreating), independently of MCP issuing any execution request. Hardcoded digests skip this check in Core 2.3.2; policies and runtime state can race.");
    }
    if matches!(p.config.image, Some(DeploymentImage::Build { .. })) {
        effects.push("Build references save the requested version. 0.0.0 selects latest at deploy, so future builds can change that selection. Core's existing redeploy_on_build policy can redeploy a running Deployment after an attached Build completes, including pinned versions. This save does not start a Build or establish artifact immutability.");
    }
    if target_changed {
        effects.push("The SDK warns about old-target cleanup, but inspected Core 2.3.2 UpdateDeployment only refreshes the new target and does not remove the old workload. Inspect both targets. Custom-name edits can rename live containers and are outside these supported fields; existing names are preserved.");
    }
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "deployment",
            id,
            name: common::bounded_text(client, name, 256),
        },
        preview_id: None,
        changes: fields,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects,
        definition: Some(common::DefinitionPreview::Deployment(DefinitionPreview {
            before,
            after,
            resolved_references: p.summaries.clone(),
            runtime_observation: runtime,
            requires_operate_to_apply: current.is_some() && p.config.image.is_some(),
            core_registry_check_possible: registry_check,
            core_auto_deploy_possible: auto_deploy,
            target_changed,
            old_target_cleanup: "not_performed_by_inspected_core_2_3_2_update_path",
        })),
    })
}
fn saved_projection(
    client: &CoreClient,
    result: &mut ConfigurationResult,
    config: &DeploymentConfig,
) {
    if let Some(common::DefinitionPreview::Deployment(d)) = &mut result.definition {
        d.after = summary(client, config);
    }
}

pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    input.patch.validate(client)?;
    let bounded = client.bounded_evidence();
    let (listed, resource) = inventory::resolve_deployment(&bounded, input.target.clone()).await?;
    let p = prepare(&bounded, &input.patch, Some(&resource.config)).await?;
    let runtime = RuntimeObservation {
        state: serde_json::to_value(listed.info.state)
            .expect("state")
            .as_str()
            .unwrap_or("unknown")
            .into(),
        core_reported_image: (!listed.info.image.is_empty())
            .then(|| image_display(&listed.info.image)),
        image_may_be_desired_fallback: true,
        running_configuration_verified: false,
        observed_at: "preview_time",
    };
    let result = result(
        client,
        Some(resource.id.clone()),
        &resource.name,
        Some(&resource.config),
        &p,
        Some(runtime),
    )?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("deployment_update", &input),
        baseline: common::resource_baseline("deployment", &resource),
        prepared: prepared_value(&p),
        result,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_image_operate(client, input.patch.image.is_some())?;
    input.patch.validate(client)?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("deployment_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, current) = inventory::resolve_deployment(&bounded, input.target).await?;
    if common::resource_baseline("deployment", &current) != entry.baseline {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let p = prepare(&bounded, &input.patch, Some(&current.config)).await?;
    let patch = common::partial_value(&p.config)?;
    if prepared_value(&p) != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let response = client
        .write(UpdateDeployment {
            id: current.id.clone(),
            config: p.config,
        })
        .await;
    Ok(common::finish_save(
        entry.result,
        response,
        &patch,
        &current.name,
        Some(&current.id),
        "UpdateDeployment",
        |result, config| saved_projection(client, result, config),
    ))
}
fn create_name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    input.config.validate(client)?;
    common::validate_payload(client, &input.name)?;
    let name = to_container_compatible_name(&input.name);
    if input.name.len() > 256
        || name.is_empty()
        || name.len() > 128
        || (name.len() == 24 && name.bytes().all(|b| b.is_ascii_hexdigit()))
        || input.config.image.is_none()
        || input.config.attachment.is_none()
    {
        return Err(AppError::arguments(
            "CreateDeployment",
            "Provide a nonempty bounded container-compatible name, explicit image or Build version, and existing Server/Swarm attachment",
        ));
    }
    Ok(name)
}
async fn require_absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_deployment(
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
            "CreateDeployment",
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
    let p = prepare(&bounded, &input.config, None).await?;
    let mut result = result(client, None, &name, None, &p, None)?;
    result.effects.push("Creation checks only bounded visible name absence. Core enforces global uniqueness, creation policy and Read/Attach on targets and Builds; hidden names and a concurrent creator can still reject the write. The normalized name is shown here. Creation does not deploy or enable auto-update.");
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("deployment_create", &input),
        baseline: json!({"kind":"deployment_create","name":name}),
        prepared: prepared_value(&p),
        result,
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
    let name = create_name(client, &submitted)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("deployment_create", &submitted),
    )?;
    if entry.baseline != json!({"kind":"deployment_create","name":name}) {
        return Err(AppError::new(ErrorKind::InvalidPreview, OP, None));
    }
    let bounded = client.bounded_evidence();
    require_absent(&bounded, &name).await?;
    let p = prepare(&bounded, &submitted.config, None).await?;
    let patch = common::partial_value(&p.config)?;
    if prepared_value(&p) != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let response = client
        .write(CreateDeployment {
            name: name.clone(),
            config: p.config,
        })
        .await;
    Ok(common::finish_save(
        entry.result,
        response,
        &patch,
        &name,
        None,
        "CreateDeployment",
        |result, config| saved_projection(client, result, config),
    ))
}
