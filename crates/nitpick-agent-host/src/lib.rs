mod api;
mod polling_state;
mod review_slots;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use fs_err as fs;
use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ActivityStatus, ActivityStore, AgentError, AgentProvider,
    AgentProviderKind, AgentResult, AgentRuntime, Artifact, ArtifactContent, ArtifactId,
    ArtifactSyncDestination, ArtifactSyncState, ChatInput, CleanupCheckoutsResult, Clock,
    CommandAgentProvider, CommandSandboxConfig, HostStatus, MemoryProcessedReviewStore,
    ProcessedReviewStore, ReviewInput, ReviewRequest, ReviewSource, SessionStatus, SystemClock,
};
use nitpick_agent_github::{
    DiscoveredPullRequest, GitHubCliDiscovery, GitHubCliReviewSyncDestination,
    GitHubCliSyncDestination, GitHubDryRunSyncDestination, PullRequestRef,
};
use polling_state::PollingState;
use review_slots::ReviewSlotManager;
use serde::Deserialize;

pub use api::api_router;

const ACTIVITY_PRUNE_AGE_SECS: u64 = 24 * 60 * 60;

#[derive(Clone)]
pub struct HostDaemon {
    config: AgentConfig,
    store: Arc<dyn ActivityStore>,
    processed_reviews: Arc<dyn ProcessedReviewStore>,
    provider: Arc<dyn AgentProvider>,
    review_source: Arc<dyn ReviewSource>,
    clock: Arc<dyn Clock>,
    automatic_checkout_cleanup: bool,
    polling_state: PollingState,
    review_slots: ReviewSlotManager,
}

impl HostDaemon {
    pub fn new(store: Arc<dyn ActivityStore>) -> Self {
        Self::with_config(store, AgentConfig::default())
    }

    pub fn with_config(store: Arc<dyn ActivityStore>, config: AgentConfig) -> Self {
        let provider = config.provider();
        let review_source = config.review_source();
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            polling_state: PollingState::new(),
            review_slots: ReviewSlotManager::new(max_concurrent),
        }
    }

    pub fn with_config_and_processed_reviews(
        store: Arc<dyn ActivityStore>,
        config: AgentConfig,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
    ) -> Self {
        let provider = config.provider();
        let review_source = config.review_source();
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store,
            processed_reviews,
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            polling_state: PollingState::new(),
            review_slots: ReviewSlotManager::new(max_concurrent),
        }
    }

    #[cfg(test)]
    pub fn with_clock(store: Arc<dyn ActivityStore>, clock: Arc<dyn Clock>) -> Self {
        let config = AgentConfig::default();
        let provider = config.provider();
        let review_source = config.review_source();
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock,
            automatic_checkout_cleanup: false,
            polling_state: PollingState::new(),
            review_slots: ReviewSlotManager::new(max_concurrent),
        }
    }

    pub fn with_provider(store: Arc<dyn ActivityStore>, provider: Arc<dyn AgentProvider>) -> Self {
        let config = AgentConfig::default();
        let review_source = config.review_source();
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            polling_state: PollingState::new(),
            review_slots: ReviewSlotManager::new(max_concurrent),
        }
    }

    pub fn with_dependencies(
        store: Arc<dyn ActivityStore>,
        config: AgentConfig,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
        provider: Arc<dyn AgentProvider>,
        review_source: Arc<dyn ReviewSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store,
            processed_reviews,
            provider,
            review_source,
            clock,
            automatic_checkout_cleanup: false,
            polling_state: PollingState::new(),
            review_slots: ReviewSlotManager::new(max_concurrent),
        }
    }

    pub fn status(&self) -> AgentResult<HostStatus> {
        let artifacts = self.store.list_artifacts()?;
        let activities = self.store.list()?;
        let reviews: Vec<_> = activities
            .iter()
            .filter(|activity| activity.kind == ActivityKind::Review)
            .collect();
        Ok(HostStatus {
            activity_count: activities.len(),
            queued_activity_count: activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Queued)
                .count(),
            running_activity_count: activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Running)
                .count(),
            completed_activity_count: activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Completed)
                .count(),
            error_activity_count: activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Error)
                .count(),
            open_review_count: self.polling_state.open_review_count()?,
            queued_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Queued)
                .count(),
            running_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Running)
                .count(),
            completed_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Completed)
                .count(),
            error_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Error)
                .count(),
            artifact_count: artifacts.len(),
            local_only_artifact_count: artifacts
                .iter()
                .filter(|artifact| artifact.sync_state == ArtifactSyncState::LocalOnly)
                .count(),
            pending_sync_artifact_count: artifacts
                .iter()
                .filter(|artifact| matches!(artifact.sync_state, ArtifactSyncState::Pending { .. }))
                .count(),
            provider: self.config.provider.clone(),
            model: self.config.model.clone(),
            review_source_name: self.config.review_source_name(),
            review_source_enabled: self.config.github_discovery.enabled,
            review_source_last_poll_unix: self.polling_state.last_poll_unix()?,
            review_source_last_poll_summary: self.polling_state.last_poll_summary()?,
        })
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub fn recover_interrupted_activities(&self) -> AgentResult<usize> {
        let message = "host restarted before activity completed";
        let mut recovered_count = 0;

        for mut activity in self.store.list()? {
            if activity.status != ActivityStatus::Running {
                continue;
            }

            activity.status = ActivityStatus::Error;
            activity.session.status = SessionStatus::Error(message.into());
            activity.error = Some(message.into());
            activity.touch();
            self.store.save(&activity)?;
            recovered_count += 1;
        }

        Ok(recovered_count)
    }

    pub fn record_checkout_cleanup_activity(
        &self,
        pull_request: &PullRequestRef,
    ) -> AgentResult<Activity> {
        let mut activity = self.store.create(ActivityKind::Maintenance)?;
        activity.status = ActivityStatus::Completed;
        activity.label = Some(format!(
            "{}/{}#{} cleaned up",
            pull_request.owner, pull_request.repo, pull_request.number
        ));
        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn prune_old_activities(&self) -> AgentResult<usize> {
        let now = self.clock.now_unix();
        let cutoff = now.saturating_sub(ACTIVITY_PRUNE_AGE_SECS);
        let mut pruned = 0;
        for activity in self.store.list()? {
            if matches!(
                activity.status,
                ActivityStatus::Completed | ActivityStatus::Error
            ) && activity.updated_at_unix < cutoff
            {
                self.store.delete(&activity.id)?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    pub fn record_review_request_detected_activity(
        &self,
        request: &ReviewRequest,
    ) -> AgentResult<Activity> {
        let mut activity = self.store.create(ActivityKind::Discovery)?;
        activity.status = ActivityStatus::Completed;
        activity.label = Some(format!("review request {}", request.display_reference()));
        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    #[tracing::instrument(skip_all)]
    pub fn cleanup_checkouts(&self) -> AgentResult<CleanupCheckoutsResult> {
        let github = self.config.github_discovery_client();
        let mut cleaned = Vec::new();

        for pull_request in github.list_checkouts()? {
            let repository = format!("{}/{}", pull_request.owner, pull_request.repo);
            if !self.config.github_discovery.allows_repository(&repository) {
                continue;
            }
            let details = github.pull_request_details(&pull_request)?;
            if !github.cleanup_checkout_for(&pull_request, &details)? {
                continue;
            }
            let reference = PullRequestRef {
                owner: pull_request.owner,
                repo: pull_request.repo,
                number: pull_request.number,
            };
            self.record_checkout_cleanup_activity(&reference)?;
            cleaned.push(format!(
                "{}/{}#{}",
                reference.owner, reference.repo, reference.number
            ));
        }

        tracing::info!(removed_count = cleaned.len(), "checkout cleanup completed");
        Ok(CleanupCheckoutsResult {
            removed_count: cleaned.len(),
            cleaned,
        })
    }

    pub fn list_activities(&self) -> AgentResult<Vec<Activity>> {
        self.store.list()
    }

    pub fn get_activity(&self, id: &ActivityId) -> AgentResult<Option<Activity>> {
        match self.store.get(id) {
            Ok(activity) => Ok(Some(activity)),
            Err(error) if error.message().starts_with("activity not found:") => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn list_artifacts_for(&self, id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        self.store.list_artifacts_for(id)
    }

    pub fn list_pending_sync_artifacts(
        &self,
        destination: Option<&str>,
    ) -> AgentResult<Vec<Artifact>> {
        Ok(self
            .store
            .list_artifacts()?
            .into_iter()
            .filter(|artifact| match &artifact.sync_state {
                ArtifactSyncState::Pending {
                    destination: artifact_destination,
                    ..
                } => destination.is_none_or(|destination| destination == artifact_destination),
                _ => false,
            })
            .collect())
    }

    pub fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Option<Artifact>> {
        match self.store.get_artifact(id) {
            Ok(artifact) => Ok(Some(artifact)),
            Err(error) if error.message().starts_with("artifact not found:") => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Option<Artifact>> {
        if self.get_artifact(id)?.is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.update_artifact_sync_state(id, sync_state)?))
    }

    pub fn sync_artifact(
        &self,
        id: &ArtifactId,
        destination: &str,
        target: Option<&str>,
    ) -> AgentResult<Option<Artifact>> {
        let Some(artifact) = self.get_artifact(id)? else {
            return Ok(None);
        };
        let sync_state = self
            .config
            .sync_destination(destination, target)?
            .sync(&artifact)?
            .sync_state;
        Ok(Some(self.store.update_artifact_sync_state(id, sync_state)?))
    }

    pub fn sync_activity_artifacts(
        &self,
        id: &ActivityId,
        destination: &str,
        target: Option<&str>,
    ) -> AgentResult<Option<Vec<Artifact>>> {
        if self.get_activity(id)?.is_none() {
            return Ok(None);
        }
        let artifacts = self.store.list_artifacts_for(id)?;
        if destination == "github-review"
            && let Some(updated) =
                self.reconcile_submitted_github_review_artifacts(&artifacts, target)?
        {
            return Ok(Some(updated));
        }
        let outcomes = self
            .config
            .sync_destination(destination, target)?
            .sync_batch(&artifacts)?;
        if outcomes.len() != artifacts.len() {
            return Err(AgentError::invalid_input(format!(
                "sync destination `{destination}` returned {} outcome(s) for {} artifact(s)",
                outcomes.len(),
                artifacts.len()
            )));
        }

        let mut updated = Vec::with_capacity(artifacts.len());
        for (artifact, outcome) in artifacts.into_iter().zip(outcomes) {
            updated.push(
                self.store
                    .update_artifact_sync_state(&artifact.id, outcome.sync_state)?,
            );
        }
        Ok(Some(updated))
    }

    fn reconcile_submitted_github_review_artifacts(
        &self,
        artifacts: &[Artifact],
        target: Option<&str>,
    ) -> AgentResult<Option<Vec<Artifact>>> {
        let pending_artifacts = artifacts
            .iter()
            .filter(|artifact| {
                matches!(
                    artifact.sync_state,
                    ArtifactSyncState::Pending {
                        ref destination,
                        remote_id: Some(_),
                        ..
                    } if destination == "github-review"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if pending_artifacts.is_empty() {
            return Ok(None);
        }
        let review_id = match &pending_artifacts[0].sync_state {
            ArtifactSyncState::Pending {
                remote_id: Some(review_id),
                ..
            } => review_id.clone(),
            _ => return Ok(None),
        };
        let target = target.ok_or_else(|| {
            AgentError::invalid_input("github-review sync requires a pull request target")
        })?;
        let target = target.parse::<PullRequestRef>().map_err(|error| {
            AgentError::invalid_input(format!("invalid GitHub sync target: {error}"))
        })?;
        let destination = GitHubCliReviewSyncDestination::new(
            target,
            self.config.github_command.as_deref().unwrap_or("gh"),
        );
        let review = match destination.fetch_review(&review_id) {
            Ok(review) => review,
            Err(_) => {
                let mut updated = Vec::with_capacity(artifacts.len());
                for artifact in artifacts {
                    let next_state = match &artifact.sync_state {
                        ArtifactSyncState::Pending {
                            destination,
                            remote_id: Some(current_review_id),
                            ..
                        } if destination == "github-review" && current_review_id == &review_id => {
                            ArtifactSyncState::LocalOnly
                        }
                        _ => artifact.sync_state.clone(),
                    };
                    updated.push(
                        self.store
                            .update_artifact_sync_state(&artifact.id, next_state)?,
                    );
                }
                return Ok(Some(updated));
            }
        };
        if review.state == "PENDING" {
            let has_new_inline_comments = artifacts.iter().any(|artifact| {
                artifact.sync_state == ArtifactSyncState::LocalOnly
                    && matches!(artifact.content, ArtifactContent::ReviewComment(_))
            });
            if has_new_inline_comments {
                return Err(AgentError::invalid_input(
                    "pending GitHub draft review already exists; submit or clear the draft review before staging new inline comments",
                ));
            }

            let remote_url = review.html_url.clone().or_else(|| {
                pending_artifacts
                    .iter()
                    .find_map(|artifact| match &artifact.sync_state {
                        ArtifactSyncState::Pending { remote_url, .. } => remote_url.clone(),
                        _ => None,
                    })
            });
            let local_summary = artifacts.iter().find_map(|artifact| {
                if artifact.sync_state != ArtifactSyncState::LocalOnly {
                    return None;
                }
                match &artifact.content {
                    ArtifactContent::ReviewSummary(summary) => Some(summary.clone()),
                    _ => None,
                }
            });
            if let Some(summary) = local_summary {
                destination.update_pending_review_body(&review_id, &summary)?;
            }
            let mut updated = Vec::with_capacity(artifacts.len());
            for artifact in artifacts {
                let next_state = if artifact.sync_state == ArtifactSyncState::LocalOnly {
                    match &artifact.content {
                        ArtifactContent::ReviewSummary(_) => ArtifactSyncState::Pending {
                            destination: "github-review".into(),
                            remote_id: Some(review_id.clone()),
                            remote_url: remote_url.clone(),
                        },
                        _ => artifact.sync_state.clone(),
                    }
                } else {
                    artifact.sync_state.clone()
                };
                updated.push(
                    self.store
                        .update_artifact_sync_state(&artifact.id, next_state)?,
                );
            }
            return Ok(Some(updated));
        }
        let remote_id = review.html_url.or_else(|| {
            pending_artifacts
                .iter()
                .find_map(|artifact| match &artifact.sync_state {
                    ArtifactSyncState::Pending { remote_url, .. } => remote_url.clone(),
                    _ => None,
                })
        });
        let mut updated = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let next_state = match &artifact.sync_state {
                ArtifactSyncState::Pending {
                    destination,
                    remote_id: Some(current_review_id),
                    ..
                } if destination == "github-review" && current_review_id == &review_id => {
                    ArtifactSyncState::Synced {
                        destination: "github-review".into(),
                        remote_id: remote_id.clone(),
                    }
                }
                _ => artifact.sync_state.clone(),
            };
            updated.push(
                self.store
                    .update_artifact_sync_state(&artifact.id, next_state)?,
            );
        }
        Ok(Some(updated))
    }

    pub fn discover_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.review_source.requested_reviews()
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

    #[deprecated(note = "use discover_review_requests")]
    pub fn discover_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discover_review_requests()?
            .into_iter()
            .map(github_pull_request_from_review_request)
            .collect()
    }

    pub fn discover_new_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.discover_allowed_review_requests()?
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

    #[deprecated(note = "use discover_new_review_requests")]
    pub fn discover_new_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discover_new_review_requests()?
            .into_iter()
            .map(github_pull_request_from_review_request)
            .collect()
    }

    #[tracing::instrument(skip_all)]
    pub fn poll_review_requests(&self) -> AgentResult<ReviewSourcePollResult> {
        let mut result = match self.run_review_source_poll() {
            Ok(result) => result,
            Err(error) => {
                let now = self.clock.now_unix();
                let message = error.message();
                tracing::warn!(error = %message, "review source poll failed");
                self.polling_state
                    .record_error(&self.store, now, &message)?;
                return Err(error);
            }
        };
        if result.skipped_reason.is_none() && self.automatic_checkout_cleanup {
            match self.cleanup_checkouts() {
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

    fn run_review_source_poll(&self) -> AgentResult<ReviewSourcePollResult> {
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
        let new_requests = discovered_requests
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
            .collect::<AgentResult<Vec<_>>>()?;
        for request in &new_requests {
            self.record_review_request_detected_activity(request)?;
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
            let activity = self.start_review(self.review_source.review_input(&request)?)?;
            if activity.status != ActivityStatus::Completed {
                continue;
            }
            self.processed_reviews.mark_processed_at(
                &request,
                Some(activity.id.to_string()),
                now,
            )?;
            enqueued_count += 1;
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

    #[deprecated(note = "use poll_review_requests")]
    pub fn poll_github_review_requests(&self) -> AgentResult<ReviewSourcePollResult> {
        self.poll_review_requests()
    }

    pub fn start_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let input = self.config.apply_review_prompt(input)?;
        self.runtime().start_review(input)
    }

    pub fn enqueue_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let input = self.config.apply_review_prompt(input)?;
        let runtime = self.runtime();
        let mut activity = runtime.create_queued_review_activity(&input)?;
        let slot_acquired = self.review_slots.try_acquire()?;
        if slot_acquired {
            activity = runtime.mark_activity_running(activity)?;
        }
        let queued = activity.clone();
        let daemon = self.clone();
        thread::spawn(move || {
            let _ = daemon.run_enqueued_review(activity, input, slot_acquired);
        });
        Ok(queued)
    }

    pub fn start_chat(&self, input: ChatInput) -> AgentResult<Activity> {
        self.runtime().start_chat(input)
    }

    pub fn enqueue_chat(&self, input: ChatInput) -> AgentResult<Activity> {
        let runtime = self.runtime();
        let activity = runtime.create_chat_activity()?;
        let queued = activity.clone();
        thread::spawn(move || {
            let _ = runtime.run_chat(activity, input);
        });
        Ok(queued)
    }

    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::new(self.provider.clone(), self.store.clone())
    }

    fn run_enqueued_review(
        &self,
        activity: Activity,
        input: ReviewInput,
        slot_acquired: bool,
    ) -> AgentResult<Activity> {
        if !slot_acquired {
            self.review_slots.wait_and_acquire()?;
        }
        let result = self.runtime().run_review(activity, input);
        self.review_slots.release()?;
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSourcePollResult {
    pub discovered_count: usize,
    pub enqueued_count: usize,
    pub cleanup_removed_count: usize,
    pub cleanup_error: Option<String>,
    pub skipped_reason: Option<String>,
}

impl ReviewSourcePollResult {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            discovered_count: 0,
            enqueued_count: 0,
            cleanup_removed_count: 0,
            cleanup_error: None,
            skipped_reason: Some(reason.into()),
        }
    }

    pub fn summary(&self) -> String {
        let mut summary = match self.skipped_reason.as_deref() {
            Some("disabled") => "disabled".into(),
            Some("interval") => "waiting for interval".into(),
            Some(reason) => format!("skipped: {reason}"),
            None => format!(
                "reviewed {} of {} PRs",
                self.enqueued_count, self.discovered_count
            ),
        };
        if self.cleanup_removed_count > 0 {
            summary.push_str(&format!(
                ", cleaned up {} checkout(s)",
                self.cleanup_removed_count
            ));
        }
        if let Some(error) = &self.cleanup_error {
            summary.push_str(&format!(", cleanup failed: {error}"));
        }
        summary
    }
}

#[deprecated(note = "use ReviewSourcePollResult")]
pub type GitHubReviewPollResult = ReviewSourcePollResult;

const DEFAULT_MAX_CONCURRENT_REVIEWS: usize = 3;
pub const CONFIG_TEMPLATE: &str = include_str!("../../../examples/config.toml");
pub const REVIEW_PROMPT_TEMPLATE: &str = include_str!("../../../examples/review-prompt.md");
const DEFAULT_REVIEW_PROMPT_FILENAME: &str = "review-prompt.md";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub command: Option<String>,
    pub github_command: Option<String>,
    pub checkout_dir: Option<String>,
    pub max_concurrent_reviews: usize,
    pub review_prompt_path: PathBuf,
    pub review_extra_instructions: String,
    pub sandbox: AgentSandboxConfig,
    pub github_discovery: GitHubDiscoveryConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: AgentProviderKind::default(),
            model: None,
            command: None,
            github_command: None,
            checkout_dir: None,
            max_concurrent_reviews: DEFAULT_MAX_CONCURRENT_REVIEWS,
            review_prompt_path: PathBuf::from(DEFAULT_REVIEW_PROMPT_FILENAME),
            review_extra_instructions: String::new(),
            sandbox: AgentSandboxConfig::default(),
            github_discovery: GitHubDiscoveryConfig::default(),
        }
    }
}

impl AgentConfig {
    pub fn from_toml(input: &str) -> AgentResult<Self> {
        Self::from_toml_with_config_dir(input, None)
    }

    fn from_toml_with_config_dir(input: &str, config_dir: Option<&Path>) -> AgentResult<Self> {
        let raw = toml::from_str::<RawConfig>(input)
            .map_err(|error| nitpick_agent_core::AgentError::config(error.to_string()))?;
        let agent = raw.agent.unwrap_or_default();
        let reviews = raw.reviews.unwrap_or_default();
        let github = raw.github.unwrap_or_default();
        let provider = match agent.provider {
            Some(provider) => provider.parse()?,
            None => AgentProviderKind::default(),
        };
        let model = agent
            .model
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty());
        let command = agent
            .command
            .map(|command| command.trim().to_owned())
            .filter(|command| !command.is_empty());
        let github_discovery = GitHubDiscoveryConfig::from_raw(&github)?;
        let github_command = github
            .command
            .map(|command| command.trim().to_owned())
            .filter(|command| !command.is_empty());
        let max_concurrent_reviews = reviews
            .max_concurrent
            .unwrap_or(DEFAULT_MAX_CONCURRENT_REVIEWS)
            .max(1);
        let review_prompt_path = review_prompt_path(reviews.prompt_path.as_deref(), config_dir);
        let review_extra_instructions = reviews
            .extra_instructions
            .map(|instructions| instructions.trim().to_owned())
            .filter(|instructions| !instructions.is_empty())
            .unwrap_or_default();
        let sandbox = AgentSandboxConfig::from_mode(agent.sandbox)?;

        Ok(Self {
            provider,
            model,
            command,
            github_command,
            checkout_dir: None,
            max_concurrent_reviews,
            review_prompt_path,
            review_extra_instructions,
            sandbox,
            github_discovery,
        })
    }

    pub fn load(path: impl AsRef<Path>) -> AgentResult<Self> {
        let path = path.as_ref();
        let input = fs::read_to_string(path).map_err(|error| {
            nitpick_agent_core::AgentError::config(format!(
                "failed to read config {}: {error}",
                path.display()
            ))
        })?;
        Self::from_toml_with_config_dir(&input, path.parent())
    }

    pub fn init_template_file(path: impl AsRef<Path>) -> AgentResult<()> {
        let path = path.as_ref();
        if path.exists() {
            let metadata = fs::metadata(path).map_err(|error| {
                nitpick_agent_core::AgentError::config(format!(
                    "failed to inspect config {}: {error}",
                    path.display()
                ))
            })?;
            if metadata.len() > 0 {
                return Ok(());
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                nitpick_agent_core::AgentError::config(format!(
                    "failed to create config directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(path, CONFIG_TEMPLATE).map_err(|error| {
            nitpick_agent_core::AgentError::config(format!(
                "failed to write config template {}: {error}",
                path.display()
            ))
        })?;
        Ok(())
    }

    pub fn write_config_example_file(config_path: impl AsRef<Path>) -> AgentResult<PathBuf> {
        let config_path = config_path.as_ref();
        let path = config_example_path(config_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                nitpick_agent_core::AgentError::config(format!(
                    "failed to create config directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&path, CONFIG_TEMPLATE).map_err(|error| {
            nitpick_agent_core::AgentError::config(format!(
                "failed to write config example {}: {error}",
                path.display()
            ))
        })?;
        Ok(path)
    }

    pub fn init_review_prompt_file(config_path: impl AsRef<Path>) -> AgentResult<PathBuf> {
        let path = default_review_prompt_path(config_path.as_ref());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                nitpick_agent_core::AgentError::config(format!(
                    "failed to create review prompt directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&path, REVIEW_PROMPT_TEMPLATE).map_err(|error| {
            nitpick_agent_core::AgentError::config(format!(
                "failed to write review prompt template {}: {error}",
                path.display()
            ))
        })?;
        Ok(path)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> AgentResult<Self> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(input) => Self::from_toml_with_config_dir(&input, path.parent()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(nitpick_agent_core::AgentError::config(format!(
                "failed to read config {}: {error}",
                path.display()
            ))),
        }
    }

    pub fn command_provider(&self) -> CommandAgentProvider {
        let provider = match &self.command {
            Some(command) => {
                CommandAgentProvider::new(self.provider.clone(), self.model.clone(), command)
            }
            None => CommandAgentProvider::for_kind(self.provider.clone(), self.model.clone()),
        };
        provider.with_sandbox(self.sandbox.command_sandbox_config())
    }

    fn provider(&self) -> Arc<dyn AgentProvider> {
        Arc::new(self.command_provider())
    }

    fn review_source(&self) -> Arc<dyn ReviewSource> {
        Arc::new(self.github_discovery_client())
    }

    fn github_discovery_client(&self) -> GitHubCliDiscovery {
        let client = match &self.checkout_dir {
            Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
                self.github_command.as_deref().unwrap_or("gh"),
                "git",
                checkout_dir,
            ),
            None => GitHubCliDiscovery::new(self.github_command.as_deref().unwrap_or("gh")),
        };
        client.with_allowlist(&self.github_discovery.allowlist)
    }

    fn sync_destination(
        &self,
        destination: &str,
        target: Option<&str>,
    ) -> AgentResult<Box<dyn ArtifactSyncDestination>> {
        match destination {
            "github" => match target {
                Some(target) => {
                    let target = target.parse::<PullRequestRef>().map_err(|error| {
                        AgentError::invalid_input(format!("invalid GitHub sync target: {error}"))
                    })?;
                    Ok(Box::new(GitHubCliSyncDestination::new(
                        target,
                        self.github_command.as_deref().unwrap_or("gh"),
                    )))
                }
                None => Ok(Box::new(GitHubDryRunSyncDestination)),
            },
            "github-review" => {
                let target = target.ok_or_else(|| {
                    AgentError::invalid_input("github-review sync requires a pull request target")
                })?;
                let target = target.parse::<PullRequestRef>().map_err(|error| {
                    AgentError::invalid_input(format!("invalid GitHub sync target: {error}"))
                })?;
                Ok(Box::new(GitHubCliReviewSyncDestination::new(
                    target,
                    self.github_command.as_deref().unwrap_or("gh"),
                )))
            }
            destination => Err(AgentError::invalid_input(format!(
                "unknown sync destination `{destination}`"
            ))),
        }
    }

    pub fn review_source_name(&self) -> String {
        "github".into()
    }

    fn apply_review_prompt(&self, mut input: ReviewInput) -> AgentResult<ReviewInput> {
        let mut prompt = match fs::read_to_string(&self.review_prompt_path) {
            Ok(prompt) => prompt,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && self.review_prompt_path.file_name()
                        == Some(std::ffi::OsStr::new(DEFAULT_REVIEW_PROMPT_FILENAME)) =>
            {
                REVIEW_PROMPT_TEMPLATE.into()
            }
            Err(error) => {
                return Err(AgentError::config(format!(
                    "failed to read review prompt {}: {error}",
                    self.review_prompt_path.display()
                )));
            }
        };
        if !self.review_extra_instructions.is_empty() {
            prompt.push_str("\n\nAdditional instructions:\n");
            prompt.push_str(&self.review_extra_instructions);
        }
        input.review_prompt = prompt;
        Ok(input)
    }
}

fn default_review_prompt_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_REVIEW_PROMPT_FILENAME)
}

fn config_example_path(config_path: &Path) -> PathBuf {
    let stem = config_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = config_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy();
    let name = if ext.is_empty() {
        format!("{stem}.example")
    } else {
        format!("{stem}.example.{ext}")
    };
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

fn review_prompt_path(raw_path: Option<&str>, config_dir: Option<&Path>) -> PathBuf {
    let path = raw_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REVIEW_PROMPT_FILENAME));
    if path.is_absolute() {
        path
    } else if let Some(config_dir) = config_dir {
        config_dir.join(path)
    } else {
        path
    }
}

pub(crate) fn github_pull_request_from_review_request(
    request: ReviewRequest,
) -> AgentResult<DiscoveredPullRequest> {
    let Some(number) = request.number else {
        return Err(AgentError::invalid_input(format!(
            "review request `{}` is missing a pull request number",
            request.display_reference()
        )));
    };
    let (owner, repo) = request.repository.split_once('/').ok_or_else(|| {
        AgentError::invalid_input(format!(
            "invalid GitHub repository name `{}`",
            request.repository
        ))
    })?;
    Ok(DiscoveredPullRequest {
        owner: owner.into(),
        repo: repo.into(),
        number,
        head_sha: request.head_sha,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    agent: Option<RawAgentConfig>,
    reviews: Option<RawReviewsConfig>,
    github: Option<RawGitHubConfig>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgentConfig {
    provider: Option<String>,
    model: Option<String>,
    command: Option<String>,
    sandbox: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReviewsConfig {
    max_concurrent: Option<usize>,
    prompt_path: Option<String>,
    extra_instructions: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSandboxConfig {
    pub mode: String,
}

impl Default for AgentSandboxConfig {
    fn default() -> Self {
        Self {
            mode: "macos-seatbelt".into(),
        }
    }
}

impl AgentSandboxConfig {
    fn from_mode(mode: Option<String>) -> AgentResult<Self> {
        let default = Self::default();
        let mode = mode
            .map(|mode| mode.trim().to_owned())
            .filter(|mode| !mode.is_empty())
            .unwrap_or(default.mode);
        if !matches!(mode.as_str(), "macos-seatbelt" | "none") {
            return Err(AgentError::config(format!(
                "unsupported agent sandbox mode `{mode}`"
            )));
        }
        Ok(Self { mode })
    }

    fn command_sandbox_config(&self) -> CommandSandboxConfig {
        if self.mode == "macos-seatbelt" {
            CommandSandboxConfig::macos_seatbelt()
        } else {
            CommandSandboxConfig::unsandboxed()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubDiscoveryConfig {
    pub enabled: bool,
    pub auto_review: bool,
    pub interval_seconds: u64,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
}

impl Default for GitHubDiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_review: false,
            interval_seconds: 60,
            allowlist: Vec::new(),
            denylist: Vec::new(),
        }
    }
}

impl GitHubDiscoveryConfig {
    pub fn allows_repository(&self, repository: &str) -> bool {
        let allowed = self.allowlist.is_empty()
            || self
                .allowlist
                .iter()
                .any(|pattern| wildcard_match(pattern, repository));
        let denied = self
            .denylist
            .iter()
            .any(|pattern| wildcard_match(pattern, repository));
        allowed && !denied
    }

    fn from_raw(raw: &RawGitHubConfig) -> AgentResult<Self> {
        let default = Self::default();
        let interval_seconds = raw
            .interval_seconds
            .unwrap_or(default.interval_seconds)
            .max(1);
        Ok(Self {
            enabled: raw.discovery.unwrap_or(default.enabled),
            auto_review: raw.auto_review.unwrap_or(default.auto_review),
            interval_seconds,
            allowlist: clean_patterns("github.allowlist", raw.allowlist.clone())?,
            denylist: clean_patterns("github.denylist", raw.denylist.clone())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nitpick_agent_core::{
        ActivityKind, ActivityStatus, ActivityStore, FixedClock, MemoryActivityStore,
    };

    use super::HostDaemon;

    #[test]
    fn prune_old_activities_removes_terminal_activities_older_than_24h() {
        let store = Arc::new(MemoryActivityStore::default());
        let now: u64 = 100_000;
        let day_secs: u64 = 24 * 60 * 60;

        let mut old_completed = store.create(ActivityKind::Review).expect("create");
        old_completed.status = ActivityStatus::Completed;
        old_completed.updated_at_unix = now - day_secs - 1;
        store.save(&old_completed).expect("save");

        let mut old_error = store.create(ActivityKind::Review).expect("create");
        old_error.status = ActivityStatus::Error;
        old_error.updated_at_unix = now - day_secs - 1;
        store.save(&old_error).expect("save");

        let mut recent_completed = store.create(ActivityKind::Review).expect("create");
        recent_completed.status = ActivityStatus::Completed;
        recent_completed.updated_at_unix = now - 3600;
        store.save(&recent_completed).expect("save");

        let mut still_running = store.create(ActivityKind::Review).expect("create");
        still_running.status = ActivityStatus::Running;
        still_running.updated_at_unix = now - day_secs - 1;
        store.save(&still_running).expect("save");

        let daemon = HostDaemon::with_clock(store.clone(), Arc::new(FixedClock(now)));
        let pruned = daemon.prune_old_activities().expect("prune");

        assert_eq!(pruned, 2);
        assert_eq!(store.list().expect("list").len(), 2);
    }
}

fn clean_patterns(name: &str, patterns: Option<Vec<String>>) -> AgentResult<Vec<String>> {
    patterns
        .unwrap_or_default()
        .into_iter()
        .map(|pattern| pattern.trim().to_owned())
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            if pattern.contains("**") {
                return Err(AgentError::config(format!(
                    "{name} pattern `{pattern}` is invalid: use `*`, not `**`"
                )));
            }
            Ok(pattern)
        })
        .collect()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == value[value_index] || pattern[pattern_index] == b'*')
        {
            if pattern[pattern_index] == b'*' {
                star_index = Some(pattern_index);
                star_value_index = value_index;
                pattern_index += 1;
            } else {
                pattern_index += 1;
                value_index += 1;
            }
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGitHubConfig {
    command: Option<String>,
    discovery: Option<bool>,
    auto_review: Option<bool>,
    interval_seconds: Option<u64>,
    allowlist: Option<Vec<String>>,
    denylist: Option<Vec<String>>,
}
