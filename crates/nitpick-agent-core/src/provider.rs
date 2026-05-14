use crate::{AgentError, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput};

pub trait AgentProvider: Send + Sync {
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput>;

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String>;

    fn attach_session(&self, _session: &AgentSession) -> AgentResult<()> {
        Err(AgentError::provider(
            "agent provider does not support session resume",
        ))
    }
}
