use nitpick_agent_core::{
    ActivityId, Artifact, ArtifactContent, ArtifactId, ArtifactKind, ArtifactSyncDestination,
    ArtifactSyncState,
};
use nitpick_agent_github::{GitHubCliSyncDestination, GitHubDryRunSyncDestination, PullRequestRef};
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
