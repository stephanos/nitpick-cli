use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ReviewInput, ReviewRequest, ReviewSubject,
    review_session_id,
};

#[test]
fn request_formats_display_reference() {
    let request = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        id: "PR_kwDOExample".into(),
        head_sha: "abc123".into(),
    };

    assert_eq!(request.display_reference(), "acme/platform#42");

    let request_without_number = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        id: "PR_kwDOExample".into(),
        head_sha: "def456".into(),
        ..ReviewRequest::default()
    };

    assert_eq!(
        request_without_number.display_reference(),
        "acme/platform#PR_kwDOExample"
    );

    let request_without_number_or_id = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        ..ReviewRequest::default()
    };

    assert_eq!(
        request_without_number_or_id.display_reference(),
        "acme/platform"
    );
}

#[test]
fn activity_labels_review_input() {
    let mut activity = Activity::new(ActivityId::new("activity-1"), ActivityKind::Review);

    activity.label_review(&ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    });

    assert_eq!(
        activity.label.as_deref(),
        Some("review on acme/platform#42")
    );
}

#[test]
fn review_session_id_matches_existing_behavior_for_pr_with_sha() {
    let input = ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    };

    assert_eq!(
        review_session_id(&input),
        "73d2591b-47c9-4092-b620-841717641589"
    );
}

#[test]
fn review_session_id_matches_existing_behavior_for_pr_without_sha() {
    let input = ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        ..ReviewInput::default()
    };

    assert_eq!(
        review_session_id(&input),
        "e3a8e6e8-8dcb-4e4f-98ca-cef0a23a55fb"
    );
}

#[test]
fn review_session_id_matches_existing_behavior_for_no_pr_number() {
    let input = ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    };

    assert_eq!(
        review_session_id(&input),
        "7dd684d9-4132-47cb-84de-c666fced65e7"
    );
}
