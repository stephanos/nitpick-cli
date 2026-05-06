use crate::{ReviewOutput, session::AgentSession};
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
}

impl Activity {
    pub fn new(id: ActivityId, kind: ActivityKind) -> Self {
        Self {
            id,
            kind,
            status: ActivityStatus::Queued,
            session: AgentSession::default(),
            output: None,
            error: None,
        }
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
