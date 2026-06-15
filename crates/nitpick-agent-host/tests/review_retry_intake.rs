use nitpick_agent_host::ReviewRetryIntake;
use nitpick_agent_model::{Activity, ActivityId, ActivityKind, ReviewMode, ReviewRetryMetadata};

#[test]
fn review_retry_intake_accepts_github_pr_retry_metadata() {
    let retry = ReviewRetryMetadata {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        head_sha: "abc123".into(),
        review_mode: ReviewMode::Requested,
        force: false,
    };

    assert!(ReviewRetryIntake::is_retryable_review_metadata(&retry));
}

#[test]
fn review_retry_intake_treats_resolved_failed_activity_as_done() {
    let mut activity = Activity::new(ActivityId::new("activity-1"), ActivityKind::Review);
    activity.retry = Some(nitpick_agent_core::ActivityRetryMetadata {
        review: None,
        resolved_by_activity: Some(ActivityId::new("activity-2")),
    });

    assert!(ReviewRetryIntake::provider_failure_resolved(&activity));
}
