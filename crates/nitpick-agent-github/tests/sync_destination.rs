use nitpick_agent_core::{
    ActivityId, Artifact, ArtifactContent, ArtifactId, ArtifactKind, ArtifactSyncDestination,
    ArtifactSyncState,
};
use nitpick_agent_github::{
    GitHubCliReviewSyncDestination, GitHubCliSyncDestination, GitHubDryRunSyncDestination,
    PullRequestRef,
};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn github_dry_run_marks_artifact_pending_for_github() {
    let destination = GitHubDryRunSyncDestination;
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("looks good".into()),
    );

    let outcome = destination.sync(&artifact).expect("sync outcome");

    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into()
        }
    );
    assert_eq!(outcome.remote_id, None);
}

#[test]
fn github_cli_destination_posts_artifact_with_gh_pr_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let args_file = dir.path().join("args");
    let body_file = dir.path().join("body");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > {}\ncat > {}\nprintf 'https://github.com/acme/platform/pull/42#issuecomment-99\\n'\n",
            args_file.display(),
            body_file.display()
        ),
    )
    .expect("write fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh, permissions).expect("chmod");
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("looks good".into()),
    );
    let destination = GitHubCliSyncDestination::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let outcome = destination.sync(&artifact).expect("sync outcome");

    assert_eq!(
        fs::read_to_string(args_file).expect("args"),
        "pr comment 42 --repo acme/platform --body-file -\n"
    );
    assert!(
        fs::read_to_string(body_file)
            .expect("body")
            .contains("looks good")
    );
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Synced {
            destination: "github".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#issuecomment-99".into())
        }
    );
}

#[test]
fn github_cli_review_destination_posts_summary_with_gh_pr_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let args_file = dir.path().join("args");
    let body_file = dir.path().join("body");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > {}\ncat > {}\nprintf 'https://github.com/acme/platform/pull/42#pullrequestreview-99\\n'\n",
            args_file.display(),
            body_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("looks good".into()),
    );
    let destination = GitHubCliReviewSyncDestination::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let outcome = destination.sync(&artifact).expect("sync outcome");

    assert_eq!(
        fs::read_to_string(args_file).expect("args"),
        "pr review 42 --repo acme/platform --comment --body-file -\n"
    );
    assert!(
        fs::read_to_string(body_file)
            .expect("body")
            .contains("looks good")
    );
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Synced {
            destination: "github-review".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into())
        }
    );
}

#[test]
fn github_cli_review_destination_posts_inline_comment_with_gh_api() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
cat > {payload}
printf '{{"html_url":"https://github.com/acme/platform/pull/42#discussion_r99"}}\n'
"#,
            commands = commands_file.display(),
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let destination = GitHubCliReviewSyncDestination::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );

    let outcome = destination.sync(&artifact).expect("sync outcome");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert_eq!(payload["event"], "COMMENT");
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["line"], 12);
    assert_eq!(payload["comments"][0]["side"], "RIGHT");
    assert_eq!(payload["comments"][0]["body"], "Prefer this.");
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Synced {
            destination: "github-review".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#discussion_r99".into())
        }
    );
}

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}
