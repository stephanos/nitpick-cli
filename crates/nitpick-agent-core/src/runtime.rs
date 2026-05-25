use std::sync::Arc;

use crate::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore, AgentProvider,
    AgentResult, ArtifactContent, ArtifactKind, ChatInput, ProviderLogSink, ReviewInput,
    ReviewOutput, SessionStatus, review_identity::ReviewIdentity,
};

const MAX_PROVIDER_LOG_BYTES: usize = 64 * 1024;

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
        let log_sink = ActivityProviderLogSink {
            store: self.store.clone(),
            activity_id: activity.id.clone(),
        };
        match self
            .provider
            .review_with_log_sink(&mut activity.session, &input, &log_sink)
        {
            Ok(output) => {
                merge_provider_logs_from_store(self.store.as_ref(), &mut activity)?;
                let artifacts = review_artifacts(self.store.as_ref(), &activity, &output)?;
                activity.status = ActivityStatus::Completed;
                activity.session.status = SessionStatus::Completed;
                activity.output = Some(ActivityOutput::Review(output));
                self.store.save_artifacts(&artifacts)?;
            }
            Err(error) => {
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
        match self.provider.chat(&mut activity.session, &input) {
            Ok(output) => {
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

struct ActivityProviderLogSink {
    store: Arc<dyn ActivityStore>,
    activity_id: crate::ActivityId,
}

impl ProviderLogSink for ActivityProviderLogSink {
    fn append_stdout(&self, bytes: &[u8]) -> AgentResult<()> {
        self.append_provider_log("provider.stdout", bytes)
    }

    fn append_stderr(&self, bytes: &[u8]) -> AgentResult<()> {
        self.append_provider_log("provider.stderr", bytes)
    }
}

impl ActivityProviderLogSink {
    fn append_provider_log(&self, role: &str, bytes: &[u8]) -> AgentResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut activity = self.store.get(&self.activity_id)?;
        append_provider_log(&mut activity, role, bytes);
        activity.touch();
        self.store.save(&activity)
    }
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
        .filter(|message| is_provider_log_role(&message.role))
    {
        upsert_provider_log(activity, &message.role, &message.content);
    }
    Ok(())
}

fn upsert_provider_log(activity: &mut Activity, role: &str, content: &str) {
    if let Some(message) = activity
        .session
        .messages
        .iter_mut()
        .find(|message| message.role == role)
    {
        message.content = content.into();
    } else {
        activity.session.messages.push(crate::AgentMessage {
            role: role.into(),
            content: content.into(),
        });
    }
}

fn append_provider_log(activity: &mut Activity, role: &str, bytes: &[u8]) {
    let existing = activity
        .session
        .messages
        .iter()
        .find(|message| message.role == role)
        .map(|message| message.content.as_bytes())
        .unwrap_or_default();
    let mut merged = Vec::with_capacity(existing.len().saturating_add(bytes.len()));
    merged.extend_from_slice(existing);
    merged.extend_from_slice(bytes);
    upsert_provider_log(activity, role, &bounded_provider_log(&merged));
}

fn bounded_provider_log(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_PROVIDER_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_PROVIDER_LOG_BYTES);
    let mut value = String::from_utf8_lossy(&bytes[start..]).trim().to_owned();
    if truncated {
        value = format!("[truncated to last {MAX_PROVIDER_LOG_BYTES} bytes]\n{value}");
    }
    value
}

fn is_provider_log_role(role: &str) -> bool {
    matches!(
        role,
        "provider.stdout" | "provider.stderr" | "provider.sandbox"
    )
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
