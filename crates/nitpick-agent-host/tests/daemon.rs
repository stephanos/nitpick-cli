use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use nitpick_agent_core::{
    ActivityKind, ActivityStatus, ActivityStore, AgentProvider, AgentProviderKind, AgentResult,
    AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState, ChatInput, FixedClock,
    FsActivityStore, HostStatus, MemoryActivityStore, MemoryProcessedReviewStore, ReviewInput,
    ReviewOutput, ReviewRequest, ReviewSource, SessionStatus,
};
use nitpick_agent_github::PullRequestRef;
use nitpick_agent_host::{AgentConfig, HostDaemon};

#[test]
fn host_status_reports_current_activity_count() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    store.save(&activity).expect("save running activity");
    let mut completed = store
        .create(ActivityKind::Chat)
        .expect("completed activity");
    completed.status = ActivityStatus::Completed;
    store.save(&completed).expect("save completed activity");
    let mut error = store.create(ActivityKind::Chat).expect("error activity");
    error.status = ActivityStatus::Error;
    store.save(&error).expect("save error activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local result".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");
    store
        .update_artifact_sync_state(
            &artifact_id,
            ArtifactSyncState::Pending {
                destination: "github".into(),
                remote_id: None,
                remote_url: None,
            },
        )
        .expect("mark pending");
    let daemon = HostDaemon::new(store);

    assert_eq!(
        daemon.status().expect("status"),
        HostStatus {
            activity_count: 3,
            queued_activity_count: 0,
            running_activity_count: 1,
            completed_activity_count: 1,
            error_activity_count: 1,
            open_review_count: 0,
            queued_review_count: 0,
            running_review_count: 1,
            completed_review_count: 0,
            error_review_count: 0,
            artifact_count: 1,
            local_only_artifact_count: 0,
            pending_sync_artifact_count: 1,
            provider: AgentProviderKind::Claude,
            model: None,
            review_source_name: "github".into(),
            review_source_enabled: false,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
        }
    );
}

#[test]
fn daemon_recovers_interrupted_filesystem_activities_after_store_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsActivityStore::new(dir.path()).expect("store");
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    activity.session.status = SessionStatus::Running;
    let activity_id = activity.id.clone();
    store.save(&activity).expect("save running activity");
    drop(store);

    let reopened = Arc::new(FsActivityStore::new(dir.path()).expect("reopen store"));
    let daemon = HostDaemon::new(reopened.clone());

    let recovered_count = daemon
        .recover_interrupted_activities()
        .expect("recover interrupted activities");

    let recovered = reopened.get(&activity_id).expect("recovered activity");
    assert_eq!(recovered_count, 1);
    assert_eq!(recovered.status, ActivityStatus::Error);
    assert_eq!(
        recovered.session.status,
        SessionStatus::Error("host restarted before activity completed".into())
    );
    assert_eq!(
        recovered.error,
        Some("host restarted before activity completed".into())
    );
}

#[test]
fn daemon_marks_interrupted_running_activities_as_errors() {
    let store = Arc::new(MemoryActivityStore::default());
    let mut activity = store.create(ActivityKind::Review).expect("activity");
    activity.status = ActivityStatus::Running;
    activity.session.status = SessionStatus::Running;
    let activity_id = activity.id.clone();
    store.save(&activity).expect("save running activity");
    let daemon = HostDaemon::new(store.clone());

    let recovered_count = daemon
        .recover_interrupted_activities()
        .expect("recover interrupted activities");

    let recovered = store.get(&activity_id).expect("recovered activity");
    assert_eq!(recovered_count, 1);
    assert_eq!(recovered.status, ActivityStatus::Error);
    assert_eq!(
        recovered.session.status,
        SessionStatus::Error("host restarted before activity completed".into())
    );
    assert_eq!(
        recovered.error,
        Some("host restarted before activity completed".into())
    );
}

#[test]
fn daemon_records_completed_checkout_cleanup_activity() {
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::new(store.clone());
    let pull_request = PullRequestRef {
        owner: "acme".into(),
        repo: "platform".into(),
        number: 42,
    };

    let activity = daemon
        .record_checkout_cleanup_activity(&pull_request)
        .expect("cleanup activity");

    assert_eq!(activity.status, ActivityStatus::Completed);
    assert_eq!(
        activity.label.as_deref(),
        Some("acme/platform#42 cleaned up")
    );
    assert_eq!(
        store
            .get(&activity.id)
            .expect("persisted activity")
            .label
            .as_deref(),
        Some("acme/platform#42 cleaned up")
    );
}

#[test]
fn enqueue_review_limits_running_reviews_to_configured_default() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    for number in 1..=4 {
        daemon
            .enqueue_review(ReviewInput {
                subject: nitpick_agent_core::ReviewSubject {
                    repository: "acme/platform".into(),
                    number: Some(number),
                    ..nitpick_agent_core::ReviewSubject::default()
                },
                ..ReviewInput::default()
            })
            .expect("enqueue review");
    }

    wait_until(|| provider.started.load(Ordering::SeqCst) >= 3);
    std::thread::sleep(std::time::Duration::from_millis(50));

    let activities = store.list().expect("activities");
    assert_eq!(
        activities
            .iter()
            .filter(|activity| activity.status == ActivityStatus::Running)
            .count(),
        3
    );
    assert_eq!(
        activities
            .iter()
            .filter(|activity| activity.status == ActivityStatus::Queued)
            .count(),
        1
    );

    provider.release();
    wait_until(|| {
        store
            .list()
            .expect("activities")
            .iter()
            .all(|activity| activity.status == ActivityStatus::Completed)
    });
}

#[test]
fn enqueue_review_reuses_active_review_for_same_pr_head_sha() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let input = review_input_for_head("sha-one");
    let first = daemon
        .enqueue_review(input.clone())
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(input)
        .expect("duplicate review reused");

    assert_eq!(second.id, first.id);
    assert_eq!(store.list().expect("activities").len(), 1);

    provider.release();
}

#[test]
fn enqueue_review_queues_same_pr_when_head_sha_changes() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let first = daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(review_input_for_head("sha-two"))
        .expect("updated head review enqueued");

    assert_ne!(second.id, first.id);
    assert_eq!(first.status, ActivityStatus::Running);
    assert_eq!(second.status, ActivityStatus::Queued);
    assert_eq!(store.list().expect("activities").len(), 2);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);

    provider.release();
    wait_until(|| provider.started.load(Ordering::SeqCst) == 2);
    wait_until(|| {
        store
            .list()
            .expect("activities")
            .iter()
            .all(|activity| activity.status == ActivityStatus::Completed)
    });
}

#[test]
fn enqueue_review_serializes_multiple_updated_heads_for_same_pr() {
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(BlockingProvider::default());
    let daemon = HostDaemon::with_provider(store.clone(), provider.clone());

    let first = daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("first review enqueued");
    let second = daemon
        .enqueue_review(review_input_for_head("sha-two"))
        .expect("second review enqueued");
    let third = daemon
        .enqueue_review(review_input_for_head("sha-three"))
        .expect("third review enqueued");

    assert_eq!(first.status, ActivityStatus::Running);
    assert_eq!(second.status, ActivityStatus::Queued);
    assert_eq!(third.status, ActivityStatus::Queued);
    wait_until(|| provider.started.load(Ordering::SeqCst) == 1);

    provider.release();
    wait_until(|| provider.started.load(Ordering::SeqCst) == 3);
}

#[test]
fn enqueue_review_creates_no_findings_file_level_draft_comment_when_completed_review_has_no_comments()
 {
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands_file = dir.path().join("commands");
    let payload_file = dir.path().join("payload");
    std::fs::write(
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
            payload = payload_file.display()
        ),
    )
    .expect("write fake gh");
    make_executable(&gh);

    let store = Arc::new(MemoryActivityStore::default());
    let config = AgentConfig {
        github_command: Some(gh.display().to_string()),
        ..AgentConfig::default()
    };
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        config,
        Arc::new(MemoryProcessedReviewStore::default()),
        Arc::new(NoFindingsProvider),
        Arc::new(EmptyReviewSource),
        Arc::new(FixedClock(1)),
    );

    daemon
        .enqueue_review(review_input_for_head("sha-one"))
        .expect("enqueue review");

    wait_until(|| {
        std::fs::read_to_string(&payload_file)
            .is_ok_and(|payload| payload.contains("Review completed: no findings"))
    });
    assert_eq!(
        std::fs::read_to_string(commands_file).expect("commands"),
        "pr view 42 --repo acme/platform --json headRefOid\napi repos/acme/platform/pulls/42/reviews --method POST --input -\n"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(payload_file).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["commit_id"], "abc123");
    assert!(payload.get("body").is_none());
    assert_eq!(payload["comments"].as_array().expect("comments").len(), 1);
    assert_eq!(payload["comments"][0]["path"], "src/lib.rs");
    assert_eq!(payload["comments"][0]["subject_type"], "file");
    assert_eq!(
        payload["comments"][0]["body"],
        "🤖 Review completed: no findings."
    );
    assert!(payload["comments"][0].get("line").is_none());
    assert!(payload["comments"][0].get("side").is_none());
    assert!(payload.get("event").is_none());
}

fn review_input_for_head(head_sha: &str) -> ReviewInput {
    ReviewInput {
        subject: nitpick_agent_core::ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..nitpick_agent_core::ReviewSubject::default()
        },
        head_sha: head_sha.into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+pub fn example() {}\n".into(),
        ..ReviewInput::default()
    }
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if condition() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "condition timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[derive(Default)]
struct BlockingProvider {
    started: AtomicUsize,
    released: Mutex<bool>,
    release_changed: Condvar,
}

impl BlockingProvider {
    fn release(&self) {
        *self.released.lock().expect("release lock") = true;
        self.release_changed.notify_all();
    }
}

impl AgentProvider for BlockingProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut released = self.released.lock().expect("release lock");
        while !*released {
            released = self.release_changed.wait(released).expect("release wait");
        }
        Ok(ReviewOutput::default())
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok("done".into())
    }
}

struct NoFindingsProvider;

impl AgentProvider for NoFindingsProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        Ok(ReviewOutput::default())
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok("done".into())
    }
}

struct EmptyReviewSource;

impl ReviewSource for EmptyReviewSource {
    fn name(&self) -> &'static str {
        "empty"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Ok(Vec::new())
    }

    fn review_input(&self, _request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Ok(ReviewInput::default())
    }
}
