use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use nitpick_agent_core::{
    ActivityKind, ActivityOutput, ActivityStatus, AgentProvider, AgentResult, AgentRuntime,
    AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState, ChatInput, MemoryActivityStore,
    ProviderLogSink, ReviewComment, ReviewInput, ReviewOutput, ReviewSubject,
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
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput> {
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

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String> {
        self.calls.lock().expect("calls lock").push("chat");
        session.provider_session_id = Some("provider-chat-session".into());

        Ok(format!("answered {}", input.prompt))
    }
}

struct StreamingProvider;

impl AgentProvider for StreamingProvider {
    fn review_with_log_sink(
        &self,
        session: &mut AgentSession,
        _input: &ReviewInput,
        log_sink: &dyn ProviderLogSink,
    ) -> AgentResult<ReviewOutput> {
        session.messages.push(nitpick_agent_core::AgentMessage {
            role: "provider.stdout".into(),
            content: "session-start\n".into(),
        });
        log_sink.append_stdout(b"streamed stdout\n")?;
        log_sink.append_stderr(b"streamed stderr\n")?;
        std::thread::sleep(Duration::from_millis(250));
        Ok(ReviewOutput {
            comments: Vec::new(),
        })
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        unreachable!("runtime should pass a provider log sink")
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        unreachable!("chat is not used in this test")
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
    assert_eq!(activity.session.messages.len(), 2);
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
