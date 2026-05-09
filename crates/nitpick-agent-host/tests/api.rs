use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use nitpick_agent_core::{
    ActivityId, ActivityKind, ActivityStatus, ActivityStore, AgentProvider, AgentProviderKind,
    AgentResult, AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState, ChatInput,
    MemoryActivityStore, ProcessedReviewStore, ReviewInput, ReviewOutput, ReviewRequest,
    ReviewSubject,
};
use nitpick_agent_host::{AgentConfig, HostDaemon, api_router};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt};
use tower::ServiceExt;

#[tokio::test]
async fn status_endpoint_reports_local_store_counts() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["activity_count"], 1);
    assert_eq!(body["running_activity_count"], 0);
    assert_eq!(body["completed_activity_count"], 0);
    assert_eq!(body["error_activity_count"], 0);
    assert_eq!(body["artifact_count"], 1);
    assert_eq!(body["local_only_artifact_count"], 1);
    assert_eq!(body["pending_sync_artifact_count"], 0);
    assert_eq!(body["provider"], "claude");
}

#[tokio::test]
async fn activity_artifacts_endpoint_returns_local_artifacts() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.to_string();
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/activities/{activity_id}/artifacts"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body.as_array().expect("array").len(), 1);
    assert_eq!(body[0]["kind"], "ReviewSummary");
    assert_eq!(body[0]["sync_state"], "LocalOnly");
}

#[tokio::test]
async fn missing_activity_returns_not_found() {
    let app = api_router(HostDaemon::new(Arc::new(MemoryActivityStore::default())));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/activities/missing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_endpoint_returns_local_artifact() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.to_string();
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/artifacts/{artifact_id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["id"], artifact_id);
    assert_eq!(body["sync_state"], "LocalOnly");
}

#[tokio::test]
async fn artifact_sync_state_endpoint_marks_artifact_pending() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store.clone()));

    let response = app
        .oneshot(json_request(
            &format!("/artifacts/{artifact_id}/sync-state"),
            &ArtifactSyncState::Pending {
                destination: "github".into(),
            },
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["sync_state"]["Pending"]["destination"], "github");
    assert_eq!(
        store
            .get_artifact(&artifact_id)
            .expect("artifact")
            .sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into()
        }
    );
}

#[tokio::test]
async fn artifact_sync_endpoint_uses_github_dry_run_destination() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store.clone()));

    let response = app
        .oneshot(json_request(
            &format!("/artifacts/{artifact_id}/sync"),
            &serde_json::json!({ "destination": "github" }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        store
            .get_artifact(&artifact_id)
            .expect("artifact")
            .sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into()
        }
    );
}

#[tokio::test]
async fn artifact_sync_endpoint_rejects_unknown_destination() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(json_request(
            &format!("/artifacts/{artifact_id}/sync"),
            &serde_json::json!({ "destination": "slack" }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await;
    assert_eq!(body["error"], "unknown sync destination `slack`");
}

#[tokio::test]
async fn artifact_sync_endpoint_posts_to_github_when_target_is_provided() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncat >/dev/null\nprintf 'https://github.com/acme/platform/pull/42#issuecomment-99\\n'\n",
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    let app = api_router(HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            provider: AgentProviderKind::Claude,
            model: None,
            command: None,
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(json_request(
            &format!("/artifacts/{artifact_id}/sync"),
            &serde_json::json!({
                "destination": "github",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        store
            .get_artifact(&artifact_id)
            .expect("artifact")
            .sync_state,
        ArtifactSyncState::Synced {
            destination: "github".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#issuecomment-99".into())
        }
    );
}

#[tokio::test]
async fn pending_sync_endpoint_lists_pending_artifacts_for_destination() {
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let pending = store
        .create_artifact(
            activity.id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("pending".into()),
        )
        .expect("pending artifact");
    let local = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local".into()),
        )
        .expect("local artifact");
    store
        .save_artifacts(&[pending.clone(), local])
        .expect("save artifacts");
    store
        .update_artifact_sync_state(
            &pending.id,
            ArtifactSyncState::Pending {
                destination: "github".into(),
            },
        )
        .expect("mark pending");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sync/pending?destination=github")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let artifacts = body.as_array().expect("artifact array");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["id"], pending.id.to_string());
}

#[tokio::test]
async fn github_review_requests_endpoint_lists_requested_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2 $3" = "search prs user-review-requested:@me" ]; then
  printf '[{"repository":{"nameWithOwner":"acme/platform"},"number":42}]'
  exit 0
fi
if [ "$1 $2" = "pr view" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let app = api_router(HostDaemon::with_config(
        Arc::new(MemoryActivityStore::default()),
        AgentConfig {
            provider: AgentProviderKind::Claude,
            model: None,
            command: None,
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/github/review-requests")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body[0]["owner"], "acme");
    assert_eq!(body[0]["repo"], "platform");
    assert_eq!(body[0]["number"], 42);
    assert_eq!(body[0]["head_sha"], "abc123");
}

#[tokio::test]
async fn github_review_requests_endpoint_can_filter_processed_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2 $3" = "search prs user-review-requested:@me" ]; then
  printf '[{"repository":{"nameWithOwner":"acme/platform"},"number":42},{"repository":{"nameWithOwner":"octo/widgets"},"number":7}]'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "42" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "7" ]; then
  printf '{"headRefOid":"def456"}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let processed = Arc::new(nitpick_agent_core::MemoryProcessedReviewStore::default());
    ProcessedReviewStore::mark_processed_at(
        processed.as_ref(),
        &ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: "abc123".into(),
        },
        Some("activity-1".into()),
        1_000,
    )
    .expect("mark processed");
    let app = api_router(HostDaemon::with_config_and_processed_reviews(
        Arc::new(MemoryActivityStore::default()),
        AgentConfig {
            provider: AgentProviderKind::Claude,
            model: None,
            command: None,
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
        processed,
    ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/github/review-requests?filter=new")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let requests = body.as_array().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["owner"], "octo");
    assert_eq!(requests[0]["head_sha"], "def456");
}

#[tokio::test]
async fn review_endpoint_runs_provider_and_stores_artifacts() {
    let store = Arc::new(MemoryActivityStore::default());
    let app = api_router(HostDaemon::with_provider(
        store.clone(),
        Arc::new(FakeProvider),
    ));

    let response = app
        .oneshot(json_request(
            "/reviews",
            &ReviewInput {
                subject: ReviewSubject {
                    repository: "acme/platform".into(),
                    ..ReviewSubject::default()
                },
                ..ReviewInput::default()
            },
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["status"], "Running");
    let activity_id = body["id"].as_str().expect("activity id");
    wait_for_completed_activity(&store, activity_id);
    assert_eq!(
        store
            .list_artifacts_for(&ActivityId::new(activity_id))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn chat_endpoint_runs_provider_and_stores_artifact() {
    let store = Arc::new(MemoryActivityStore::default());
    let app = api_router(HostDaemon::with_provider(
        store.clone(),
        Arc::new(FakeProvider),
    ));

    let response = app
        .oneshot(json_request(
            "/chats",
            &ChatInput {
                prompt: "what changed?".into(),
                ..ChatInput::default()
            },
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["status"], "Running");
    let activity_id = body["id"].as_str().expect("activity id");
    wait_for_completed_activity(&store, activity_id);
    let activity = store.get(&ActivityId::new(activity_id)).expect("activity");
    assert_eq!(activity.status, ActivityStatus::Completed);
    assert_eq!(store.list_artifacts_for(&activity.id).unwrap().len(), 1);
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    serde_json::from_slice(&bytes).expect("json body")
}

fn json_request<T: serde::Serialize>(uri: &str, body: &T) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).expect("json body")))
        .expect("request")
}

fn wait_for_completed_activity(store: &MemoryActivityStore, activity_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let activity = store
            .get(&ActivityId::new(activity_id))
            .expect("activity exists");
        if activity.status == ActivityStatus::Completed {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "activity {activity_id} did not complete"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

struct FakeProvider;

impl AgentProvider for FakeProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        Ok(ReviewOutput {
            summary: "looks good".into(),
            ..ReviewOutput::default()
        })
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok("chat response".into())
    }
}
