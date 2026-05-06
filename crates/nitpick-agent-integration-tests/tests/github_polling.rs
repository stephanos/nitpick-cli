use std::sync::Arc;

use nitpick_agent_core::{ActivityKind, ActivityStatus, ActivityStore, FsActivityStore};
use nitpick_agent_github::{FsProcessedReviewStore, ProcessedReviewStore};
use nitpick_agent_host::{AgentConfig, GitHubDiscoveryConfig, HostDaemon};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, pull_request,
};

#[test]
fn github_polling_creates_local_review_and_marks_pr_head_processed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let provider = Arc::new(RecordingProvider::default());
    let clock = Arc::new(ManualClock::new(1_000));
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_auto_review_config(),
        processed.clone(),
        provider.clone(),
        discovery,
        clock,
    );

    let result = daemon.poll_github_review_requests().expect("poll succeeds");

    assert_eq!(result.discovered_count, 1);
    assert_eq!(result.enqueued_count, 1);
    let activities = store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].kind, ActivityKind::Review);
    assert_eq!(activities[0].status, ActivityStatus::Completed);
    assert_eq!(
        store.list_artifacts_for(&activities[0].id).unwrap().len(),
        1
    );
    assert!(
        !processed
            .needs_review(&pull_request("sha-one"))
            .expect("processed state")
    );
    assert_eq!(provider.reviewed_subjects(), ["stephanos/nitpick-agent#42"]);
}

#[test]
fn github_polling_skips_until_interval_passes_and_rereviews_changed_heads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let provider = Arc::new(RecordingProvider::default());
    let clock = Arc::new(ManualClock::new(1_000));
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_auto_review_config(),
        processed,
        provider,
        discovery.clone(),
        clock.clone(),
    );

    assert_eq!(
        daemon
            .poll_github_review_requests()
            .expect("first poll")
            .enqueued_count,
        1
    );
    discovery.set_pull_requests(vec![pull_request("sha-two")]);

    assert_eq!(
        daemon
            .poll_github_review_requests()
            .expect("too early")
            .skipped_reason
            .as_deref(),
        Some("interval")
    );

    clock.advance(300);

    assert_eq!(
        daemon
            .poll_github_review_requests()
            .expect("second poll")
            .enqueued_count,
        1
    );
    assert_eq!(store.list().expect("activities").len(), 2);
}

fn github_auto_review_config() -> AgentConfig {
    AgentConfig {
        github_discovery: GitHubDiscoveryConfig {
            enabled: true,
            auto_review: true,
            interval_seconds: 300,
        },
        ..AgentConfig::default()
    }
}
