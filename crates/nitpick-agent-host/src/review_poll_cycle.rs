use std::sync::Arc;

use nitpick_agent_core::{AgentResult, CleanupCheckoutsResult, Clock, ReviewInput, ReviewRequest};

use crate::{
    ReviewSourcePollResult, polling_state::PollingState, review_intake::ReviewRequestIntake,
};

pub(crate) struct ReviewPollCycle {
    intake: ReviewRequestIntake,
    clock: Arc<dyn Clock>,
    polling_state: PollingState,
    automatic_checkout_cleanup: bool,
}

impl ReviewPollCycle {
    pub(crate) fn new(
        intake: ReviewRequestIntake,
        clock: Arc<dyn Clock>,
        polling_state: PollingState,
        automatic_checkout_cleanup: bool,
    ) -> Self {
        Self {
            intake,
            clock,
            polling_state,
            automatic_checkout_cleanup,
        }
    }

    pub(crate) fn run(
        &self,
        record_detected: impl FnMut(&ReviewRequest) -> AgentResult<String>,
        enqueue_review: impl FnMut(ReviewInput) -> AgentResult<Option<String>>,
        cleanup_checkouts: impl FnOnce() -> AgentResult<CleanupCheckoutsResult>,
    ) -> AgentResult<ReviewSourcePollResult> {
        let mut result = self.intake.poll(record_detected, enqueue_review)?;
        if result.skipped_reason.is_none() && self.automatic_checkout_cleanup {
            match cleanup_checkouts() {
                Ok(cleanup) => {
                    result.cleanup_removed_count = cleanup.removed_count;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "automatic checkout cleanup failed");
                    result.cleanup_error = Some(error.to_string());
                }
            }
        }
        if result.skipped_reason.is_none() {
            let now = self.clock.now_unix();
            self.polling_state.record_result(now, &result)?;
        }
        Ok(result)
    }
}
