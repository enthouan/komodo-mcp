//! One saved Action run; explicit update-targeted cancellation with ownership checks.
use crate::{
    actions,
    client::CoreClient,
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
    operations::{
        self, CancellationResult, ConfiguredEffects, OperationResource, RequestedScope,
        SubmissionReceipt, WorkloadKind, WorkloadOperation,
    },
};
use komodo_client::{
    api::execute::{CancelAction, RunAction},
    entities::{Operation, action::Action},
};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub target: DetailInput,
    #[serde(default)]
    pub args: Option<BTreeMap<String, Value>>,
    pub expected_effects: actions::Acknowledgement,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    pub target: DetailInput,
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub expected_update_id: String,
}
async fn inspect(client: &CoreClient, target: DetailInput) -> Result<Action, AppError> {
    let (_, p) = inventory::resolve_action(&client.bounded_evidence(), target).await?;
    if p.template {
        Err(AppError::new(
            ErrorKind::UnsupportedTarget,
            "action_operation",
            None,
        ))
    } else {
        Ok(p)
    }
}
fn resource(client: &CoreClient, p: &Action) -> OperationResource {
    OperationResource {
        kind: WorkloadKind::Action,
        id: p.id.clone(),
        name: crate::configuration::bounded_text(client, &p.name, 256),
    }
}
fn effects(p: &Action) -> ConfiguredEffects {
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
        procedure: None,
        action: Some(actions::effects(p)),
    }
}
pub async fn run(client: &CoreClient, input: RunInput) -> Result<SubmissionReceipt, AppError> {
    operations::require_operate(client, "RunAction")?;
    if let Some(args) = &input.args {
        validate_args(args)?;
    }
    let p = inspect(client, input.target).await?;
    if input.expected_effects != actions::effects(&p).acknowledgement {
        return Err(AppError::arguments(
            "RunAction",
            "First inspect get_action.effects; acknowledge built-in Core administrator execution authority and possible broader effects. No execution was submitted.",
        ));
    }
    operations::submit(
        client,
        resource(client, &p),
        RequestedScope::WholeAction {},
        WorkloadOperation::Run,
        Operation::RunAction,
        effects(&p),
        RunAction {
            action: p.id,
            args: input.args.map(|a| a.into_iter().collect()),
        },
    )
    .await
}
pub async fn cancel(
    client: &CoreClient,
    input: CancelInput,
) -> Result<CancellationResult, AppError> {
    operations::require_operate(client, "CancelAction")?;
    let original = crate::history::UpdateInput {
        id: input.expected_update_id,
    }
    .validate("CancelAction")?;
    let p = inspect(client, input.target).await?;
    let context = crate::cancellation::Context {
        resource: resource(client, &p),
        original_update_id: original.clone(),
        scope: RequestedScope::WholeAction {},
        run_operation: Operation::RunAction,
        cancel_operation: Operation::CancelAction,
        effects: effects(&p),
        limit: actions::CANCEL_LIMIT,
    };
    crate::cancellation::update(
        client,
        context,
        CancelAction {
            action: p.id,
            update_id: Some(original),
        },
    )
    .await
}
fn validate_args(args: &BTreeMap<String, Value>) -> Result<(), AppError> {
    crate::procedures::validate_args(args).map_err(|_|AppError::arguments("RunAction","Use a JSON object of at most 8192 bytes, depth 8 and 64 entries per collection with bounded keys. Core merges these overrides with saved defaults; no source or defaults are persisted."))
}

pub(crate) async fn guard_execute(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if matches!(op, "RunAction" | "CancelAction") {
        let id = p["action"]
            .as_str()
            .filter(|s| crate::deletion::object_id(s))
            .ok_or_else(|| AppError::arguments(op, "Select a stable Action ID"))?;
        inspect(
            client,
            DetailInput {
                id: Some(id.into()),
                name: None,
            },
        )
        .await?;
        if op == "CancelAction" {
            let uid=p["update_id"].as_str().filter(|s|crate::deletion::object_id(s)).ok_or_else(||AppError::arguments(op,"An explicit RunAction update ID is required; no resource-only cancellation fallback is allowed."))?;
            let u = crate::history::get_update(
                &client.bounded_evidence(),
                crate::history::UpdateInput { id: uid.into() },
            )
            .await?;
            if u.summary.target.kind != crate::history::ResourceKind::Action
                || u.summary.target.id != id
                || u.summary.operation != "RunAction"
            {
                return Err(AppError::arguments(
                    op,
                    "The cancellation update ID must belong to the exact selected Action and RunAction operation.",
                ));
            }
            if u.summary.status == crate::history::UpdateStatus::Complete {
                return Err(AppError::arguments(
                    op,
                    "The original RunAction is already complete; no cancellation was submitted.",
                ));
            }
        } else if !p["args"].is_null() {
            let args = serde_json::from_value(p["args"].clone())
                .map_err(|_| AppError::arguments(op, "Use bounded JSON argument objects"))?;
            validate_args(&args)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities::CapabilityPolicy, client::RequestPolicy, config::Config};
    use axum::{Json, Router, extract::State, routing::post};
    use komodo_client::entities::{
        ResourceTarget,
        update::{Update, UpdateStatus},
    };
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    const ACTION: &str = "000000000000000000000040";
    const RUN: &str = "000000000000000000000100";
    async fn mock(
        State((mode, count)): State<(usize, Arc<AtomicUsize>)>,
        Json(v): Json<Value>,
    ) -> Json<Value> {
        let update = |op, status| Update {
            id: RUN.into(),
            operation: op,
            status,
            target: ResourceTarget::Action(if mode == 0 {
                "000000000000000000000099".into()
            } else {
                ACTION.into()
            }),
            ..Default::default()
        };
        Json(match v["type"].as_str().unwrap() {
            "ListActions" => {
                json!([{"id":ACTION,"name":"owned","template":false,"type":"Action","tags":[],"info":{"state":"Running","last_run_at":null,"next_scheduled_run":null,"schedule_error":null}}])
            }
            "GetAction" => serde_json::to_value(Action {
                id: ACTION.into(),
                name: "owned".into(),
                ..Default::default()
            })
            .unwrap(),
            "GetUpdate" => serde_json::to_value(update(
                if mode == 1 {
                    Operation::CancelAction
                } else {
                    Operation::RunAction
                },
                if mode == 2 {
                    UpdateStatus::Complete
                } else {
                    UpdateStatus::InProgress
                },
            ))
            .unwrap(),
            "CancelAction" => {
                count.fetch_add(1, Ordering::SeqCst);
                serde_json::to_value(update(Operation::CancelAction, UpdateStatus::Queued)).unwrap()
            }
            other => panic!("unexpected request {other}"),
        })
    }
    #[tokio::test]
    async fn shared_transport_checks_action_run_ownership_and_requires_explicit_id_before_post() {
        for mode in 0..5 {
            let count = Arc::new(AtomicUsize::new(0));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            let app = Router::new()
                .route("/read", post(mock))
                .route("/execute", post(mock))
                .with_state((mode, count.clone()));
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let client = CoreClient::new(
                Config::parse(&address, "fixture-key", "fixture-secret")
                    .unwrap()
                    .with_capabilities(CapabilityPolicy::new(true, false)),
                RequestPolicy::default(),
            )
            .unwrap();
            let r = client
                .execute(CancelAction {
                    action: ACTION.into(),
                    update_id: (mode != 3).then(|| RUN.into()),
                })
                .await;
            if mode < 4 {
                assert!(matches!(r,Err(e)if e.kind==ErrorKind::InvalidArguments));
                assert_eq!(count.load(Ordering::SeqCst), 0);
            } else {
                assert!(r.unwrap().is_ok());
                assert_eq!(count.load(Ordering::SeqCst), 1);
            }
            task.abort();
            let _ = task.await;
        }
    }
}
