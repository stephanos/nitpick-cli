#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    message: String,
}

impl AgentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentError {}

pub type AgentResult<T> = Result<T, AgentError>;
