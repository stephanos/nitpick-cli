use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub provider: Option<crate::AgentProviderKind>,
    pub provider_session_id: Option<String>,
    pub status: SessionStatus,
    pub messages: Vec<AgentMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    #[default]
    Ready,
    Running,
    Completed,
    Error(String),
}
