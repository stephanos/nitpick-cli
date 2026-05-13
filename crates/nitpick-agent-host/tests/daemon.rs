use std::sync::Arc;

use nitpick_agent_core::{
    ActivityKind, ActivityStatus, ActivityStore, AgentProviderKind, ArtifactContent, ArtifactKind,
    ArtifactSyncState, FsActivityStore, MemoryActivityStore, SessionStatus,
};
use nitpick_agent_github::PullRequestRef;
use nitpick_agent_host::{HostDaemon, HostStatus};

#[test]
fn host_status_reports_current_activity_count() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    store.save(&activity).expect("save running activity");
    let mut completed = store
        .create(ActivityKind::Chat)
        .expect("completed activity");
    completed.status = ActivityStatus::Completed;
    store.save(&completed).expect("save completed activity");
    let mut error = store.create(ActivityKind::Chat).expect("error activity");
    error.status = ActivityStatus::Error;
    store.save(&error).expect("save error activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local result".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    store
        .update_artifact_sync_state(
            &artifact_id,
            ArtifactSyncState::Pending {
                destination: "github".into(),
                remote_id: None,
                remote_url: None,
            },
        )
        .expect("mark pending");
    let daemon = HostDaemon::new(store);

    assert_eq!(
        daemon.status().expect("status"),
        HostStatus {
            activity_count: 3,
            running_activity_count: 1,
            completed_activity_count: 1,
            error_activity_count: 1,
            artifact_count: 1,
            local_only_artifact_count: 0,
            pending_sync_artifact_count: 1,
            provider: AgentProviderKind::Claude,
            model: None,
            review_source_name: "github".into(),
            review_source_enabled: false,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
        }
    );
}

#[test]
fn daemon_recovers_interrupted_filesystem_activities_after_store_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsActivityStore::new(dir.path()).expect("store");
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    activity.session.status = SessionStatus::Running;
    let activity_id = activity.id.clone();
    store.save(&activity).expect("save running activity");
    drop(store);

    let reopened = Arc::new(FsActivityStore::new(dir.path()).expect("reopen store"));
    let daemon = HostDaemon::new(reopened.clone());

    let recovered_count = daemon
        .recover_interrupted_activities()
        .expect("recover interrupted activities");

    let recovered = reopened.get(&activity_id).expect("recovered activity");
    assert_eq!(recovered_count, 1);
    assert_eq!(recovered.status, ActivityStatus::Error);
    assert_eq!(
        recovered.session.status,
        SessionStatus::Error("host restarted before activity completed".into())
    );
    assert_eq!(
        recovered.error,
        Some("host restarted before activity completed".into())
    );
}

#[test]
fn daemon_marks_interrupted_running_activities_as_errors() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    activity.session.status = SessionStatus::Running;
    let activity_id = activity.id.clone();
    store.save(&activity).expect("save running activity");
    let daemon = HostDaemon::new(store.clone());

    let recovered_count = daemon
        .recover_interrupted_activities()
        .expect("recover interrupted activities");

    let recovered = store.get(&activity_id).expect("recovered activity");
    assert_eq!(recovered_count, 1);
    assert_eq!(recovered.status, ActivityStatus::Error);
    assert_eq!(
        recovered.session.status,
        SessionStatus::Error("host restarted before activity completed".into())
    );
    assert_eq!(
        recovered.error,
        Some("host restarted before activity completed".into())
    );
}

#[test]
fn daemon_records_completed_checkout_cleanup_activity() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::new(store.clone());
    let pull_request = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let activity = daemon
        .record_checkout_cleanup_activity(&pull_request)
        .expect("cleanup activity");

    assert_eq!(activity.status, ActivityStatus::Completed);
    assert_eq!(
        activity.label.as_deref(),
        Some("acme/platform#42 cleaned up")
    );
    assert_eq!(
        store
            .get(&activity.id)
            .expect("persisted activity")
            .label
            .as_deref(),
        Some("acme/platform#42 cleaned up")
    );
}
