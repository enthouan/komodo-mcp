//! Managed Swarm replica saves, separately reviewed Deploy, and read-only evidence.
//! Raw definitions/references stay in the shared process-local preview store.
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use komodo_client::{
    api::read::{
        GetBuild, GetDeployment, GetSwarm, InspectSwarmService, InspectSwarmTask,
        ListSwarmServices, ListSwarmTasks,
    },
    entities::{
        deployment::{Deployment, DeploymentImage},
        swarm::Swarm,
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
    },
    deployment_configuration::{self as definitions, DeploymentPatch},
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
    operations::{self, SubmissionReceipt},
    swarms,
};

const OP: &str = "deployment_scale";
const MAX_PRIVATE_BYTES: usize = 256 * 1024;
const EFFECTS: &[&str] = &[
    "Saving changes only replicas and its managed --replicas=N argument. Other saved arguments, secrets, image/Build references and targets are preserved. Saved flags remain trusted configuration and can have broader effects.",
    "Pinned Periphery 2.3.2 ignores the replicas field when creating a service. The managed argument supplies the requested count, including zero. Complex/conflicting replica, mode, name or shell forms are rejected rather than rewritten.",
    "Saving never submits Deploy. Review and deploy the saved definition explicitly with operate access. Core Write and Execute remain independent permissions; a failed/denied/uncertain deployment does not roll back a successful save.",
    "Core Deploy removes and recreates the Swarm service: interruption and a new service ID are possible. This is not an in-place service update. Core resolves image tags, latest Builds, secrets and registry credentials at execution time.",
    "Reviews share sixteen private, ten-minute, single-use records per process. Apply rechecks full private configuration/references; Core provides no conditional save/deploy or invocation idempotency. Another writer can race after the final reads. Never automatically replay an uncertain mutation.",
    "Execution acceptance/completion, observed replicas and workload health are separate. Use observe_deployment_scale and independent application health checks; an update ID or successful command is not convergence.",
];

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScaleInput {
    #[schemars(schema_with = "target_schema")]
    pub target: DetailInput,
    #[schemars(range(min = 0, max = 1000))]
    pub replicas: i32,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    #[schemars(schema_with = "target_schema")]
    pub target: DetailInput,
    #[schemars(range(min = 0, max = 1000))]
    pub replicas: i32,
    #[schemars(length(min = 1, max = 128))]
    pub preview_id: String,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct ScalePreview {
    pub preview_id: Option<String>,
    pub target: ConfigurationTarget,
    pub swarm_id: String,
    pub current_saved_replicas: i32,
    pub current_service: swarms::Service,
    pub cached_service: swarms::Service,
    pub requested_replicas: i32,
    pub configuration: ConfigurationResult,
    pub effects: Vec<&'static str>,
}
#[derive(Serialize, JsonSchema)]
pub struct ScaleSave {
    pub requested_replicas: i32,
    pub configuration: ConfigurationResult,
    pub deployment_submitted: bool,
    pub execution_review_required: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DeployReview {
    pub preview_id: Option<String>,
    pub target: ConfigurationTarget,
    pub swarm_id: String,
    pub requested_replicas: i32,
    pub saved_definition_verified: bool,
    pub writes_submitted: u8,
    pub executions_submitted: u8,
    pub effects: Vec<&'static str>,
}
#[derive(Serialize, JsonSchema)]
pub struct ScaleDeployment {
    pub requested_replicas: i32,
    pub saved_definition_verified: bool,
    pub configuration_saved_by_this_call: bool,
    pub receipt: Option<SubmissionReceipt>,
    pub not_submitted_error: Option<AppError>,
    pub partial_outcome: bool,
    pub convergence_observed: bool,
    pub application_health_verified: bool,
    pub effects: Vec<&'static str>,
}

struct Managed {
    deployment: Deployment,
    swarm: Swarm,
    baseline: Value,
    references: Value,
    raw_config: Value,
}
// No serialization/Debug: an existing definition review, with its original TTL,
// is retained privately instead of consuming a second persistent capacity slot.
struct SaveReview {
    inner: Preview,
    patch: DeploymentPatch,
    expected_config: Value,
}
struct ExecutionReview {
    final_definition: Value,
}
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Select one existing managed Swarm Deployment and replicas 0–1000; normalize complex/conflicting saved replica, mode or name arguments with the existing definition editor first",
    )
}
fn response(op: &'static str) -> AppError {
    AppError::new(ErrorKind::InvalidResponse, op, None)
}
fn id(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn stable(value: &str) -> DetailInput {
    DetailInput {
        id: Some(value.into()),
        name: None,
    }
}
fn target(client: &CoreClient, d: &Deployment) -> ConfigurationTarget {
    ConfigurationTarget {
        kind: "deployment",
        id: Some(d.id.clone()),
        name: if service_name(&d.name) {
            common::bounded_text(client, &d.name, 128)
        } else {
            "[omitted]".into()
        },
    }
}
fn validate(input: &ScaleInput) -> Result<(), AppError> {
    validate_target(&input.target)?;
    if !(0..=1000).contains(&input.replicas) {
        return Err(invalid());
    }
    Ok(())
}
fn target_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"object","properties":{"id":{"type":"string","pattern":"^[0-9a-fA-F]{24}$"},"name":{"type":"string","minLength":1,"maxLength":128}},"oneOf":[{"required":["id"]},{"required":["name"]}],"additionalProperties":false})
}
fn validate_target(target: &DetailInput) -> Result<(), AppError> {
    match (&target.id, &target.name) {
        (Some(value), None) if id(value) => Ok(()),
        (None, Some(value))
            if !value.trim().is_empty()
                && value.len() <= 128
                && !value.chars().any(char::is_control) =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}
fn require_operate(client: &CoreClient) -> Result<(), AppError> {
    if !client.capabilities().allows(Capability::Operate) {
        return Err(AppError::new(ErrorKind::CapabilityDisabled, OP, None));
    }
    Ok(())
}
fn raw_baseline(
    raw: &Value,
    expected_id: &str,
    name: &str,
    config: &impl Serialize,
    op: &'static str,
) -> Result<Value, AppError> {
    let typed = serde_json::to_value(config).expect("typed config");
    if raw["_id"]["$oid"] != expected_id
        || raw["name"] != name
        || raw["template"] != false
        || !raw["config"].is_object()
        || typed
            .as_object()
            .expect("config object")
            .iter()
            .any(|(k, v)| raw["config"].get(k) != Some(v))
    {
        return Err(response(op));
    }
    Ok(json!({"id":expected_id,"name":name,"template":false,"config":raw["config"]}))
}
async fn managed(client: &CoreClient, input: DetailInput) -> Result<Managed, AppError> {
    let c = client.bounded_evidence();
    let (_, d) = inventory::resolve_deployment(&c, input).await?;
    if d.template || !id(&d.config.swarm_id) {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
    }
    let raw = c
        .read_references(GetDeployment {
            deployment: d.id.clone(),
        })
        .await?;
    let mut baseline = raw_baseline(&raw, &d.id, &d.name, &d.config, "GetDeployment")?;
    let deployed_name = raw["info"]["deployed_name"]
        .as_str()
        .ok_or_else(|| response("GetDeployment"))?;
    if deployed_name != d.info.deployed_name {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    baseline["deployed_name"] = json!(deployed_name);
    let (_, swarm) = inventory::resolve_swarm(&c, stable(&d.config.swarm_id)).await?;
    if swarm.template
        || swarm.config.server_ids.is_empty()
        || swarm.config.server_ids.len() > 8
        || swarm.config.server_ids.iter().any(|v| !id(v))
    {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
    }
    let raw_swarm = c
        .read_references(GetSwarm {
            swarm: swarm.id.clone(),
        })
        .await?;
    let mut refs = vec![raw_baseline(
        &raw_swarm,
        &swarm.id,
        &swarm.name,
        &swarm.config,
        "GetSwarm",
    )?];
    if let DeploymentImage::Build { build_id, .. } = &d.config.image {
        let (_, build) = inventory::resolve_build(&c, stable(build_id)).await?;
        if build.template {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        let raw_build = c
            .read_references(GetBuild {
                build: build.id.clone(),
            })
            .await?;
        refs.push(raw_baseline(
            &raw_build,
            &build.id,
            &build.name,
            &build.config,
            "GetBuild",
        )?);
    }
    let references = json!(refs);
    if serde_json::to_vec(&json!([baseline, references]))
        .expect("private baseline")
        .len()
        > MAX_PRIVATE_BYTES
    {
        return Err(AppError::new(ErrorKind::IncompleteResolution, OP, None));
    }
    Ok(Managed {
        deployment: d,
        swarm,
        baseline,
        references,
        raw_config: raw["config"].clone(),
    })
}

/// Only the replica argument is rewritten. Other accepted strings/order remain
/// byte-for-byte intact; compound replica fragments are never shell-reparsed.
fn argument_words(value: &str) -> Result<Vec<String>, AppError> {
    if value.is_empty() || value.len() > 512 {
        return Err(invalid());
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for ch in value.chars() {
        if ch.is_control() || ch == '\\' {
            return Err(invalid());
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                if q == '"' && matches!(ch, '$' | '`') {
                    return Err(invalid());
                }
                word.push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_ascii_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else if matches!(ch, ';' | '|' | '&' | '<' | '>' | '`' | '$' | '(' | ')') {
            return Err(invalid());
        } else {
            word.push(ch);
        }
    }
    if quote.is_some() {
        return Err(invalid());
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.is_empty() {
        return Err(invalid());
    }
    Ok(words)
}
fn replica_args(args: &[String], replicas: i32) -> Result<Vec<String>, AppError> {
    if args.len() > 32 {
        return Err(invalid());
    }
    let mut result = Vec::new();
    let mut found = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let words = argument_words(arg)?;
        let flag = |name: &str| {
            words.iter().any(|w| {
                w == name
                    || w.strip_prefix(name)
                        .is_some_and(|suffix| suffix.starts_with('='))
            })
        };
        if words.iter().any(|s| s == "--") || flag("--name") {
            return Err(invalid());
        }
        if flag("--mode") {
            if arg == "--mode" && args.get(i + 1).is_some_and(|v| v == "replicated") {
                result.extend_from_slice(&args[i..i + 2]);
                i += 2;
                continue;
            }
            if arg != "--mode=replicated" && arg != "--mode replicated" {
                return Err(invalid());
            }
        }
        if flag("--replicas") {
            if found {
                return Err(invalid());
            }
            let value;
            if arg == "--replicas" {
                i += 1;
                value = args.get(i).ok_or_else(invalid)?.as_str();
            } else if let Some(v) = arg.strip_prefix("--replicas=") {
                value = v;
            } else if let Some(v) = arg.strip_prefix("--replicas ") {
                value = v;
            } else {
                return Err(invalid());
            }
            if value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || value.parse::<u64>().is_err()
            {
                return Err(invalid());
            }
            result.push(format!("--replicas={replicas}"));
            found = true;
        } else {
            result.push(arg.clone());
        }
        i += 1;
    }
    if !found {
        result.push(format!("--replicas={replicas}"));
    }
    if result.len() > 32 {
        return Err(invalid());
    }
    Ok(result)
}

pub async fn preview(client: &CoreClient, input: ScaleInput) -> Result<ScalePreview, AppError> {
    common::require_edit(client)?;
    validate(&input)?;
    let m = managed(client, input.target.clone()).await?;
    let runtime = runtime(client, &m.deployment).await?;
    let prepared = json!({"references":m.references,"runtime":runtime.baseline});
    if serde_json::to_vec(&json!([m.baseline, prepared]))
        .expect("private save review")
        .len()
        > MAX_PRIVATE_BYTES
    {
        return Err(AppError::new(ErrorKind::IncompleteResolution, OP, None));
    }
    let patch = DeploymentPatch {
        replicas: Some(input.replicas),
        extra_args: Some(replica_args(
            &m.deployment.config.extra_args,
            input.replicas,
        )?),
        ..Default::default()
    };
    let inner_input = definitions::PreviewInput {
        target: stable(&m.deployment.id),
        patch: patch.clone(),
    };
    let mut configuration = definitions::preview_update(
        client,
        definitions::PreviewInput {
            target: stable(&m.deployment.id),
            patch: patch.clone(),
        },
    )
    .await?;
    let inner_id = configuration
        .preview_id
        .take()
        .expect("definition preview token");
    let inner = client.previews().take(
        &inner_id,
        &common::bound_input("deployment_update", &inner_input),
    )?;
    if inner.baseline != common::resource_baseline("deployment", &m.deployment) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    configuration.resource = target(client, &m.deployment);
    let mut expected_config = m.raw_config;
    expected_config["replicas"] = json!(input.replicas);
    expected_config["extra_args"] = json!(patch.extra_args);
    let mut public = ScalePreview {
        preview_id: None,
        target: target(client, &m.deployment),
        swarm_id: m.swarm.id,
        current_saved_replicas: m.deployment.config.replicas,
        current_service: runtime.fresh,
        cached_service: runtime.cached,
        requested_replicas: input.replicas,
        configuration,
        effects: EFFECTS.to_vec(),
    };
    let token = client.previews().insert_entry(Preview {
        created: inner.created,
        input: common::bound_input("scale_save", &input),
        baseline: m.baseline,
        prepared,
        result: SaveReview {
            inner,
            patch,
            expected_config,
        },
    })?;
    public.preview_id = Some(token);
    Ok(public)
}

pub async fn save(client: &CoreClient, input: ApplyInput) -> Result<ScaleSave, AppError> {
    common::require_edit(client)?;
    if input.preview_id.is_empty() || input.preview_id.len() > 128 {
        return Err(invalid());
    }
    let submitted = ScaleInput {
        target: input.target,
        replicas: input.replicas,
    };
    validate(&submitted)?;
    let entry = client.previews().take_for::<SaveReview>(
        &input.preview_id,
        &common::bound_input("scale_save", &submitted),
    )?;
    let m = managed(client, submitted.target).await?;
    let public_target = target(client, &m.deployment);
    let runtime = runtime(client, &m.deployment).await?;
    if m.baseline != entry.baseline
        || json!({"references":m.references,"runtime":runtime.baseline}) != entry.prepared
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let SaveReview {
        inner,
        patch,
        expected_config,
    } = entry.result;
    let inner_token = client.previews().insert_entry(inner)?;
    let mut configuration = definitions::apply_update(
        client,
        definitions::ApplyInput {
            target: stable(&m.deployment.id),
            patch,
            preview_id: inner_token,
        },
    )
    .await?;
    if matches!(
        configuration.outcome,
        ConfigurationOutcome::Saved | ConfigurationOutcome::NoOp
    ) {
        let readback = client
            .read_references(GetDeployment {
                deployment: m.deployment.id,
            })
            .await;
        configuration = common::verify_readback(
            configuration,
            readback,
            &expected_config,
            &m.deployment.name,
            "GetDeployment",
        );
    }
    configuration.resource = public_target;
    Ok(ScaleSave {
        requested_replicas: submitted.replicas,
        configuration,
        deployment_submitted: false,
        execution_review_required: true,
    })
}

pub async fn review_deploy(
    client: &CoreClient,
    input: ScaleInput,
) -> Result<DeployReview, AppError> {
    require_operate(client)?;
    validate(&input)?;
    let m = managed(client, input.target.clone()).await?;
    if m.deployment.config.replicas != input.replicas
        || replica_args(&m.deployment.config.extra_args, input.replicas)?
            != m.deployment.config.extra_args
    {
        return Err(invalid());
    }
    let runtime = runtime(client, &m.deployment).await?;
    let prepared = json!({"references":m.references,"runtime":runtime.baseline});
    if serde_json::to_vec(&json!([m.baseline, prepared]))
        .expect("private review")
        .len()
        > MAX_PRIVATE_BYTES
    {
        return Err(AppError::new(ErrorKind::IncompleteResolution, OP, None));
    }
    let mut public = DeployReview {
        preview_id: None,
        target: target(client, &m.deployment),
        swarm_id: m.swarm.id,
        requested_replicas: input.replicas,
        saved_definition_verified: true,
        writes_submitted: 0,
        executions_submitted: 0,
        effects: EFFECTS.to_vec(),
    };
    let token = client.previews().insert_entry(Preview {
        created: Instant::now(),
        input: common::bound_input("scale_deploy", &input),
        baseline: m.baseline,
        prepared,
        result: ExecutionReview {
            final_definition: common::resource_baseline("deployment", &m.deployment),
        },
    })?;
    public.preview_id = Some(token);
    Ok(public)
}
pub async fn deploy(client: &CoreClient, input: ApplyInput) -> Result<ScaleDeployment, AppError> {
    require_operate(client)?;
    if input.preview_id.is_empty() || input.preview_id.len() > 128 {
        return Err(invalid());
    }
    let submitted = ScaleInput {
        target: input.target,
        replicas: input.replicas,
    };
    validate(&submitted)?;
    let entry = client.previews().take_for::<ExecutionReview>(
        &input.preview_id,
        &common::bound_input("scale_deploy", &submitted),
    )?;
    let mut result = ScaleDeployment {
        requested_replicas: submitted.replicas,
        saved_definition_verified: false,
        configuration_saved_by_this_call: false,
        receipt: None,
        not_submitted_error: None,
        partial_outcome: true,
        convergence_observed: false,
        application_health_verified: false,
        effects: EFFECTS.to_vec(),
    };
    let checked = async {
        let m = managed(client, submitted.target).await?;
        let runtime = runtime(client, &m.deployment).await?;
        if m.baseline != entry.baseline
            || json!({"references":m.references,"runtime":runtime.baseline}) != entry.prepared
        {
            return Err(AppError::new(ErrorKind::StalePreview, OP, None));
        }
        result.saved_definition_verified = true;
        let public_name = target(client, &m.deployment).name;
        let mut receipt = operations::deploy_reviewed_deployment(
            client,
            stable(&m.deployment.id),
            &entry.result.final_definition,
        )
        .await?;
        receipt.resource.name = public_name;
        Ok(receipt)
    }
    .await;
    match checked {
        Ok(receipt) => {
            result.partial_outcome = !matches!(
                receipt.latest_update.as_ref().map(|u| u.summary.outcome),
                Some(crate::history::UpdateOutcome::Succeeded)
            );
            result.receipt = Some(receipt);
        }
        Err(error) => {
            // A later read can invalidate the earlier observation. Preserve the
            // separately returned save result, but do not claim this definition
            // remained verified at the unsubmitted execution boundary.
            result.saved_definition_verified = false;
            result.not_submitted_error = Some(error);
        }
    }
    Ok(result)
}

struct Runtime {
    cached: swarms::Service,
    fresh: swarms::Service,
    baseline: Value,
}
fn docker_id(v: &str) -> bool {
    v.len() == 25
        && v.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}
fn service_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.as_bytes()[0].is_ascii_alphanumeric()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
// Read + Inspect on the Swarm. No alternative endpoint or credential fallback.
async fn runtime(client: &CoreClient, d: &Deployment) -> Result<Runtime, AppError> {
    let name = d.deployed_name();
    if !service_name(name) || !service_name(d.custom_name()) {
        return Err(invalid());
    }
    let c = client.bounded_evidence();
    let services = c
        .read(ListSwarmServices {
            swarm: d.config.swarm_id.clone(),
        })
        .await?;
    if services.len() > swarms::MAX_SCAN {
        return Err(AppError::new(ErrorKind::IncompleteResolution, OP, None));
    }
    let mut ids = BTreeSet::new();
    for s in &services {
        if s.id
            .as_deref()
            .is_none_or(|v| !docker_id(v) || !ids.insert(v.to_owned()))
        {
            return Err(response("ListSwarmServices"));
        }
    }
    let mut matches = services.iter().filter(|s| s.name.as_deref() == Some(name));
    let cached = matches
        .next()
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, "ListSwarmServices", None))?;
    if matches.next().is_some() {
        return Err(AppError::new(ErrorKind::AmbiguousTarget, OP, None));
    }
    if name != d.custom_name()
        && services
            .iter()
            .any(|s| s.name.as_deref() == Some(d.custom_name()))
    {
        return Err(AppError::new(ErrorKind::AmbiguousTarget, OP, None));
    }
    let service_id = cached.id.as_ref().expect("checked ID");
    let raw = c
        .read(InspectSwarmService {
            swarm: d.config.swarm_id.clone(),
            service: service_id.clone(),
        })
        .await?;
    let raw_value = serde_json::to_value(&raw).expect("private Docker spec");
    if raw_value["Spec"]["Name"] != name || raw.id.as_deref() != Some(service_id) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    if raw_value["Spec"]["Labels"]
        .get("com.docker.stack.namespace")
        .is_some()
    {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
    }
    let baseline = json!({"id":service_id,"version":raw_value["Version"],"spec":raw_value["Spec"]});
    let fresh = swarms::service_detail(&c, raw)?;
    if fresh.mode.as_deref() != Some("replicated") {
        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
    }
    let spec_replicas = raw_value["Spec"]["Mode"]["Replicated"]["Replicas"].as_u64();
    if spec_replicas.is_none() || fresh.desired_replicas != spec_replicas {
        return Err(response("InspectSwarmService"));
    }
    Ok(Runtime {
        cached: swarms::service_list(&c, cached.clone())?,
        fresh,
        baseline,
    })
}

fn default_timeout() -> u64 {
    5
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveInput {
    #[schemars(schema_with = "target_schema")]
    pub target: DetailInput,
    #[schemars(range(min = 0, max = 1000))]
    pub replicas: i32,
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 30))]
    pub timeout_seconds: u64,
}
#[derive(Serialize, JsonSchema)]
pub struct ReplicaSample {
    pub service: swarms::Service,
    pub cached_service: swarms::Service,
    pub current_running: usize,
    pub pending_current: usize,
    pub failed_current: usize,
    pub unknown_tasks: usize,
    pub historical_tasks: usize,
    pub terminating_running: usize,
    pub other_service_tasks_ignored: usize,
    pub fresh_tasks: Vec<swarms::Task>,
    pub scan_incomplete: bool,
    pub fresh_task_coverage_incomplete: bool,
    pub matching_replica_observations: bool,
    pub core_task_health: String,
    pub application_health: &'static str,
    pub gaps: Vec<AppError>,
}
#[derive(Serialize, JsonSchema)]
pub struct ScaleObservation {
    pub target: Option<ConfigurationTarget>,
    pub swarm_id: Option<String>,
    pub requested_replicas: i32,
    pub saved_replicas: Option<i32>,
    pub samples: u8,
    pub end: &'static str,
    pub latest: Option<ReplicaSample>,
    pub observation_error: Option<AppError>,
    pub mutations_submitted: u8,
    pub evidence_limit: &'static str,
}

async fn sample(
    client: &CoreClient,
    d: &Deployment,
    replicas: i32,
) -> Result<ReplicaSample, AppError> {
    let c = client.bounded_evidence();
    let runtime = runtime(&c, d).await?;
    let items = c
        .read(ListSwarmTasks {
            swarm: d.config.swarm_id.clone(),
        })
        .await?;
    let scan_incomplete = items.len() > swarms::MAX_SCAN;
    let mut ids = BTreeSet::new();
    let mut tasks = Vec::new();
    let mut ignored = 0;
    for raw in items.into_iter().take(swarms::MAX_SCAN) {
        let t = swarms::task_list(&c, raw)?;
        if !ids.insert(t.id.clone()) {
            return Err(response("ListSwarmTasks"));
        }
        if t.service_id.as_deref() == Some(&runtime.fresh.id) {
            tasks.push(t);
        } else {
            ignored += 1;
        }
    }
    let current_running = tasks
        .iter()
        .filter(|t| t.replica_evidence == "current" && t.state.as_deref() == Some("running"))
        .count();
    let pending = tasks
        .iter()
        .filter(|t| {
            t.replica_evidence == "current"
                && matches!(
                    t.state.as_deref(),
                    Some(
                        "new"
                            | "allocated"
                            | "pending"
                            | "assigned"
                            | "accepted"
                            | "preparing"
                            | "ready"
                            | "starting"
                    )
                )
        })
        .count();
    let failed = tasks
        .iter()
        .filter(|t| {
            t.replica_evidence == "current"
                && matches!(
                    t.state.as_deref(),
                    Some("failed" | "rejected" | "orphaned" | "shutdown" | "complete" | "remove")
                )
        })
        .count();
    let unknown = tasks
        .iter()
        .filter(|t| t.replica_evidence == "unknown" || t.state.is_none())
        .count();
    let historical = tasks
        .iter()
        .filter(|t| t.replica_evidence == "historical")
        .count();
    let terminating = tasks
        .iter()
        .filter(|t| t.replica_evidence == "historical" && t.state.as_deref() == Some("running"))
        .count();
    let candidates = tasks
        .iter()
        .filter(|t| t.replica_evidence != "historical" || t.state.as_deref() == Some("running"))
        .collect::<Vec<_>>();
    let mut fresh_tasks = Vec::new();
    let mut gaps = Vec::new();
    for task in candidates.iter().take(16) {
        let fresh = c
            .read(InspectSwarmTask {
                swarm: d.config.swarm_id.clone(),
                task: task.id.clone(),
            })
            .await
            .and_then(|t| swarms::task_detail(&c, t));
        match fresh {
            Ok(t) if t.id == task.id && t.service_id.as_deref() == Some(&runtime.fresh.id) => {
                fresh_tasks.push(t)
            }
            Ok(_) => gaps.push(response("InspectSwarmTask")),
            Err(e) => gaps.push(e),
        }
    }
    let incomplete = scan_incomplete || candidates.len() != fresh_tasks.len();
    let fresh_running = fresh_tasks
        .iter()
        .filter(|t| t.replica_evidence == "current" && t.state.as_deref() == Some("running"))
        .count();
    let fresh_other = fresh_tasks.iter().any(|t| {
        t.replica_evidence == "unknown"
            || (t.replica_evidence == "current" && t.state.as_deref() != Some("running"))
            || (t.replica_evidence == "historical" && t.state.as_deref() == Some("running"))
    });
    let matching = !incomplete
        && gaps.is_empty()
        && unknown == 0
        && pending == 0
        && failed == 0
        && terminating == 0
        && current_running == replicas as usize
        && fresh_running == replicas as usize
        && !fresh_other
        && runtime.fresh.desired_replicas == Some(replicas as u64)
        && runtime.cached.desired_replicas == Some(replicas as u64)
        && runtime.cached.running_tasks == Some(replicas as u64);
    Ok(ReplicaSample {
        core_task_health: runtime.fresh.state.clone(),
        service: runtime.fresh,
        cached_service: runtime.cached,
        current_running,
        pending_current: pending,
        failed_current: failed,
        unknown_tasks: unknown,
        historical_tasks: historical,
        terminating_running: terminating,
        other_service_tasks_ignored: ignored,
        fresh_tasks,
        scan_incomplete,
        fresh_task_coverage_incomplete: incomplete,
        matching_replica_observations: matching,
        application_health: "not_verified",
        gaps,
    })
}
pub async fn observe(
    client: &CoreClient,
    input: ObserveInput,
) -> Result<ScaleObservation, AppError> {
    validate_target(&input.target)?;
    if !(0..=1000).contains(&input.replicas) || !(1..=30).contains(&input.timeout_seconds) {
        return Err(invalid());
    }
    let mut result = ScaleObservation {
        target: None,
        swarm_id: None,
        requested_replicas: input.replicas,
        saved_replicas: None,
        samples: 0,
        end: "deadline",
        latest: None,
        observation_error: None,
        mutations_submitted: 0,
        evidence_limit: "At most three samples, sixteen fresh current/terminating tasks per sample, a 512-task scan and thirty seconds total. Task discovery and service counts are Core caches without generation/completeness guarantees; matching observations are not an atomic cluster proof. Historical tasks and old service IDs never supply current replicas. Missing service/cache cannot establish zero. Core task health is not application health. Reads use Deployment Read and Swarm Read plus Inspect; no permission fallback or mutations.",
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(input.timeout_seconds);
    let mut selected = None;
    for index in 0..3 {
        let read = async {
            let (_, d) =
                inventory::resolve_deployment(&client.bounded_evidence(), input.target.clone())
                    .await?;
            if d.template || !id(&d.config.swarm_id) {
                return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
            }
            let (_, swarm) =
                inventory::resolve_swarm(&client.bounded_evidence(), stable(&d.config.swarm_id))
                    .await?;
            if swarm.template || swarm.config.server_ids.is_empty() {
                return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
            }
            let sample = sample(client, &d, input.replicas).await?;
            Ok((d, sample))
        };
        match tokio::time::timeout_at(deadline, read).await {
            Ok(Ok((d, sample))) => {
                let identity = (
                    d.id.clone(),
                    d.config.swarm_id.clone(),
                    sample.service.id.clone(),
                );
                if selected.as_ref().is_some_and(|s| s != &identity) {
                    result.end = "identity_changed";
                    result.observation_error =
                        Some(AppError::new(ErrorKind::StalePreview, OP, None));
                    break;
                }
                selected = Some(identity);
                result.target = Some(target(client, &d));
                result.swarm_id = Some(d.config.swarm_id);
                result.saved_replicas = Some(d.config.replicas);
                result.samples += 1;
                let matches =
                    sample.matching_replica_observations && d.config.replicas == input.replicas;
                result.latest = Some(sample);
                if matches {
                    result.end = "matching_observations";
                    break;
                }
            }
            Ok(Err(e)) => {
                result.end = "read_failed";
                result.observation_error = Some(e);
                break;
            }
            Err(_) => break,
        }
        if index == 2 {
            result.end = "sample_limit";
            break;
        }
        if tokio::time::timeout_at(deadline, tokio::time::sleep(Duration::from_millis(500)))
            .await
            .is_err()
        {
            break;
        }
    }
    Ok(result)
}
