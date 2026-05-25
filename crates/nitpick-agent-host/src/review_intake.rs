use std::{collections::HashSet, sync::Arc};

use nitpick_agent_core::{
    ActivityStore, AgentResult, Clock, ProcessedReviewStore, ReviewInput, ReviewRequest,
    ReviewSource,
};

use crate::{AgentConfig, ReviewSourcePollResult, polling_state::PollingState};

pub(crate) struct ReviewRequestIntake {
    config: AgentConfig,
    store: Arc<dyn ActivityStore>,
    processed_reviews: Arc<dyn ProcessedReviewStore>,
    review_source: Arc<dyn ReviewSource>,
    clock: Arc<dyn Clock>,
    polling_state: PollingState,
}

impl ReviewRequestIntake {
    pub(crate) fn new(
        config: AgentConfig,
        store: Arc<dyn ActivityStore>,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
        review_source: Arc<dyn ReviewSource>,
        clock: Arc<dyn Clock>,
        polling_state: PollingState,
    ) -> Self {
        Self {
            config,
            store,
            processed_reviews,
            review_source,
            clock,
            polling_state,
        }
    }

    pub(crate) fn discover_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.review_source.requested_reviews()
    }

    pub(crate) fn discover_new_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        let discovered_requests = self.discover_allowed_review_requests()?;
        self.filter_new_review_requests(discovered_requests)
    }

    pub(crate) fn poll(
        &self,
        record_detected: impl FnMut(&ReviewRequest) -> AgentResult<String>,
        enqueue_review: impl FnMut(ReviewInput) -> AgentResult<Option<String>>,
    ) -> AgentResult<ReviewSourcePollResult> {
        match self.run_poll(record_detected, enqueue_review) {
            Ok(result) => Ok(result),
            Err(error) => {
                let now = self.clock.now_unix();
                let message = error.message();
                tracing::warn!(error = %message, "review source poll failed");
                self.polling_state
                    .record_error(&self.store, now, &message)?;
                Err(error)
            }
        }
    }

    fn run_poll(
        &self,
        mut record_detected: impl FnMut(&ReviewRequest) -> AgentResult<String>,
        mut enqueue_review: impl FnMut(ReviewInput) -> AgentResult<Option<String>>,
    ) -> AgentResult<ReviewSourcePollResult> {
        if !self.config.github_discovery.enabled {
            tracing::debug!("review source poll skipped because discovery is disabled");
            return Ok(ReviewSourcePollResult::skipped("disabled"));
        }

        let now = self.clock.now_unix();
        {
            if let Some(last_poll) = self.polling_state.last_poll_unix()?
                && now.saturating_sub(last_poll) < self.config.github_discovery.interval_seconds
            {
                tracing::debug!("review source poll skipped because interval has not elapsed");
                return Ok(ReviewSourcePollResult::skipped("interval"));
            }
            self.polling_state.update_last_poll(now)?;
        }

        let discovered_requests = self.discover_allowed_review_requests()?;
        let discovered_count = discovered_requests.len();
        let new_requests =
            deduplicate_review_requests(self.filter_new_review_requests(discovered_requests)?);
        for request in &new_requests {
            let activity_id = record_detected(request)?;
            if !self.config.github_discovery.auto_review {
                self.processed_reviews
                    .mark_processed_at(request, Some(activity_id), now)?;
            }
        }
        if !self.config.github_discovery.auto_review {
            return Ok(ReviewSourcePollResult {
                discovered_count,
                enqueued_count: 0,
                cleanup_removed_count: 0,
                cleanup_error: None,
                skipped_reason: None,
            });
        }

        let mut enqueued_count = 0;
        for request in new_requests {
            let input = self.review_source.review_input(&request)?;
            if let Some(activity_id) = enqueue_review(input)? {
                self.processed_reviews
                    .mark_processed_at(&request, Some(activity_id), now)?;
                enqueued_count += 1;
            }
        }

        let result = ReviewSourcePollResult {
            discovered_count,
            enqueued_count,
            cleanup_removed_count: 0,
            cleanup_error: None,
            skipped_reason: None,
        };
        tracing::info!(
            discovered_count = result.discovered_count,
            enqueued_count = result.enqueued_count,
            "review source poll completed"
        );
        Ok(result)
    }

    fn discover_allowed_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.discover_review_requests().map(|requests| {
            requests
                .into_iter()
                .filter(|request| {
                    self.config
                        .github_discovery
                        .allows_repository(&request.repository)
                })
                .collect()
        })
    }

    fn filter_new_review_requests(
        &self,
        requests: Vec<ReviewRequest>,
    ) -> AgentResult<Vec<ReviewRequest>> {
        requests
            .into_iter()
            .filter_map(
                |request| match self.processed_reviews.needs_review(&request) {
                    Ok(true) => Some(Ok(request)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .filter_map(|request| match request {
                Ok(request) => match self.review_source.already_reviewed(&request) {
                    Ok(true) => None,
                    Ok(false) => Some(Ok(request)),
                    Err(error) => Some(Err(error)),
                },
                Err(error) => Some(Err(error)),
            })
            .collect()
    }
}

fn deduplicate_review_requests(requests: Vec<ReviewRequest>) -> Vec<ReviewRequest> {
    let mut seen = HashSet::new();
    requests
        .into_iter()
        .filter(|request| seen.insert(review_request_version_key(request)))
        .collect()
}

fn review_request_version_key(request: &ReviewRequest) -> (String, Option<u64>, String, String) {
    (
        request.repository.clone(),
        request.number,
        request.id.clone(),
        request.head_sha.clone(),
    )
}
