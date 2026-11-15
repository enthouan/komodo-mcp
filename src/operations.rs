//! Single-target execution. Submission evidence never authorizes an automatic retry.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use komodo_client::{
    api::execute::{
        Deploy, DeployStack, KomodoExecuteRequest, RestartDeployment, RestartStack,
        StartDeployment, StartStack, StopDeployment, StopStack,
    },
    entities::{Operation, ResourceTarget, stack::Stack, update::Update},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    error::{AppError, ErrorKind},
    history::{self, UpdateDetail, UpdateInput},
    inventory::{self, DetailInput},
};

pub const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_RECEIPT_BYTES: usize = 65_536;
const MAX_SERVICES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StackScope {
    // An empty struct variant is intentional: serde's unit variant accepts
    // leftover fields, which could discard a conflicting service selection.
    WholeStack {},
    /// One exact Compose service. No wildcard, array, or display-name matching.
    Service {
        #[schemars(
            length(min = 1, max = 128),
            regex(pattern = "^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$")
        )]
        service: String,
    },
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackOperationInput {
    pub target: DetailInput,
    /// Required: choose whole_stack or one exact service explicitly.
    pub scope: StackScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    Stack,
    Deployment,
    Repo,
    Build,
    Procedure,
    Action,
    ResourceSync,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationResource {
    pub kind: WorkloadKind,
    pub id: String,
    /// Sanitized display only. Never used for execution or recovery resolution.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestedScope {
    WholeStack {},
    Service {
        service: String,
    },
    WholeDeployment {},
    WholeRepo {},
    WholeBuild {},
    WholeProcedure {},
    WholeAction {},
    ResourceSync {
        selection: crate::sync_plans::ExecutionScope,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadOperation {
    Deploy,
    Start,
    Stop,
    Restart,
    Clone,
    Pull,
    Run,
    Cancel,
}

impl WorkloadOperation {
    pub(crate) fn stack_variants(self) -> Result<(&'static str, Operation, Operation), AppError> {
        Ok(match self {
            WorkloadOperation::Deploy => (
                "DeployStack",
                Operation::DeployStack,
                Operation::DeployStackService,
            ),
            WorkloadOperation::Start => (
                "StartStack",
                Operation::StartStack,
                Operation::StartStackService,
            ),
            WorkloadOperation::Stop => (
                "StopStack",
                Operation::StopStack,
                Operation::StopStackService,
            ),
            WorkloadOperation::Restart => (
                "RestartStack",
                Operation::RestartStack,
                Operation::RestartStackService,
            ),
            _ => return Err(unsupported("StackExecution")),
        })
    }
    pub(crate) fn deployment_variant(self) -> Result<(&'static str, Operation), AppError> {
        Ok(match self {
            WorkloadOperation::Deploy => ("Deploy", Operation::Deploy),
            WorkloadOperation::Start => ("StartDeployment", Operation::StartDeployment),
            WorkloadOperation::Stop => ("StopDeployment", Operation::StopDeployment),
            WorkloadOperation::Restart => ("RestartDeployment", Operation::RestartDeployment),
            _ => return Err(unsupported("DeploymentExecution")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    Accepted,
    Rejected,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionWindow {
    /// Local UTC Unix milliseconds; absent if the local clock cannot provide it.
    pub started_ts: Option<i64>,
    pub ended_ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredEffects {
    /// Settings used by this operation, not proof that hooks ran or dependencies changed.
    pub pre_deploy_hook: bool,
    pub post_deploy_hook: bool,
    pub auto_pull: Option<bool>,
    pub saved_extra_args: bool,
    pub saved_command_wrapper: bool,
    /// Named-service targeting is not an isolation guarantee. Saved hooks, flags,
    /// wrappers, Compose dependencies or project changes may affect more resources.
    pub broader_effects_possible: bool,
    /// Lifecycle uses the deployed Compose project; deploy uses saved sources.
    pub compose_dependencies: Option<ComposeDependencies>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<crate::repositories::RepoEffects>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<crate::build_operations::BuildEffects>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<crate::procedures::ProcedureEffects>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<crate::actions::ActionEffects>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComposeDependencies {
    DeploySavedDefinition,
    StartExistingWithDependencies,
    StopSelectedOnly,
    RestartWithConfiguredDependents,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmissionReceipt {
    pub resource: OperationResource,
    pub requested_scope: RequestedScope,
    pub operation: WorkloadOperation,
    /// Core uses a distinct operation variant for a named-service execution.
    pub core_operation: String,
    pub submission: SubmissionState,
    pub submission_window: SubmissionWindow,
    pub update_id: Option<String>,
    /// Latest bounded update observation; acknowledgement is retained on read failure.
    pub latest_update: Option<UpdateDetail>,
    pub observation_error: Option<AppError>,
    pub submission_error: Option<AppError>,
    pub configured_effects: ConfiguredEffects,
    pub automatic_resubmission: bool,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancellationTargeting {
    Resource,
    Update,
}
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancellationOutcome {
    NotSubmitted,
    Acknowledged,
    Rejected,
    Unknown,
}
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancellationStop {
    OriginalComplete,
    NotActive,
    ReplacementRun,
    AmbiguousActiveRuns,
    IncompleteActiveEvidence,
}

/// Reused across resource-specific cancellation APIs. The cancellation's own
/// receipt and the original update are independent observations, never rollback.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancellationResult {
    pub resource: OperationResource,
    pub original_update_id: String,
    pub targeting: CancellationTargeting,
    pub outcome: CancellationOutcome,
    pub not_submitted_reason: Option<CancellationStop>,
    /// For update targeting, this describes only the checked original ID, not
    /// an exhaustive list of simultaneous runs.
    pub active_update_id: Option<String>,
    pub active_evidence_complete: bool,
    pub cancellation_receipt: Option<Box<SubmissionReceipt>>,
    pub original_update: Option<UpdateDetail>,
    pub original_observation_error: Option<AppError>,
    pub original_terminal_confirmed: bool,
    pub replacement_run_observed: bool,
    pub remote_cancellation_requested: bool,
    pub automatic_resubmission: bool,
    pub targeting_limit: &'static str,
}
impl CancellationResult {
    pub(crate) fn bound(mut self, client: &CoreClient) -> Self {
        self.original_terminal_confirmed = self
            .original_update
            .as_ref()
            .is_some_and(|u| u.summary.status == history::UpdateStatus::Complete);
        while serde_json::to_vec(&inventory::projected_value(client, &self))
            .expect("cancellation JSON")
            .len()
            > MAX_RECEIPT_BYTES
        {
            if let Some(original) = self.original_update.as_mut()
                && original.stages.pop().is_some()
            {
                original.stages_truncated = true;
                continue;
            }
            if let Some(latest) = self
                .cancellation_receipt
                .as_mut()
                .and_then(|r| r.latest_update.as_mut())
                && latest.stages.pop().is_some()
            {
                latest.stages_truncated = true;
                continue;
            }
            unreachable!("bounded fixed cancellation fields fit their budget");
        }
        self
    }
}

fn now_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

pub(crate) fn require_operate(
    client: &CoreClient,
    operation: &'static str,
) -> Result<(), AppError> {
    if client.capabilities().allows(Capability::Operate) {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::CapabilityDisabled,
            operation,
            None,
        ))
    }
}

fn object_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unsupported(operation: &'static str) -> AppError {
    AppError::new(ErrorKind::UnsupportedTarget, operation, None)
}

impl StackScope {
    pub(crate) fn validate(&self, operation: &'static str) -> Result<(), AppError> {
        if let Self::Service { service } = self
            && (service.is_empty()
                || service.len() > 128
                || service.starts_with('-')
                || !service
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-')))
        {
            return Err(AppError::arguments(
                operation,
                "Choose whole_stack or one exact 1..128-byte Compose service name using ASCII letters, digits, underscores, dots or hyphens, without a leading hyphen",
            ));
        }
        Ok(())
    }
    fn requested(&self) -> RequestedScope {
        match self {
            Self::WholeStack {} => RequestedScope::WholeStack {},
            Self::Service { service } => RequestedScope::Service {
                service: service.clone(),
            },
        }
    }
    fn services(&self) -> Vec<String> {
        match self {
            Self::WholeStack {} => vec![],
            Self::Service { service } => vec![service.clone()],
        }
    }
}

fn check_target(
    template: bool,
    server: &str,
    swarm: &str,
    operation: &'static str,
) -> Result<(), AppError> {
    let target = if swarm.is_empty() { server } else { swarm };
    if template || target.is_empty() {
        return Err(unsupported(operation));
    }
    if !object_id(target) {
        return Err(AppError::new(ErrorKind::InvalidResponse, operation, None));
    }
    Ok(())
}

fn validate_stack_scope(
    stack: &Stack,
    scope: &StackScope,
    action: WorkloadOperation,
    operation: &'static str,
) -> Result<(), AppError> {
    check_target(
        stack.template,
        &stack.config.server_id,
        &stack.config.swarm_id,
        operation,
    )?;
    if action != WorkloadOperation::Deploy && !stack.config.swarm_id.is_empty() {
        return Err(unsupported(operation));
    }
    if let StackScope::Service { service } = scope {
        if !stack.config.swarm_id.is_empty() {
            return Err(unsupported(operation));
        }
        // Deploy uses saved sources; lifecycle uses the existing deployed project.
        // Never select newly saved services for a lifecycle action or fall back
        // from an absent deployed inventory to a whole-stack execution.
        let names = if action == WorkloadOperation::Deploy {
            stack.info.latest_services.as_slice()
        } else {
            stack.info.deployed_services.as_deref().unwrap_or_default()
        };
        if names.is_empty() || names.len() > MAX_SERVICES {
            return Err(AppError::arguments(
                operation,
                "Core has no bounded service inventory for this action; inspect the saved definition for deploy or deployed project for lifecycle before selecting a service",
            ));
        }
        if names
            .iter()
            .filter(|name| name.service_name == *service)
            .count()
            != 1
        {
            return Err(AppError::arguments(
                operation,
                "The exact service is absent or ambiguous in Core's service inventory for this action; inspect the stack. No execution was submitted",
            ));
        }
    }
    Ok(())
}

fn resource(client: &CoreClient, kind: WorkloadKind, id: String, name: &str) -> OperationResource {
    let safe = client.redact(name);
    let mut end = safe.len().min(256);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    OperationResource {
        kind,
        id,
        name: safe[..end].into(),
    }
}

pub async fn deploy_stack(
    client: &CoreClient,
    input: StackOperationInput,
) -> Result<SubmissionReceipt, AppError> {
    stack_operation(client, input, WorkloadOperation::Deploy).await
}

pub async fn stack_operation(
    client: &CoreClient,
    input: StackOperationInput,
    action: WorkloadOperation,
) -> Result<SubmissionReceipt, AppError> {
    let (operation, whole, service) = action.stack_variants()?;
    require_operate(client, operation)?;
    input.scope.validate(operation)?;
    let (_, stack) = inventory::resolve_stack(&client.bounded_evidence(), input.target).await?;
    validate_stack_scope(&stack, &input.scope, action, operation)?;
    let core_operation = match &input.scope {
        StackScope::WholeStack {} => whole,
        StackScope::Service { .. } => service,
    };
    let deploy = action == WorkloadOperation::Deploy;
    let effects = ConfiguredEffects {
        pre_deploy_hook: deploy && !stack.config.pre_deploy.is_none(),
        post_deploy_hook: deploy && !stack.config.post_deploy.is_none(),
        auto_pull: deploy.then_some(stack.config.auto_pull),
        saved_extra_args: deploy && !stack.config.extra_args.is_empty(),
        saved_command_wrapper: deploy && !stack.config.compose_cmd_wrapper.is_empty(),
        broader_effects_possible: true,
        compose_dependencies: stack.config.swarm_id.is_empty().then_some(match action {
            WorkloadOperation::Deploy => ComposeDependencies::DeploySavedDefinition,
            WorkloadOperation::Start => ComposeDependencies::StartExistingWithDependencies,
            WorkloadOperation::Stop => ComposeDependencies::StopSelectedOnly,
            WorkloadOperation::Restart => ComposeDependencies::RestartWithConfiguredDependents,
            _ => return Err(unsupported(operation)),
        }),
        repository: None,
        build: None,
        procedure: None,
        action: None,
    };
    let target = resource(client, WorkloadKind::Stack, stack.id.clone(), &stack.name);
    let scope = input.scope.requested();
    let services = input.scope.services();
    let id = stack.id;
    match action {
        WorkloadOperation::Deploy => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                DeployStack {
                    stack: id,
                    services,
                    stop_time: None,
                },
            )
            .await
        }
        WorkloadOperation::Start => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                StartStack {
                    stack: id,
                    services,
                },
            )
            .await
        }
        WorkloadOperation::Stop => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                StopStack {
                    stack: id,
                    services,
                    stop_time: None,
                },
            )
            .await
        }
        WorkloadOperation::Restart => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                RestartStack {
                    stack: id,
                    services,
                },
            )
            .await
        }
        _ => Err(unsupported(operation)),
    }
}

pub async fn deploy_deployment(
    client: &CoreClient,
    input: DetailInput,
) -> Result<SubmissionReceipt, AppError> {
    deployment_operation(client, input, WorkloadOperation::Deploy).await
}

pub async fn deployment_operation(
    client: &CoreClient,
    input: DetailInput,
    action: WorkloadOperation,
) -> Result<SubmissionReceipt, AppError> {
    let (operation, core_operation) = action.deployment_variant()?;
    require_operate(client, operation)?;
    let (_, deployment) = inventory::resolve_deployment(&client.bounded_evidence(), input).await?;
    check_target(
        deployment.template,
        &deployment.config.server_id,
        &deployment.config.swarm_id,
        operation,
    )?;
    if action != WorkloadOperation::Deploy && !deployment.config.swarm_id.is_empty() {
        return Err(unsupported(operation));
    }
    let target = resource(
        client,
        WorkloadKind::Deployment,
        deployment.id.clone(),
        &deployment.name,
    );
    let effects = ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: action == WorkloadOperation::Deploy
            && !deployment.config.extra_args.is_empty(),
        saved_command_wrapper: false,
        broader_effects_possible: action == WorkloadOperation::Deploy,
        compose_dependencies: None,
        repository: None,
        build: None,
        procedure: None,
        action: None,
    };
    let scope = RequestedScope::WholeDeployment {};
    let id = deployment.id;
    match action {
        WorkloadOperation::Deploy => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                Deploy {
                    deployment: id,
                    stop_signal: None,
                    stop_time: None,
                },
            )
            .await
        }
        WorkloadOperation::Start => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                StartDeployment { deployment: id },
            )
            .await
        }
        WorkloadOperation::Stop => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                StopDeployment {
                    deployment: id,
                    signal: None,
                    time: None,
                },
            )
            .await
        }
        WorkloadOperation::Restart => {
            submit(
                client,
                target,
                scope,
                action,
                core_operation,
                effects,
                RestartDeployment { deployment: id },
            )
            .await
        }
        _ => Err(unsupported(operation)),
    }
}

pub(crate) async fn submit<T>(
    client: &CoreClient,
    resource: OperationResource,
    requested_scope: RequestedScope,
    operation: WorkloadOperation,
    core_operation: Operation,
    configured_effects: ConfiguredEffects,
    request: T,
) -> Result<SubmissionReceipt, AppError>
where
    T: Serialize + KomodoExecuteRequest<Response = Update>,
{
    submit_future(
        client,
        resource,
        requested_scope,
        operation,
        core_operation,
        configured_effects,
        client.execute(request),
    )
    .await
}

/// Reuse acknowledgement/uncertainty handling with a narrower authorized
/// transport, without adding RunSync to the generic execution allowlist.
pub(crate) async fn submit_future(
    client: &CoreClient,
    resource: OperationResource,
    requested_scope: RequestedScope,
    operation: WorkloadOperation,
    core_operation: Operation,
    configured_effects: ConfiguredEffects,
    execution: impl std::future::Future<Output = Result<Result<Update, AppError>, AppError>>,
) -> Result<SubmissionReceipt, AppError> {
    let started_ts = now_ms();
    let reply = execution.await?;
    let mut receipt = SubmissionReceipt {
        resource,
        requested_scope,
        operation,
        core_operation: core_operation.to_string(),
        submission: SubmissionState::Unknown,
        submission_window: SubmissionWindow {
            started_ts,
            ended_ts: now_ms(),
        },
        update_id: None,
        latest_update: None,
        observation_error: None,
        submission_error: None,
        configured_effects,
        automatic_resubmission: false,
    };
    match reply {
        Err(error) => {
            // These client-error responses reject submission; proxy/server errors,
            // transport loss, deadlines and malformed acknowledgements remain unknown.
            if error
                .status
                .is_some_and(|status| matches!(status, 400 | 401 | 403 | 404 | 405 | 409 | 422))
            {
                receipt.submission = SubmissionState::Rejected;
            }
            receipt.submission_error = Some(error);
        }
        Ok(update) => {
            let expected = match receipt.resource.kind {
                WorkloadKind::Stack => ResourceTarget::Stack(receipt.resource.id.clone()),
                WorkloadKind::Deployment => ResourceTarget::Deployment(receipt.resource.id.clone()),
                WorkloadKind::Repo => ResourceTarget::Repo(receipt.resource.id.clone()),
                WorkloadKind::Build => ResourceTarget::Build(receipt.resource.id.clone()),
                WorkloadKind::Procedure => ResourceTarget::Procedure(receipt.resource.id.clone()),
                WorkloadKind::Action => ResourceTarget::Action(receipt.resource.id.clone()),
                WorkloadKind::ResourceSync => {
                    ResourceTarget::ResourceSync(receipt.resource.id.clone())
                }
            };
            if update.operation != core_operation
                || update.target != expected
                || !object_id(&update.id)
            {
                receipt.submission_error = Some(AppError::new(
                    ErrorKind::InvalidResponse,
                    "execute_acknowledgement",
                    None,
                ));
            } else {
                receipt.submission = SubmissionState::Accepted;
                let id = update.id.to_ascii_lowercase();
                receipt.update_id = Some(id.clone());
                receipt.latest_update = Some(history::project_update_detail(client, &update)?);
                // One read, not a rollout wait. Preserve the acknowledged ID and
                // observation when history is unavailable, denied or slow.
                match tokio::time::timeout(
                    OBSERVATION_TIMEOUT,
                    history::get_update(client, UpdateInput { id }),
                )
                .await
                {
                    Ok(Ok(observation))
                        if observation.summary.target
                            == receipt.latest_update.as_ref().unwrap().summary.target
                            && observation.summary.operation == receipt.core_operation =>
                    {
                        receipt.latest_update = Some(observation)
                    }
                    Ok(Ok(_)) => {
                        receipt.observation_error =
                            Some(AppError::new(ErrorKind::InvalidResponse, "GetUpdate", None))
                    }
                    Ok(Err(error)) => receipt.observation_error = Some(error),
                    Err(_) => {
                        receipt.observation_error =
                            Some(AppError::new(ErrorKind::Timeout, "GetUpdate", None))
                    }
                }
            }
        }
    }
    if receipt.submission == SubmissionState::Unknown
        && let Some(error) = &mut receipt.submission_error
    {
        error.correction = Some(
            "Submission acceptance is unknown. Use get_update/wait_for_update for a saved ID, or reconcile_operation with the retained target, scope, operation and time window. Never automatically repeat execution.",
        );
    }
    // Bound after sanitization and JSON escaping. Normal maximum stage evidence
    // fits; remove trailing stages (failures are already first) rather than ever
    // dropping the ID or all useful failure evidence just because escaping grows.
    while serde_json::to_vec(&inventory::projected_value(client, &receipt))
        .expect("serializable receipt")
        .len()
        > MAX_RECEIPT_BYTES
    {
        let update = receipt
            .latest_update
            .as_mut()
            .expect("only stage evidence can exceed the fixed receipt budget");
        assert!(
            update.stages.pop().is_some(),
            "fixed receipt fields fit their budget"
        );
        update.stages_truncated = true;
    }
    Ok(receipt)
}
