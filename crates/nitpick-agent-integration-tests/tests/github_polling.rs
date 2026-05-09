use std::sync::Arc;

use nitpick_agent_core::FsProcessedReviewStore;
use nitpick_agent_core::ProcessedReviewStore;
use nitpick_agent_core::{ActivityKind, ActivityStatus, ActivityStore, FsActivityStore};
use nitpick_agent_host::{GitHubDiscoveryConfig, HostDaemon};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, TestHarness, github_auto_review_config,
    github_disabled_config, github_discovery_only_config, pull_request, review_request,
};

#[test]
fn github_polling_creates_local_review_and_marks_pr_head_processed() {
    let harness = TestHarness::new(
        github_auto_review_config(),
        Arc::new(StubDiscovery::new(vec![pull_request("sha-one")])),
    );

    let result = harness
        .daemon
        .poll_review_requests()
        .expect("poll succeeds");

    assert_eq!(result.discovered_count, 1);
    assert_eq!(result.enqueued_count, 1);
    let activities = harness.store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].kind, ActivityKind::Review);
    assert_eq!(activities[0].status, ActivityStatus::Completed);
    assert_eq!(
        harness
            .store
            .list_artifacts_for(&activities[0].id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        !harness
            .processed
            .needs_review(&review_request("sha-one"))
            .expect("processed state")
    );
    assert_eq!(
        harness.provider.reviewed_subjects(),
        ["stephanos/nitpick-agent#42"]
    );
    let reviewed_input = harness
        .provider
        .reviewed_inputs()
        .into_iter()
        .next()
        .expect("review input");
    assert_eq!(reviewed_input.subject.title, "Stub PR");
    assert_eq!(reviewed_input.subject.author, "stub-author");
    assert_eq!(reviewed_input.diff, "diff for sha-one");
}

#[test]
fn github_polling_tick_runs_one_due_poll_and_reports_status() {
    let harness = TestHarness::new(
        github_auto_review_config(),
        Arc::new(StubDiscovery::new(vec![pull_request("sha-one")])),
    );
    let poller = nitpick_agent_host::ReviewSourcePoller::new(harness.daemon.clone());

    let result = poller.tick().expect("tick");

    assert_eq!(result.discovered_count, 1);
    assert_eq!(result.enqueued_count, 1);
    let status = harness.daemon.status().expect("status");
    assert!(status.review_source_enabled);
    assert_eq!(status.review_source_last_poll_unix, Some(1_000));
    assert_eq!(
        status.review_source_last_poll_summary.as_deref(),
        Some("reviewed 1 of 1 PRs")
    );
}

#[test]
fn github_polling_skips_until_interval_passes_and_rereviews_changed_heads() {
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let harness = TestHarness::new(github_auto_review_config(), discovery.clone());

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("first poll")
            .enqueued_count,
        1
    );
    discovery.set_pull_requests(vec![pull_request("sha-two")]);

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("too early")
            .skipped_reason
            .as_deref(),
        Some("interval")
    );

    harness.clock.advance(300);

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("second poll")
            .enqueued_count,
        1
    );
    assert_eq!(harness.activity_count(), 2);
}

#[test]
fn github_polling_respects_disabled_and_discovery_only_config() {
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let disabled = TestHarness::new(github_disabled_config(), discovery.clone());

    let disabled_result = disabled
        .daemon
        .poll_review_requests()
        .expect("disabled poll");

    assert_eq!(disabled_result.skipped_reason.as_deref(), Some("disabled"));
    assert_eq!(discovery.calls(), 0);
    assert_eq!(disabled.provider.reviewed_subjects().len(), 0);
    assert_eq!(disabled.activity_count(), 0);

    let discovery_only = TestHarness::new(github_discovery_only_config(), discovery.clone());

    let discovery_only_result = discovery_only
        .daemon
        .poll_review_requests()
        .expect("discovery-only poll");

    assert_eq!(discovery_only_result.discovered_count, 1);
    assert_eq!(discovery_only_result.enqueued_count, 0);
    assert_eq!(discovery.calls(), 1);
    assert!(discovery_only.provider.reviewed_subjects().is_empty());
    assert!(
        discovery_only
            .processed
            .needs_review(&review_request("sha-one"))
            .expect("not marked processed")
    );
}

#[test]
fn github_polling_skips_already_processed_prs_after_store_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store_dir = temp.path().join("store");
    let processed_dir = temp.path().join("processed-reviews");
    let discovery = Arc::new(StubDiscovery::new(vec![pull_request("sha-one")]));
    let provider = Arc::new(RecordingProvider::default());
    let clock = Arc::new(ManualClock::new(1_000));
    let store = Arc::new(FsActivityStore::new(&store_dir).expect("store"));
    let processed = Arc::new(FsProcessedReviewStore::new(&processed_dir).expect("processed"));
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_auto_review_config(),
        processed.clone(),
        provider.clone(),
        discovery.clone(),
        clock.clone(),
    );
    assert_eq!(
        daemon
            .poll_review_requests()
            .expect("first poll")
            .enqueued_count,
        1
    );
    drop(daemon);
    drop(store);
    drop(processed);

    let reopened_store = Arc::new(FsActivityStore::new(&store_dir).expect("reopened store"));
    let reopened_processed =
        Arc::new(FsProcessedReviewStore::new(&processed_dir).expect("reopened processed"));
    let reopened_daemon = HostDaemon::with_dependencies(
        reopened_store.clone(),
        github_auto_review_config(),
        reopened_processed,
        Arc::new(RecordingProvider::default()),
        discovery,
        clock,
    );

    let result = reopened_daemon.poll_review_requests().expect("reopen poll");

    assert_eq!(result.discovered_count, 0);
    assert_eq!(result.enqueued_count, 0);
    assert_eq!(reopened_store.list().expect("activities").len(), 1);
}

#[test]
fn github_polling_does_not_mark_failed_reviews_processed() {
    let harness = TestHarness::new(
        github_auto_review_config(),
        Arc::new(StubDiscovery::new(vec![pull_request("sha-one")])),
    );
    harness.provider.fail_reviews("provider failed");

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("poll")
            .enqueued_count,
        0
    );

    let activities = harness.store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].status, ActivityStatus::Error);
    assert!(
        harness
            .processed
            .needs_review(&review_request("sha-one"))
            .expect("failed review not processed")
    );
}

#[test]
fn github_polling_reviews_multiple_prs_and_only_rereviews_changed_heads() {
    let first = pull_request("sha-one");
    let second = nitpick_agent_github::DiscoveredPullRequest {
        owner: "stephanos".into(),
        repo: "nitpick-agent".into(),
        number: 43,
        head_sha: "sha-two".into(),
    };
    let discovery = Arc::new(StubDiscovery::new(vec![first.clone(), second.clone()]));
    let harness = TestHarness::new(github_auto_review_config(), discovery.clone());

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("first poll")
            .enqueued_count,
        2
    );
    assert_eq!(harness.activity_count(), 2);
    assert!(
        !harness
            .processed
            .needs_review(&first.clone().into())
            .expect("first processed")
    );
    assert!(
        !harness
            .processed
            .needs_review(&second.clone().into())
            .expect("second processed")
    );

    let changed_second = nitpick_agent_github::DiscoveredPullRequest {
        head_sha: "sha-three".into(),
        ..second
    };
    discovery.set_pull_requests(vec![first, changed_second]);
    harness.clock.advance(300);

    assert_eq!(
        harness
            .daemon
            .poll_review_requests()
            .expect("second poll")
            .enqueued_count,
        1
    );
    assert_eq!(harness.activity_count(), 3);
}

#[test]
fn github_polling_filters_discovered_prs_with_allowlist_and_denylist() {
    let allowed = pull_request("sha-one");
    let denied_by_allowlist = nitpick_agent_github::DiscoveredPullRequest {
        owner: "other".into(),
        repo: "repo".into(),
        number: 7,
        head_sha: "sha-two".into(),
    };
    let denied_by_denylist = nitpick_agent_github::DiscoveredPullRequest {
        owner: "stephanos".into(),
        repo: "archive-old".into(),
        number: 8,
        head_sha: "sha-three".into(),
    };
    let discovery = Arc::new(StubDiscovery::new(vec![
        allowed.clone(),
        denied_by_allowlist,
        denied_by_denylist,
    ]));
    let harness = TestHarness::new(
        nitpick_agent_host::AgentConfig {
            github_discovery: GitHubDiscoveryConfig {
                allowlist: vec!["stephanos/*".into()],
                denylist: vec!["*/archive-*".into()],
                ..github_auto_review_config().github_discovery
            },
            ..nitpick_agent_host::AgentConfig::default()
        },
        discovery,
    );

    let result = harness
        .daemon
        .poll_review_requests()
        .expect("filtered poll");

    assert_eq!(result.discovered_count, 1);
    assert_eq!(result.enqueued_count, 1);
    assert_eq!(
        harness.provider.reviewed_subjects(),
        ["stephanos/nitpick-agent#42"]
    );
    assert!(
        !harness
            .processed
            .needs_review(&allowed.into())
            .expect("processed")
    );
}
