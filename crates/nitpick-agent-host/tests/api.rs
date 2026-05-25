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
    ActivityId, ActivityKind, ActivityStatus, ActivityStore, AgentError, AgentProvider,
    AgentProviderKind, AgentResult, AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState,
    ChatInput, MemoryActivityStore, MemoryProcessedReviewStore, ProcessedReviewStore, ReviewInput,
    ReviewOutput, ReviewRequest, ReviewSource, ReviewSubject, SystemClock,
};
use nitpick_agent_host::{AgentConfig, GitHubDiscoveryConfig, HostDaemon, api_router};
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
    assert_eq!(body["open_review_count"], 0);
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
async fn activities_endpoint_filters_review_history_and_applies_limit() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut older = store.create(ActivityKind::Review).expect("older");
    older.status = ActivityStatus::Completed;
    older.updated_at_unix = 1_000;
    store.save(&older).expect("save older");
    let mut newer = store.create(ActivityKind::Review).expect("newer");
    newer.status = ActivityStatus::Error;
    newer.updated_at_unix = 2_000;
    store.save(&newer).expect("save newer");
    let mut active = store.create(ActivityKind::Review).expect("active");
    active.status = ActivityStatus::Running;
    active.updated_at_unix = 3_000;
    store.save(&active).expect("save active");
    let mut chat = store.create(ActivityKind::Chat).expect("chat");
    chat.status = ActivityStatus::Completed;
    chat.updated_at_unix = 4_000;
    store.save(&chat).expect("save chat");
    let app = api_router(HostDaemon::new(store));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/activities?kind=review&status=history&limit=1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let activities = body.as_array().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0]["id"], newer.id.to_string());
}

#[tokio::test]
async fn reset_endpoint_clears_local_state() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Completed;
    store.save(&activity).expect("save activity");
    let artifact = store
        .create_artifact(
            activity.id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local summary".into()),
        )
        .expect("artifact");
    store.save_artifacts(&[artifact]).expect("save artifact");
    let processed = Arc::new(MemoryProcessedReviewStore::default());
    ProcessedReviewStore::mark_processed_at(
        processed.as_ref(),
        &ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: "abc123".into(),
        },
        Some(activity.id.to_string()),
        1_000,
    )
    .expect("mark processed");
    let checkout_root = data_dir.path().join("checkouts");
    fs::create_dir_all(checkout_root.join("acme-platform-42")).expect("checkout dir");
    let log_path = data_dir.path().join("logs").join("daemon.log");
    fs::create_dir_all(log_path.parent().expect("log parent")).expect("log dir");
    fs::write(&log_path, "old log").expect("log");
    let app = api_router(
        HostDaemon::with_config_and_processed_reviews(
            store.clone(),
            AgentConfig::default(),
            processed.clone(),
        )
        .with_data_dir(data_dir.path()),
    );

    let response = app
        .oneshot(json_request(
            "/system/reset",
            &serde_json::json!({ "force": false }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["removed_activity_count"], 1);
    assert_eq!(body["removed_artifact_count"], 1);
    assert_eq!(body["removed_processed_review_count"], 1);
    assert_eq!(body["removed_checkout_count"], 1);
    assert_eq!(body["truncated_log"], true);
    assert!(store.list().expect("activities").is_empty());
    assert!(store.list_artifacts().expect("artifacts").is_empty());
    assert!(processed.list_processed().expect("processed").is_empty());
    assert!(checkout_root.exists());
    assert_eq!(fs::read_to_string(&log_path).expect("log"), "");
}

#[tokio::test]
async fn reset_endpoint_rejects_active_reviews_without_force() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    store.save(&activity).expect("save activity");
    let app = api_router(HostDaemon::new(store.clone()));

    let response = app
        .oneshot(json_request(
            "/system/reset",
            &serde_json::json!({ "force": false }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.list().expect("activities").len(), 1);
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
                remote_id: None,
                remote_url: None,
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
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
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
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
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

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
async fn artifact_sync_endpoint_posts_to_github_review_destination() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncat >/dev/null\nprintf 'https://github.com/acme/platform/pull/42#pullrequestreview-99\\n'\n",
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
                "destination": "github-review",
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
            destination: "github-review".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into())
        }
    );
}

#[tokio::test]
async fn activity_artifact_sync_endpoint_stages_pending_github_review_draft() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
cat > {payload}
printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
"#,
            commands = commands_file.display(),
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.clone();
    let summary = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("summary body".into()),
        )
        .expect("summary artifact");
    let comment = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "Prefer this.".into(),
            }),
        )
        .expect("comment artifact");
    store
        .save_artifacts(&[summary.clone(), comment.clone()])
        .expect("save artifacts");
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
            &format!("/activities/{activity_id}/artifact-sync"),
            &serde_json::json!({
                "destination": "github-review",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body.as_array().expect("array").len(), 2);
    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["body"], "summary body");
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 1);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["body"], "🤖 Prefer this.");
    assert_eq!(
        store.get_artifact(&summary.id).expect("summary").sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            )
        }
    );
    assert_eq!(
        store.get_artifact(&comment.id).expect("comment").sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            )
        }
    );
}

#[tokio::test]
async fn activity_artifact_sync_endpoint_marks_pending_artifacts_synced_after_manual_submission() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf '{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"COMMENT","commit_id":"abc123"}'
  exit 0
fi
if [ "$1" = "pr" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
cat >/dev/null
printf '{"id":100,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-100","state":"PENDING","commit_id":"abc123"}'
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.clone();
    let summary = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("summary body".into()),
        )
        .expect("summary artifact");
    store
        .save_artifacts(std::slice::from_ref(&summary))
        .expect("save artifacts");
    store
        .update_artifact_sync_state(
            &summary.id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
                ),
            },
        )
        .expect("mark pending");
    let app = api_router(HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(json_request(
            &format!("/activities/{activity_id}/artifact-sync"),
            &serde_json::json!({
                "destination": "github-review",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        store.get_artifact(&summary.id).expect("summary").sync_state,
        ArtifactSyncState::Synced {
            destination: "github-review".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into())
        }
    );
}

#[tokio::test]
async fn activity_artifact_sync_endpoint_propagates_ambiguous_pending_review_404() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf 'HTTP 404: Not Found' >&2
  exit 1
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.clone();
    let summary = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("summary body".into()),
        )
        .expect("summary artifact");
    store
        .save_artifacts(std::slice::from_ref(&summary))
        .expect("save artifacts");
    store
        .update_artifact_sync_state(
            &summary.id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
                ),
            },
        )
        .expect("mark pending");
    let app = api_router(HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(json_request(
            &format!("/activities/{activity_id}/artifact-sync"),
            &serde_json::json!({
                "destination": "github-review",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        store.get_artifact(&summary.id).expect("summary").sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            )
        }
    );
}

#[tokio::test]
async fn activity_artifact_sync_endpoint_refuses_new_inline_comments_when_pending_review_exists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf '{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}'
  exit 0
fi
if [ "$1" = "pr" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.clone();
    let summary = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("summary body".into()),
        )
        .expect("summary artifact");
    let comment = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "Prefer this.".into(),
            }),
        )
        .expect("comment artifact");
    store
        .save_artifacts(&[summary.clone(), comment.clone()])
        .expect("save artifacts");
    store
        .update_artifact_sync_state(
            &summary.id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
                ),
            },
        )
        .expect("mark pending");
    let app = api_router(HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(json_request(
            &format!("/activities/{activity_id}/artifact-sync"),
            &serde_json::json!({
                "destination": "github-review",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["error"]
            .as_str()
            .expect("error text")
            .contains("submit or clear the draft review")
    );
}

#[tokio::test]
async fn activity_artifact_sync_endpoint_does_not_update_pending_body_for_local_comments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  if [ "$3" = "--method" ]; then
    cat >/dev/null
    printf 'unexpected pending body update' >&2
    exit 1
  fi
  printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let activity = store.create(ActivityKind::Review).expect("activity");
    let activity_id = activity.id.clone();
    let pending_comment = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "Already staged.".into(),
            }),
        )
        .expect("pending comment artifact");
    let local_comment = store
        .create_artifact(
            activity_id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
                path: "src/main.rs".into(),
                line: 8,
                body: "New comment.".into(),
            }),
        )
        .expect("local comment artifact");
    store
        .save_artifacts(&[pending_comment.clone(), local_comment])
        .expect("save artifacts");
    store
        .update_artifact_sync_state(
            &pending_comment.id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
                ),
            },
        )
        .expect("mark pending");
    let app = api_router(HostDaemon::with_config(
        store,
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(json_request(
            &format!("/activities/{activity_id}/artifact-sync"),
            &serde_json::json!({
                "destination": "github-review",
                "target": "acme/platform#42"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api repos/acme/platform/pulls/42/reviews/99\n"
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
                remote_id: None,
                remote_url: None,
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
async fn cleanup_checkouts_endpoint_removes_closed_checkouts_and_records_activity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    fs::create_dir_all(checkout_root.join("acme/platform/pr-42/.git")).expect("closed checkout");
    fs::create_dir_all(checkout_root.join("octo/widgets/pr-7/.git")).expect("open checkout");
    fs::create_dir_all(checkout_root.join("temporalio/temporal/pr-99/.git"))
        .expect("denied checkout");
    let gh = dir.path().join("gh");
    let log = dir.path().join("gh.log");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
echo "$*" >> '{}'
if [ "$1 $2" = "pr view" ] && [ "$3" = "42" ]; then
  printf '{{"title":"Closed PR","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"closed-branch","state":"CLOSED","mergedAt":null}}'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "7" ]; then
  printf '{{"title":"Open PR","author":{{"login":"octo"}},"url":"https://github.com/octo/widgets/pull/7","headRefOid":"def456","headRefName":"open-branch","state":"OPEN","mergedAt":null}}'
  exit 0
fi
exit 1
"#,
            log.display()
        ),
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let store = Arc::new(MemoryActivityStore::default());
    let app = api_router(HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            checkout_dir: Some(checkout_root.display().to_string()),
            github_discovery: GitHubDiscoveryConfig {
                allowlist: vec!["acme/*".into(), "octo/*".into()],
                ..GitHubDiscoveryConfig::default()
            },
            ..AgentConfig::default()
        },
    ));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/maintenance/cleanup-checkouts")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["removed_count"], 1);
    assert_eq!(body["cleaned"][0], "acme/platform#42");
    assert!(!checkout_root.join("acme/platform/pr-42").exists());
    assert!(checkout_root.join("octo/widgets/pr-7").exists());
    assert!(checkout_root.join("temporalio/temporal/pr-99").exists());
    let commands = fs::read_to_string(log).expect("commands");
    assert!(commands.contains("--repo acme/platform"));
    assert!(commands.contains("--repo octo/widgets"));
    assert!(!commands.contains("temporalio/temporal"));
    let activities = store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(
        activities[0].label.as_deref(),
        Some("acme/platform#42 cleaned up")
    );
}

#[test]
fn review_source_poller_runs_checkout_cleanup_after_due_poll() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    fs::create_dir_all(checkout_root.join("acme/platform/pr-42/.git")).expect("closed checkout");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2" = "search prs" ]; then
  printf '[]'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "42" ]; then
  printf '{"title":"Closed PR","author":{"login":"stephan"},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"closed-branch","state":"CLOSED","mergedAt":null}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_config(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            checkout_dir: Some(checkout_root.display().to_string()),
            github_discovery: GitHubDiscoveryConfig {
                enabled: true,
                auto_review: false,
                interval_seconds: 300,
                ..GitHubDiscoveryConfig::default()
            },
            ..AgentConfig::default()
        },
    );

    let result = daemon.poll_review_requests().expect("tick");

    assert_eq!(result.discovered_count, 0);
    assert_eq!(result.enqueued_count, 0);
    assert_eq!(result.cleanup_removed_count, 1);
    assert!(!checkout_root.join("acme/platform/pr-42").exists());
    assert_eq!(
        store.list().expect("activities")[0].label.as_deref(),
        Some("acme/platform#42 cleaned up")
    );
    assert_eq!(
        daemon
            .status()
            .expect("status")
            .review_source_last_poll_summary
            .as_deref(),
        Some("reviewed 0 of 0 PRs, cleaned up 1 checkout(s)")
    );
}

#[tokio::test]
async fn review_requests_endpoint_rejects_unknown_filter() {
    let app = api_router(HostDaemon::new(Arc::new(MemoryActivityStore::default())));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/review-requests?filter=stale")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["error"], "unknown review request filter `stale`");
}

#[tokio::test]
async fn review_requests_endpoint_maps_rate_limit_to_429() {
    let app = api_router(HostDaemon::with_dependencies(
        Arc::new(MemoryActivityStore::default()),
        AgentConfig {
            github_discovery: GitHubDiscoveryConfig {
                enabled: true,
                ..GitHubDiscoveryConfig::default()
            },
            ..AgentConfig::default()
        },
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(FakeProvider),
        Arc::new(RateLimitedReviewSource),
        Arc::new(SystemClock),
    ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/review-requests")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("60")
    );
    let body = json_body(response).await;
    assert!(
        body["error"]
            .as_str()
            .expect("error text")
            .contains("rate limited")
    );
}

#[tokio::test]
async fn github_review_requests_endpoint_lists_requested_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2" = "search prs" ]; then
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
if [ "$1 $2" = "search prs" ]; then
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
if [ "$1" = "api" ]; then
  printf '[]'
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

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

struct FakeProvider;

impl AgentProvider for FakeProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        Ok(ReviewOutput {
            comments: vec![nitpick_agent_core::ReviewComment {
                path: "src/lib.rs".into(),
                line: 1,
                body: "looks good".into(),
            }],
        })
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok("chat response".into())
    }
}

struct RateLimitedReviewSource;

impl ReviewSource for RateLimitedReviewSource {
    fn name(&self) -> &'static str {
        "github"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Err(AgentError::github_rate_limited(
            "GitHub rate limited the request; retry after 60 seconds.",
            Some(60),
        ))
    }

    fn review_input(&self, _request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Err(AgentError::invalid_input("review input is not available"))
    }
}
