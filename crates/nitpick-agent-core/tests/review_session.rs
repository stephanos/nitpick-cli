use std::fs;

use nitpick_agent_core::ReviewCommentValidator;

#[test]
fn review_comment_validator_accepts_added_line_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");

    let validator = ReviewCommentValidator::for_diff(
        &repo_dir,
        "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n",
    )
    .expect("diff validator");

    let comment = validator
        .validate_comment("src.rs", 1, "added line note")
        .expect("added line comment");

    assert_eq!(comment.path, "src.rs");
    assert_eq!(comment.line, 1);
    assert_eq!(comment.body, "added line note");
}

#[test]
fn review_comment_validator_rejects_unchanged_line_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\nfn unchanged() {}\n").expect("repo file");

    let validator = ReviewCommentValidator::for_diff(
        &repo_dir,
        "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -1,2 +1,2 @@\n+fn main() {}\n fn unchanged() {}\n",
    )
    .expect("diff validator");

    let error = validator
        .validate_comment("src.rs", 2, "unchanged line note")
        .expect_err("unchanged line rejected");

    assert_eq!(
        error.to_string(),
        "review comment line is outside the diff changeset: src.rs:2"
    );
}
