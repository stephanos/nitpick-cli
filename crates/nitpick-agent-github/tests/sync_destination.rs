use nitpick_agent_core::ArtifactSyncDestination;
use nitpick_agent_github::{
    GitHubCliReviewSyncDestination, GitHubCliSyncDestination, GitHubDryRunSyncDestination,
    GitHubReviewSyncCoordinator, GitHubReviewWorkflowSync, PullRequestRef,
};
use nitpick_agent_model::{
    ActivityId, Artifact, ArtifactContent, ArtifactId, ArtifactKind, ArtifactSyncState,
    ReviewComment,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Barrier},
    thread,
};

const DIFF_WITH_SRC_LIB: &str = r#"diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"#;

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
        ArtifactContent::ReviewComment(ReviewComment {
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
fn github_cli_review_destination_stages_summary_in_pending_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let args_file = dir.path().join("args");
    let body_file = dir.path().join("body");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nif [ \"$1\" = \"pr\" ]; then printf '{{\"headRefOid\":\"abc123\"}}\\n'; exit 0; fi\ncat > {}\nprintf '{{\"id\":99,\"html_url\":\"https://github.com/acme/platform/pull/42#pullrequestreview-99\",\"state\":\"PENDING\",\"commit_id\":\"abc123\"}}\\n'\n",
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
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(body_file).expect("body")).expect("payload json");
    assert_eq!(
        payload["body"],
        "looks good\n\n<!-- nitpick-agent:artifact-1 -->"
    );
    assert!(payload.get("event").is_none());
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            ),
        }
    );
}

#[test]
fn github_cli_review_destination_reads_pull_request_context() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "pr view 42 --repo acme/platform --json title,author,url,body,headRefOid,headRefName,state,mergedAt" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","body":"Please review the watcher changes.","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}\n'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/issues/42/comments" ]; then
  printf '[{{"id":100,"body":"Can you explain the retry behavior?","user":{{"login":"alice"}},"created_at":"2026-05-30T12:00:00Z","updated_at":"2026-05-30T12:30:00Z","html_url":"https://github.com/acme/platform/pull/42#issuecomment-100"}}]\n'
  exit 0
fi
exit 1
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

    let context = destination
        .pull_request_context()
        .expect("pull request context");

    assert_eq!(context.title, "Add watcher");
    assert_eq!(context.author, "stephan");
    assert_eq!(context.url, "https://github.com/acme/platform/pull/42");
    assert_eq!(context.body, "Please review the watcher changes.");
    assert_eq!(context.head_sha, "abc123");
    assert_eq!(context.head_ref_name, "feature/watcher");
    assert_eq!(context.state, "open");
    assert_eq!(context.conversation_comments.len(), 1);
    assert_eq!(context.conversation_comments[0].id, "100");
    assert_eq!(
        context.conversation_comments[0].author.as_deref(),
        Some("alice")
    );
    assert_eq!(
        context.conversation_comments[0].url.as_deref(),
        Some("https://github.com/acme/platform/pull/42#issuecomment-100")
    );
    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json title,author,url,body,headRefOid,headRefName,state,mergedAt\napi repos/acme/platform/issues/42/comments\n"
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
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
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
    assert!(payload.get("event").is_none());
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["line"], 12);
    assert_eq!(payload["comments"][0]["side"], "RIGHT");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->"
    );
    assert_eq!(
        outcome.sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            ),
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
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "🤖 Prefer this.".into(),
        }),
    );

    destination.sync(&artifact).expect("sync outcome");

    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->"
    );
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
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let second_comment = Artifact::local(
        ArtifactId::new("artifact-3"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
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
    assert_eq!(
        payload["body"],
        "summary body\n\n<!-- nitpick-agent:artifact-1 -->"
    );
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 2);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["line"], 12);
    assert_eq!(payload["comments"][0]["side"], "RIGHT");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-2 -->"
    );
    assert_eq!(payload["comments"][1]["path"], "src/main.rs");
    assert_eq!(payload["comments"][1]["line"], 8);
    assert_eq!(payload["comments"][1]["side"], "RIGHT");
    assert_eq!(
        payload["comments"][1]["body"],
        "🤖 Also this.\n\n<!-- nitpick-agent:artifact-3 -->"
    );
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
fn github_cli_review_destination_posts_no_findings_as_file_level_draft_comment() {
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
        .create_pending_file_comment("src/lib.rs", "🤖 Review completed: no findings.")
        .expect("sync outcome");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 1);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["subject_type"], "file");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Review completed: no findings.\n\n<!-- nitpick-agent:no-findings -->"
    );
    assert!(payload["comments"][0].get("line").is_none());
    assert!(payload["comments"][0].get("side").is_none());
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
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let second_comment = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
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
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->"
    );
    assert_eq!(
        payload["comments"][1]["body"],
        "🤖 Already prefixed.\n\n<!-- nitpick-agent:artifact-2 -->"
    );
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
fn github_review_workflow_sync_marks_pending_artifacts_synced_after_manual_submission() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{"login":"nitpick"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then printf '[]'; exit 0; fi
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf '{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"COMMENT","commit_id":"abc123"}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut summary = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary body".into()),
    );
    summary.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let updates = sync
        .reconcile_pending_artifact_states(std::slice::from_ref(&summary))
        .expect("reconcile")
        .expect("updates");

    assert_eq!(
        updates,
        vec![(
            summary.id,
            ArtifactSyncState::Synced {
                destination: "github-review".into(),
                remote_id: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
                ),
            },
        )]
    );
}

#[test]
fn coordinator_stages_local_artifacts_after_prior_review_was_submitted() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99" ]; then
  printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"COMMENTED","commit_id":"abc123"}}'
  exit 0
fi
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]'
  exit 0
fi
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"def456"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat > {payload}
  printf '{{"id":100,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-100","state":"PENDING","commit_id":"def456"}}'
  exit 0
fi
exit 1
"#,
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut submitted_summary = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("old summary".into()),
    );
    submitted_summary.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let local_comment = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "New finding.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);

    let outcomes = coordinator
        .synchronize(
            &PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            &[submitted_summary, local_comment],
        )
        .expect("synchronize");

    assert!(matches!(
        outcomes[0].sync_state,
        ArtifactSyncState::Synced { .. }
    ));
    assert_eq!(
        outcomes[1].sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("100".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-100".into()
            ),
        }
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 1);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
}

#[test]
fn github_review_workflow_sync_propagates_pending_review_fetch_failures() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{"login":"nitpick"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then printf '[]'; exit 0; fi
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf 'HTTP 403: Forbidden\n' >&2
  exit 1
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut summary = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary body".into()),
    );
    summary.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let error = sync
        .reconcile_pending_artifact_states(std::slice::from_ref(&summary))
        .expect_err("fetch failure should propagate");

    assert!(error.message().contains("HTTP 403"));
    assert!(error.message().contains("review 99"));
    assert!(error.message().contains("artifact artifact-1"));
}

#[test]
fn github_review_workflow_sync_restages_artifact_when_pending_review_disappears() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$1" = "api" ] && [ "$2" = "repos/acme/platform/pulls/42/reviews/99" ]; then
  printf 'HTTP 404: Not Found\n' >&2
  exit 1
fi
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]'
  exit 0
fi
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  printf '{{"id":100,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-100","state":"PENDING","commit_id":"abc123","user":{{"login":"nitpick"}}}}'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut summary = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary body".into()),
    );
    summary.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let states = sync
        .reconcile_pending_artifact_states(std::slice::from_ref(&summary))
        .expect("missing review should be restaged")
        .expect("reconciled states");

    assert_eq!(
        states,
        vec![(
            summary.id.clone(),
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("100".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-100".into()
                ),
            },
        )]
    );
    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api user\napi repos/acme/platform/pulls/42/reviews\napi repos/acme/platform/pulls/42/reviews/99\npr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
}

#[test]
fn github_review_workflow_sync_updates_pending_review_body_for_local_summary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99 --method PUT --input -" ]; then
  cat > {payload}
  printf '{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display(),
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut pending_comment = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Already staged.".into(),
        }),
    );
    pending_comment.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let summary = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("updated summary".into()),
    );
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let updates = sync
        .reconcile_pending_artifact_states(&[pending_comment.clone(), summary.clone()])
        .expect("reconcile")
        .expect("updates");

    assert_eq!(
        fs::read_to_string(commands_file).expect("commands"),
        "api user\napi repos/acme/platform/pulls/42/reviews\napi repos/acme/platform/pulls/42/reviews/99 --method PUT --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["body"],
        "updated summary\n\n<!-- nitpick-agent:artifact-2 -->"
    );
    assert_eq!(
        updates,
        vec![
            (pending_comment.id, pending_comment.sync_state),
            (
                summary.id,
                ArtifactSyncState::Pending {
                    destination: "github-review".into(),
                    remote_id: Some("99".into()),
                    remote_url: Some(
                        "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
                    ),
                },
            ),
        ]
    );
}

#[test]
fn github_review_workflow_sync_appends_local_comment_to_known_pending_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{"login":"nitpick"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then printf '[]'; exit 0; fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{"headRefOid":"abc123"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf '{"id":101}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut pending = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Already staged.".into(),
        }),
    );
    pending.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("99".into()),
        remote_url: Some("https://github.com/acme/platform/pull/42#pullrequestreview-99".into()),
    };
    let local = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/main.rs".into(),
            line: 8,
            body: "New finding.".into(),
        }),
    );
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let updates = sync
        .reconcile_pending_artifact_states(&[pending, local])
        .expect("append comment")
        .expect("updates");

    assert_eq!(updates.len(), 2);
    assert!(
        updates
            .iter()
            .all(|(_, state)| matches!(state, ArtifactSyncState::Pending { .. }))
    );
}

#[test]
fn github_review_workflow_sync_creates_no_findings_file_level_draft_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]'
  exit 0
fi
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
    let sync = GitHubReviewWorkflowSync::new(
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
        },
        &gh,
    );

    let comment = sync
        .no_findings_draft_file_comment(DIFF_WITH_SRC_LIB)
        .expect("no findings draft")
        .expect("comment");
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(comment),
    );

    let state_change = sync
        .sync_no_findings_draft_file_comment(&artifact)
        .expect("sync no findings draft");

    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(
        state_change,
        (
            artifact.id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
                ),
            },
        )
    );
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["subject_type"], "file");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Review completed: no findings.\n\n<!-- nitpick-agent:artifact-1 -->"
    );
}

#[test]
fn github_review_sync_coordinator_recovers_create_422_by_reusing_pending_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let raced_review_file = dir.path().join("raced-review");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  if [ -f {raced_review} ]; then
    printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{{"headRefOid":"abc123"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  : > {raced_review}
  printf 'gh: Validation Failed (HTTP 422)' >&2
  exit 1
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat > {payload}
  printf '{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"new","user":{{"login":"nitpick"}},"state":"PENDING"}}'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display(),
            raced_review = raced_review_file.display(),
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let outcomes = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect("recover raced pending review");

    let commands = fs::read_to_string(commands_file).expect("commands");
    assert_eq!(
        commands
            .matches("api repos/acme/platform/pulls/42/reviews --method POST --input -\n")
            .count(),
        1
    );
    assert_eq!(
        commands
            .matches("api repos/acme/platform/pulls/42/comments --method POST --input -\n")
            .count(),
        1
    );
    assert_eq!(
        outcomes[0].sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            ),
        }
    );
}

#[test]
fn github_review_sync_coordinator_does_not_retry_append_422() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then printf '[]'; exit 0; fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf 'gh: Validation Failed (HTTP 422)' >&2
  exit 1
fi
exit 1
"#,
            commands = commands_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let error = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect_err("append validation failure");

    assert!(error.message().contains("HTTP 422"));
    assert_eq!(error.github_http_status(), Some(422));
    assert!(error.message().contains("artifact artifact-1"));
    let commands = fs::read_to_string(commands_file).expect("commands");
    assert_eq!(commands.matches("api user\n").count(), 1);
    assert_eq!(
        commands
            .matches("api repos/acme/platform/pulls/42/reviews\n")
            .count(),
        1
    );
    assert!(!commands.contains("api repos/acme/platform/pulls/42/reviews --method POST --input -"));
}

#[test]
fn github_review_sync_coordinator_does_not_treat_head_lookup_422_as_create_race() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then printf '[]'; exit 0; fi
if [ "$1" = "pr" ]; then printf 'HTTP 422: Validation Failed' >&2; exit 1; fi
exit 1
"#,
            commands = commands_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("Summary".into()),
    );

    let error = GitHubReviewSyncCoordinator::new(&gh)
        .synchronize(
            &PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            &[artifact],
        )
        .expect_err("head lookup validation failure");

    assert_eq!(error.github_http_status(), Some(422));
    let commands = fs::read_to_string(commands_file).expect("commands");
    assert_eq!(commands.matches("api user\n").count(), 1);
    assert_eq!(
        commands
            .matches("api repos/acme/platform/pulls/42/reviews\n")
            .count(),
        1
    );
    assert!(!commands.contains("--method POST"));
}

#[test]
fn github_review_sync_coordinator_preserves_rate_limit_error_type() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then
  printf 'HTTP 429: secondary rate limit; Retry-After: 12' >&2
  exit 1
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("Summary".into()),
    );

    let error = GitHubReviewSyncCoordinator::new(&gh)
        .synchronize(
            &PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            &[artifact],
        )
        .expect_err("rate limit");

    assert!(matches!(
        error,
        nitpick_agent_core::AgentError::GitHubRateLimited {
            retry_after_seconds: Some(12),
            ..
        }
    ));
}

#[test]
fn github_review_sync_coordinator_contextualizes_comment_list_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{"login":"nitpick"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{"id":99,"html_url":"https://example.test/99","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  printf 'HTTP 403: Forbidden' >&2
  exit 1
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Finding.".into(),
        }),
    );

    let error = GitHubReviewSyncCoordinator::new(&gh)
        .synchronize(
            &PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            &[artifact],
        )
        .expect_err("comment list failure");

    assert!(error.message().contains("HTTP 403"));
    assert!(error.message().contains("pending review 99"));
    assert!(error.message().contains("artifact artifact-1"));
}

#[test]
fn github_review_sync_coordinator_rejects_ambiguous_owned_pending_reviews() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{"login":"nitpick"}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{"id":98,"html_url":"https://example.test/98","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}},{"id":99,"html_url":"https://example.test/99","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/98" ]; then
  printf '{"id":98,"html_url":"https://example.test/98","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}'
  exit 0
fi
exit 1
"#,
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("Summary".into()),
    );
    artifact.sync_state = ArtifactSyncState::Pending {
        destination: "github-review".into(),
        remote_id: Some("98".into()),
        remote_url: Some("https://example.test/98".into()),
    };
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let error = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect_err("ambiguous pending reviews");

    assert!(error.message().contains("has 2 pending reviews"));
}

#[test]
fn github_review_sync_coordinator_leaves_synced_artifacts_untouched() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let calls_file = dir.path().join("calls");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf 'called\\n' >> {}\nexit 1\n",
            calls_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    artifact.sync_state = ArtifactSyncState::Synced {
        destination: "github-review".into(),
        remote_id: Some("https://example.test/review-98".into()),
    };
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let outcomes = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect("keep synced artifact");

    assert!(!calls_file.exists());
    assert_eq!(outcomes[0].sync_state, artifact.sync_state);
}

#[test]
fn github_review_sync_coordinator_skips_exact_and_semantic_remote_matches() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then
  printf '{{"login":"nitpick"}}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  printf '%s' '[{{"id":101,"pull_request_review_id":99,"path":"old/path.rs","line":3,"body":"🤖 old body\n\n<!-- nitpick-agent:artifact-1 -->","user":{{"login":"nitpick"}},"state":"PENDING"}},{{"id":102,"pull_request_review_id":99,"path":"src/main.rs","line":8,"body":"🤖 Also this.\n\n<!-- nitpick-agent:older-artifact -->","user":{{"login":"nitpick"}},"state":"PENDING"}}]'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let first = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-2"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "changed body".into(),
        }),
    );
    let second = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity-2"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/main.rs".into(),
            line: 8,
            body: "Also this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let outcomes = coordinator
        .synchronize(&target, &[first, second])
        .expect("deduplicate comments");

    let commands = fs::read_to_string(commands_file).expect("commands");
    assert!(!commands.contains("--method POST"));
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| outcome.sync_state
        == ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into()
            ),
        }));
}

#[test]
fn github_review_sync_coordinator_preserves_manual_body_while_appending_comment() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"My manual notes","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then printf '[]'; exit 0; fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf '{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"new","user":{{"login":"nitpick"}},"state":"PENDING"}}'
  exit 0
fi
exit 1
"#,
            commands = commands_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let summary = Artifact::local(
        ArtifactId::new("summary"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("Nitpick summary".into()),
    );
    let comment = Artifact::local(
        ArtifactId::new("comment"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let outcomes = coordinator
        .synchronize(&target, &[summary, comment])
        .expect("append with manual body");

    let commands = fs::read_to_string(commands_file).expect("commands");
    assert!(!commands.contains("reviews/99 --method PUT"));
    assert_eq!(outcomes[0].sync_state, ArtifactSyncState::LocalOnly);
    assert!(matches!(
        outcomes[1].sync_state,
        ArtifactSyncState::Pending { .. }
    ));
}

#[test]
fn github_review_sync_coordinator_retries_failed_artifact_in_existing_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let payload_file = dir.path().join("payload");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then printf '[]'; exit 0; fi
if [ "$1" = "pr" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat > {payload}
  printf '{{}}'
  exit 0
fi
exit 1
"#,
            payload = payload_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let mut artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Retry this finding.".into(),
        }),
    );
    artifact.sync_state = ArtifactSyncState::Failed {
        destination: "github-review".into(),
        error: "lost response".into(),
    };

    let outcomes = GitHubReviewSyncCoordinator::new(&gh)
        .synchronize(
            &PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            std::slice::from_ref(&artifact),
        )
        .expect("retry failed artifact");

    assert!(matches!(
        outcomes[0].sync_state,
        ArtifactSyncState::Pending { .. }
    ));
    let payload = fs::read_to_string(payload_file).expect("append payload");
    assert!(payload.contains("Retry this finding."));
    assert!(payload.contains("<!-- nitpick-agent:artifact-1 -->"));
}

#[test]
fn github_review_sync_coordinator_retry_after_lost_append_response_is_idempotent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let append_calls_file = dir.path().join("append-calls");
    let remote_comment_file = dir.path().join("remote-comment");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  if [ -f {remote_comment} ]; then
    printf '%s' '[{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->","user":{{"login":"nitpick"}},"state":"PENDING"}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf 'call\n' >> {append_calls}
  : > {remote_comment}
  printf 'connection closed after remote success' >&2
  exit 1
fi
exit 1
"#,
            append_calls = append_calls_file.display(),
            remote_comment = remote_comment_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect_err("lost append response");
    let outcomes = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect("retry recognizes remote comment");

    assert_eq!(
        fs::read_to_string(append_calls_file).expect("append calls"),
        "call\n"
    );
    assert!(matches!(
        outcomes[0].sync_state,
        ArtifactSyncState::Pending { .. }
    ));
}

#[test]
fn github_review_sync_coordinator_retry_after_lost_create_response_is_idempotent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let create_calls_file = dir.path().join("create-calls");
    let created_review_file = dir.path().join("created-review");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  if [ -f {created_review} ]; then
    printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  printf 'call\n' >> {create_calls}
  : > {created_review}
  printf 'connection closed after remote success' >&2
  exit 1
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  printf '%s' '[{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->","user":{{"login":"nitpick"}},"state":"PENDING"}}]'
  exit 0
fi
exit 1
"#,
            create_calls = create_calls_file.display(),
            created_review = created_review_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer this.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect_err("lost create response");
    let outcomes = coordinator
        .synchronize(&target, std::slice::from_ref(&artifact))
        .expect("retry discovers created review");

    assert_eq!(
        fs::read_to_string(create_calls_file).expect("create calls"),
        "call\n"
    );
    assert!(matches!(
        outcomes[0].sync_state,
        ArtifactSyncState::Pending { .. }
    ));
}

#[test]
fn github_review_sync_coordinator_retry_after_partial_append_skips_prior_success() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let append_calls_file = dir.path().join("append-calls");
    let current_payload_file = dir.path().join("current-payload");
    let first_remote_file = dir.path().join("first-remote");
    let second_failed_file = dir.path().join("second-failed");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  if [ -f {first_remote} ]; then
    printf '%s' '[{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"🤖 First.\n\n<!-- nitpick-agent:artifact-1 -->","user":{{"login":"nitpick"}},"state":"PENDING"}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat > {current_payload}
  printf 'call\n' >> {append_calls}
  if grep -q 'nitpick-agent:artifact-1' {current_payload}; then
    : > {first_remote}
    printf '{{"id":101}}'
    exit 0
  fi
  if [ ! -f {second_failed} ]; then
    : > {second_failed}
    printf 'temporary append failure' >&2
    exit 1
  fi
  printf '{{"id":102}}'
  exit 0
fi
exit 1
"#,
            append_calls = append_calls_file.display(),
            current_payload = current_payload_file.display(),
            first_remote = first_remote_file.display(),
            second_failed = second_failed_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let first = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "First.".into(),
        }),
    );
    let second = Artifact::local(
        ArtifactId::new("artifact-2"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/main.rs".into(),
            line: 8,
            body: "Second.".into(),
        }),
    );
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let error = coordinator
        .synchronize(&target, &[first.clone(), second.clone()])
        .expect_err("second append fails");
    assert!(error.message().contains("artifact-2"));
    assert!(error.message().contains("review 99"));
    let outcomes = coordinator
        .synchronize(&target, &[first, second])
        .expect("retry appends only missing comment");

    assert_eq!(
        fs::read_to_string(append_calls_file).expect("append calls"),
        "call\ncall\ncall\n"
    );
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome.sync_state, ArtifactSyncState::Pending { .. }))
    );
}

#[test]
fn github_review_sync_coordinator_appends_one_copy_of_duplicate_local_findings() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let append_calls_file = dir.path().join("append-calls");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://github.com/acme/platform/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then printf '[]'; exit 0; fi
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf 'call\n' >> {append_calls}
  printf '{{"id":101}}'
  exit 0
fi
exit 1
"#,
            append_calls = append_calls_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let finding = |id: &str| {
        Artifact::local(
            ArtifactId::new(id),
            ActivityId::new("activity"),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "Prefer this.".into(),
            }),
        )
    };
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let outcomes = coordinator
        .synchronize(&target, &[finding("artifact-1"), finding("artifact-2")])
        .expect("synchronize duplicate findings");

    assert_eq!(
        fs::read_to_string(append_calls_file).expect("append calls"),
        "call\n"
    );
    assert_eq!(outcomes.len(), 2);
}

#[test]
fn concurrent_same_target_sync_appends_one_remote_copy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let append_calls_file = dir.path().join("append-calls");
    let remote_comment_file = dir.path().join("remote-comment");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
if [ "$*" = "api user" ]; then printf '{{"login":"nitpick"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[{{"id":99,"html_url":"https://example.test/99","state":"PENDING","commit_id":"abc123","body":"","user":{{"login":"nitpick"}}}}]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  if [ -f {remote_comment} ]; then
    printf '%s' '[{{"id":101,"pull_request_review_id":99,"path":"src/lib.rs","line":12,"body":"🤖 Finding.\n\n<!-- nitpick-agent:artifact-1 -->","user":{{"login":"nitpick"}},"state":"PENDING"}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$1" = "pr" ]; then printf '{{"headRefOid":"abc123"}}'; exit 0; fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  printf 'call\n' >> {append_calls}
  : > {remote_comment}
  printf '{{}}'
  exit 0
fi
exit 1
"#,
            append_calls = append_calls_file.display(),
            remote_comment = remote_comment_file.display(),
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);
    let coordinator = GitHubReviewSyncCoordinator::new(&gh);
    let target = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity"),
        ArtifactKind::ReviewComment,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Finding.".into(),
        }),
    );
    let barrier = Arc::new(Barrier::new(3));

    let workers = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let coordinator = coordinator.clone();
            let target = target.clone();
            let artifact = artifact.clone();
            thread::spawn(move || {
                barrier.wait();
                coordinator.synchronize(&target, &[artifact])
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        let outcomes = worker.join().expect("sync thread").expect("synchronize");
        assert!(matches!(
            outcomes[0].sync_state,
            ArtifactSyncState::Pending { .. }
        ));
    }

    assert_eq!(
        fs::read_to_string(append_calls_file).expect("append calls"),
        "call\n"
    );
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
    assert!(!comments[0].draft);
    assert_eq!(comments[1].id, "11");
    assert_eq!(comments[1].body, "🤖 Old note.");
    assert!(comments[1].draft);
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
