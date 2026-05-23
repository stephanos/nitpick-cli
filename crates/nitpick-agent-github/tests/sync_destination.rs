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
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
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
fn github_cli_destination_posts_raw_pr_comment_body() {
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
    make_executable(&gh);
    let destination = GitHubCliSyncDestination::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let outcome = destination
        .post_comment("🤖 Review completed: no findings.")
        .expect("sync outcome");

    assert_eq!(
        fs::read_to_string(args_file).expect("args"),
        "pr comment 42 --repo acme/platform --body-file -\n"
    );
    assert_eq!(
        fs::read_to_string(body_file).expect("body"),
        "🤖 Review completed: no findings."
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
fn github_cli_destination_does_not_prefix_plain_review_comment_body() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let body_file = dir.path().join("body");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncat > {}\nprintf 'https://github.com/acme/platform/pull/42#issuecomment-99\\n'\n",
            body_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
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
    let destination = GitHubCliSyncDestination::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    destination.sync(&artifact).expect("sync outcome");

    let body = fs::read_to_string(body_file).expect("body");
    assert!(body.contains("Prefer this."));
    assert!(!body.contains("🤖 Prefer this."));
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
    assert_eq!(payload["comments"][0]["body"], "🤖 Prefer this.");
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Synced {
            destination: "github-review".into(),
            remote_id: Some("https://github.com/acme/platform/pull/42#discussion_r99".into())
        }
    );
}

#[test]
fn github_cli_review_destination_prefixes_inline_comment_body_once() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
cat > {payload}
printf '{{"html_url":"https://github.com/acme/platform/pull/42#discussion_r99"}}\n'
"#,
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
            body: "🤖 Prefer this.".into(),
        }),
    );

    destination.sync(&artifact).expect("sync outcome");

    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["comments"][0]["body"], "🤖 Prefer this.");
}

#[test]
fn github_cli_review_destination_batches_summary_and_inline_comments_into_pending_review() {
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
printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
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
    let summary = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary body".into()),
    );
    let first_comment = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let second_comment = Artifact::local(
        ArtifactId::new("artifact-3"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/main.rs".into(),
            line: 8,
            body: "Also this.".into(),
        }),
    );

    let outcomes = destination
        .sync_batch(&[summary, first_comment, second_comment])
        .expect("sync outcomes");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert!(payload.get("event").is_none());
    assert_eq!(payload["body"], "summary body");
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 2);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["line"], 12);
    assert_eq!(payload["comments"][0]["side"], "RIGHT");
    assert_eq!(payload["comments"][0]["body"], "🤖 Prefer this.");
    assert_eq!(payload["comments"][1]["path"], "src/main.rs");
    assert_eq!(payload["comments"][1]["line"], 8);
    assert_eq!(payload["comments"][1]["side"], "RIGHT");
    assert_eq!(payload["comments"][1]["body"], "🤖 Also this.");
    assert_eq!(outcomes.len(), 3);
    assert!(outcomes.iter().all(|outcome| outcome.sync_state
        == ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            )
        }));
}

#[test]
fn github_cli_review_destination_posts_body_only_as_pending_review() {
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
printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
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

    let outcome = destination
        .create_pending_review_body("🤖 Review completed: no findings.")
        .expect("sync outcome");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert_eq!(payload["body"], "🤖 Review completed: no findings.");
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 0);
    assert!(payload.get("event").is_none());
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            )
        }
    );
}

#[test]
fn github_cli_review_destination_batches_inline_comments_without_review_body() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
cat > {payload}
printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
"#,
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
    let first_comment = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let second_comment = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/main.rs".into(),
            line: 8,
            body: "🤖 Already prefixed.".into(),
        }),
    );

    let outcomes = destination
        .sync_batch(&[first_comment, second_comment])
        .expect("sync outcomes");

    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 2);
    assert_eq!(payload["comments"][0]["body"], "🤖 Prefer this.");
    assert_eq!(payload["comments"][1]["body"], "🤖 Already prefixed.");
    assert_eq!(outcomes.len(), 2);
}

#[test]
fn github_cli_review_destination_updates_pending_review_body() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
cat > {payload}
printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
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

    let review = destination
        .update_pending_review_body("99", "updated summary")
        .expect("review");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api repos/acme/platform/pulls/42/reviews/99 --method PUT --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["body"], "updated summary");
    assert_eq!(review.state, "PENDING");
}

#[test]
fn github_cli_review_destination_lists_review_comments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[{{"id":10,"pull_request_review_id":98,"path":"src/lib.rs","line":12,"body":"Please adjust.","user":{{"login":"alice"}},"state":"SUBMITTED"}}]\n'
elif [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":98,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-98","state":"COMMENTED","commit_id":"abc123"}},{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"def456"}}]\n'
elif [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  printf '[{{"id":11,"pull_request_review_id":99,"path":"src/lib.rs","line":13,"body":"🤖 Old note.","user":{{"login":"nitpick"}},"state":"PENDING"}}]\n'
fi
"#,
            commands = commands_file.display(),
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

    let comments = destination.review_comments().expect("comments");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api repos/acme/platform/pulls/42/comments\napi repos/acme/platform/pulls/42/reviews\napi repos/acme/platform/pulls/42/reviews/99/comments\n"
    );
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].id, "10");
    assert_eq!(comments[0].author.as_deref(), Some("alice"));
    assert_eq!(comments[0].draft, false);
    assert_eq!(comments[1].id, "11");
    assert_eq!(comments[1].body, "🤖 Old note.");
    assert_eq!(comments[1].draft, true);
}

#[test]
fn github_cli_review_destination_deletes_review_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n",
            commands_file.display(),
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

    destination
        .delete_review_comment("11")
        .expect("delete comment");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api repos/acme/platform/pulls/comments/11 --method DELETE\n"
    );
}

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}
