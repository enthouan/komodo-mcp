//! Read-only, stateless observation of one known Core update.

use std::{future::Future, time::Duration};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep, sleep_until};

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    history::{self, UpdateDetail, UpdateInput, UpdateStatus},
    inventory,
};

pub const MAX_POLLS: usize = 10;
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
pub const MAX_OUTPUT_BYTES: usize = 65_536;

fn default_timeout() -> u8 {
    10
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitInput {
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
    /// Total local observation budget, including Core requests and sleeps.
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 30))]
    pub timeout_seconds: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitEnd {
    /// Core reported Complete. Use latest_update.outcome for success or failure.
    Complete,
    Deadline,
    PollLimit,
    Cancelled,
    ObservationFailed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitResult {
    pub update_id: String,
    pub end: WaitEnd,
    pub poll_count: usize,
    pub elapsed_ms: u64,
    /// Preserved across a later timeout, cancellation or read failure.
    pub latest_update: Option<UpdateDetail>,
    /// A gap in local observation, not evidence of remote execution failure.
    pub observation_error: Option<AppError>,
    pub remote_cancellation_requested: bool,
}

impl WaitResult {
    fn finish(mut self, client: &CoreClient, end: WaitEnd, started: Instant) -> Self {
        self.end = end;
        self.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // Match submission receipt bounds after sanitization and JSON escaping.
        // History prioritizes failed stages, so trim trailing evidence first.
        while serde_json::to_vec(&inventory::projected_value(client, &self))
            .expect("serializable wait result")
            .len()
            > MAX_OUTPUT_BYTES
        {
            let update = self
                .latest_update
                .as_mut()
                .expect("only stages can exceed budget");
            assert!(
                update.stages.pop().is_some(),
                "fixed wait fields fit budget"
            );
            update.stages_truncated = true;
        }
        self
    }
}

pub async fn wait_for_update(
    client: &CoreClient,
    input: WaitInput,
    cancelled: impl Future<Output = ()>,
) -> Result<WaitResult, AppError> {
    wait_checked(client, input, None, cancelled).await
}

pub(crate) async fn wait_checked(
    client: &CoreClient,
    input: WaitInput,
    expected: Option<(history::HistoryTarget, &'static str)>,
    cancelled: impl Future<Output = ()>,
) -> Result<WaitResult, AppError> {
    let id = UpdateInput { id: input.id }.validate("WaitForUpdate")?;
    if !(1..=30).contains(&input.timeout_seconds) {
        return Err(AppError::arguments(
            "WaitForUpdate",
            "Use timeout_seconds from 1 through 30",
        ));
    }
    let started = Instant::now();
    let deadline = started + Duration::from_secs(u64::from(input.timeout_seconds));
    let mut result = WaitResult {
        update_id: id.clone(),
        end: WaitEnd::Deadline,
        poll_count: 0,
        elapsed_ms: 0,
        latest_update: None,
        observation_error: None,
        remote_cancellation_requested: false,
    };
    tokio::pin!(cancelled);
    loop {
        // Increment only when the request future is actually polled. A ready
        // cancellation/deadline makes no request, even on the initial iteration.
        let read = async {
            result.poll_count += 1;
            history::get_update(client, UpdateInput { id: id.clone() }).await
        };
        let observation = tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(result.finish(client, WaitEnd::Cancelled, started)),
            _ = sleep_until(deadline) => return Ok(result.finish(client, WaitEnd::Deadline, started)),
            observation = read => observation,
        };
        match observation {
            Ok(update) => {
                if expected.as_ref().is_some_and(|(target, operation)| {
                    &update.summary.target != target || update.summary.operation != *operation
                }) {
                    result.observation_error =
                        Some(AppError::new(ErrorKind::InvalidResponse, "GetUpdate", None));
                    return Ok(result.finish(client, WaitEnd::ObservationFailed, started));
                }
                let complete = update.summary.status == UpdateStatus::Complete;
                result.latest_update = Some(update);
                if complete {
                    return Ok(result.finish(client, WaitEnd::Complete, started));
                }
            }
            Err(error) => {
                result.observation_error = Some(error);
                return Ok(result.finish(client, WaitEnd::ObservationFailed, started));
            }
        }
        if result.poll_count == MAX_POLLS {
            return Ok(result.finish(client, WaitEnd::PollLimit, started));
        }
        tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(result.finish(client, WaitEnd::Cancelled, started)),
            _ = sleep_until(deadline) => return Ok(result.finish(client, WaitEnd::Deadline, started)),
            _ = sleep(POLL_INTERVAL) => (),
        }
    }
}
