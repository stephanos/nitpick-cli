use std::sync::{Arc, Mutex};

use nitpick_agent_core::{ActivityKind, ActivityStatus, ActivityStore, AgentError, AgentResult};

use crate::ReviewSourcePollResult;

#[derive(Clone)]
pub(crate) struct PollingState {
    last_poll_unix: Arc<Mutex<Option<u64>>>,
    last_poll_summary: Arc<Mutex<Option<String>>>,
}

impl PollingState {
    pub(crate) fn new() -> Self {
        Self {
            last_poll_unix: Arc::new(Mutex::new(None)),
            last_poll_summary: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn last_poll_unix(&self) -> AgentResult<Option<u64>> {
        Ok(*self
            .last_poll_unix
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))?)
    }

    pub(crate) fn last_poll_summary(&self) -> AgentResult<Option<String>> {
        Ok(self
            .last_poll_summary
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))?
            .clone())
    }

    pub(crate) fn record_result(
        &self,
        now: u64,
        result: &ReviewSourcePollResult,
    ) -> AgentResult<()> {
        *self
            .last_poll_unix
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))? = Some(now);
        *self
            .last_poll_summary
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))? =
            Some(result.summary());
        Ok(())
    }

    pub(crate) fn record_error(
        &self,
        store: &Arc<dyn ActivityStore>,
        now: u64,
        error: &str,
    ) -> AgentResult<()> {
        *self
            .last_poll_unix
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))? = Some(now);
        *self
            .last_poll_summary
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))? =
            Some(review_source_error_summary(error));
        let mut activity = store.create(ActivityKind::Discovery)?;
        activity.status = ActivityStatus::Error;
        activity.label = Some("discovery poll".into());
        activity.error = Some(error.into());
        activity.touch();
        store.save(&activity)?;
        Ok(())
    }

    pub(crate) fn update_last_poll(&self, now: u64) -> AgentResult<()> {
        *self
            .last_poll_unix
            .lock()
            .map_err(|_| AgentError::io("polling state lock", "poisoned"))? = Some(now);
        Ok(())
    }
}

fn review_source_error_summary(error: &str) -> String {
    if error.contains("failed to start GitHub CLI") {
        return format!("github unavailable: {error}");
    }
    format!("review source failed: {error}")
}
