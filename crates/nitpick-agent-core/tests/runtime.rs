use std::sync::{Arc, Mutex};

use nitpick_agent_core::{
    ActivityKind, ActivityOutput, ActivityStatus, AgentProvider, AgentResult, AgentRuntime,
    AgentSession, ArtifactContent, ArtifactKind, ArtifactSyncState, ChatInput, MemoryActivityStore,
    ReviewComment, ReviewInput, ReviewOutput, ReviewSubject,
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

        Ok(ReviewOutput {
            summary: format!("reviewed {}", input.subject.repository),
            comments: vec![ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "Prefer a local artifact before syncing.".into(),
            }],
        })
    }

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String> {
        self.calls.lock().expect("calls lock").push("chat");
        session.provider_session_id = Some("provider-chat-session".into());

        Ok(format!("answered {}", input.prompt))
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
    assert_eq!(
        activity.session.provider_session_id.as_deref(),
        Some("provider-review-session")
    );
    assert!(matches!(
        activity.output,
        Some(ActivityOutput::Review(ReviewOutput { ref summary, .. }))
            if summary == "reviewed acme/platform"
    ));

    let persisted = store.get(&activity.id).expect("persisted activity");
    assert_eq!(persisted, activity);

    let artifacts = store
        .list_artifacts_for(&activity.id)
        .expect("local artifacts");
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0].activity_id, activity.id);
    assert_eq!(artifacts[0].kind, ArtifactKind::ReviewSummary);
    assert_eq!(artifacts[0].sync_state, ArtifactSyncState::LocalOnly);
    assert_eq!(
        artifacts[0].content,
        ArtifactContent::ReviewSummary("reviewed acme/platform".into())
    );
    assert_eq!(artifacts[1].kind, ArtifactKind::ReviewComment);
    assert_eq!(artifacts[1].sync_state, ArtifactSyncState::LocalOnly);
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
