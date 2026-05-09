use std::time::{SystemTime, UNIX_EPOCH};

use crate::{ReviewInput, ReviewOutput, session::AgentSession};
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
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "unix_now")]
    pub created_at_unix: u64,
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
            label: None,
            created_at_unix: now,
            updated_at_unix: now,
        }
    }

    pub fn touch(&mut self) {
        self.updated_at_unix = unix_now();
    }

    pub fn label_review(&mut self, input: &ReviewInput) {
        self.label = Some(match input.subject.number {
            Some(number) => format!("review on {}#{number}", input.subject.repository),
            None => format!("review on {}", input.subject.repository),
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityKind {
    Review,
    Chat,
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
