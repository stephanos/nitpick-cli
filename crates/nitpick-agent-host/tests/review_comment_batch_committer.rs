use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Barrier},
    thread,
};

use nitpick_agent_core::{ActivityStore, MemoryActivityStore};
use nitpick_agent_github::PullRequestRef;
use nitpick_agent_host::ReviewCommentBatchCommitter;
use nitpick_agent_model::{
    ActivityId, ActivityKind, ActivityStatus, ExistingReviewComment, FinishReviewCommentBatchInput,
    PullRequestContext, ReviewChatSessionSnapshot, ReviewComment, ReviewCommentBatch,
};

#[test]
fn empty_batch_returns_no_op_without_contacting_github() {
    let fixture = CommitFixture::new("#!/bin/sh\nprintf called >> \"$CALLS_FILE\"\nexit 1\n");
    let input = finish_input("batch-1", vec![]);

    let result = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("empty batch");

    assert!(result.no_op);
    assert_eq!(result.committed_addition_count, 0);
    assert_eq!(result.committed_deletion_count, 0);
    assert!(!fixture.calls.exists());
}

#[test]
fn stale_head_rejects_batch_before_creating_artifacts_or_mutating_github() {
    let fixture = CommitFixture::new(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS_FILE"
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"def456"}'
  exit 0
fi
exit 1
"#,
    );
    let input = finish_input("batch-1", vec![review_comment()]);
    fixture.save_session(&[]);

    let error = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect_err("stale head");

    assert!(error.to_string().contains("pull request head changed"));
    assert!(
        fixture
            .store
            .list_artifacts()
            .expect("artifacts")
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(&fixture.calls)
            .expect("calls")
            .lines()
            .count(),
        1
    );
}

#[test]
fn completed_batch_replays_stored_receipt_without_posting_twice() {
    let fixture = CommitFixture::new(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS_FILE"
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$*" = "api user" ]; then
  printf '{"login":"nitpick"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  printf '{"id":99,"html_url":"https://example.test/review-99","state":"PENDING","commit_id":"abc123"}'
  exit 0
fi
exit 1
"#,
    );
    let input = finish_input("batch-1", vec![review_comment()]);
    fixture.save_session(&[]);

    let first = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("first commit");
    let replay = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("receipt replay");

    assert_eq!(replay, first);
    assert_eq!(first.committed_addition_count, 1);
    let calls = fs::read_to_string(&fixture.calls).expect("calls");
    assert_eq!(calls.matches("--method POST --input -").count(), 1);
}

#[test]
fn retry_treats_an_already_deleted_eligible_draft_as_completed() {
    let fixture = CommitFixture::new(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS_FILE"
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/comments/11 --method DELETE" ]; then
  printf 'HTTP 404: Not Found' >&2
  exit 1
fi
exit 1
"#,
    );
    let input = FinishReviewCommentBatchInput {
        pinned_head_sha: "abc123".into(),
        batch: ReviewCommentBatch {
            id: "batch-delete".into(),
            additions: Vec::new(),
            deletion_ids: vec!["11".into()],
        },
    };
    let existing = [ExistingReviewComment {
        id: "11".into(),
        review_id: Some("99".into()),
        path: "src.rs".into(),
        line: Some(1),
        body: "🤖 Old finding.".into(),
        author: Some("nitpick".into()),
        draft: true,
    }];
    fixture.save_session(&existing);

    let result = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("already deleted draft");
    let replay = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("receipt replay");

    assert_eq!(result.committed_deletion_count, 1);
    assert_eq!(replay, result);
    let calls = fs::read_to_string(&fixture.calls).expect("calls");
    assert_eq!(calls.matches("--method DELETE").count(), 1);
}

#[test]
fn reusing_a_batch_id_with_different_contents_is_rejected() {
    let fixture = CommitFixture::new("#!/bin/sh\nexit 1\n");
    let first = finish_input("batch-1", Vec::new());
    fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &first)
        .expect("first batch");
    let changed = finish_input("batch-1", vec![review_comment()]);

    let error = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &changed)
        .expect_err("mismatched reuse");

    assert!(error.to_string().contains("different contents"));
}

#[test]
fn concurrent_different_payloads_cannot_claim_the_same_batch_id() {
    let fixture = Arc::new(CommitFixture::new("#!/bin/sh\nexit 1\n"));
    let barrier = Arc::new(Barrier::new(2));
    let threads = ["abc123", "def456"]
        .into_iter()
        .map(|head_sha| {
            let fixture = fixture.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut input = finish_input("batch-race", Vec::new());
                input.pinned_head_sha = head_sha.into();
                barrier.wait();
                fixture
                    .committer
                    .commit(&fixture.activity_id, &fixture.target, &input)
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().expect("commit thread"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                result
                    .as_ref()
                    .is_err_and(|error| error.to_string().contains("different contents"))
            })
            .count(),
        1
    );
}

#[test]
fn retry_resumes_after_a_partial_addition_failure() {
    let fixture = CommitFixture::new(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS_FILE"
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$*" = "api user" ]; then
  printf '{"login":"nitpick"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  if [ -f "$STATE_DIR/review" ]; then
    printf '[{"id":99,"html_url":"https://example.test/review-99","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  touch "$STATE_DIR/review"
  printf '{"id":99,"html_url":"https://example.test/review-99","state":"PENDING","commit_id":"abc123"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  if [ -f "$STATE_DIR/appended" ]; then
    printf '[{"id":101,"pull_request_review_id":99,"path":"src.rs","line":1,"body":"🤖 First.","user":{"login":"nitpick"},"state":"PENDING"},{"id":102,"pull_request_review_id":99,"path":"src.rs","line":1,"body":"🤖 Second.","user":{"login":"nitpick"},"state":"PENDING"}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments --method POST --input -" ]; then
  cat >/dev/null
  if [ ! -f "$STATE_DIR/failed" ]; then
    touch "$STATE_DIR/failed"
    printf 'temporary failure' >&2
    exit 1
  fi
  touch "$STATE_DIR/appended"
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[]'
  exit 0
fi
exit 1
"#,
    );
    fixture.save_session(&[]);
    let input = finish_input(
        "batch-partial",
        vec![
            ReviewComment {
                body: "First.".into(),
                ..review_comment()
            },
            ReviewComment {
                body: "Second.".into(),
                ..review_comment()
            },
        ],
    );

    let error = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect_err("partial failure");
    assert!(error.to_string().contains("after 1 of 2 addition(s)"));

    let result = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &input)
        .expect("retry partial batch");

    assert_eq!(result.committed_addition_count, 2);
    assert_eq!(result.existing_comments.len(), 2);
    let calls = fs::read_to_string(&fixture.calls).expect("calls");
    assert_eq!(
        calls
            .matches("pulls/42/reviews --method POST --input -")
            .count(),
        1
    );
    assert_eq!(
        calls
            .matches("pulls/42/comments --method POST --input -")
            .count(),
        2
    );
}

#[test]
fn later_batch_can_delete_a_comment_created_by_an_earlier_batch() {
    let fixture = CommitFixture::new(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS_FILE"
if [ "$*" = "pr view 42 --repo acme/platform --json headRefOid" ]; then
  printf '{"headRefOid":"abc123"}'
  exit 0
fi
if [ "$*" = "api user" ]; then
  printf '{"login":"nitpick"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  if [ -f "$STATE_DIR/review" ]; then
    printf '[{"id":99,"html_url":"https://example.test/review-99","state":"PENDING","commit_id":"abc123","body":"","user":{"login":"nitpick"}}]'
  else
    printf '[]'
  fi
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews --method POST --input -" ]; then
  cat >/dev/null
  touch "$STATE_DIR/review"
  printf '{"id":99,"html_url":"https://example.test/review-99","state":"PENDING","commit_id":"abc123"}'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[]'
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/42/reviews/99/comments" ]; then
  if [ -f "$STATE_DIR/deleted" ]; then
    printf '[]'
  else
    printf '[{"id":101,"pull_request_review_id":99,"path":"src.rs","line":1,"body":"🤖 Temporary.","user":{"login":"nitpick"},"state":"PENDING"}]'
  fi
  exit 0
fi
if [ "$*" = "api repos/acme/platform/pulls/comments/101 --method DELETE" ]; then
  touch "$STATE_DIR/deleted"
  exit 0
fi
exit 1
"#,
    );
    fixture.save_session(&[]);
    let addition = finish_input(
        "batch-add",
        vec![ReviewComment {
            body: "Temporary.".into(),
            ..review_comment()
        }],
    );
    let addition_result = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &addition)
        .expect("commit addition");
    assert_eq!(addition_result.existing_comments[0].id, "101");
    assert!(addition_result.existing_comments[0].draft);

    let deletion = FinishReviewCommentBatchInput {
        pinned_head_sha: "abc123".into(),
        batch: ReviewCommentBatch {
            id: "batch-delete-later".into(),
            additions: Vec::new(),
            deletion_ids: vec!["101".into()],
        },
    };
    let deletion_result = fixture
        .committer
        .commit(&fixture.activity_id, &fixture.target, &deletion)
        .expect("commit later deletion");

    assert_eq!(deletion_result.committed_deletion_count, 1);
    assert!(deletion_result.existing_comments.is_empty());
}

struct CommitFixture {
    _dir: tempfile::TempDir,
    repo_dir: std::path::PathBuf,
    calls: std::path::PathBuf,
    _state_dir: std::path::PathBuf,
    store: Arc<MemoryActivityStore>,
    activity_id: ActivityId,
    target: PullRequestRef,
    committer: ReviewCommentBatchCommitter,
}

impl CommitFixture {
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs::create_dir(&repo_dir).expect("repo dir");
        fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
        let calls = dir.path().join("calls");
        let state_dir = dir.path().join("state");
        fs::create_dir(&state_dir).expect("state dir");
        let gh = dir.path().join("gh");
        fs::write(
            &gh,
            script
                .replace("$CALLS_FILE", calls.to_str().expect("calls path"))
                .replace("$STATE_DIR", state_dir.to_str().expect("state path")),
        )
        .expect("gh script");
        let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh, permissions).expect("chmod");
        let store = Arc::new(MemoryActivityStore::default());
        let mut activity = store.create(ActivityKind::Review).expect("activity");
        activity.status = ActivityStatus::Completed;
        activity.label = Some("review on acme/platform#42".into());
        store.save(&activity).expect("save activity");
        let activity_id = activity.id;
        let committer = ReviewCommentBatchCommitter::new(dir.path(), store.clone(), &gh);

        Self {
            _dir: dir,
            repo_dir,
            calls,
            _state_dir: state_dir,
            store,
            activity_id,
            target: PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            committer,
        }
    }

    fn save_session(&self, existing_comments: &[ExistingReviewComment]) {
        self.committer
            .save_session_snapshot(&ReviewChatSessionSnapshot {
                activity_id: self.activity_id.clone(),
                repository: "acme/platform".into(),
                number: 42,
                repo_dir: self.repo_dir.clone(),
                head_sha: "abc123".into(),
                diff: DIFF.into(),
                pull_request_context: PullRequestContext::default(),
                existing_comments: existing_comments.to_vec(),
                mcp_server_command: "nitpick-agent-host".into(),
            })
            .expect("save review chat session");
    }
}

fn finish_input(batch_id: &str, additions: Vec<ReviewComment>) -> FinishReviewCommentBatchInput {
    FinishReviewCommentBatchInput {
        pinned_head_sha: "abc123".into(),
        batch: ReviewCommentBatch {
            id: batch_id.into(),
            additions,
            deletion_ids: vec![],
        },
    }
}

fn review_comment() -> ReviewComment {
    ReviewComment {
        path: "src.rs".into(),
        line: 1,
        body: "Prefer this.".into(),
    }
}

const DIFF: &str =
    "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n";
