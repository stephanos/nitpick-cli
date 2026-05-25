use axum::{
    Json, Router,
    extract::{Path as PathParam, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ActivityStatus, AgentError, AgentResult, Artifact,
    ArtifactId, ArtifactSyncState, ChatInput, CleanupCheckoutsResult, HostStatus,
    LocalStateResetResult, ReviewInput, ReviewRequest,
};
use nitpick_agent_github::DiscoveredPullRequest;
use serde::Deserialize;

use crate::{HostDaemon, github_pull_request_from_review_request};

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
        .route("/system/reset", post(reset_local_state))
        .route("/reviews", post(review))
        .route("/chats", post(chat))
        .with_state(daemon)
}

async fn status(State(daemon): State<HostDaemon>) -> Result<Json<HostStatus>, ApiError> {
    Ok(Json(daemon.status()?))
}

async fn activities(
    State(daemon): State<HostDaemon>,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<Vec<Activity>>, ApiError> {
    let mut activities = daemon.list_activities()?;
    if let Some(kind) = query.kind.as_deref() {
        let kind = parse_activity_kind(kind)?;
        activities.retain(|activity| activity.kind == kind);
    }
    if let Some(status) = query.status.as_deref() {
        let status = ActivityStatusFilter::parse(status)?;
        activities.retain(|activity| status.matches(&activity.status));
    }
    activities.sort_by(|lhs, rhs| {
        rhs.updated_at_unix
            .cmp(&lhs.updated_at_unix)
            .then_with(|| rhs.id.cmp(&lhs.id))
    });
    if let Some(limit) = query.limit {
        activities.truncate(limit.max(1));
    }
    Ok(Json(activities))
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
struct ActivitiesQuery {
    kind: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
}

fn parse_activity_kind(kind: &str) -> AgentResult<ActivityKind> {
    match kind {
        "review" => Ok(ActivityKind::Review),
        "chat" => Ok(ActivityKind::Chat),
        "maintenance" => Ok(ActivityKind::Maintenance),
        "discovery" => Ok(ActivityKind::Discovery),
        _ => Err(AgentError::invalid_input(format!(
            "unknown activity kind `{kind}`"
        ))),
    }
}

enum ActivityStatusFilter {
    Active,
    History,
    Any,
    Exact(ActivityStatus),
}

impl ActivityStatusFilter {
    fn parse(status: &str) -> AgentResult<Self> {
        match status {
            "active" => Ok(Self::Active),
            "history" => Ok(Self::History),
            "any" => Ok(Self::Any),
            "queued" => Ok(Self::Exact(ActivityStatus::Queued)),
            "running" => Ok(Self::Exact(ActivityStatus::Running)),
            "completed" => Ok(Self::Exact(ActivityStatus::Completed)),
            "error" => Ok(Self::Exact(ActivityStatus::Error)),
            "cancelled" => Ok(Self::Exact(ActivityStatus::Cancelled)),
            _ => Err(AgentError::invalid_input(format!(
                "unknown activity status `{status}`"
            ))),
        }
    }

    fn matches(&self, status: &ActivityStatus) -> bool {
        match self {
            Self::Active => matches!(status, ActivityStatus::Queued | ActivityStatus::Running),
            Self::History => matches!(
                status,
                ActivityStatus::Completed | ActivityStatus::Error | ActivityStatus::Cancelled
            ),
            Self::Any => true,
            Self::Exact(expected) => status == expected,
        }
    }
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

async fn reset_local_state(
    State(daemon): State<HostDaemon>,
    Json(input): Json<ResetLocalStateInput>,
) -> Result<Json<LocalStateResetResult>, ApiError> {
    Ok(Json(daemon.reset_local_state(input.force)?))
}

#[derive(Deserialize)]
struct ArtifactSyncInput {
    destination: String,
    target: Option<String>,
}

#[derive(Deserialize)]
struct ResetLocalStateInput {
    force: bool,
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

struct ApiError(AgentError);

impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
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
