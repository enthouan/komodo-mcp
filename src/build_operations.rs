//! Saved Server-backed Build execution and explicitly requested cancellation.
use crate::{
    build_configuration as definitions, builders,
    client::CoreClient,
    configuration::bounded_text,
    error::AppError,
    inventory::{self, DetailInput},
    operations::{
        self, CancellationResult, ConfiguredEffects, OperationResource, RequestedScope,
        SubmissionReceipt, WorkloadKind, WorkloadOperation,
    },
};
use komodo_client::{
    api::{
        execute::{CancelBuild, RunBuild},
        read::GetBuildActionState,
    },
    entities::{Operation, build::Build},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectedEffects {
    pub pre_build: bool,
    pub registry_pushes: bool,
    /// Must acknowledge possible hidden/concurrent dependent redeployments.
    pub dependent_redeployments_possible: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub target: DetailInput,
    pub expected_effects: ExpectedEffects,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    pub target: DetailInput,
    /// Original RunBuild update, checked locally; Core cannot receive this guard.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub expected_update_id: String,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BuildEffects {
    pub acknowledgement: ExpectedEffects,
    pub definition: definitions::DefinitionSummary,
    pub builder: builders::DefinitionSummary,
    pub dependent_deployments: definitions::DependentEffects,
    pub version_policy: &'static str,
    pub server_selection_evidence: &'static str,
    pub configuration_race: &'static str,
    pub execution_authority: &'static str,
}
pub(crate) async fn effects(
    client: &CoreClient,
    b: &Build,
    builder: builders::DefinitionSummary,
) -> BuildEffects {
    BuildEffects {
        acknowledgement: ExpectedEffects {
            pre_build: !b.config.pre_build.is_none(),
            registry_pushes: b.config.image_registry.iter().any(|r| !r.domain.is_empty()),
            dependent_redeployments_possible: true,
        },
        definition: definitions::summary(client, &b.config),
        builder,
        dependent_deployments: definitions::dependents(client, Some(&b.id)).await,
        version_policy: "Core increments the attempted version when auto_increment_version is enabled and stores config.version on successful build. MCP does not rewrite the saved definition. Update.version is attempt evidence, not image/deployment health.",
        server_selection_evidence: "Preflight reads the actual Server Builder and ordered Server definitions/trust. Core selects from cached-healthy Servers using least current usage, breaking ties by configured order; the Build update does not identify the selected Server. This is not an attestation of the actual runner.",
        configuration_race: "MCP rechecks the actual Build-to-Builder association and Server variant in the shared transport before one stable-ID submission. Core reads saved configuration again asynchronously; source, hooks, push/dependent policy and Builder type can change after inspection. No CAS or cloud-isolation guarantee is available across that race.",
        execution_authority: "The saved pre_build command, Dockerfile, arguments and interpolation are trusted code on the Builder Server. Core explicitly suppresses linked Repo clone/pull hooks in this Build path. Registry pushes and asynchronous dependent Deployments can have broader effects under Core authority. Successful build/push does not establish deployment health.",
    }
}
async fn inspect(
    client: &CoreClient,
    target: DetailInput,
) -> Result<(Build, BuildEffects), AppError> {
    let bounded = client.bounded_evidence();
    let (_, b) = inventory::resolve_build(&bounded, target).await?;
    if b.template {
        return Err(builders::unsupported());
    }
    let (builder, _) = builders::resolve_supported(
        &bounded,
        DetailInput {
            id: Some(b.config.builder_id.clone()),
            name: None,
        },
    )
    .await?;
    if builders::server_config(&builder)?.server_ids.is_empty() {
        return Err(builders::unsupported());
    }
    let e = effects(&bounded, &b, builders::summary(&builder.config)).await;
    Ok((b, e))
}
fn resource(client: &CoreClient, b: &Build) -> OperationResource {
    OperationResource {
        kind: WorkloadKind::Build,
        id: b.id.clone(),
        name: bounded_text(client, &b.name, 256),
    }
}
fn configured(b: &Build, e: BuildEffects) -> ConfiguredEffects {
    ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: !b.config.extra_args.is_empty(),
        saved_command_wrapper: false,
        broader_effects_possible: true,
        compose_dependencies: None,
        repository: None,
        build: Some(e),
        procedure: None,
        action: None,
    }
}
pub async fn run(client: &CoreClient, input: RunInput) -> Result<SubmissionReceipt, AppError> {
    operations::require_operate(client, "RunBuild")?;
    let (b, e) = inspect(client, input.target).await?;
    if e.acknowledgement != input.expected_effects {
        return Err(AppError::arguments(
            "RunBuild",
            "Inspect get_build.configured_effects and copy its acknowledgement flags, including possible hidden/concurrent dependent redeployments. Effects changed or were not acknowledged; no execution was submitted.",
        ));
    }
    let target = resource(client, &b);
    let effects = configured(&b, e);
    operations::submit(
        client,
        target,
        RequestedScope::WholeBuild {},
        WorkloadOperation::Run,
        Operation::RunBuild,
        effects,
        RunBuild { build: b.id },
    )
    .await
}

/// This additional preflight is inside the shared mutation boundary so another
/// internal caller cannot bypass the direct Build tools' Server-only scope.
pub(crate) async fn guard_execute(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if !matches!(op, "RunBuild" | "CancelBuild") {
        return Ok(());
    }
    let id = p["build"]
        .as_str()
        .filter(|s| crate::deletion::object_id(s))
        .ok_or_else(builders::unsupported)?;
    let bounded = client.bounded_evidence();
    let (_, b) = inventory::resolve_build(
        &bounded,
        DetailInput {
            id: Some(id.into()),
            name: None,
        },
    )
    .await?;
    if b.template {
        return Err(builders::unsupported());
    }
    let (builder, _) = builders::resolve_supported(
        &bounded,
        DetailInput {
            id: Some(b.config.builder_id),
            name: None,
        },
    )
    .await?;
    if builders::server_config(&builder)?.server_ids.is_empty() {
        return Err(builders::unsupported());
    }
    Ok(())
}
pub async fn cancel(
    client: &CoreClient,
    input: CancelInput,
) -> Result<CancellationResult, AppError> {
    operations::require_operate(client, "CancelBuild")?;
    let original = crate::history::UpdateInput {
        id: input.expected_update_id,
    }
    .validate("CancelBuild")?;
    let (b, e) = inspect(client, input.target).await?;
    let context = crate::cancellation::Context {
        resource: resource(client, &b),
        original_update_id: original,
        scope: RequestedScope::WholeBuild {},
        run_operation: Operation::RunBuild,
        cancel_operation: Operation::CancelBuild,
        effects: configured(&b, e),
        limit: "Core 2.3.2 CancelBuild accepts only the Build ID, not an expected update ID. MCP checks the selected RunBuild update, a bounded active-run filter and action state, then rechecks before submission. Another run can replace it after the final reads; cancellation may affect that replacement. The cancellation update completing only acknowledges queuing (or Core's 60-second cleanup); it does not prove the original run terminated. No rollback or atomic historical-run cancellation is promised.",
    };
    let bounded = client.bounded_evidence();
    let id = b.id.clone();
    crate::cancellation::resource(client, context, CancelBuild { build: b.id }, || async {
        bounded
            .read(GetBuildActionState { build: id.clone() })
            .await
            .map(|s| s.building)
    })
    .await
}
