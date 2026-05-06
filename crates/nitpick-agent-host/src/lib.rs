use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use axum::{
    Json, Router,
    extract::{Path as PathParam, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use nitpick_agent_core::{
    Activity, ActivityId, ActivityStatus, ActivityStore, AgentError, AgentProvider,
    AgentProviderKind, AgentResult, AgentRuntime, Artifact, ArtifactId, ArtifactSyncDestination,
    ArtifactSyncState, ChatInput, Clock, CommandAgentProvider, ReviewInput, ReviewSubject,
    SessionStatus, SystemClock,
};
use nitpick_agent_github::{
    DiscoveredPullRequest, GitHubCliDiscovery, GitHubCliSyncDestination,
    GitHubDryRunSyncDestination, MemoryProcessedReviewStore, ProcessedReviewStore, PullRequestRef,
    ReviewRequestDiscovery,
};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HostDaemon {
    config: AgentConfig,
    store: Arc<dyn ActivityStore>,
    processed_reviews: Arc<dyn ProcessedReviewStore>,
    provider: Arc<dyn AgentProvider>,
    discovery: Arc<dyn ReviewRequestDiscovery>,
    clock: Arc<dyn Clock>,
    last_github_poll_unix: Arc<Mutex<Option<u64>>>,
}

impl HostDaemon {
    pub fn new(store: Arc<dyn ActivityStore>) -> Self {
        Self::with_config(store, AgentConfig::default())
    }

    pub fn with_config(store: Arc<dyn ActivityStore>, config: AgentConfig) -> Self {
        let provider = config.provider();
        let discovery = config.discovery();
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            discovery,
            clock: Arc::new(SystemClock),
            last_github_poll_unix: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_config_and_processed_reviews(
        store: Arc<dyn ActivityStore>,
        config: AgentConfig,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
    ) -> Self {
        let provider = config.provider();
        let discovery = config.discovery();
        Self {
            config,
            store,
            processed_reviews,
            provider,
            discovery,
            clock: Arc::new(SystemClock),
            last_github_poll_unix: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_provider(store: Arc<dyn ActivityStore>, provider: Arc<dyn AgentProvider>) -> Self {
        let config = AgentConfig::default();
        let discovery = config.discovery();
        Self {
            config,
            store,
            processed_reviews: Arc::new(MemoryProcessedReviewStore::default()),
            provider,
            discovery,
            clock: Arc::new(SystemClock),
            last_github_poll_unix: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_dependencies(
        store: Arc<dyn ActivityStore>,
        config: AgentConfig,
        processed_reviews: Arc<dyn ProcessedReviewStore>,
        provider: Arc<dyn AgentProvider>,
        discovery: Arc<dyn ReviewRequestDiscovery>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            config,
            store,
            processed_reviews,
            provider,
            discovery,
            clock,
            last_github_poll_unix: Arc::new(Mutex::new(None)),
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
            self.store.save(&activity)?;
            recovered_count += 1;
        }

        Ok(recovered_count)
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
        let sync_state = match destination {
            "github" => match target {
                Some(target) => {
                    let target = target.parse::<PullRequestRef>().map_err(|error| {
                        AgentError::new(format!("invalid GitHub sync target: {error}"))
                    })?;
                    GitHubCliSyncDestination::new(
                        target,
                        self.config.github_command.as_deref().unwrap_or("gh"),
                    )
                    .sync(&artifact)?
                    .sync_state
                }
                None => GitHubDryRunSyncDestination.sync(&artifact)?.sync_state,
            },
            destination => {
                return Err(AgentError::new(format!(
                    "unknown sync destination `{destination}`"
                )));
            }
        };
        Ok(Some(self.store.update_artifact_sync_state(id, sync_state)?))
    }

    pub fn discover_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discovery.requested_reviews()
    }

    pub fn discover_new_github_review_requests(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        self.discover_github_review_requests()?
            .into_iter()
            .filter_map(
                |pull_request| match self.processed_reviews.needs_review(&pull_request) {
                    Ok(true) => Some(Ok(pull_request)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect()
    }

    pub fn poll_github_review_requests(&self) -> AgentResult<GitHubReviewPollResult> {
        if !self.config.github_discovery.enabled {
            return Ok(GitHubReviewPollResult::skipped("disabled"));
        }

        let now = self.clock.now_unix();
        {
            let mut last_poll = self
                .last_github_poll_unix
                .lock()
                .map_err(|_| AgentError::new("github poll state lock poisoned"))?;
            if let Some(last_poll) = *last_poll
                && now.saturating_sub(last_poll) < self.config.github_discovery.interval_seconds
            {
                return Ok(GitHubReviewPollResult::skipped("interval"));
            }
            *last_poll = Some(now);
        }

        let pull_requests = self.discover_new_github_review_requests()?;
        let discovered_count = pull_requests.len();
        if !self.config.github_discovery.auto_review {
            return Ok(GitHubReviewPollResult {
                discovered_count,
                enqueued_count: 0,
                skipped_reason: None,
            });
        }

        let mut enqueued_count = 0;
        for pull_request in pull_requests {
            let activity = self.start_review(review_input_for_pull_request(&pull_request))?;
            if activity.status != ActivityStatus::Completed {
                continue;
            }
            self.processed_reviews.mark_processed_at(
                &pull_request,
                Some(activity.id.to_string()),
                now,
            )?;
            enqueued_count += 1;
        }

        Ok(GitHubReviewPollResult {
            discovered_count,
            enqueued_count,
            skipped_reason: None,
        })
    }

    pub fn start_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        self.runtime().start_review(input)
    }

    pub fn enqueue_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let runtime = self.runtime();
        let activity = runtime.create_review_activity()?;
        let queued = activity.clone();
        thread::spawn(move || {
            let _ = runtime.run_review(activity, input);
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubReviewPollResult {
    pub discovered_count: usize,
    pub enqueued_count: usize,
    pub skipped_reason: Option<String>,
}

impl GitHubReviewPollResult {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            discovered_count: 0,
            enqueued_count: 0,
            skipped_reason: Some(reason.into()),
        }
    }
}

fn review_input_for_pull_request(pull_request: &DiscoveredPullRequest) -> ReviewInput {
    let repository = format!("{}/{}", pull_request.owner, pull_request.repo);
    ReviewInput {
        repo_dir: PathBuf::from("."),
        instructions: format!(
            "Review GitHub pull request {repository}#{} at head {}.",
            pull_request.number, pull_request.head_sha
        ),
        subject: ReviewSubject {
            repository,
            number: Some(pull_request.number),
            title: String::new(),
            author: String::new(),
        },
        diff: String::new(),
    }
}

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
}

pub fn api_router(daemon: HostDaemon) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/activities", get(activities))
        .route("/activities/{id}", get(activity))
        .route("/activities/{id}/artifacts", get(activity_artifacts))
        .route("/sync/pending", get(pending_sync_artifacts))
        .route("/github/review-requests", get(github_review_requests))
        .route("/artifacts/{id}", get(artifact))
        .route("/artifacts/{id}/sync-state", post(artifact_sync_state))
        .route("/artifacts/{id}/sync", post(artifact_sync))
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
    match query.filter.as_deref() {
        Some("new") => Ok(Json(daemon.discover_new_github_review_requests()?)),
        Some(filter) => {
            Err(AgentError::new(format!("unknown review request filter `{filter}`")).into())
        }
        None => Ok(Json(daemon.discover_github_review_requests()?)),
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub command: Option<String>,
    pub github_command: Option<String>,
    pub github_discovery: GitHubDiscoveryConfig,
}

impl AgentConfig {
    pub fn from_toml(input: &str) -> AgentResult<Self> {
        let raw = toml::from_str::<RawConfig>(input)
            .map_err(|error| nitpick_agent_core::AgentError::new(error.to_string()))?;
        let agent = raw.agent.unwrap_or_default();
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
        let github_command = agent
            .github_command
            .map(|command| command.trim().to_owned())
            .filter(|command| !command.is_empty());
        let github_discovery = raw
            .github
            .and_then(|github| github.discovery)
            .map(GitHubDiscoveryConfig::from_raw)
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            provider,
            model,
            command,
            github_command,
            github_discovery,
        })
    }

    pub fn load(path: impl AsRef<Path>) -> AgentResult<Self> {
        let path = path.as_ref();
        let input = fs::read_to_string(path).map_err(|error| {
            nitpick_agent_core::AgentError::new(format!(
                "failed to read config {}: {error}",
                path.display()
            ))
        })?;
        Self::from_toml(&input)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> AgentResult<Self> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(input) => Self::from_toml(&input),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(nitpick_agent_core::AgentError::new(format!(
                "failed to read config {}: {error}",
                path.display()
            ))),
        }
    }

    fn provider(&self) -> Arc<dyn AgentProvider> {
        match &self.command {
            Some(command) => Arc::new(CommandAgentProvider::new(
                self.provider.clone(),
                self.model.clone(),
                command,
            )),
            None => Arc::new(CommandAgentProvider::for_kind(
                self.provider.clone(),
                self.model.clone(),
            )),
        }
    }

    fn discovery(&self) -> Arc<dyn ReviewRequestDiscovery> {
        Arc::new(GitHubCliDiscovery::new(
            self.github_command.as_deref().unwrap_or("gh"),
        ))
    }
}

#[derive(Deserialize)]
struct RawConfig {
    agent: Option<RawAgentConfig>,
    github: Option<RawGitHubConfig>,
}

#[derive(Default, Deserialize)]
struct RawAgentConfig {
    provider: Option<String>,
    model: Option<String>,
    command: Option<String>,
    github_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubDiscoveryConfig {
    pub enabled: bool,
    pub auto_review: bool,
    pub interval_seconds: u64,
}

impl Default for GitHubDiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_review: false,
            interval_seconds: 300,
        }
    }
}

impl GitHubDiscoveryConfig {
    fn from_raw(raw: RawGitHubDiscoveryConfig) -> AgentResult<Self> {
        let default = Self::default();
        let interval_seconds = raw
            .interval_seconds
            .unwrap_or(default.interval_seconds)
            .max(1);
        Ok(Self {
            enabled: raw.enabled.unwrap_or(default.enabled),
            auto_review: raw.auto_review.unwrap_or(default.auto_review),
            interval_seconds,
        })
    }
}

#[derive(Deserialize)]
struct RawGitHubConfig {
    discovery: Option<RawGitHubDiscoveryConfig>,
}

#[derive(Deserialize)]
struct RawGitHubDiscoveryConfig {
    enabled: Option<bool>,
    auto_review: Option<bool>,
    interval_seconds: Option<u64>,
}
