mod api;
mod polling_state;
mod review_intake;
pub mod review_mcp;
mod review_queue;
mod review_slots;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use fs_err as fs;
use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore, AgentError,
    AgentProvider, AgentProviderKind, AgentResult, AgentRuntime, Artifact, ArtifactContent,
    ArtifactId, ArtifactKind, ArtifactSyncDestination, ArtifactSyncState, ChatInput,
    CleanupCheckoutsResult, Clock, CommandAgentProvider, CommandSandboxConfig, HostStatus,
    LocalStateResetResult, MemoryProcessedReviewStore, ProcessedReviewStore,
    ProviderDiagnosticInput, ProviderReviewContext, ProviderRunContext, ReviewInput, ReviewMode,
    ReviewOutput, ReviewRequest, ReviewSource, SessionStatus, SystemClock, default_data_dir,
};
use nitpick_agent_github::{
    DiscoveredPullRequest, GitHubCliDiscovery, GitHubCliReviewSyncDestination,
    GitHubCliSyncDestination, GitHubDryRunSyncDestination, GitHubPullRequestContext,
    GitHubPullRequestConversationComment, GitHubReviewComment, GitHubReviewWorkflowSync,
    PullRequestRef,
};
use polling_state::PollingState;
use review_intake::ReviewRequestIntake;
use review_queue::ReviewExecutionQueue;
use serde::Deserialize;

pub use api::api_router;

const ACTIVITY_PRUNE_AGE_SECS: u64 = 24 * 60 * 60;

#[derive(Clone)]
pub struct HostReviewProvider {
    inner: Arc<dyn AgentProvider>,
    github_command: Option<String>,
}

impl HostReviewProvider {
    pub fn new(inner: Arc<dyn AgentProvider>, github_command: Option<String>) -> Self {
        Self {
            inner,
            github_command,
        }
    }
}

impl AgentProvider for HostReviewProvider {
    fn review(
        &self,
        session: &mut nitpick_agent_core::AgentSession,
        input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        if !self.inner.supports_review_tools() {
            return self.inner.review(session, input, context);
        }

        let handle = review_mcp::ReviewMcpServerHandle::start(
            input,
            self.existing_review_comments(input),
            self.pull_request_context(input),
            self.review_mcp_github_target(input),
        )?;
        let tools = handle.tool_config();
        self.inner.review(
            session,
            input,
            ProviderReviewContext::new(context.run_sink).with_tools(&tools),
        )?;
        let state = handle.session_state()?;
        if !state.finished {
            return Err(AgentError::provider(
                "provider exited before calling finish_review",
            ));
        }
        self.delete_review_comments(&state)?;
        Ok(ReviewOutput {
            comments: state.comments,
        })
    }

    fn supports_review_tools(&self) -> bool {
        self.inner.supports_review_tools()
    }

    fn chat(
        &self,
        session: &mut nitpick_agent_core::AgentSession,
        input: &ChatInput,
        context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        self.inner.chat(session, input, context)
    }

    fn attach_session(&self, session: &nitpick_agent_core::AgentSession) -> AgentResult<()> {
        self.inner.attach_session(session)
    }
}

impl HostReviewProvider {
    fn existing_review_comments(
        &self,
        input: &ReviewInput,
    ) -> Vec<review_mcp::ExistingReviewComment> {
        let Some(destination) = self.github_review_destination(input) else {
            return Vec::new();
        };
        match destination.review_comments() {
            Ok(comments) => comments.into_iter().map(existing_review_comment).collect(),
            Err(error) => {
                tracing::warn!(error = %error, "fetch existing GitHub review comments failed");
                Vec::new()
            }
        }
    }

    fn pull_request_context(&self, input: &ReviewInput) -> review_mcp::PullRequestContext {
        let Some(destination) = self.github_review_destination(input) else {
            return review_mcp::PullRequestContext::default();
        };
        match destination.pull_request_context() {
            Ok(context) => pull_request_context(context),
            Err(error) => {
                tracing::warn!(error = %error, "fetch GitHub pull request context failed");
                review_mcp::PullRequestContext::default()
            }
        }
    }

    fn delete_review_comments(&self, state: &review_mcp::ReviewMcpSessionState) -> AgentResult<()> {
        let Some(github) = &state.github else {
            return Ok(());
        };
        let destination = GitHubCliReviewSyncDestination::new(
            PullRequestRef {
                owner: github.owner.clone(),
                repo: github.repo.clone(),
                number: github.number,
            },
            &github.command,
        );
        for comment_id in &state.deleted_comment_ids {
            destination.delete_review_comment(comment_id)?;
        }
        Ok(())
    }

    fn review_mcp_github_target(
        &self,
        input: &ReviewInput,
    ) -> Option<review_mcp::ReviewMcpGitHubTarget> {
        let reference = pull_request_ref_from_review_input(input)?;
        Some(review_mcp::ReviewMcpGitHubTarget {
            owner: reference.owner,
            repo: reference.repo,
            number: reference.number,
            command: self.github_command.as_deref().unwrap_or("gh").into(),
        })
    }

    fn github_review_destination(
        &self,
        input: &ReviewInput,
    ) -> Option<GitHubCliReviewSyncDestination> {
        let reference = pull_request_ref_from_review_input(input)?;
        Some(GitHubCliReviewSyncDestination::new(
            reference,
            self.github_command.as_deref().unwrap_or("gh"),
        ))
    }
}

fn existing_review_comment(comment: GitHubReviewComment) -> review_mcp::ExistingReviewComment {
    review_mcp::ExistingReviewComment {
        id: comment.id,
        review_id: comment.review_id,
        path: comment.path,
        line: comment.line,
        body: comment.body,
        author: comment.author,
        draft: comment.draft,
    }
}

fn pull_request_context(context: GitHubPullRequestContext) -> review_mcp::PullRequestContext {
    review_mcp::PullRequestContext {
        title: context.title,
        author: context.author,
        url: context.url,
        body: context.body,
        head_sha: context.head_sha,
        head_ref_name: context.head_ref_name,
        state: context.state,
        conversation_comments: context
            .conversation_comments
            .into_iter()
            .map(pull_request_conversation_comment)
            .collect(),
    }
}

fn pull_request_conversation_comment(
    comment: GitHubPullRequestConversationComment,
) -> review_mcp::PullRequestConversationComment {
    review_mcp::PullRequestConversationComment {
        id: comment.id,
        body: comment.body,
        author: comment.author,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        url: comment.url,
    }
}

fn pull_request_ref_from_review_input(input: &ReviewInput) -> Option<PullRequestRef> {
    let number = input.subject.number?;
    let (owner, repo) = input.subject.repository.split_once('/')?;
    Some(PullRequestRef {
        owner: owner.into(),
        repo: repo.into(),
        number,
    })
}

#[derive(Clone)]
pub struct HostDaemon {
    config: AgentConfig,
    store: Arc<dyn ActivityStore>,
    processed_reviews: Arc<dyn ProcessedReviewStore>,
    provider: Arc<dyn AgentProvider>,
    review_source: Arc<dyn ReviewSource>,
    clock: Arc<dyn Clock>,
    automatic_checkout_cleanup: bool,
    data_dir: PathBuf,
    polling_state: PollingState,
    review_queue: ReviewExecutionQueue,
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
            store: store.clone(),
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            data_dir: default_data_dir(),
            polling_state: PollingState::new(),
            review_queue: ReviewExecutionQueue::new(store, max_concurrent),
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
            store: store.clone(),
            processed_reviews,
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            data_dir: default_data_dir(),
            polling_state: PollingState::new(),
            review_queue: ReviewExecutionQueue::new(store, max_concurrent),
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
            store: store.clone(),
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock,
            automatic_checkout_cleanup: false,
            data_dir: default_data_dir(),
            polling_state: PollingState::new(),
            review_queue: ReviewExecutionQueue::new(store, max_concurrent),
        }
    }

    pub fn with_provider(store: Arc<dyn ActivityStore>, provider: Arc<dyn AgentProvider>) -> Self {
        let config = AgentConfig::default();
        let review_source = config.review_source();
        let max_concurrent = config.max_concurrent_reviews;
        Self {
            config,
            store: store.clone(),
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            review_source,
            clock: Arc::new(SystemClock),
            automatic_checkout_cleanup: true,
            data_dir: default_data_dir(),
            polling_state: PollingState::new(),
            review_queue: ReviewExecutionQueue::new(store, max_concurrent),
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
            store: store.clone(),
            processed_reviews,
            provider,
            review_source,
            clock,
            automatic_checkout_cleanup: false,
            data_dir: default_data_dir(),
            polling_state: PollingState::new(),
            review_queue: ReviewExecutionQueue::new(store, max_concurrent),
        }
    }

    pub fn with_data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        self.provider = Arc::new(self.config.command_provider_with_data_dir(&data_dir));
        self.data_dir = data_dir;
        self
    }

    pub fn status(&self) -> AgentResult<HostStatus> {
        let artifacts = self.store.list_artifacts()?;
        let activities = self.store.list()?;
        let attention = provider_attention(&activities, &self.config.provider);
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
            attention,
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

    pub fn reset_local_state(&self, force: bool) -> AgentResult<LocalStateResetResult> {
        if !force && self.has_active_review_activity()? {
            return Err(AgentError::invalid_input(
                "cannot reset local state while reviews are active; rerun with --force to reset anyway",
            ));
        }

        let removed_artifact_count = self.store.clear_artifacts()?;
        let removed_activity_count = self.store.clear_activities()?;
        let removed_processed_review_count = self.processed_reviews.clear_processed()?;
        let removed_checkout_count = self.clear_checkouts()?;
        self.polling_state.clear()?;
        let truncated_log = self.truncate_daemon_log()?;

        Ok(LocalStateResetResult {
            removed_activity_count,
            removed_artifact_count,
            removed_processed_review_count,
            removed_checkout_count,
            truncated_log,
        })
    }

    fn has_active_review_activity(&self) -> AgentResult<bool> {
        Ok(self.store.list()?.into_iter().any(|activity| {
            activity.kind == ActivityKind::Review
                && matches!(
                    activity.status,
                    ActivityStatus::Queued | ActivityStatus::Running
                )
        }))
    }

    fn clear_checkouts(&self) -> AgentResult<usize> {
        let checkout_root = self.checkout_root();
        if !checkout_root.exists() {
            return Ok(0);
        }

        let removed_count = fs::read_dir(&checkout_root)
            .map_err(|error| AgentError::io_path("read checkout root", &checkout_root, error))?
            .filter_map(|entry| entry.ok())
            .count();
        fs::remove_dir_all(&checkout_root)
            .map_err(|error| AgentError::io_path("clear checkout root", &checkout_root, error))?;
        fs::create_dir_all(&checkout_root)
            .map_err(|error| AgentError::io_path("create checkout root", &checkout_root, error))?;
        Ok(removed_count)
    }

    fn checkout_root(&self) -> PathBuf {
        self.config
            .checkout_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("checkouts"))
    }

    fn truncate_daemon_log(&self) -> AgentResult<bool> {
        let path = self.data_dir.join("logs").join("daemon.log");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| AgentError::io_path("create log dir", parent, error))?;
        }
        fs::write(&path, "")
            .map_err(|error| AgentError::io_path("truncate daemon log", &path, error))?;
        Ok(true)
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
            && let Some(updated) = self.reconcile_github_review_artifacts(&artifacts, target)?
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

    fn sync_completed_github_review(
        &self,
        activity: &Activity,
        target: &str,
        input: &ReviewInput,
    ) -> AgentResult<()> {
        self.sync_activity_artifacts(&activity.id, "github-review", Some(target))?;
        if self.completed_review_has_no_comments(activity)? {
            let sync = self.github_review_workflow_sync(target)?;
            let Some(comment) = sync.no_findings_draft_file_comment(&input.diff)? else {
                return Ok(());
            };
            let artifact = self.store.create_artifact(
                activity.id.clone(),
                ArtifactKind::ReviewComment,
                ArtifactContent::ReviewComment(comment),
            )?;
            self.store.save_artifacts(std::slice::from_ref(&artifact))?;
            let (artifact_id, sync_state) = sync.sync_no_findings_draft_file_comment(&artifact)?;
            self.store
                .update_artifact_sync_state(&artifact_id, sync_state)?;
        }
        Ok(())
    }

    fn completed_review_has_no_comments(&self, activity: &Activity) -> AgentResult<bool> {
        if let Some(ActivityOutput::Review(output)) = &activity.output
            && !output.comments.is_empty()
        {
            return Ok(false);
        }
        Ok(!self
            .store
            .list_artifacts_for(&activity.id)?
            .iter()
            .any(|artifact| matches!(artifact.content, ArtifactContent::ReviewComment(_))))
    }

    fn reconcile_github_review_artifacts(
        &self,
        artifacts: &[Artifact],
        target: Option<&str>,
    ) -> AgentResult<Option<Vec<Artifact>>> {
        let target = target.ok_or_else(|| {
            AgentError::invalid_input("github-review sync requires a pull request target")
        })?;
        let updates = self
            .github_review_workflow_sync(target)?
            .reconcile_pending_artifact_states(artifacts)?;
        let Some(updates) = updates else {
            return Ok(None);
        };
        let mut updated = Vec::with_capacity(artifacts.len());
        for (artifact_id, sync_state) in updates {
            updated.push(
                self.store
                    .update_artifact_sync_state(&artifact_id, sync_state)?,
            );
        }
        Ok(Some(updated))
    }

    fn github_review_workflow_sync(&self, target: &str) -> AgentResult<GitHubReviewWorkflowSync> {
        let target = target.parse::<PullRequestRef>().map_err(|error| {
            AgentError::invalid_input(format!("invalid GitHub sync target: {error}"))
        })?;
        Ok(GitHubReviewWorkflowSync::new(
            target,
            self.config.github_command.as_deref().unwrap_or("gh"),
        ))
    }

    pub fn discover_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.review_intake().discover_review_requests()
    }

    #[deprecated(note = "use discover_review_requests")]
    pub fn discover_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discover_review_requests()?
            .into_iter()
            .map(github_pull_request_from_review_request)
            .collect()
    }

    pub fn discover_new_review_requests(&self) -> AgentResult<Vec<ReviewRequest>> {
        self.review_intake().discover_new_review_requests()
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
        let mut result = self.review_intake().poll(
            |request| {
                self.record_review_request_detected_activity(request)
                    .map(|activity| activity.id.to_string())
            },
            |input| {
                let activity = self.start_review(input)?;
                if activity.status == ActivityStatus::Completed {
                    Ok(Some(activity.id.to_string()))
                } else {
                    Ok(None)
                }
            },
        )?;
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

    fn review_intake(&self) -> ReviewRequestIntake {
        ReviewRequestIntake::new(
            self.config.clone(),
            self.store.clone(),
            self.processed_reviews.clone(),
            self.review_source.clone(),
            self.clock.clone(),
            self.polling_state.clone(),
        )
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
        let daemon = self.clone();
        let post_review_daemon = self.clone();
        self.review_queue.enqueue(
            input,
            self.runtime(),
            move |activity, input| daemon.run_enqueued_review(activity, input),
            move |result, input| {
                post_review_daemon.sync_completed_enqueued_review(result, input);
            },
        )
    }

    pub fn retry_failed_activities(
        &self,
        input: nitpick_agent_core::RetryFailedActivitiesInput,
    ) -> AgentResult<nitpick_agent_core::RetryFailedActivitiesResult> {
        let activities = self.list_activities()?;
        let mut queued = Vec::new();
        let mut skipped = 0;
        for activity in activities {
            if activity.status != ActivityStatus::Error || activity.kind != ActivityKind::Review {
                continue;
            }
            if provider_failure_resolved(&activity) {
                continue;
            }
            let Some(classification) = nitpick_agent_core::classify_provider_failure(&activity)
            else {
                continue;
            };
            if classification.kind != input.kind {
                continue;
            }
            let Some(review) = activity
                .retry
                .as_ref()
                .and_then(|retry| retry.review.as_ref())
            else {
                skipped += 1;
                continue;
            };
            if !is_retryable_review_metadata(review) {
                skipped += 1;
                continue;
            }
            if !review.force && self.has_active_review_for(&review.repository, review.number)? {
                skipped += 1;
                continue;
            }
            let Ok(review_input) = self.retry_review_input(review) else {
                skipped += 1;
                continue;
            };
            let new_activity = self.enqueue_review(review_input)?;
            let new_activity_id = new_activity.id.clone();
            let mut resolved_activity = activity;
            if let Some(retry) = resolved_activity.retry.as_mut() {
                retry.resolved_by_activity = Some(new_activity_id.clone());
            }
            self.store.save(&resolved_activity)?;
            queued.push(new_activity_id);
        }
        Ok(nitpick_agent_core::RetryFailedActivitiesResult {
            queued: queued.len(),
            skipped,
            activities: queued,
        })
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

    pub fn enqueue_provider_diagnostic(
        &self,
        input: ProviderDiagnosticInput,
    ) -> AgentResult<Activity> {
        let mut config = self.config.clone();
        if let Some(provider) = input.provider {
            config.provider = provider;
        }
        if input.model.is_some() {
            config.model = input.model;
        }
        if input.disable_sandbox {
            config.sandbox = AgentSandboxConfig {
                mode: "none".into(),
            };
        }
        let runtime = AgentRuntime::new(
            Arc::new(config.command_provider_with_data_dir(&self.data_dir)),
            self.store.clone(),
        );
        let mut activity = runtime.create_chat_activity()?;
        activity.label = Some("provider diagnostic".into());
        self.store.save(&activity)?;
        let provider_debug_file =
            provider_diagnostic_debug_file(&self.data_dir, &activity, &config.provider)?;
        let queued = activity.clone();
        thread::spawn(move || {
            let _ = runtime.run_chat(
                activity,
                ChatInput {
                    repo_dir: input.repo_dir,
                    prompt: "Hi. Reply with exactly: OK".into(),
                    context: "Nitpick provider diagnostic.".into(),
                    disable_sandbox: input.disable_sandbox,
                    provider_timeout_ms: Some(PROVIDER_DIAGNOSTIC_TIMEOUT_MS),
                    provider_debug_file,
                },
            );
        });
        Ok(queued)
    }

    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::new(
            Arc::new(HostReviewProvider::new(
                self.provider.clone(),
                self.config.github_command.clone(),
            )),
            self.store.clone(),
        )
    }

    fn run_enqueued_review(&self, activity: Activity, input: ReviewInput) -> AgentResult<Activity> {
        self.runtime().run_review(activity, input)
    }

    fn sync_completed_enqueued_review(&self, result: &AgentResult<Activity>, input: &ReviewInput) {
        let github_sync_target = github_review_sync_target(input);
        if let Ok(activity) = result
            && activity.status == ActivityStatus::Completed
            && let Some(target) = github_sync_target.as_deref()
            && let Err(error) = self.sync_completed_github_review(activity, target, input)
        {
            tracing::warn!(
                activity_id = %activity.id,
                target,
                error = %error,
                "sync completed review artifacts failed"
            );
        }
    }

    fn has_active_review_for(&self, repository: &str, number: Option<u64>) -> AgentResult<bool> {
        Ok(self.list_activities()?.into_iter().any(|activity| {
            activity.kind == ActivityKind::Review
                && matches!(
                    activity.status,
                    ActivityStatus::Queued | ActivityStatus::Running
                )
                && active_review_matches(&activity, repository, number)
        }))
    }

    fn retry_review_input(
        &self,
        retry: &nitpick_agent_core::ReviewRetryMetadata,
    ) -> AgentResult<ReviewInput> {
        let number = retry.number.ok_or_else(|| {
            AgentError::invalid_input("retryable GitHub review is missing pull request number")
        })?;
        if retry.repository.split_once('/').is_none() {
            return Err(AgentError::invalid_input(format!(
                "invalid retry repository `{}`",
                retry.repository
            )));
        }
        let request = ReviewRequest {
            source: "github".into(),
            repository: retry.repository.clone(),
            number: Some(number),
            id: number.to_string(),
            head_sha: retry.head_sha.clone(),
        };
        let mut input = self.review_source.review_input(&request)?;
        input.source = retry.source.clone();
        input.review_mode = retry.review_mode.clone();
        input.force = retry.force;
        Ok(input)
    }
}

fn active_review_matches(activity: &Activity, repository: &str, number: Option<u64>) -> bool {
    activity
        .retry
        .as_ref()
        .and_then(|retry| retry.review.as_ref())
        .is_some_and(|review| review.repository == repository && review.number == number)
        || activity.label.as_deref().is_some_and(|label| {
            label
                == match number {
                    Some(number) => format!("review on {repository}#{number}"),
                    None => format!("review on {repository}"),
                }
        })
}

fn is_retryable_review_metadata(retry: &nitpick_agent_core::ReviewRetryMetadata) -> bool {
    retry.source == "github"
        && retry.number.is_some()
        && retry
            .repository
            .split_once('/')
            .is_some_and(|(owner, repo)| !owner.is_empty() && !repo.is_empty())
}

fn provider_attention(
    activities: &[Activity],
    provider: &AgentProviderKind,
) -> Option<nitpick_agent_core::HostAttention> {
    let mut classified = activities
        .iter()
        .filter_map(|activity| {
            let mut classified_activity = activity.clone();
            classified_activity
                .session
                .provider
                .get_or_insert_with(|| provider.clone());
            if provider_failure_resolved(&classified_activity) {
                return None;
            }
            nitpick_agent_core::classify_provider_failure(&classified_activity)
                .map(|classification| (activity.updated_at_unix, classification))
        })
        .collect::<Vec<_>>();
    classified.sort_by_key(|(updated_at_unix, classification)| {
        (
            provider_failure_priority(&classification.kind),
            std::cmp::Reverse(*updated_at_unix),
        )
    });
    let (_, classification) = classified.first()?;
    let retryable_activity_count = activities
        .iter()
        .filter(|activity| {
            activity.kind == ActivityKind::Review
                && activity.status == ActivityStatus::Error
                && activity
                    .retry
                    .as_ref()
                    .and_then(|retry| retry.review.as_ref())
                    .is_some_and(is_retryable_review_metadata)
                && !provider_failure_resolved(activity)
                && nitpick_agent_core::classify_provider_failure(activity)
                    .is_some_and(|candidate| candidate.kind == classification.kind)
        })
        .count();
    Some(nitpick_agent_core::HostAttention {
        kind: classification.kind.clone(),
        title: "provider needs attention".into(),
        detail: provider_attention_detail(classification),
        retryable_activity_count,
    })
}

fn provider_failure_resolved(activity: &Activity) -> bool {
    activity
        .retry
        .as_ref()
        .and_then(|retry| retry.resolved_by_activity.as_ref())
        .is_some()
}

fn provider_attention_detail(
    classification: &nitpick_agent_core::ProviderFailureClassification,
) -> String {
    match classification.suggested_action.as_deref() {
        Some(action) => format!(
            "{}: {} {}",
            classification.title, classification.detail, action
        ),
        None => format!("{}: {}", classification.title, classification.detail),
    }
}

fn provider_failure_priority(kind: &nitpick_agent_core::ProviderFailureKind) -> u8 {
    match kind {
        nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials => 0,
        nitpick_agent_core::ProviderFailureKind::SandboxPermissionDenied => 1,
        nitpick_agent_core::ProviderFailureKind::ProviderUnavailable => 2,
        nitpick_agent_core::ProviderFailureKind::UnknownProviderFailure => 3,
    }
}

fn provider_diagnostic_debug_file(
    data_dir: &Path,
    activity: &Activity,
    provider: &AgentProviderKind,
) -> AgentResult<Option<PathBuf>> {
    if provider != &AgentProviderKind::Claude {
        return Ok(None);
    }
    let dir = data_dir.join("logs").join("provider-debug");
    fs::create_dir_all(&dir).map_err(|error| {
        AgentError::io(
            format!("create provider debug log directory {}", dir.display()),
            error.to_string(),
        )
    })?;
    Ok(Some(dir.join(format!("{}.log", activity.id))))
}

fn github_review_sync_target(input: &ReviewInput) -> Option<String> {
    input
        .subject
        .number
        .map(|number| format!("{}#{}", input.subject.repository, number))
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
const PROVIDER_DIAGNOSTIC_TIMEOUT_MS: u64 = 30_000;
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
    pub review_extra_prompt_path: Option<PathBuf>,
    pub review_self_extra_prompt_path: Option<PathBuf>,
    pub review_requested_extra_prompt_path: Option<PathBuf>,
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
            review_extra_prompt_path: None,
            review_self_extra_prompt_path: None,
            review_requested_extra_prompt_path: None,
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
        let review_prompt_path = review_prompt_path(config_dir);
        let review_extra_prompt_path = parse_review_extra_prompt_path(
            "review extra prompt",
            reviews.extra_prompt_path.as_deref(),
        )?;
        let review_self_extra_prompt_path = parse_review_extra_prompt_path(
            "self-review extra prompt",
            reviews.self_review_extra_prompt_path.as_deref(),
        )?;
        let review_requested_extra_prompt_path = parse_review_extra_prompt_path(
            "requested-review extra prompt",
            reviews.requested_review_extra_prompt_path.as_deref(),
        )?;
        let sandbox = AgentSandboxConfig::from_mode(agent.sandbox)?;

        Ok(Self {
            provider,
            model,
            command,
            github_command,
            checkout_dir: None,
            max_concurrent_reviews,
            review_prompt_path,
            review_extra_prompt_path,
            review_self_extra_prompt_path,
            review_requested_extra_prompt_path,
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
        self.command_provider_with_data_dir(&default_data_dir())
    }

    pub fn command_provider_with_data_dir(&self, data_dir: &Path) -> CommandAgentProvider {
        let provider = match &self.command {
            Some(command) => {
                CommandAgentProvider::new(self.provider.clone(), self.model.clone(), command)
            }
            None => CommandAgentProvider::for_kind(self.provider.clone(), self.model.clone()),
        };
        provider.with_sandbox(
            self.sandbox
                .command_sandbox_config()
                .with_nono_profile_cache_dir(data_dir.join("nono"))
                .with_read_paths(self.review_prompt_sandbox_read_paths()),
        )
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
        if let Some(path) = &self.review_extra_prompt_path {
            append_prompt_file(&mut prompt, "Configured extra review prompt", path)?;
        }
        prompt.push_str("\n\n");
        prompt.push_str(review_mode_prompt(&input.review_mode));
        let mode_prompt_path = match input.review_mode {
            ReviewMode::Requested => &self.review_requested_extra_prompt_path,
            ReviewMode::SelfReview => &self.review_self_extra_prompt_path,
        };
        if let Some(path) = mode_prompt_path {
            let label = match input.review_mode {
                ReviewMode::Requested => "Configured requested-review extra prompt",
                ReviewMode::SelfReview => "Configured self-review extra prompt",
            };
            append_prompt_file(&mut prompt, label, path)?;
        }
        input.review_prompt = prompt;
        Ok(input)
    }

    fn review_prompt_sandbox_read_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.review_prompt_path.clone()];
        paths.extend(self.review_extra_prompt_path.iter().cloned());
        paths.extend(self.review_self_extra_prompt_path.iter().cloned());
        paths.extend(self.review_requested_extra_prompt_path.iter().cloned());
        paths
    }
}

fn review_mode_prompt(mode: &ReviewMode) -> &'static str {
    match mode {
        ReviewMode::Requested => {
            "Review mode: requested review.\nTreat this as feedback to another author. Prioritize correctness, maintainability, and actionable comments. Keep comments respectful and concise."
        }
        ReviewMode::SelfReview => {
            "Review mode: self-review.\nTreat this as a pre-submit pass by the author. Be direct about likely test failures, missing updates, accidental changes, and local-only assumptions."
        }
    }
}

fn append_prompt_file(prompt: &mut String, label: &str, path: &Path) -> AgentResult<()> {
    let instructions = fs::read_to_string(path).map_err(|error| {
        AgentError::config(format!(
            "failed to read {} {}: {error}",
            label.to_lowercase(),
            path.display()
        ))
    })?;
    let instructions = instructions.trim();
    if !instructions.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(label);
        prompt.push_str(":\n");
        prompt.push_str(instructions);
    }
    Ok(())
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

fn review_prompt_path(config_dir: Option<&Path>) -> PathBuf {
    resolve_config_path(PathBuf::from(DEFAULT_REVIEW_PROMPT_FILENAME), config_dir)
}

fn parse_review_extra_prompt_path(
    label: &str,
    raw_path: Option<&str>,
) -> AgentResult<Option<PathBuf>> {
    let path = raw_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    if let Some(path) = &path
        && !path.is_absolute()
    {
        return Err(AgentError::config(format!(
            "{label} path must be absolute: {}",
            path.display()
        )));
    }
    if let Some(path) = &path
        && !path.is_file()
    {
        return Err(AgentError::config(format!(
            "{label} path is not a file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn resolve_config_path(path: PathBuf, config_dir: Option<&Path>) -> PathBuf {
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
    extra_prompt_path: Option<String>,
    self_review_extra_prompt_path: Option<String>,
    requested_review_extra_prompt_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSandboxConfig {
    pub mode: String,
}

impl Default for AgentSandboxConfig {
    fn default() -> Self {
        Self {
            mode: "nono".into(),
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
        if !matches!(mode.as_str(), "nono" | "none") {
            return Err(AgentError::config(format!(
                "unsupported agent sandbox mode `{mode}`"
            )));
        }
        Ok(Self { mode })
    }

    fn command_sandbox_config(&self) -> CommandSandboxConfig {
        match self.mode.as_str() {
            "nono" => CommandSandboxConfig::nono(),
            _ => CommandSandboxConfig::unsandboxed(),
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
