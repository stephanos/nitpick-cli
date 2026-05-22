use std::path::PathBuf;

use crate::{AgentError, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewToolConfig {
    pub mcp_config_path: PathBuf,
    pub instructions: String,
}

pub trait AgentProvider: Send + Sync {
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput>;

    fn supports_review_tools(&self) -> bool {
        false
    }

    fn review_with_tools(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        _tools: &ReviewToolConfig,
    ) -> AgentResult<ReviewOutput> {
        self.review(session, input)
    }

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String>;

    fn attach_session(&self, _session: &AgentSession) -> AgentResult<()> {
        Err(AgentError::provider(
            "agent provider does not support session resume",
        ))
    }
}
