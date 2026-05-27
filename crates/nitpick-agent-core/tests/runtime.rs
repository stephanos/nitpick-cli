use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use nitpick_agent_core::{
    Activity, ActivityId, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore,
    AgentProvider, AgentResult, AgentRuntime, AgentSession, Artifact, ArtifactContent, ArtifactId,
    ArtifactKind, ArtifactStore, ArtifactSyncState, ChatInput, MemoryActivityStore,
    ProviderReviewContext, ProviderRunContext, ReviewComment, ReviewInput, ReviewMode,
    ReviewOutput, ReviewSubject,
};

#[derive(Default)]
struct RecordingProvider {
    calls: Mutex<Vec<&'static str>>,
    review_session_ids: Mutex<Vec<Option<String>>>,
}

impl RecordingProvider {
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn review_session_ids(&self) -> Vec<Option<String>> {
        self.review_session_ids
            .lock()
            .expect("session lock")
            .clone()
    }
}

impl AgentProvider for RecordingProvider {
    fn review(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        self.calls.lock().expect("calls lock").push("review");
        self.review_session_ids
            .lock()
            .expect("session lock")
            .push(session.provider_session_id.clone());
        session.provider_session_id = Some("provider-review-session".into());
        session.messages.push(nitpick_agent_core::AgentMessage {
            role: "provider.stdout".into(),
            content: "review progress\n".into(),
        });
        session.messages.push(nitpick_agent_core::AgentMessage {
            role: "provider.stderr".into(),
            content: "review warning\n".into(),
        });

        Ok(ReviewOutput {
            comments: vec![ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: format!(
                    "Prefer a local artifact before syncing {}.",
                    input.subject.repository
                ),
            }],
        })
    }

    fn chat(
        &self,
        session: &mut AgentSession,
        input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        self.calls.lock().expect("calls lock").push("chat");
        session.provider_session_id = Some("provider-chat-session".into());

        Ok(format!("answered {}", input.prompt))
    }
}

struct StreamingProvider;

impl AgentProvider for StreamingProvider {
    fn review(
        &self,
        session: &mut AgentSession,
        _input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        session.messages.push(nitpick_agent_core::AgentMessage {
            role: "provider.stdout".into(),
            content: "session-start\n".into(),
        });
        context.run_sink.append_stdout(b"streamed stdout\n")?;
        context.run_sink.append_stderr(b"streamed stderr\n")?;
        std::thread::sleep(Duration::from_millis(250));
        Ok(ReviewOutput {
            comments: Vec::new(),
        })
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        unreachable!("chat is not used in this test")
    }
}

struct StreamingChatProvider;

impl AgentProvider for StreamingChatProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        unreachable!("review is not used in this test")
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        context.run_sink.append_stdout(b"chat stdout\n")?;
        context.run_sink.append_stderr(b"chat stderr\n")?;
        std::thread::sleep(Duration::from_millis(250));
        Ok("chat response".into())
    }
}

struct BurstyChatProvider;

impl AgentProvider for BurstyChatProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        unreachable!("review is not used in this test")
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        for index in 0..6 {
            context
                .run_sink
                .append_stdout(format!("chunk-{index}\n").as_bytes())?;
        }
        Ok("chat response".into())
    }
}

#[derive(Default)]
struct CountingStore {
    inner: MemoryActivityStore,
    saves: AtomicUsize,
}

impl CountingStore {
    fn save_count(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

impl ActivityStore for CountingStore {
    fn create(&self, kind: ActivityKind) -> AgentResult<Activity> {
        self.inner.create(kind)
    }

    fn save(&self, activity: &Activity) -> AgentResult<()> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.save(activity)
    }

    fn get(&self, id: &ActivityId) -> AgentResult<Activity> {
        self.inner.get(id)
    }

    fn list(&self) -> AgentResult<Vec<Activity>> {
        self.inner.list()
    }

    fn delete(&self, id: &ActivityId) -> AgentResult<()> {
        <MemoryActivityStore as ActivityStore>::delete(&self.inner, id)
    }

    fn clear_activities(&self) -> AgentResult<usize> {
        self.inner.clear_activities()
    }
}

impl ArtifactStore for CountingStore {
    fn create_artifact(
        &self,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> AgentResult<Artifact> {
        self.inner.create_artifact(activity_id, kind, content)
    }

    fn save_artifacts(&self, artifacts: &[Artifact]) -> AgentResult<()> {
        self.inner.save_artifacts(artifacts)
    }

    fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        self.inner.list_artifacts_for(activity_id)
    }

    fn list_artifacts(&self) -> AgentResult<Vec<Artifact>> {
        self.inner.list_artifacts()
    }

    fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact> {
        self.inner.get_artifact(id)
    }

    fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact> {
        self.inner.update_artifact_sync_state(id, sync_state)
    }

    fn clear_artifacts(&self) -> AgentResult<usize> {
        self.inner.clear_artifacts()
    }
}

#[test]
fn review_activity_assigns_provider_compatible_session_id_before_provider_call() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider.clone(), store);

    runtime
        .start_review(ReviewInput {
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review activity starts");

    let session_ids = provider.review_session_ids();
    let session_id = session_ids[0].as_deref().expect("session id");
    assert!(is_uuid_like(session_id), "{session_id}");
}

#[test]
fn review_activities_get_fresh_provider_session_ids_for_same_review() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);
    let input = ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    };

    let first = runtime
        .create_queued_review_activity(&input)
        .expect("first activity");
    let second = runtime
        .create_queued_review_activity(&input)
        .expect("second activity");

    let first_session = first
        .session
        .provider_session_id
        .as_deref()
        .expect("first session");
    let second_session = second
        .session
        .provider_session_id
        .as_deref()
        .expect("second session");
    assert!(is_uuid_like(first_session), "{first_session}");
    assert!(is_uuid_like(second_session), "{second_session}");
    assert_ne!(first_session, second_session);
}

#[test]
fn review_activity_runs_provider_and_persists_completion() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider.clone(), store.clone());

    let activity = runtime
        .start_review(ReviewInput {
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                title: "Improve review agent".into(),
                author: "stephan".into(),
            },
            diff: "diff --git a/lib.rs b/lib.rs".into(),
            ..ReviewInput::default()
        })
        .expect("review activity starts");

    assert_eq!(provider.calls(), ["review"]);
    assert_eq!(activity.kind, ActivityKind::Review);
    assert_eq!(activity.status, ActivityStatus::Completed);
    assert!(
        activity.started_at_unix.is_some(),
        "review should record when it started running"
    );
    assert_eq!(
        activity.session.provider_session_id.as_deref(),
        Some("provider-review-session")
    );
    assert!(
        activity
            .session
            .messages
            .iter()
            .any(|message| message.role == "nitpick.review.head_sha")
    );
    assert!(matches!(
        activity.output,
        Some(ActivityOutput::Review(ReviewOutput { ref comments }))
            if comments.len() == 1 && comments[0].path == "src/lib.rs"
    ));

    let persisted = store.get(&activity.id).expect("persisted activity");
    assert_eq!(persisted, activity);

    let artifacts = store
        .list_artifacts_for(&activity.id)
        .expect("local artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].activity_id, activity.id);
    assert_eq!(artifacts[0].kind, ArtifactKind::ReviewComment);
    assert_eq!(artifacts[0].sync_state, ArtifactSyncState::LocalOnly);
    assert_eq!(
        artifacts[0].content,
        ArtifactContent::ReviewComment(ReviewComment {
            path: "src/lib.rs".into(),
            line: 12,
            body: "Prefer a local artifact before syncing acme/platform.".into(),
        })
    );
}

#[test]
fn review_activity_persists_retry_metadata() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let input = ReviewInput {
        repo_dir: temp_repo_dir(),
        source: "github".into(),
        review_mode: ReviewMode::Requested,
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            title: "Add watcher".into(),
            author: "stephan".into(),
        },
        head_sha: "abc123".into(),
        diff: "diff --git a/src.rs b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
        ..ReviewInput::default()
    };

    let activity = runtime.start_review(input.clone()).expect("review");

    let review_retry = activity
        .retry
        .as_ref()
        .expect("retry")
        .review
        .as_ref()
        .expect("review");
    assert_eq!(review_retry.source, "github");
    assert_eq!(review_retry.repository, "acme/platform");
    assert_eq!(review_retry.number, Some(42));
    assert_eq!(review_retry.head_sha, "abc123");
    assert_eq!(review_retry.review_mode, ReviewMode::Requested);
    assert!(!review_retry.force);
    assert_eq!(
        store.get(&activity.id).expect("stored").retry,
        activity.retry
    );
}

#[test]
fn review_activity_persists_explicit_retry_source() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);
    let input = ReviewInput {
        repo_dir: temp_repo_dir(),
        source: "manual-github".into(),
        review_mode: ReviewMode::Requested,
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    };

    let activity = runtime.start_review(input).expect("review");

    assert_eq!(
        activity
            .retry
            .as_ref()
            .expect("retry")
            .review
            .as_ref()
            .expect("review")
            .source,
        "manual-github"
    );
}

#[test]
fn self_review_activity_retry_source_defaults_to_local() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);
    let input = ReviewInput {
        repo_dir: temp_repo_dir(),
        review_mode: ReviewMode::SelfReview,
        subject: ReviewSubject {
            repository: "local-repo".into(),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        ..ReviewInput::default()
    };

    let activity = runtime.start_review(input).expect("review");

    assert_eq!(
        activity
            .retry
            .as_ref()
            .expect("retry")
            .review
            .as_ref()
            .expect("review")
            .source,
        "local"
    );
}

#[test]
fn queued_review_activity_persists_retry_metadata() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let input = ReviewInput {
        repo_dir: temp_repo_dir(),
        source: "github".into(),
        review_mode: ReviewMode::Requested,
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        head_sha: "abc123".into(),
        force: true,
        ..ReviewInput::default()
    };

    let activity = runtime
        .create_queued_review_activity(&input)
        .expect("queued review");

    let review_retry = activity
        .retry
        .as_ref()
        .expect("retry")
        .review
        .as_ref()
        .expect("review");
    assert_eq!(review_retry.source, "github");
    assert_eq!(review_retry.repository, "acme/platform");
    assert_eq!(review_retry.number, Some(42));
    assert_eq!(review_retry.head_sha, "abc123");
    assert_eq!(review_retry.review_mode, ReviewMode::Requested);
    assert!(review_retry.force);
    assert_eq!(
        store.get(&activity.id).expect("stored").retry,
        activity.retry
    );
}

#[test]
fn activity_json_without_retry_deserializes_with_no_retry_metadata() {
    let activity: Activity = serde_json::from_value(serde_json::json!({
        "id": "activity-1",
        "kind": "Review",
        "status": "Queued",
        "session": {
            "provider": null,
            "provider_session_id": null,
            "status": "Ready",
            "messages": []
        },
        "output": null,
        "error": null
    }))
    .expect("activity");

    assert_eq!(activity.retry, None);
}

#[test]
fn review_activity_persists_provider_logs_while_provider_is_running() {
    let provider = Arc::new(StreamingProvider);
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let activity = runtime
        .create_review_activity(&ReviewInput::default())
        .expect("activity");
    let activity_id = activity.id.clone();
    let runtime_thread = std::thread::spawn(move || {
        runtime
            .run_review(activity, ReviewInput::default())
            .expect("review")
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let persisted = store.get(&activity_id).expect("persisted activity");
        let stdout = persisted
            .session
            .messages
            .iter()
            .find(|message| message.role == "provider.stdout")
            .map(|message| message.content.as_str());
        let stderr = persisted
            .session
            .messages
            .iter()
            .find(|message| message.role == "provider.stderr")
            .map(|message| message.content.as_str());
        if stdout == Some("streamed stdout") && stderr == Some("streamed stderr") {
            assert_eq!(persisted.status, ActivityStatus::Running);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "provider logs were not persisted while running"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let activity = runtime_thread.join().expect("runtime thread");
    assert_eq!(activity.status, ActivityStatus::Completed);
}

#[test]
fn chat_activity_persists_provider_logs_while_provider_is_running() {
    let provider = Arc::new(StreamingChatProvider);
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let activity = runtime.create_chat_activity().expect("activity");
    let activity_id = activity.id.clone();
    let runtime_thread = std::thread::spawn(move || {
        runtime
            .run_chat(activity, ChatInput::default())
            .expect("chat")
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let persisted = store.get(&activity_id).expect("persisted activity");
        let stdout = persisted
            .session
            .messages
            .iter()
            .find(|message| message.role == "provider.stdout")
            .map(|message| message.content.as_str());
        let stderr = persisted
            .session
            .messages
            .iter()
            .find(|message| message.role == "provider.stderr")
            .map(|message| message.content.as_str());
        if stdout == Some("chat stdout") && stderr == Some("chat stderr") {
            assert_eq!(persisted.status, ActivityStatus::Running);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "chat provider logs were not persisted while running"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let activity = runtime_thread.join().expect("runtime thread");
    assert_eq!(activity.status, ActivityStatus::Completed);
}

#[test]
fn provider_log_sink_throttles_bursty_activity_saves() {
    let provider = Arc::new(BurstyChatProvider);
    let store = Arc::new(CountingStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());

    let activity = runtime
        .start_chat(ChatInput::default())
        .expect("chat activity");

    let stdout = activity
        .session
        .messages
        .iter()
        .find(|message| message.role == "provider.stdout")
        .map(|message| message.content.as_str());
    assert_eq!(
        stdout,
        Some("chunk-0\nchunk-1\nchunk-2\nchunk-3\nchunk-4\nchunk-5")
    );
    assert!(
        store.save_count() <= 5,
        "bursty provider output should not save once per chunk"
    );
}

#[test]
fn chat_activity_uses_the_same_activity_model() {
    let provider = Arc::new(RecordingProvider::default());
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider.clone(), store.clone());

    let activity = runtime
        .start_chat(ChatInput {
            prompt: "what changed?".into(),
            context: "full pull request".into(),
            ..ChatInput::default()
        })
        .expect("chat activity starts");

    assert_eq!(provider.calls(), ["chat"]);
    assert_eq!(activity.kind, ActivityKind::Chat);
    assert_eq!(activity.status, ActivityStatus::Completed);
    assert!(
        activity.started_at_unix.is_some(),
        "chat should record when it started running"
    );
    assert_eq!(
        activity.output,
        Some(ActivityOutput::Chat("answered what changed?".into()))
    );
    assert_eq!(
        activity.session.provider_session_id.as_deref(),
        Some("provider-chat-session")
    );

    let artifacts = store
        .list_artifacts_for(&activity.id)
        .expect("local artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::ChatResponse);
    assert_eq!(artifacts[0].sync_state, ArtifactSyncState::LocalOnly);
    assert_eq!(
        artifacts[0].content,
        ArtifactContent::ChatResponse("answered what changed?".into())
    );
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .filter(|(index, _)| ![8, 13, 18, 23].contains(index))
            .all(|(_, byte)| byte.is_ascii_hexdigit())
}

fn temp_repo_dir() -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.keep();
    std::fs::write(path.join("src.rs"), "fn main() {}\n").expect("write source");
    path
}
