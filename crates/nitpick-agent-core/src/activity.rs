use std::time::{SystemTime, UNIX_EPOCH};

use crate::{ReviewInput, ReviewOutput, review_identity::ReviewIdentity, session::AgentSession};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActivityId(String);

impl ActivityId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ActivityId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub id: ActivityId,
    pub kind: ActivityKind,
    pub status: ActivityStatus,
    pub session: AgentSession,
    pub output: Option<ActivityOutput>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<ActivityRetryMetadata>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "unix_now")]
    pub created_at_unix: u64,
    #[serde(default)]
    pub started_at_unix: Option<u64>,
    #[serde(default = "unix_now")]
    pub updated_at_unix: u64,
}

impl Activity {
    pub fn new(id: ActivityId, kind: ActivityKind) -> Self {
        let now = unix_now();
        Self {
            id,
            kind,
            status: ActivityStatus::Queued,
            session: AgentSession::default(),
            output: None,
            error: None,
            retry: None,
            label: None,
            created_at_unix: now,
            started_at_unix: None,
            updated_at_unix: now,
        }
    }

    pub fn mark_started(&mut self) {
        self.started_at_unix.get_or_insert_with(unix_now);
        self.touch();
    }

    pub fn touch(&mut self) {
        self.updated_at_unix = unix_now();
    }

    pub fn label_review(&mut self, input: &ReviewInput) {
        self.label = Some(ReviewIdentity::from_input(input).activity_label());
    }

    pub fn set_review_retry(&mut self, input: &ReviewInput, force: bool) {
        self.retry = Some(ActivityRetryMetadata {
            review: Some(ReviewRetryMetadata {
                source: review_retry_source(input),
                repository: input.subject.repository.clone(),
                number: input.subject.number,
                head_sha: input.head_sha.clone(),
                review_mode: input.review_mode.clone(),
                force,
            }),
            resolved_by_activity: None,
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRetryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewRetryMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by_activity: Option<ActivityId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRetryMetadata {
    pub source: String,
    pub repository: String,
    pub number: Option<u64>,
    pub head_sha: String,
    pub review_mode: crate::ReviewMode,
    pub force: bool,
}

fn review_retry_source(input: &ReviewInput) -> String {
    if !input.source.is_empty() {
        return input.source.clone();
    }
    match input.review_mode {
        crate::ReviewMode::Requested => "github".into(),
        crate::ReviewMode::SelfReview => "local".into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityKind {
    Review,
    Chat,
    Maintenance,
    Discovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityStatus {
    Queued,
    Running,
    Completed,
    Error,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityOutput {
    Review(ReviewOutput),
    Chat(String),
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
