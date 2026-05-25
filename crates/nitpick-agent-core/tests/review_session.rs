use std::fs;

use nitpick_agent_core::{ReviewCommentValidator, first_changed_file_for_diff};

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
fn first_changed_file_for_diff_returns_first_target_file() {
    let path = first_changed_file_for_diff(
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+pub fn lib() {}\n\
diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -0,0 +1 @@\n+fn main() {}\n",
    )
    .expect("valid diff");

    assert_eq!(path.as_deref(), Some("src/lib.rs"));
}

#[test]
fn first_changed_file_for_diff_uses_source_file_for_deleted_file() {
    let path = first_changed_file_for_diff(
        "diff --git a/src/old.rs b/src/old.rs\n--- a/src/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-fn old() {}\n",
    )
    .expect("valid diff");

    assert_eq!(path.as_deref(), Some("src/old.rs"));
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
