use nitpick_agent_github::{
    DiscoveredPullRequest, PullRequestDetails, PullRequestState, prepare_github_review_input,
};

#[test]
fn github_review_request_preparation_builds_review_input_from_pr_data() {
    let input = prepare_github_review_input(
        &DiscoveredPullRequest {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
            head_sha: "old".into(),
        },
        PullRequestDetails {
            title: "Improve review".into(),
            author: "alice".into(),
            url: "https://github.com/acme/platform/pull/42".into(),
            body: "Body".into(),
            head_sha: "abc123".into(),
            head_ref_name: "feature".into(),
            state: PullRequestState::Open,
        },
        "diff --git a/src/lib.rs b/src/lib.rs".into(),
        "/tmp/acme/platform/pr-42".into(),
    );

    assert_eq!(input.source, "github");
    assert_eq!(input.subject.repository, "acme/platform");
    assert_eq!(input.subject.number, Some(42));
    assert_eq!(input.subject.title, "Improve review");
    assert_eq!(input.head_sha, "abc123");
    assert!(
        input
            .instructions
            .contains("Review GitHub pull request acme/platform#42.")
    );
    assert!(input.instructions.contains("Head ref: feature."));
}
