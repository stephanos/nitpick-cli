use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use nitpick_agent_core::{
    ActivityKind, ActivityStatus, ActivityStore, AgentError, AgentProvider, AgentProviderKind,
    AgentResult, AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState, ChatInput,
    FixedClock, FsActivityStore, HostStatus, MemoryActivityStore, MemoryProcessedReviewStore,
    ProviderReviewContext, ProviderRunContext, ReviewInput, ReviewMode, ReviewOutput,
    ReviewRequest, ReviewSource, SessionStatus,
};
use nitpick_agent_github::PullRequestRef;
use nitpick_agent_host::{AgentConfig, GitHubDiscoveryConfig, HostDaemon};

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
            queued_activity_count: 0,
            running_activity_count: 1,
            completed_activity_count: 1,
            error_activity_count: 1,
            open_review_count: 0,
            queued_review_count: 0,
            running_review_count: 1,
            completed_review_count: 0,
            error_review_count: 0,
            artifact_count: 1,
            local_only_artifact_count: 0,
            pending_sync_artifact_count: 1,
            provider: AgentProviderKind::Claude,
            model: None,
            review_source_name: "github".into(),
            review_source_enabled: false,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
            attention: None,
        }
    );
}

#[test]
fn host_status_reports_provider_auth_attention_for_failed_review() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Error;
    activity.error = Some("claude provider command failed with status exit status: 1".into());
    activity
        .session
        .messages
        .push(nitpick_agent_core::AgentMessage {
            role: "provider.stdout".into(),
            content: "Failed to authenticate. API Error: 401 Invalid authentication credentials"
                .into(),
        });
    store.save(&activity).expect("save activity");
    let daemon = HostDaemon::with_provider(store, Arc::new(NoFindingsProvider));

    let status = daemon.status().expect("status");

    let attention = status.attention.expect("attention");
    assert_eq!(
        attention.kind,
        nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials
    );
    assert_eq!(attention.title, "provider needs attention");
    assert!(attention.detail.contains("Claude authentication failed"));
    assert!(
        attention
            .detail
            .contains("Invalid authentication credentials")
    );
    assert!(attention.detail.contains("claude auth logout"));
    assert_eq!(attention.retryable_activity_count, 0);
}

#[test]
fn host_status_counts_retryable_provider_auth_reviews() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = provider_auth_failed_review(&store);
    activity.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&activity).expect("save activity");
    let daemon = HostDaemon::with_provider(store, Arc::new(NoFindingsProvider));

    let attention = daemon
        .status()
        .expect("status")
        .attention
        .expect("attention");

    assert_eq!(attention.retryable_activity_count, 1);
}

#[test]
fn host_status_retryable_count_excludes_invalid_retry_metadata() {
    let store = Arc::new(MemoryActivityStore::default());
    for retry in [
        review_retry_metadata("github", "acme/platform", Some(42), "abc123"),
        review_retry_metadata("local", "acme/platform", Some(43), "def456"),
        review_retry_metadata("github", "acme/platform", None, "ghi789"),
        review_retry_metadata("github", "not-a-repository", Some(44), "jkl012"),
    ] {
        let mut activity = provider_auth_failed_review(&store);
        activity.retry = Some(retry);
        store.save(&activity).expect("save activity");
    }
    let daemon = HostDaemon::with_provider(store, Arc::new(NoFindingsProvider));

    let attention = daemon
        .status()
        .expect("status")
        .attention
        .expect("attention");

    assert_eq!(attention.retryable_activity_count, 1);
}

#[test]
fn retry_failed_activities_requeues_retryable_provider_auth_review() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(RecordingRetryProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&failed).expect("save");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 1);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.activities.len(), 1);
    wait_until(|| {
        provider
            .reviewed
            .lock()
            .expect("reviewed lock")
            .contains(&"acme/platform".to_string())
    });
    let resolved = store.get(&failed.id).expect("resolved activity");
    assert_eq!(
        resolved.retry.expect("retry metadata").resolved_by_activity,
        Some(result.activities[0].clone())
    );
}

#[test]
fn provider_attention_clears_after_retry_succeeds() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(RecordingRetryProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&failed).expect("save");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 1);
    let retry_activity_id = result.activities[0].clone();
    wait_until(|| {
        store
            .get(&retry_activity_id)
            .expect("retry activity")
            .status
            == ActivityStatus::Completed
    });

    assert!(daemon.status().expect("status").attention.is_none());
    let second_retry = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("second retry");
    assert_eq!(second_retry.queued, 0);
    assert_eq!(second_retry.skipped, 0);
    assert!(second_retry.activities.is_empty());
}

#[test]
fn retry_failed_activities_skips_bad_metadata_and_queues_valid_activity() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(RecordingRetryProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut invalid = provider_auth_failed_review(&store);
    invalid.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        None,
        "abc123",
    ));
    store.save(&invalid).expect("save invalid");
    let mut valid = provider_auth_failed_review(&store);
    valid.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&valid).expect("save valid");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 1);
    assert_eq!(result.skipped, 1);
    assert_eq!(result.activities.len(), 1);
    wait_until(|| {
        provider
            .reviewed
            .lock()
            .expect("reviewed lock")
            .contains(&"acme/platform".to_string())
    });
}

#[test]
fn retry_failed_activities_skips_unsupported_retry_source() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(RecordingRetryProvider::default()),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata(
        "local",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&failed).expect("save");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 0);
    assert_eq!(result.skipped, 1);
    assert!(result.activities.is_empty());
}

#[test]
fn retry_failed_activities_skips_when_same_review_is_already_active() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(RecordingRetryProvider::default()),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&failed).expect("save failed");
    let mut active = store.create(ActivityKind::Review).expect("active");
    active.status = ActivityStatus::Queued;
    active.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&active).expect("save active");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 0);
    assert_eq!(result.skipped, 1);
    assert!(result.activities.is_empty());
}

#[test]
fn retry_failed_activities_force_supersedes_active_same_pr_review() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(RecordingRetryProvider::default()),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata_with_force(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
        true,
    ));
    store.save(&failed).expect("save failed");
    let mut active = store.create(ActivityKind::Review).expect("active");
    active.status = ActivityStatus::Queued;
    active.label = Some("review on acme/platform#42".into());
    let active_id = active.id.clone();
    store.save(&active).expect("save active");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 1);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.activities.len(), 1);
    let active = store.get(&active_id).expect("active activity");
    assert_eq!(active.status, ActivityStatus::Error);
    assert_eq!(active.error.as_deref(), Some("superseded by forced review"));
    let retry_activity = store
        .get(&result.activities[0])
        .expect("retry activity")
        .retry
        .expect("retry metadata")
        .review
        .expect("review retry metadata");
    assert!(retry_activity.force);
}

#[test]
fn retry_failed_activities_skips_when_same_review_is_active_without_retry_metadata() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(RecordingRetryProvider::default()),
        Arc::new(RetryReviewSource),
        Arc::new(FixedClock(1)),
    );
    let mut failed = provider_auth_failed_review(&store);
    failed.retry = Some(review_retry_metadata(
        "github",
        "acme/platform",
        Some(42),
        "abc123",
    ));
    store.save(&failed).expect("save failed");
    let mut active = store.create(ActivityKind::Review).expect("active");
    active.status = ActivityStatus::Running;
    active.label = Some("review on acme/platform#42".into());
    store.save(&active).expect("save active");

    let result = daemon
        .retry_failed_activities(nitpick_agent_core::RetryFailedActivitiesInput {
            kind: nitpick_agent_core::ProviderFailureKind::AuthInvalidCredentials,
        })
        .expect("retry");

    assert_eq!(result.queued, 0);
    assert_eq!(result.skipped, 1);
    assert!(result.activities.is_empty());
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

#[test]
fn discovery_poll_does_not_relog_seen_requests_after_restart() {
    let store = Arc::new(MemoryActivityStore::default());
    let processed = Arc::new(MemoryProcessedReviewStore::default());
    let config = AgentConfig {
        github_discovery: GitHubDiscoveryConfig {
            enabled: true,
            auto_review: false,
            ..GitHubDiscoveryConfig::default()
        },
        ..AgentConfig::default()
    };
    let first = HostDaemon::with_dependencies(
        store.clone(),
        config.clone(),
        processed.clone(),
        Arc::new(NoFindingsProvider),
        Arc::new(SingleReviewSource::new("sha-one")),
        Arc::new(FixedClock(1_000)),
    );
    first.poll_review_requests().expect("first poll");

    let restarted = HostDaemon::with_dependencies(
        store.clone(),
        config,
        processed,
        Arc::new(NoFindingsProvider),
        Arc::new(SingleReviewSource::new("sha-one")),
        Arc::new(FixedClock(2_000)),
    );
    restarted.poll_review_requests().expect("restart poll");

    let activities = store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    assert_eq!(
        activities[0].label.as_deref(),
        Some("review request acme/platform#42")
    );
}

#[test]
fn discovery_poll_records_updated_head_after_seen_request_changes() {
    let store = Arc::new(MemoryActivityStore::default());
    let processed = Arc::new(MemoryProcessedReviewStore::default());
    let config = AgentConfig {
        github_discovery: GitHubDiscoveryConfig {
            enabled: true,
            auto_review: false,
            ..GitHubDiscoveryConfig::default()
        },
        ..AgentConfig::default()
    };
    let first = HostDaemon::with_dependencies(
        store.clone(),
        config.clone(),
        processed.clone(),
        Arc::new(NoFindingsProvider),
        Arc::new(SingleReviewSource::new("sha-one")),
        Arc::new(FixedClock(1_000)),
    );
    first.poll_review_requests().expect("first poll");

    let restarted = HostDaemon::with_dependencies(
        store.clone(),
        config,
        processed,
        Arc::new(NoFindingsProvider),
        Arc::new(SingleReviewSource::new("sha-two")),
        Arc::new(FixedClock(2_000)),
    );
    restarted.poll_review_requests().expect("updated head poll");

    let activities = store.list().expect("activities");
    assert_eq!(activities.len(), 2);
    assert!(activities.iter().all(|activity| {
        activity.kind == ActivityKind::Discovery
            && activity.status == ActivityStatus::Completed
            && activity.label.as_deref() == Some("review request acme/platform#42")
    }));
}

#[test]
fn enqueue_review_limits_running_reviews_to_configured_default() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    for number in 1..=4 {
        daemon
            .enqueue_review(ReviewInput {
                subject: nitpick_agent_core::ReviewSubject {
                    repository: "acme/platform".into(),
                    number: Some(number),
                    ..nitpick_agent_core::ReviewSubject::default()
                },
                ..ReviewInput::default()
            })
            .expect("enqueue review");
    }

    wait_until(|| provider.started.load(Ordering::SeqCst) >= 3);
    std::thread::sleep(std::time::Duration::from_millis(50));

    let activities = store.list().expect("activities");
    assert_eq!(
        activities
            .iter()
            .filter(|activity| activity.status == ActivityStatus::Running)
            .count(),
        3
    );
    assert_eq!(
        activities
            .iter()
            .filter(|activity| activity.status == ActivityStatus::Queued)
            .count(),
        1
    );

    provider.release();
    wait_until(|| {
        store
            .list()
            .expect("activities")
            .iter()
            .all(|activity| activity.status == ActivityStatus::Completed)
    });
}

#[test]
fn enqueue_review_reuses_active_review_for_same_pr_head_sha() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let input = review_input_for_head("sha-one");
    let first = daemon
        .enqueue_review(input.clone())
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(input)
        .expect("duplicate review reused");

    assert_eq!(second.id, first.id);
    assert_eq!(store.list().expect("activities").len(), 1);

    provider.release();
}

#[test]
fn enqueue_review_restarts_stale_running_review_for_same_pr_head_sha() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());
    let input = review_input_for_head("sha-one");
    let mut stale = store.create(ActivityKind::Review).expect("activity");
    stale.label_review(&input);
    stale.status = ActivityStatus::Running;
    stale.session.status = SessionStatus::Running;
    stale.session.provider_session_id = Some("123e4567-e89b-42d3-a456-426614174000".into());
    stale
        .session
        .messages
        .push(nitpick_agent_core::AgentMessage {
            role: "nitpick.review.head_sha".into(),
            content: input.head_sha.clone(),
        });
    let stale_id = stale.id.clone();
    store.save(&stale).expect("save stale activity");

    let restarted = daemon
        .enqueue_review(input)
        .expect("stale review restarted");

    let stale = store.get(&stale_id).expect("stale activity");
    assert_eq!(stale.status, ActivityStatus::Error);
    assert_eq!(
        stale.error.as_deref(),
        Some("stale running review recovered")
    );
    assert_ne!(restarted.id, stale_id);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);
    provider.release();
}

#[test]
fn enqueue_review_force_supersedes_active_review_for_same_pr() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig::default(),
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(EmptyReviewSource),
        Arc::new(FixedClock(1)),
    );
    let first = daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("first review enqueued");
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);
    let mut forced_input = review_input_for_head("sha-one");
    forced_input.force = true;

    let forced = daemon
        .enqueue_review(forced_input)
        .expect("forced review enqueued");

    let first = store.get(&first.id).expect("first activity");
    assert_eq!(first.status, ActivityStatus::Error);
    assert_eq!(first.error.as_deref(), Some("superseded by forced review"));
    assert_ne!(forced.id, first.id);
    provider.release();
}

#[test]
fn enqueue_review_queues_same_pr_when_head_sha_changes() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let first = daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(review_input_for_head("sha-two"))
        .expect("updated head review enqueued");

    assert_ne!(second.id, first.id);
    assert_eq!(first.status, ActivityStatus::Running);
    assert_eq!(second.status, ActivityStatus::Queued);
    assert_eq!(store.list().expect("activities").len(), 2);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);

    provider.release();
    wait_until(|| provider.started.load(Ordering::SeqCst) == 2);
    wait_until(|| {
        store
            .list()
            .expect("activities")
            .iter()
            .all(|activity| activity.status == ActivityStatus::Completed)
    });
}

#[test]
fn enqueue_review_serializes_multiple_updated_heads_for_same_pr() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let first = daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(review_input_for_head("sha-two"))
        .expect("second review enqueued");
    let third = daemon
        .enqueue_review(review_input_for_head("sha-three"))
        .expect("third review enqueued");

    assert_eq!(first.status, ActivityStatus::Running);
    assert_eq!(second.status, ActivityStatus::Queued);
    assert_eq!(third.status, ActivityStatus::Queued);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);

    provider.release();
    wait_until(|| provider.started.load(Ordering::SeqCst) == 3);
}

#[test]
fn enqueue_review_releases_slot_when_provider_errors() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(ErrorThenBlockingProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig {
            max_concurrent_reviews: 1,
            ..AgentConfig::default()
        },
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(EmptyReviewSource),
        Arc::new(FixedClock(1)),
    );

    let first = daemon
        .enqueue_review(review_input_for_pr(41, "sha-one"))
        .expect("first review enqueued");
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);
    let second = daemon
        .enqueue_review(review_input_for_pr(42, "sha-one"))
        .expect("second review enqueued");

    assert_eq!(first.status, ActivityStatus::Running);
    assert_eq!(second.status, ActivityStatus::Queued);

    provider.release_error();
    wait_until(|| provider.started.load(Ordering::SeqCst) == 2);
    provider.release_success();
    wait_until(|| {
        let first = store.get(&first.id).expect("first activity");
        let second = store.get(&second.id).expect("second activity");
        first.status == ActivityStatus::Error && second.status == ActivityStatus::Completed
    });
}

#[test]
fn enqueue_review_releases_slot_before_completed_artifact_sync() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let sync_started = dir.path().join("sync-started");
    let release_sync = dir.path().join("release-sync");
    std::fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
if [ "$1" = "api" ]; then
  : > {sync_started}
  while [ ! -f {release_sync} ]; do
    sleep 0.05
  done
  exit 1
fi
exit 1
"#,
            sync_started = sync_started.display(),
            release_sync = release_sync.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);

    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(CountingNoFindingsProvider::default());
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        AgentConfig {
            github_command: Some(gh.display().to_string()),
            max_concurrent_reviews: 1,
            ..AgentConfig::default()
        },
        Arc::new(MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(EmptyReviewSource),
        Arc::new(FixedClock(1)),
    );

    let first = daemon
        .enqueue_review(review_input_for_pr(41, "sha-one"))
        .expect("first review enqueued");
    wait_until(|| sync_started.exists());
    let second = daemon
        .enqueue_review(review_input_for_pr(42, "sha-one"))
        .expect("second review enqueued");

    assert_eq!(first.status, ActivityStatus::Running);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 2);

    std::fs::write(&release_sync, "").expect("release sync");
    wait_until(|| {
        let first = store.get(&first.id).expect("first activity");
        let second = store.get(&second.id).expect("second activity");
        first.status == ActivityStatus::Completed && second.status == ActivityStatus::Completed
    });
}

#[test]
fn enqueue_review_creates_no_findings_file_level_draft_comment_when_completed_review_has_no_comments()
 {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    std::fs::write(
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
            payload = payload_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);

    let store = Arc::new(MemoryActivityStore::default());
    let config = AgentConfig {
        github_command: Some(gh.display().to_string()),
        ..AgentConfig::default()
    };
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        config,
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(NoFindingsProvider),
        Arc::new(EmptyReviewSource),
        Arc::new(FixedClock(1)),
    );

    daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("enqueue review");

    wait_until(|| {
        std::fs::read_to_string(&payload_file)
            .is_ok_and(|payload| payload.contains("Review completed: no findings"))
            && store
                .list()
                .expect("activities")
                .first()
                .and_then(|activity| {
                    store
                        .list_artifacts_for(&activity.id)
                        .expect("artifacts")
                        .into_iter()
                        .next()
                })
                .is_some_and(|artifact| {
                    matches!(
                        artifact.sync_state,
                        ArtifactSyncState::Pending {
                            ref destination,
                            ..
                        } if destination == "github-review"
                    )
                })
    });
    assert_eq!(
        std::fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 1);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["subject_type"], "file");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Review completed: no findings."
    );
    assert!(payload["comments"][0].get("line").is_none());
    assert!(payload["comments"][0].get("side").is_none());
    assert!(payload.get("event").is_none());

    let activity = store
        .list()
        .expect("activities")
        .into_iter()
        .next()
        .expect("activity");
    let artifacts = store.list_artifacts_for(&activity.id).expect("artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::ReviewComment);
    assert_eq!(
        artifacts[0].content,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/lib.rs".into(),
            line: 0,
            body: "🤖 Review completed: no findings.".into(),
        })
    );
    assert_eq!(
        artifacts[0].sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            ),
        }
    );
}

fn review_input_for_head(head_sha: &str) -> ReviewInput {
    review_input_for_pr(42, head_sha)
}

fn review_input_for_pr(number: u64, head_sha: &str) -> ReviewInput {
    ReviewInput {
        subject: nitpick_agent_core::ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(number),
            ..nitpick_agent_core::ReviewSubject::default()
        },
        head_sha: head_sha.into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+pub fn example() {}\n".into(),
        ..ReviewInput::default()
    }
}

fn provider_auth_failed_review(store: &MemoryActivityStore) -> nitpick_agent_core::Activity {
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Error;
    activity.error = Some("claude provider command failed with status exit status: 1".into());
    activity
        .session
        .messages
        .push(nitpick_agent_core::AgentMessage {
            role: "provider.stdout".into(),
            content: "Failed to authenticate. API Error: 401 Invalid authentication credentials"
                .into(),
        });
    activity
}

fn review_retry_metadata(
    source: &str,
    repository: &str,
    number: Option<u64>,
    head_sha: &str,
) -> nitpick_agent_core::ActivityRetryMetadata {
    review_retry_metadata_with_force(source, repository, number, head_sha, false)
}

fn review_retry_metadata_with_force(
    source: &str,
    repository: &str,
    number: Option<u64>,
    head_sha: &str,
    force: bool,
) -> nitpick_agent_core::ActivityRetryMetadata {
    nitpick_agent_core::ActivityRetryMetadata {
        review: Some(nitpick_agent_core::ReviewRetryMetadata {
            source: source.into(),
            repository: repository.into(),
            number,
            head_sha: head_sha.into(),
            review_mode: ReviewMode::Requested,
            force,
        }),
        resolved_by_activity: None,
    }
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if condition() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "condition timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[derive(Default)]
struct RecordingRetryProvider {
    reviewed: Mutex<Vec<String>>,
}

impl AgentProvider for RecordingRetryProvider {
    fn review(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        session.provider = Some(AgentProviderKind::Claude);
        self.reviewed
            .lock()
            .expect("reviewed lock")
            .push(input.subject.repository.clone());
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok("done".into())
    }
}

struct RetryReviewSource;

impl ReviewSource for RetryReviewSource {
    fn name(&self) -> &'static str {
        "github"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Ok(Vec::new())
    }

    fn review_input(&self, request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Ok(review_input_for_pr(
            request.number.expect("pull request number"),
            &request.head_sha,
        ))
    }
}

#[derive(Default)]
struct BlockingProvider {
    started: AtomicUsize,
    released: Mutex<bool>,
    release_changed: Condvar,
}

impl BlockingProvider {
    fn release(&self) {
        *self.released.lock().expect("release lock") = true;
        self.release_changed.notify_all();
    }
}

impl AgentProvider for BlockingProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut released = self.released.lock().expect("release lock");
        while !*released {
            released = self.release_changed.wait(released).expect("release wait");
        }
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok("done".into())
    }
}

#[derive(Default)]
struct ErrorThenBlockingProvider {
    started: AtomicUsize,
    error_released: Mutex<bool>,
    error_changed: Condvar,
    success_released: Mutex<bool>,
    success_changed: Condvar,
}

impl ErrorThenBlockingProvider {
    fn release_error(&self) {
        *self.error_released.lock().expect("error release lock") = true;
        self.error_changed.notify_all();
    }

    fn release_success(&self) {
        *self.success_released.lock().expect("success release lock") = true;
        self.success_changed.notify_all();
    }
}

impl AgentProvider for ErrorThenBlockingProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        let started = self.started.fetch_add(1, Ordering::SeqCst);
        if started == 0 {
            let mut released = self.error_released.lock().expect("error release lock");
            while !*released {
                released = self
                    .error_changed
                    .wait(released)
                    .expect("error release wait");
            }
            return Err(AgentError::provider("provider failed"));
        }

        let mut released = self.success_released.lock().expect("success release lock");
        while !*released {
            released = self
                .success_changed
                .wait(released)
                .expect("success release wait");
        }
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok("done".into())
    }
}

struct NoFindingsProvider;

impl AgentProvider for NoFindingsProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok("done".into())
    }
}

#[derive(Default)]
struct CountingNoFindingsProvider {
    started: AtomicUsize,
}

impl AgentProvider for CountingNoFindingsProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        self.started.fetch_add(1, Ordering::SeqCst);
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok("done".into())
    }
}

struct EmptyReviewSource;

impl ReviewSource for EmptyReviewSource {
    fn name(&self) -> &'static str {
        "empty"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Ok(Vec::new())
    }

    fn review_input(&self, _request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Ok(ReviewInput::default())
    }
}

struct SingleReviewSource {
    head_sha: &'static str,
}

impl SingleReviewSource {
    fn new(head_sha: &'static str) -> Self {
        Self { head_sha }
    }
}

impl ReviewSource for SingleReviewSource {
    fn name(&self) -> &'static str {
        "single"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Ok(vec![ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: self.head_sha.into(),
        }])
    }

    fn review_input(&self, _request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Ok(review_input_for_head(self.head_sha))
    }
}
