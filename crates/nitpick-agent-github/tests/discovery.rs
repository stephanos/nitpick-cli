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
        .mark_processed(&current, Some("activity-1".into()))
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
        .mark_processed(&pull_request, Some("activity-1".into()))
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
