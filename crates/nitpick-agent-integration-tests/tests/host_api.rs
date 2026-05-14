use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use nitpick_agent_client::HostClient;
use nitpick_agent_core::FsProcessedReviewStore;
use nitpick_agent_core::{
    ActivityKind, ActivityStatus, ActivityStore, ArtifactSyncState, FsActivityStore, ReviewRequest,
    SessionStatus,
};
use nitpick_agent_host::{AgentConfig, HostDaemon, api_router};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, github_auto_review_config, pull_request,
};

#[tokio::test(flavor = "multi_thread")]
async fn host_api_exposes_discovery_polling_activities_artifacts_and_pending_sync() {
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_auto_review_config(),
        processed,
        Arc::new(RecordingProvider::default()),
        discovery,
        Arc::new(ManualClock::new(1_000)),
    );
    let client = serve_host(daemon.clone()).await;

    let requests = client.review_requests(true).expect("new review requests");
    assert_eq!(
        requests,
        vec![ReviewRequest {
            source: "github".into(),
            repository: "stephanos/nitpick-agent".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: "sha-one".into(),
        }]
    );

    assert_eq!(
        daemon.poll_review_requests().expect("poll").enqueued_count,
        1
    );

    let activities = client.activities().expect("activities");
    assert_eq!(activities.len(), 2);
    assert!(
        activities
            .iter()
            .any(|activity| activity.kind == ActivityKind::Discovery
                && activity.status == ActivityStatus::Completed)
    );
    let activity = activities
        .iter()
        .find(|activity| activity.kind == ActivityKind::Review)
        .expect("review activity");
    assert_eq!(activity.status, ActivityStatus::Completed);
    let artifacts = client
        .activity_artifacts(&activity.id.to_string())
        .expect("activity artifacts");
    assert_eq!(artifacts.len(), 1);

    client
        .sync_artifact(&artifacts[0].id.to_string(), "github", None)
        .expect("mark pending");
    let pending = client
        .pending_sync_artifacts(Some("github"))
        .expect("pending sync");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, artifacts[0].id);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_artifact_sync_lifecycle_marks_pending_then_synced() {
    let temp = tempfile::tempdir().expect("tempdir");
    let gh = fake_gh_command(
        &temp,
        "https://github.com/stephanos/nitpick-agent/pull/42#issuecomment-1",
    );
    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let config = AgentConfig {
        github_command: Some(gh.display().to_string()),
        ..github_auto_review_config()
    };
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        config,
        processed,
        Arc::new(RecordingProvider::default()),
        Arc::new(StubDiscovery::new(vec![pull_request("sha-one")])),
        Arc::new(ManualClock::new(1_000)),
    );
    let client = serve_host(daemon.clone()).await;

    daemon.poll_review_requests().expect("poll");
    let activity = client
        .activities()
        .expect("activities")
        .into_iter()
        .find(|activity| activity.kind == ActivityKind::Review)
        .expect("review activity");
    let artifact = client
        .activity_artifacts(&activity.id.to_string())
        .expect("artifacts")
        .into_iter()
        .next()
        .expect("artifact");

    let pending = client
        .sync_artifact(&artifact.id.to_string(), "github", None)
        .expect("pending");
    assert_eq!(
        pending.sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
        }
    );
    assert_eq!(
        client
            .pending_sync_artifacts(Some("github"))
            .expect("pending list")
            .len(),
        1
    );

    let synced = client
        .sync_artifact(
            &artifact.id.to_string(),
            "github",
            Some("stephanos/nitpick-agent#42"),
        )
        .expect("synced");
    assert_eq!(
        synced.sync_state,
        ArtifactSyncState::Synced {
            destination: "github".into(),
            remote_id: Some(
                "https://github.com/stephanos/nitpick-agent/pull/42#issuecomment-1".into()
            )
        }
    );
}

#[test]
fn filesystem_daemon_recovery_marks_interrupted_activities_as_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store_dir = temp.path().join("store");
    let store = FsActivityStore::new(&store_dir).expect("store");
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    activity.session.status = SessionStatus::Running;
    store.save(&activity).expect("save running activity");
    drop(store);

    let reopened = Arc::new(FsActivityStore::new(&store_dir).expect("reopen store"));
    let daemon = HostDaemon::new(reopened.clone());

    assert_eq!(
        daemon
            .recover_interrupted_activities()
            .expect("recover interrupted"),
        1
    );
    let recovered = reopened.get(&activity.id).expect("recovered activity");
    assert_eq!(recovered.status, ActivityStatus::Error);
    assert_eq!(
        recovered.error.as_deref(),
        Some("host restarted before activity completed")
    );
}

async fn serve_host(daemon: HostDaemon) -> HostClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, api_router(daemon))
            .await
            .expect("serve host");
    });
    HostClient::new(addr.to_string())
}

fn fake_gh_command(temp: &tempfile::TempDir, remote_id: &str) -> std::path::PathBuf {
    let gh = temp.path().join("gh");
    fs::write(
        &gh,
        format!("#!/bin/sh\ncat >/dev/null\nprintf '{}\\n'\n", remote_id),
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    gh
}
