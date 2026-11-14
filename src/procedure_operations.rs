//! One explicit saved Procedure run or resource-level cancellation.
use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
    operations::{
        self, CancellationResult, ConfiguredEffects, OperationResource, RequestedScope,
        SubmissionReceipt, WorkloadKind, WorkloadOperation,
    },
    procedures,
};
use komodo_client::{
    api::{
        execute::{CancelProcedure, RunProcedure},
        read::GetProcedureActionState,
    },
    entities::{Operation, procedure::Procedure},
};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub target: DetailInput,
    pub expected_effects: procedures::Acknowledgement,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    pub target: DetailInput,
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub expected_update_id: String,
}
async fn inspect(client: &CoreClient, target: DetailInput) -> Result<Procedure, AppError> {
    let (_, p) = inventory::resolve_procedure(&client.bounded_evidence(), target).await?;
    if p.template {
        Err(AppError::new(
            ErrorKind::UnsupportedTarget,
            "procedure_operation",
            None,
        ))
    } else {
        Ok(p)
    }
}
fn resource(client: &CoreClient, p: &Procedure) -> OperationResource {
    OperationResource {
        kind: WorkloadKind::Procedure,
        id: p.id.clone(),
        name: crate::configuration::bounded_text(client, &p.name, 256),
    }
}
fn effects(client: &CoreClient, p: &Procedure) -> ConfiguredEffects {
    ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: false,
        saved_command_wrapper: false,
        broader_effects_possible: true,
        compose_dependencies: None,
        repository: None,
        build: None,
        procedure: Some(procedures::effects(client, p)),
    }
}
pub async fn run(client: &CoreClient, input: RunInput) -> Result<SubmissionReceipt, AppError> {
    operations::require_operate(client, "RunProcedure")?;
    let p = inspect(client, input.target).await?;
    if input.expected_effects != procedures::effects(client, &p).acknowledgement {
        return Err(AppError::arguments(
            "RunProcedure",
            "First inspect get_procedure.effects; acknowledge built-in Core administrator execution authority and possible broader effects. No execution was submitted.",
        ));
    }
    operations::submit(
        client,
        resource(client, &p),
        RequestedScope::WholeProcedure {},
        WorkloadOperation::Run,
        Operation::RunProcedure,
        effects(client, &p),
        RunProcedure { procedure: p.id },
    )
    .await
}
pub async fn cancel(
    client: &CoreClient,
    input: CancelInput,
) -> Result<CancellationResult, AppError> {
    operations::require_operate(client, "CancelProcedure")?;
    let original = crate::history::UpdateInput {
        id: input.expected_update_id,
    }
    .validate("CancelProcedure")?;
    let p = inspect(client, input.target).await?;
    let context = crate::cancellation::Context {
        resource: resource(client, &p),
        original_update_id: original,
        scope: RequestedScope::WholeProcedure {},
        run_operation: Operation::RunProcedure,
        cancel_operation: Operation::CancelProcedure,
        effects: effects(client, &p),
        limit: procedures::CANCEL_LIMIT,
    };
    let bounded = client.bounded_evidence();
    let id = p.id.clone();
    crate::cancellation::resource(
        client,
        context,
        CancelProcedure {
            procedure: p.id,
            update_id: None,
        },
        || async {
            bounded
                .read(GetProcedureActionState {
                    procedure: id.clone(),
                })
                .await
                .map(|s| s.running)
        },
    )
    .await
}
pub(crate) async fn guard_execute(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if matches!(op, "RunProcedure" | "CancelProcedure") {
        let id = p["procedure"]
            .as_str()
            .filter(|s| crate::deletion::object_id(s))
            .ok_or_else(|| AppError::arguments(op, "Select a stable Procedure ID"))?;
        inspect(
            client,
            DetailInput {
                id: Some(id.into()),
                name: None,
            },
        )
        .await?;
        if op == "CancelProcedure" && !p["update_id"].is_null() {
            return Err(AppError::arguments(
                op,
                "Core ignores update_id for Procedure cancellation; validate the original locally and send only the resource guard.",
            ));
        }
    }
    Ok(())
}
