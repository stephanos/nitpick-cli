use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore, AgentProvider,
    AgentResult, ArtifactContent, ArtifactKind, ChatInput, ProviderReviewContext,
    ProviderRunContext, ProviderRunSink, ReviewInput, ReviewOutput, SessionStatus, provider_log,
    review_identity::ReviewIdentity,
};

const PROVIDER_LOG_SAVE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct AgentRuntime {
    provider: Arc<dyn AgentProvider>,
    store: Arc<dyn ActivityStore>,
}

impl AgentRuntime {
    pub fn new(provider: Arc<dyn AgentProvider>, store: Arc<dyn ActivityStore>) -> Self {
        Self { provider, store }
    }

    pub fn start_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let activity = self.create_review_activity(&input)?;

        self.run_review(activity, input)
    }

    pub fn create_review_activity(&self, input: &ReviewInput) -> AgentResult<Activity> {
        let activity = self.create_queued_review_activity(input)?;
        self.mark_activity_running(activity)
    }

    pub fn create_queued_review_activity(&self, input: &ReviewInput) -> AgentResult<Activity> {
        let mut activity = self.store.create(ActivityKind::Review)?;
        activity.label_review(input);
        if activity.session.provider_session_id.is_none() {
            activity.session.provider_session_id = Some(review_session_id(input));
        }
        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn mark_activity_running(&self, mut activity: Activity) -> AgentResult<Activity> {
        activity.status = ActivityStatus::Running;
        activity.session.status = SessionStatus::Running;
        activity.mark_started();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn run_review(&self, mut activity: Activity, input: ReviewInput) -> AgentResult<Activity> {
        activity = self.mark_activity_running(activity)?;
        activity.label_review(&input);
        if activity.session.provider_session_id.is_none() {
            activity.session.provider_session_id = Some(review_session_id(&input));
        }
        activity.touch();
        self.store.save(&activity)?;
        let run_sink = ActivityProviderRunSink::new(self.store.clone(), activity.id.clone());
        let context = ProviderReviewContext::new(&run_sink);
        match self.provider.review(&mut activity.session, &input, context) {
            Ok(output) => {
                run_sink.flush()?;
                merge_provider_logs_from_store(self.store.as_ref(), &mut activity)?;
                let artifacts = review_artifacts(self.store.as_ref(), &activity, &output)?;
                activity.status = ActivityStatus::Completed;
                activity.session.status = SessionStatus::Completed;
                activity.output = Some(ActivityOutput::Review(output));
                self.store.save_artifacts(&artifacts)?;
            }
            Err(error) => {
                run_sink.flush()?;
                merge_provider_logs_from_store(self.store.as_ref(), &mut activity)?;
                activity.status = ActivityStatus::Error;
                activity.session.status = SessionStatus::Error(error.to_string());
                activity.error = Some(error.to_string());
            }
        }

        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn start_chat(&self, input: ChatInput) -> AgentResult<Activity> {
        let activity = self.create_chat_activity()?;

        self.run_chat(activity, input)
    }

    pub fn create_chat_activity(&self) -> AgentResult<Activity> {
        self.create_running_activity(ActivityKind::Chat)
    }

    pub fn run_chat(&self, mut activity: Activity, input: ChatInput) -> AgentResult<Activity> {
        let run_sink = ActivityProviderRunSink::new(self.store.clone(), activity.id.clone());
        let context = ProviderRunContext::new(&run_sink);
        match self.provider.chat(&mut activity.session, &input, context) {
            Ok(output) => {
                run_sink.flush()?;
                merge_provider_logs_from_store(self.store.as_ref(), &mut activity)?;
                let artifact = self.store.create_artifact(
                    activity.id.clone(),
                    ArtifactKind::ChatResponse,
                    ArtifactContent::ChatResponse(output.clone()),
                )?;
                activity.status = ActivityStatus::Completed;
                activity.session.status = SessionStatus::Completed;
                activity.output = Some(ActivityOutput::Chat(output));
                self.store.save_artifacts(&[artifact])?;
            }
            Err(error) => {
                run_sink.flush()?;
                merge_provider_logs_from_store(self.store.as_ref(), &mut activity)?;
                activity.status = ActivityStatus::Error;
                activity.session.status = SessionStatus::Error(error.to_string());
                activity.error = Some(error.to_string());
            }
        }

        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn list_activities(&self) -> AgentResult<Vec<Activity>> {
        self.store.list()
    }

    fn create_running_activity(&self, kind: ActivityKind) -> AgentResult<Activity> {
        let mut activity = self.store.create(kind)?;
        activity.status = ActivityStatus::Running;
        activity.session.status = SessionStatus::Running;
        activity.mark_started();
        self.store.save(&activity)?;
        Ok(activity)
    }
}

struct ActivityProviderRunSink {
    store: Arc<dyn ActivityStore>,
    activity_id: crate::ActivityId,
    state: Mutex<ActivityProviderRunSinkState>,
}

impl ActivityProviderRunSink {
    fn new(store: Arc<dyn ActivityStore>, activity_id: crate::ActivityId) -> Self {
        Self {
            store,
            activity_id,
            state: Mutex::new(ActivityProviderRunSinkState::default()),
        }
    }
}

#[derive(Default)]
struct ActivityProviderRunSinkState {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    run_diagnostic: Option<String>,
    last_save: Option<Instant>,
    dirty: bool,
}

impl ProviderRunSink for ActivityProviderRunSink {
    fn append_stdout(&self, bytes: &[u8]) -> AgentResult<()> {
        self.append_provider_log(ProviderRunStream::Stdout, bytes)
    }

    fn append_stderr(&self, bytes: &[u8]) -> AgentResult<()> {
        self.append_provider_log(ProviderRunStream::Stderr, bytes)
    }

    fn set_run_diagnostic(&self, content: &str) -> AgentResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::AgentError::io("provider run sink lock", "poisoned"))?;
        state.run_diagnostic = Some(content.into());
        state.dirty = true;
        self.save_provider_logs(&mut state)
    }

    fn flush(&self) -> AgentResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::AgentError::io("provider run sink lock", "poisoned"))?;
        if !state.dirty {
            return Ok(());
        }
        self.save_provider_logs(&mut state)
    }
}

impl ActivityProviderRunSink {
    fn append_provider_log(&self, stream: ProviderRunStream, bytes: &[u8]) -> AgentResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::AgentError::io("provider run sink lock", "poisoned"))?;
        let first_chunk_for_stream = match stream {
            ProviderRunStream::Stdout => state.stdout.is_empty(),
            ProviderRunStream::Stderr => state.stderr.is_empty(),
        };
        match stream {
            ProviderRunStream::Stdout => state.stdout.extend_from_slice(bytes),
            ProviderRunStream::Stderr => state.stderr.extend_from_slice(bytes),
        }
        state.dirty = true;
        let should_save = state
            .last_save
            .map(|last_save| last_save.elapsed() >= PROVIDER_LOG_SAVE_INTERVAL)
            .unwrap_or(true)
            || first_chunk_for_stream;
        if should_save {
            self.save_provider_logs(&mut state)?;
        }
        Ok(())
    }

    fn save_provider_logs(&self, state: &mut ActivityProviderRunSinkState) -> AgentResult<()> {
        let mut activity = self.store.get(&self.activity_id)?;
        if !state.stdout.is_empty() {
            provider_log::upsert_provider_log(
                &mut activity.session,
                "provider.stdout",
                &provider_log::bounded_provider_log(&state.stdout),
            );
        }
        if !state.stderr.is_empty() {
            provider_log::upsert_provider_log(
                &mut activity.session,
                "provider.stderr",
                &provider_log::bounded_provider_log(&state.stderr),
            );
        }
        if let Some(run_diagnostic) = &state.run_diagnostic {
            provider_log::upsert_provider_log(
                &mut activity.session,
                "provider.run",
                run_diagnostic,
            );
        }
        activity.touch();
        self.store.save(&activity)?;
        state.last_save = Some(Instant::now());
        state.dirty = false;
        Ok(())
    }
}

enum ProviderRunStream {
    Stdout,
    Stderr,
}

fn merge_provider_logs_from_store(
    store: &dyn ActivityStore,
    activity: &mut Activity,
) -> AgentResult<()> {
    let persisted = store.get(&activity.id)?;
    for message in persisted
        .session
        .messages
        .iter()
        .filter(|message| provider_log::is_provider_log_role(&message.role))
    {
        provider_log::upsert_provider_log(&mut activity.session, &message.role, &message.content);
    }
    Ok(())
}

pub fn review_session_id(input: &ReviewInput) -> String {
    uuid_from_key(&ReviewIdentity::from_input(input).session_key())
}

fn uuid_from_key(key: &str) -> String {
    let mut hash = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58du128;
    for byte in key.as_bytes() {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013bu128);
    }
    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn review_artifacts(
    store: &dyn ActivityStore,
    activity: &Activity,
    output: &ReviewOutput,
) -> AgentResult<Vec<crate::Artifact>> {
    let mut artifacts = Vec::with_capacity(output.comments.len());

    for comment in &output.comments {
        artifacts.push(store.create_artifact(
            activity.id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(comment.clone()),
        )?);
    }

    Ok(artifacts)
}
