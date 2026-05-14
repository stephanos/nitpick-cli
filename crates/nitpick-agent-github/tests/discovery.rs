use std::{fs, os::unix::fs::PermissionsExt};

use nitpick_agent_core::{ProcessedReviewStore, ReviewRequest, ReviewSource};
use nitpick_agent_github::{
    DiscoveredPullRequest, FsProcessedReviewStore, GitHubCliDiscovery, PullRequestState,
};

#[test]
fn github_cli_discovery_lists_requested_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1 $2" = "search prs" ]; then
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
fn github_cli_discovery_scopes_requested_reviews_to_allowlist_queries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let log = dir.path().join("commands.log");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
echo "$*" >> '{}'
if [ "$1 $2" = "search prs" ] && [ "$7 $8" = "--owner stephanos" ]; then
  printf '[{{"repository":{{"nameWithOwner":"stephanos/nitpick-agent"}},"number":42}}]'
  exit 0
fi
if [ "$1 $2" = "search prs" ] && [ "$7 $8" = "--repo stephanos/nitpick-agent" ]; then
  printf '[{{"repository":{{"nameWithOwner":"stephanos/nitpick-agent"}},"number":42}},{{"repository":{{"nameWithOwner":"stephanos/nitpick-cli"}},"number":7}}]'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "42" ]; then
  printf '{{"headRefOid":"abc123"}}'
  exit 0
fi
if [ "$1 $2" = "pr view" ] && [ "$3" = "7" ]; then
  printf '{{"headRefOid":"def456"}}'
  exit 0
fi
exit 1
"#,
            log.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);

    let prs = GitHubCliDiscovery::new(&gh)
        .with_allowlist(&["stephanos/*".into(), "stephanos/nitpick-agent".into()])
        .requested_reviews()
        .expect("requested reviews");

    assert_eq!(
        prs,
        vec![
            DiscoveredPullRequest {
                owner: "stephanos".into(),
                repo: "nitpick-agent".into(),
                number: 42,
                head_sha: "abc123".into(),
            },
            DiscoveredPullRequest {
                owner: "stephanos".into(),
                repo: "nitpick-cli".into(),
                number: 7,
                head_sha: "def456".into(),
            },
        ]
    );
    assert_eq!(
        fs::read_to_string(log).expect("log"),
        "search prs --review-requested @me --state open --owner stephanos --limit 100 --json repository,number\n\
search prs --review-requested @me --state open --repo stephanos/nitpick-agent --limit 100 --json repository,number\n\
pr view 42 --repo stephanos/nitpick-agent --json headRefOid\n\
pr view 7 --repo stephanos/nitpick-cli --json headRefOid\n"
    );
}

#[test]
fn github_cli_discovery_builds_review_input_from_pr_metadata_and_diff() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let git = dir.path().join("git");
    let checkout_root = dir.path().join("checkouts");
    let log = dir.path().join("commands.log");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
echo "gh $*" >> '{}'
if [ "$1 $2" = "pr view" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
  exit 0
fi
if [ "$1 $2" = "pr diff" ]; then
  printf 'diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n'
  exit 0
fi
if [ "$1 $2" = "repo clone" ]; then
  mkdir -p "$4/.git"
  exit 0
fi
exit 1
"#,
            log.display()
        ),
    )
    .expect("write fake gh");
    fs::write(
        &git,
        format!(
            r#"#!/bin/sh
echo "git $*" >> '{}'
exit 0
"#,
            log.display()
        ),
    )
    .expect("write fake git");
    make_executable(&gh);
    make_executable(&git);
    let pull_request = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };

    let input = GitHubCliDiscovery::with_checkout_commands(&gh, &git, &checkout_root)
        .review_input(&pull_request)
        .expect("review input");
    let checkout_dir = checkout_root.join("acme/platform/pr-42");

    assert_eq!(input.subject.repository, "acme/platform");
    assert_eq!(input.subject.number, Some(42));
    assert_eq!(input.subject.title, "Add watcher");
    assert_eq!(input.subject.author, "stephan");
    assert_eq!(input.repo_dir, checkout_dir);
    assert_eq!(
        input.diff,
        "diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n"
    );
    assert!(input.instructions.contains("acme/platform#42"));
    assert!(
        input
            .instructions
            .contains("https://github.com/acme/platform/pull/42")
    );
    assert!(input.instructions.contains("abc123"));
    assert!(input.instructions.contains("feature/watcher"));
    assert!(input.instructions.contains("open"));
    assert_eq!(
        fs::read_to_string(log).expect("command log"),
        format!(
            "gh pr view 42 --repo acme/platform --json title,author,url,headRefOid,headRefName,state,mergedAt\n\
gh pr diff 42 --repo acme/platform\n\
gh repo clone acme/platform {} -- --quiet\n\
git -C {} fetch origin feature/watcher --quiet\n\
git -C {} checkout -B feature/watcher origin/feature/watcher --quiet\n",
            checkout_dir.display(),
            checkout_dir.display(),
            checkout_dir.display(),
        )
    );
}

#[test]
fn github_cli_discovery_detects_existing_nitpick_review_for_current_head() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let args_file = dir.path().join("args");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > {}
printf '%s\n' '[{{"commit_id":"abc123","body":"<!-- nitpick-agent:artifact-1 -->\\n\\nlooks good"}}]'
"#,
            args_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let request = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        id: "42".into(),
        head_sha: "abc123".into(),
    };

    let already_reviewed = GitHubCliDiscovery::new(&gh)
        .already_reviewed(&request)
        .expect("already reviewed");

    assert!(already_reviewed);
    assert_eq!(
        fs::read_to_string(args_file).expect("args"),
        "api repos/acme/platform/pulls/42/reviews\n"
    );
}

#[test]
fn github_cli_discovery_ignores_nitpick_review_for_old_head() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
printf '%s\n' '[{"commit_id":"old-sha","body":"<!-- nitpick-agent:artifact-1 -->\\n\\nlooks good"}]'
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let request = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        id: "42".into(),
        head_sha: "abc123".into(),
    };

    let already_reviewed = GitHubCliDiscovery::new(&gh)
        .already_reviewed(&request)
        .expect("already reviewed");

    assert!(!already_reviewed);
}

#[test]
fn github_cli_discovery_parses_pull_request_state_metadata() {
    let cases = [
        ("OPEN", "null", PullRequestState::Open),
        ("CLOSED", "null", PullRequestState::Closed),
        (
            "CLOSED",
            r#""2026-05-09T12:34:56Z""#,
            PullRequestState::Merged,
        ),
    ];

    for (raw_state, merged_at, expected_state) in cases {
        let dir = tempfile::tempdir().expect("temp dir");
        let gh = dir.path().join("gh");
        fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
if [ "$1 $2" = "pr view" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"feature/watcher","state":"{}","mergedAt":{}}}'
  exit 0
fi
exit 1
"#,
                raw_state, merged_at
            ),
        )
        .expect("write fake gh");
        make_executable(&gh);
        let pull_request = DiscoveredPullRequest {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
            head_sha: "abc123".into(),
        };

        let details = GitHubCliDiscovery::new(&gh)
            .pull_request_details(&pull_request)
            .expect("details");

        assert_eq!(details.state, expected_state);
        assert_eq!(details.url, "https://github.com/acme/platform/pull/42");
        assert_eq!(details.head_sha, "abc123");
        assert_eq!(details.head_ref_name, "feature/watcher");
    }
}

#[test]
fn github_cli_discovery_removes_closed_or_merged_checkouts_but_keeps_open_ones() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    let discovery = GitHubCliDiscovery::with_checkout_commands(
        dir.path().join("gh"),
        dir.path().join("git"),
        &checkout_root,
    );
    let pull_request = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };
    let checkout_dir = checkout_root.join("acme/platform/pr-42");
    fs::create_dir_all(checkout_dir.join(".git")).expect("checkout");

    let removed = discovery
        .cleanup_checkout_for(
            &pull_request,
            &pull_request_details_for_state(PullRequestState::Open),
        )
        .expect("open cleanup");

    assert!(!removed);
    assert!(checkout_dir.exists());

    let removed = discovery
        .cleanup_checkout_for(
            &pull_request,
            &pull_request_details_for_state(PullRequestState::Closed),
        )
        .expect("closed cleanup");

    assert!(removed);
    assert!(!checkout_dir.exists());

    fs::create_dir_all(checkout_dir.join(".git")).expect("checkout");
    let removed = discovery
        .cleanup_checkout_for(
            &pull_request,
            &pull_request_details_for_state(PullRequestState::Merged),
        )
        .expect("merged cleanup");

    assert!(removed);
    assert!(!checkout_dir.exists());
}

#[test]
fn github_cli_discovery_treats_missing_closed_checkout_as_noop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    let discovery = GitHubCliDiscovery::with_checkout_commands(
        dir.path().join("gh"),
        dir.path().join("git"),
        &checkout_root,
    );
    let pull_request = DiscoveredPullRequest {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
        head_sha: "abc123".into(),
    };

    let removed = discovery
        .cleanup_checkout_for(
            &pull_request,
            &pull_request_details_for_state(PullRequestState::Closed),
        )
        .expect("missing cleanup");

    assert!(!removed);
}

#[test]
fn github_cli_discovery_lists_checkout_prs_from_checkout_root() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    fs::create_dir_all(checkout_root.join("acme/platform/pr-42/.git")).expect("checkout");
    fs::create_dir_all(checkout_root.join("acme/platform/not-a-pr")).expect("ignored");
    fs::create_dir_all(checkout_root.join("octo/widgets/pr-7/.git")).expect("checkout");
    let discovery = GitHubCliDiscovery::with_checkout_commands(
        dir.path().join("gh"),
        dir.path().join("git"),
        &checkout_root,
    );

    let checkouts = discovery.list_checkouts().expect("checkouts");

    assert_eq!(
        checkouts,
        vec![
            DiscoveredPullRequest {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
                head_sha: String::new(),
            },
            DiscoveredPullRequest {
                owner: "octo".into(),
                repo: "widgets".into(),
                number: 7,
                head_sha: String::new(),
            },
        ]
    );
}

#[test]
fn github_cli_discovery_resolves_checkout_path_for_pr_ref() {
    let dir = tempfile::tempdir().expect("temp dir");
    let checkout_root = dir.path().join("checkouts");
    let discovery = GitHubCliDiscovery::with_checkout_commands(
        dir.path().join("gh"),
        dir.path().join("git"),
        &checkout_root,
    );
    let pull_request = "acme/platform#42"
        .parse::<nitpick_agent_github::PullRequestRef>()
        .expect("reference");

    assert_eq!(
        discovery.checkout_path_for(&pull_request),
        checkout_root.join("acme/platform/pr-42")
    );
}

fn pull_request_details_for_state(
    state: PullRequestState,
) -> nitpick_agent_github::PullRequestDetails {
    nitpick_agent_github::PullRequestDetails {
        title: "Add watcher".into(),
        author: "stephan".into(),
        url: "https://github.com/acme/platform/pull/42".into(),
        head_sha: "abc123".into(),
        head_ref_name: "feature/watcher".into(),
        state,
    }
}

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

#[test]
fn memory_processed_review_store_filters_already_reviewed_heads() {
    let store = nitpick_agent_core::MemoryProcessedReviewStore::default();
    let current = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        id: "42".into(),
        head_sha: "abc123".into(),
    };
    let changed = ReviewRequest {
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
    let pull_request = ReviewRequest {
        source: "github".into(),
        repository: "acme/platform".into(),
        number: Some(42),
        id: "42".into(),
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
    assert_eq!(processed.request.head_sha, "abc123");
    assert_eq!(processed.activity_id, Some("activity-1".into()));
    assert!(!reopened.needs_review(&pull_request).expect("same sha"));
}
