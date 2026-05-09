use std::{fs, os::unix::fs::PermissionsExt};

use nitpick_agent_github::{
    DiscoveredPullRequest, FsProcessedReviewStore, GitHubCliDiscovery, ProcessedReviewStore,
};

#[test]
fn github_cli_discovery_lists_requested_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2 $3" = "search prs user-review-requested:@me" ]; then
  printf '[{"repository":{"nameWithOwner":"acme/platform"},"number":42},{"repository":{"nameWithOwner":"octo/widgets"},"number":7}]'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "42" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "7" ]; then
  printf '{"headRefOid":"def456"}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");

    let prs = GitHubCliDiscovery::new(&gh)
        .requested_reviews()
        .expect("requested reviews");

    assert_eq!(
        prs,
        vec![
            DiscoveredPullRequest {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
                head_sha: "abc123".into(),
            },
            DiscoveredPullRequest {
                owner: "octo".into(),
                repo: "widgets".into(),
                number: 7,
                head_sha: "def456".into(),
            },
        ]
    );
}

#[test]
fn github_cli_discovery_builds_review_input_from_pr_metadata_and_diff() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2" = "pr view" ]; then
  printf '{"title":"Add watcher","author":{"login":"stephan"},"headRefOid":"abc123"}'
  exit 0
fi
if [ "$1 $2" = "pr diff" ]; then
  printf 'diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let pull_request = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };

    let input = GitHubCliDiscovery::new(&gh)
        .review_input(&pull_request)
        .expect("review input");

    assert_eq!(input.subject.repository, "acme/platform");
    assert_eq!(input.subject.number, Some(42));
    assert_eq!(input.subject.title, "Add watcher");
    assert_eq!(input.subject.author, "stephan");
    assert_eq!(
        input.diff,
        "diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n"
    );
    assert!(input.instructions.contains("acme/platform#42"));
    assert!(input.instructions.contains("abc123"));
}

#[test]
fn memory_processed_review_store_filters_already_reviewed_heads() {
    let store = nitpick_agent_github::MemoryProcessedReviewStore::default();
    let current = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };
    let changed = DiscoveredPullRequest {
        head_sha: "def456".into(),
        ..current.clone()
    };

    store
        .mark_processed_at(&current, Some("activity-1".into()), 1_000)
        .expect("mark processed");

    assert!(!store.needs_review(&current).expect("current processed"));
    assert!(store.needs_review(&changed).expect("changed needs review"));
}

#[test]
fn filesystem_processed_review_store_survives_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsProcessedReviewStore::new(dir.path()).expect("store");
    let pull_request = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };

    store
        .mark_processed_at(&pull_request, Some("activity-1".into()), 1_000)
        .expect("mark processed");

    let reopened = FsProcessedReviewStore::new(dir.path()).expect("reopen store");
    let processed = reopened
        .get_processed(&pull_request)
        .expect("get processed")
        .expect("processed review exists");
    assert_eq!(processed.head_sha, "abc123");
    assert_eq!(processed.activity_id, Some("activity-1".into()));
    assert!(!reopened.needs_review(&pull_request).expect("same sha"));
}
