use nitpick_agent_github::{GitHubPullRequestClient, PullRequestRef};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn parses_owner_repo_number_reference() {
    let reference = "acme/platform#42"
        .parse::<PullRequestRef>()
        .expect("reference parses");

    assert_eq!(reference.owner, "acme");
    assert_eq!(reference.repo, "platform");
    assert_eq!(reference.number, 42);
}

#[test]
fn parses_github_pull_request_url() {
    let reference = "https://github.com/acme/platform/pull/42"
        .parse::<PullRequestRef>()
        .expect("url parses");

    assert_eq!(reference.owner, "acme");
    assert_eq!(reference.repo, "platform");
    assert_eq!(reference.number, 42);
}

#[test]
fn parses_github_pull_request_url_with_query_and_fragment() {
    let reference =
        "https://github.com/acme/platform/pull/42?notification_referrer_id=abc#discussion"
            .parse::<PullRequestRef>()
            .expect("url parses");

    assert_eq!(reference.owner, "acme");
    assert_eq!(reference.repo, "platform");
    assert_eq!(reference.number, 42);
}

#[test]
fn pull_request_client_reads_head_sha() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"abc123"}\n'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let client = GitHubPullRequestClient::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    assert_eq!(client.head_sha().expect("head sha"), "abc123");
}
