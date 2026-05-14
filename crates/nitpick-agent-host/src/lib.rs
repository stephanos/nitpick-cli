use std::{
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use axum::{
    Json, Router,
    extract::{Path as PathParam, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use fs_err as fs;
use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ActivityStatus, ActivityStore, AgentError, AgentProvider,
    AgentProviderKind, AgentResult, AgentRuntime, Artifact, ArtifactContent, ArtifactId,
    ArtifactSyncDestination, ArtifactSyncState, ChatInput, Clock, CommandAgentProvider,
    CommandSandboxConfig, MemoryProcessedReviewStore, ProcessedReviewStore, ReviewInput,
    ReviewRequest, ReviewSource, SessionStatus, SystemClock,
};
use nitpick_agent_github::{
    DiscoveredPullRequest, GitHubCliDiscovery, GitHubCliReviewSyncDestination,
    GitHubCliSyncDestination, GitHubDryRunSyncDestination, PullRequestRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HostDaemon {
    config: AgentConfig,
    store: Arc<dyn ActivityStore>,
    processed_reviews: Arc<dyn ProcessedReviewStore>,
    provider: Arc<dyn AgentProvider>,
    review_source: Arc<dyn ReviewSource>,
    clock: Arc<dyn Clock>,
    automatic_checkout_cleanup: bool,
    last_review_source_poll_unix: Arc<Mutex<Option<u64>>>,
    last_review_source_poll_summary: Arc<Mutex<Option<String>>>,
    review_slots: Arc<(Mutex<usize>, Condvar)>,
}

impl HostDaemon {
    pub fn new(store: Arc<dyn ActivityStore>) -> Self {
        Self::with_config(store, AgentConfig::default())
    }

    pub fn with_config(store: Arc<dyn ActivityStore>, config: AgentConfig) -> Self {
        let provider = config.provider();
        let review_source = config.review_source();
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            last_review_source_poll_unix: Arc::new(Mutex::new(None)),
            last_review_source_poll_summary: Arc::new(Mutex::new(None)),
            review_slots: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    pub fn with_config_and_processed_reviews(
        store: Arc<dyn ActivityStore>,
        config: AgentConfig,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
    ) -> Self {
        let provider = config.provider();
        let review_source = config.review_source();
        Self {
            config,
            store,
            processed_reviews,
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            last_review_source_poll_unix: Arc::new(Mutex::new(None)),
            last_review_source_poll_summary: Arc::new(Mutex::new(None)),
            review_slots: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    pub fn with_provider(store: Arc<dyn ActivityStore>, provider: Arc<dyn AgentProvider>) -> Self {
        let config = AgentConfig::default();
        let review_source = config.review_source();
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            last_review_source_poll_unix: Arc::new(Mutex::new(None)),
            last_review_source_poll_summary: Arc::new(Mutex::new(None)),
            review_slots: Arc::new((Mutex::new(0), Condvar::new())),
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
        Self {
            config,
            store,
            processed_reviews,
            provider,
            review_source,
            clock,
            automatic_checkout_cleanup: false,
            last_review_source_poll_unix: Arc::new(Mutex::new(None)),
            last_review_source_poll_summary: Arc::new(Mutex::new(None)),
            review_slots: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    pub fn status(&self) -> AgentResult<HostStatus> {
        let artifacts = self.store.list_artifacts()?;
        let activities = self.store.list()?;
        Ok(HostStatus {
            activity_count: activities.len(),
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
            review_source_last_poll_unix: *self
                .last_review_source_poll_unix
                .lock()
                .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))?,
            review_source_last_poll_summary: self
                .last_review_source_poll_summary
                .lock()
                .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))?
                .clone(),
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

    pub fn record_review_request_detected_activity(
        &self,
        request: &ReviewRequest,
    ) -> AgentResult<Activity> {
        let mut activity = self.store.create(ActivityKind::Discovery)?;
        activity.status = ActivityStatus::Completed;
        activity.label = Some(format!(
            "detected review request {}",
            request.display_reference()
        ));
        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    #[tracing::instrument(skip_all)]
    pub fn cleanup_checkouts(&self) -> AgentResult<CleanupCheckoutsResult> {
        let github = self.config.github_discovery_client();
        let mut cleaned = Vec::new();

        for pull_request in github.list_checkouts()? {
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

    #[deprecated(note = "use discover_review_requests")]
    pub fn discover_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discover_review_requests()?
            .into_iter()
            .map(github_pull_request_from_review_request)
            .collect()
    }

    pub fn discover_new_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.discover_review_requests()?
            .into_iter()
            .filter(|request| {
                self.config
                    .github_discovery
                    .allows_repository(&request.repository)
            })
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
        if !self.config.github_discovery.enabled {
            tracing::debug!("review source poll skipped because discovery is disabled");
            return Ok(ReviewSourcePollResult::skipped("disabled"));
        }

        let now = self.clock.now_unix();
        {
            let mut last_poll = self
                .last_review_source_poll_unix
                .lock()
                .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))?;
            if let Some(last_poll) = *last_poll
                && now.saturating_sub(last_poll) < self.config.github_discovery.interval_seconds
            {
                tracing::debug!("review source poll skipped because interval has not elapsed");
                return Ok(ReviewSourcePollResult::skipped("interval"));
            }
            *last_poll = Some(now);
        }

        let requests = self.discover_new_review_requests()?;
        let discovered_count = requests.len();
        for request in &requests {
            self.record_review_request_detected_activity(request)?;
        }
        if !self.config.github_discovery.auto_review {
            let result = ReviewSourcePollResult {
                discovered_count,
                enqueued_count: 0,
                cleanup_removed_count: 0,
                cleanup_error: None,
                skipped_reason: None,
            };
            self.record_review_source_poll_result(now, &result)?;
            return Ok(result);
        }

        let mut enqueued_count = 0;
        for request in requests {
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
        self.record_review_source_poll_result(now, &result)?;
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
        self.runtime().start_review(input)
    }

    pub fn enqueue_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let runtime = self.runtime();
        let mut activity = runtime.create_queued_review_activity(&input)?;
        let slot_acquired = self.try_acquire_review_slot()?;
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
            self.wait_for_review_slot()?;
        }
        let result = self.runtime().run_review(activity, input);
        self.release_review_slot()?;
        result
    }

    fn try_acquire_review_slot(&self) -> AgentResult<bool> {
        let (running, _) = self.review_slots.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| AgentError::io("review slots lock", "poisoned"))?;
        if *running >= self.config.max_concurrent_reviews.max(1) {
            return Ok(false);
        }
        *running += 1;
        Ok(true)
    }

    fn wait_for_review_slot(&self) -> AgentResult<()> {
        let (running, changed) = self.review_slots.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| AgentError::io("review slots lock", "poisoned"))?;
        while *running >= self.config.max_concurrent_reviews.max(1) {
            running = changed
                .wait(running)
                .map_err(|_| AgentError::io("review slots lock", "poisoned"))?;
        }
        *running += 1;
        Ok(())
    }

    fn release_review_slot(&self) -> AgentResult<()> {
        let (running, changed) = self.review_slots.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| AgentError::io("review slots lock", "poisoned"))?;
        *running = running.saturating_sub(1);
        changed.notify_one();
        Ok(())
    }

    fn record_review_source_poll_result(
        &self,
        now: u64,
        result: &ReviewSourcePollResult,
    ) -> AgentResult<()> {
        *self
            .last_review_source_poll_unix
            .lock()
            .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))? = Some(now);
        *self
            .last_review_source_poll_summary
            .lock()
            .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))? =
            Some(result.summary());
        Ok(())
    }

    fn record_review_source_poll_error(&self, now: u64, error: &str) -> AgentResult<()> {
        *self
            .last_review_source_poll_unix
            .lock()
            .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))? = Some(now);
        *self
            .last_review_source_poll_summary
            .lock()
            .map_err(|_| AgentError::io("review source poll state lock", "poisoned"))? =
            Some(review_source_error_summary(error));
        let mut activity = self.store.create(ActivityKind::Discovery)?;
        activity.status = ActivityStatus::Error;
        activity.label = Some("discovery poll".into());
        activity.error = Some(error.into());
        activity.touch();
        self.store.save(&activity)?;
        Ok(())
    }
}

fn review_source_error_summary(error: &str) -> String {
    if error.contains("failed to start GitHub CLI") {
        return format!("github unavailable: {error}");
    }
    format!("review source failed: {error}")
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

#[derive(Clone)]
pub struct ReviewSourcePoller {
    daemon: HostDaemon,
}

impl ReviewSourcePoller {
    pub fn new(daemon: HostDaemon) -> Self {
        Self { daemon }
    }

    pub fn tick(&self) -> AgentResult<ReviewSourcePollResult> {
        let mut result = match self.daemon.poll_review_requests() {
            Ok(result) => result,
            Err(error) => {
                let now = self.daemon.clock.now_unix();
                let message = error.message();
                tracing::warn!(error = %message, "review source poll failed");
                self.daemon.record_review_source_poll_error(now, &message)?;
                return Err(error);
            }
        };
        if result.skipped_reason.is_none() && self.daemon.automatic_checkout_cleanup {
            match self.daemon.cleanup_checkouts() {
                Ok(cleanup) => {
                    result.cleanup_removed_count = cleanup.removed_count;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "automatic checkout cleanup failed");
                    result.cleanup_error = Some(error.to_string());
                }
            }
            let now = self.daemon.clock.now_unix();
            self.daemon.record_review_source_poll_result(now, &result)?;
        }
        Ok(result)
    }
}

#[deprecated(note = "use ReviewSourcePollResult")]
pub type GitHubReviewPollResult = ReviewSourcePollResult;

#[deprecated(note = "use ReviewSourcePoller")]
pub type GitHubReviewPoller = ReviewSourcePoller;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostStatus {
    pub activity_count: usize,
    pub running_activity_count: usize,
    pub completed_activity_count: usize,
    pub error_activity_count: usize,
    pub artifact_count: usize,
    pub local_only_artifact_count: usize,
    pub pending_sync_artifact_count: usize,
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCheckoutsResult {
    pub removed_count: usize,
    pub cleaned: Vec<String>,
}

pub fn api_router(daemon: HostDaemon) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/activities", get(activities))
        .route("/activities/{id}", get(activity))
        .route("/activities/{id}/artifacts", get(activity_artifacts))
        .route(
            "/activities/{id}/artifact-sync",
            post(activity_artifact_sync),
        )
        .route("/sync/pending", get(pending_sync_artifacts))
        .route("/review-requests", get(review_requests))
        .route("/github/review-requests", get(github_review_requests))
        .route("/artifacts/{id}", get(artifact))
        .route("/artifacts/{id}/sync-state", post(artifact_sync_state))
        .route("/artifacts/{id}/sync", post(artifact_sync))
        .route("/maintenance/cleanup-checkouts", post(cleanup_checkouts))
        .route("/reviews", post(review))
        .route("/chats", post(chat))
        .with_state(daemon)
}

async fn status(State(daemon): State<HostDaemon>) -> Result<Json<HostStatus>, ApiError> {
    Ok(Json(daemon.status()?))
}

async fn activities(State(daemon): State<HostDaemon>) -> Result<Json<Vec<Activity>>, ApiError> {
    Ok(Json(daemon.list_activities()?))
}

async fn activity(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
) -> Result<Response, ApiError> {
    match daemon.get_activity(&ActivityId::new(id))? {
        Some(activity) => Ok(Json(activity).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn activity_artifacts(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
) -> Result<Response, ApiError> {
    let id = ActivityId::new(id);
    if daemon.get_activity(&id)?.is_none() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    Ok(Json(daemon.list_artifacts_for(&id)?).into_response())
}

async fn pending_sync_artifacts(
    State(daemon): State<HostDaemon>,
    Query(query): Query<PendingSyncQuery>,
) -> Result<Json<Vec<Artifact>>, ApiError> {
    Ok(Json(daemon.list_pending_sync_artifacts(
        query.destination.as_deref(),
    )?))
}

async fn github_review_requests(
    State(daemon): State<HostDaemon>,
    Query(query): Query<ReviewRequestsQuery>,
) -> Result<Json<Vec<DiscoveredPullRequest>>, ApiError> {
    let requests = match query.filter.as_deref() {
        Some("new") => daemon.discover_new_review_requests()?,
        Some(filter) => {
            return Err(AgentError::invalid_input(format!(
                "unknown review request filter `{filter}`"
            ))
            .into());
        }
        None => daemon.discover_review_requests()?,
    };
    Ok(Json(
        requests
            .into_iter()
            .map(github_pull_request_from_review_request)
            .collect::<AgentResult<Vec<_>>>()?,
    ))
}

async fn review_requests(
    State(daemon): State<HostDaemon>,
    Query(query): Query<ReviewRequestsQuery>,
) -> Result<Json<Vec<ReviewRequest>>, ApiError> {
    match query.filter.as_deref() {
        Some("new") => Ok(Json(daemon.discover_new_review_requests()?)),
        Some(filter) => Err(AgentError::invalid_input(format!(
            "unknown review request filter `{filter}`"
        ))
        .into()),
        None => Ok(Json(daemon.discover_review_requests()?)),
    }
}

#[derive(Deserialize)]
struct ReviewRequestsQuery {
    filter: Option<String>,
}

#[derive(Deserialize)]
struct PendingSyncQuery {
    destination: Option<String>,
}

async fn artifact(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
) -> Result<Response, ApiError> {
    match daemon.get_artifact(&ArtifactId::new(id))? {
        Some(artifact) => Ok(Json(artifact).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn artifact_sync_state(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
    Json(sync_state): Json<ArtifactSyncState>,
) -> Result<Response, ApiError> {
    match daemon.update_artifact_sync_state(&ArtifactId::new(id), sync_state)? {
        Some(artifact) => Ok(Json(artifact).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn artifact_sync(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
    Json(input): Json<ArtifactSyncInput>,
) -> Result<Response, ApiError> {
    match daemon.sync_artifact(
        &ArtifactId::new(id),
        &input.destination,
        input.target.as_deref(),
    )? {
        Some(artifact) => Ok(Json(artifact).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn activity_artifact_sync(
    State(daemon): State<HostDaemon>,
    PathParam(id): PathParam<String>,
    Json(input): Json<ArtifactSyncInput>,
) -> Result<Response, ApiError> {
    match daemon.sync_activity_artifacts(
        &ActivityId::new(id),
        &input.destination,
        input.target.as_deref(),
    )? {
        Some(artifacts) => Ok(Json(artifacts).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn cleanup_checkouts(
    State(daemon): State<HostDaemon>,
) -> Result<Json<CleanupCheckoutsResult>, ApiError> {
    Ok(Json(daemon.cleanup_checkouts()?))
}

#[derive(Deserialize)]
struct ArtifactSyncInput {
    destination: String,
    target: Option<String>,
}

async fn review(
    State(daemon): State<HostDaemon>,
    Json(input): Json<ReviewInput>,
) -> Result<Json<Activity>, ApiError> {
    Ok(Json(daemon.enqueue_review(input)?))
}

async fn chat(
    State(daemon): State<HostDaemon>,
    Json(input): Json<ChatInput>,
) -> Result<Json<Activity>, ApiError> {
    Ok(Json(daemon.enqueue_chat(input)?))
}

struct ApiError(nitpick_agent_core::AgentError);

impl From<nitpick_agent_core::AgentError> for ApiError {
    fn from(error: nitpick_agent_core::AgentError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after_seconds = match &self.0 {
            AgentError::GitHubRateLimited {
                retry_after_seconds,
                ..
            } => *retry_after_seconds,
            _ => None,
        };
        let status = api_error_status(&self.0);
        let mut response = (
            status,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response();
        if let Some(seconds) = retry_after_seconds
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

fn api_error_status(error: &AgentError) -> StatusCode {
    match error {
        AgentError::InvalidInput { .. } | AgentError::Config { .. } => StatusCode::BAD_REQUEST,
        AgentError::NotFound { .. } => StatusCode::NOT_FOUND,
        AgentError::GitHubRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
        AgentError::Message { .. }
        | AgentError::Io { .. }
        | AgentError::Json { .. }
        | AgentError::Provider { .. }
        | AgentError::Sandbox { .. }
        | AgentError::GitHubCli { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

const DEFAULT_MAX_CONCURRENT_REVIEWS: usize = 3;
pub const CONFIG_TEMPLATE: &str = include_str!("../../../config.example.toml");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub command: Option<String>,
    pub github_command: Option<String>,
    pub checkout_dir: Option<String>,
    pub max_concurrent_reviews: usize,
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
            sandbox: AgentSandboxConfig::default(),
            github_discovery: GitHubDiscoveryConfig::default(),
        }
    }
}

impl AgentConfig {
    pub fn from_toml(input: &str) -> AgentResult<Self> {
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
        let github_discovery = GitHubDiscoveryConfig::from_raw(&github);
        let github_command = github
            .command
            .map(|command| command.trim().to_owned())
            .filter(|command| !command.is_empty());
        let max_concurrent_reviews = reviews
            .max_concurrent
            .unwrap_or(DEFAULT_MAX_CONCURRENT_REVIEWS)
            .max(1);
        let sandbox = AgentSandboxConfig::from_mode(agent.sandbox)?;

        Ok(Self {
            provider,
            model,
            command,
            github_command,
            checkout_dir: None,
            max_concurrent_reviews,
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
        Self::from_toml(&input)
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

    pub fn load_or_default(path: impl AsRef<Path>) -> AgentResult<Self> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(input) => Self::from_toml(&input),
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
}

fn github_pull_request_from_review_request(
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

    fn from_raw(raw: &RawGitHubConfig) -> Self {
        let default = Self::default();
        let interval_seconds = raw
            .interval_seconds
            .unwrap_or(default.interval_seconds)
            .max(1);
        Self {
            enabled: raw.discovery.unwrap_or(default.enabled),
            auto_review: raw.auto_review.unwrap_or(default.auto_review),
            interval_seconds,
            allowlist: clean_patterns(raw.allowlist.clone()),
            denylist: clean_patterns(raw.denylist.clone()),
        }
    }
}

fn clean_patterns(patterns: Option<Vec<String>>) -> Vec<String> {
    patterns
        .unwrap_or_default()
        .into_iter()
        .map(|pattern| pattern.trim().to_owned())
        .filter(|pattern| !pattern.is_empty())
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
