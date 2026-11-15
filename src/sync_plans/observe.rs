//! Independent, bounded resource reads after a terminal parent update.
use super::*;
#[derive(Serialize, JsonSchema)]
pub struct ResourceObservation {
    pub resource_type: Kind,
    pub id: Option<String>,
    pub name: String,
    pub expected_change: Option<ChangeKind>,
    pub deployment_requested: bool,
    pub visible_after: Option<bool>,
    pub definition_changed: Option<bool>,
    pub error: Option<AppError>,
    pub desired_state_verified: bool,
    pub workload_health_verified: bool,
    pub evidence_limit: &'static str,
}
// No Debug/Serialize: full before definitions can include secrets.
pub(super) struct Before {
    kind: Kind,
    id: Option<String>,
    name: String,
    change: Option<ChangeKind>,
    deployment_requested: bool,
    baseline: Option<Value>,
    error: Option<AppError>,
    truncated: bool,
}
fn selector(id: &Option<String>, name: &str) -> DetailInput {
    match id {
        Some(id) => DetailInput {
            id: Some(id.clone()),
            name: None,
        },
        None => DetailInput {
            id: None,
            name: Some(name.into()),
        },
    }
}
pub(super) async fn before(client: &CoreClient, plan: &PendingOutput) -> Vec<Before> {
    let mut observations = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut targets = plan
        .resource_changes
        .items
        .iter()
        .filter_map(|r| {
            Some((
                r.resource_type?,
                r.id.clone().filter(|id| valid_id(id)),
                r.name.clone()?,
                Some(r.action),
                false,
            ))
        })
        .collect::<Vec<_>>();
    for deploy in &plan.pending_deployments.items {
        if let Some(t) = targets
            .iter_mut()
            .find(|t| t.0 == deploy.resource_type && t.2 == deploy.name)
        {
            t.4 = true;
        } else {
            targets.push((deploy.resource_type, None, deploy.name.clone(), None, true));
        }
    }
    let total = targets.len();
    for (kind, id, name, change, deployment_requested) in targets.into_iter().take(8) {
        let result = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + Duration::from_secs(1)),
            super::audit::read_resource(client, kind, selector(&id, &name)),
        )
        .await;
        let (baseline, error) = match result {
            Ok(Ok(v)) => (Some(v), None),
            Ok(Err(e)) => (None, Some(e)),
            Err(_) => (
                None,
                Some(AppError::new(
                    ErrorKind::Timeout,
                    "ObserveResourceSync",
                    None,
                )),
            ),
        };
        observations.push(Before {
            kind,
            id,
            name,
            change,
            deployment_requested,
            baseline,
            error,
            truncated: total > 8
                || plan.resource_changes.truncated
                || plan.pending_deployments.truncated,
        });
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    if observations.len() < total {
        for b in &mut observations {
            b.truncated = true;
        }
    }
    observations
}
pub(super) async fn after(
    client: &CoreClient,
    before: Vec<Before>,
) -> (Vec<ResourceObservation>, bool) {
    let mut observations = Vec::new();
    let mut incomplete = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    for b in before {
        incomplete |= b.truncated;
        let result = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + Duration::from_secs(1)),
            super::audit::read_resource(client, b.kind, selector(&b.id, &b.name)),
        )
        .await;
        let (visible, changed, error) = match result {
            Ok(Ok(value)) => (Some(true), b.baseline.as_ref().map(|v| v != &value), None),
            Ok(Err(e)) => {
                incomplete = true;
                (None, None, Some(e))
            }
            Err(_) => {
                incomplete = true;
                (
                    None,
                    None,
                    Some(AppError::new(
                        ErrorKind::Timeout,
                        "ObserveResourceSync",
                        None,
                    )),
                )
            }
        };
        incomplete |= b.error.is_some();
        observations.push(ResourceObservation{resource_type:b.kind,id:b.id,name:b.name,expected_change:b.change,deployment_requested:b.deployment_requested,visible_after:visible,definition_changed:changed,error:error.or(b.error),desired_state_verified:false,workload_health_verified:false,evidence_limit:"Independent visible definition read, not proof the sync caused this change or that complete desired state matches. Hidden/missing/unreadable never proves deletion. No raw configuration is returned. Verify workload health separately."});
    }
    (observations, incomplete)
}
