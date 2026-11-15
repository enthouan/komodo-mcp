//! Stateless read-only resumption of an explicitly retained sync update ID.
use super::*;
use crate::{
    history::{HistoryTarget, ResourceKind},
    wait::{self, WaitInput, WaitResult},
};
fn default_timeout() -> u8 {
    10
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowInput {
    pub binding: ApplyBinding,
    pub update_id: String,
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 30))]
    pub timeout_seconds: u8,
}
#[derive(Serialize, JsonSchema)]
pub struct FollowResult {
    pub binding: ApplyBinding,
    pub wait: WaitResult,
    pub sync_and_operation_verified: bool,
    pub selection_verified_by_core_history: bool,
    pub caller_attribution: &'static str,
    pub automatic_resubmission: bool,
    pub evidence_limit: &'static str,
}
pub async fn follow(
    client: &CoreClient,
    mut input: FollowInput,
    cancelled: impl std::future::Future<Output = ()>,
) -> Result<FollowResult, AppError> {
    if !valid_id(&input.binding.sync_id) {
        return Err(AppError::arguments(
            "WaitForResourceSync",
            "Retain the original stable sync ID and explicitly known update ID",
        ));
    }
    super::execution::validate_scope(&input.binding.selection)?;
    input.binding.sync_id.make_ascii_lowercase();
    let wait = wait::wait_checked(
        client,
        WaitInput {
            id: input.update_id,
            timeout_seconds: input.timeout_seconds,
        },
        Some((
            HistoryTarget {
                kind: ResourceKind::ResourceSync,
                id: input.binding.sync_id.clone(),
            },
            "RunSync",
        )),
        cancelled,
    )
    .await?;
    Ok(FollowResult {
        sync_and_operation_verified: wait.latest_update.is_some(),
        binding: input.binding,
        wait,
        selection_verified_by_core_history: false,
        caller_attribution: "not_verified_by_history",
        automatic_resubmission: false,
        evidence_limit: "This reads one explicitly supplied update ID and checks its sync ID and RunSync operation on every poll, retaining only matching observations. Core history does not record reliable RunSync selectors or invocation ownership; the retained binding is an expectation, not inferred scope proof. Resume the same acknowledged ID after restart; unknown-ID candidates remain unconfirmed. No refresh, execution, remote cancellation or replay occurs. Parent completion does not verify child deployment success or workload health.",
    })
}
