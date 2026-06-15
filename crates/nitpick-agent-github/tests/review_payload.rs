use nitpick_agent_github::GitHubReviewPayload;
use nitpick_agent_model::ReviewComment;

#[test]
fn github_review_payload_builds_file_level_comment_shape() {
    let payload = GitHubReviewPayload::comment(ReviewComment {
        path: "src/lib.rs".into(),
        line: 0,
        body: "Looks good.".into(),
    });

    assert_eq!(payload["path"], "src/lib.rs");
    assert_eq!(payload["subject_type"], "file");
    assert_eq!(payload["body"], "🤖 Looks good.");
    assert!(payload.get("line").is_none());
}
