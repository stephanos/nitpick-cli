use std::path::PathBuf;

use crate::{AgentError, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewToolConfig {
    pub mcp_config_path: PathBuf,
    pub instructions: String,
}

pub trait AgentProvider: Send + Sync {
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput>;

    fn review_with_log_sink(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        _log_sink: &dyn ProviderLogSink,
    ) -> AgentResult<ReviewOutput> {
        self.review(session, input)
    }

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

    fn review_with_tools_and_log_sink(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        tools: &ReviewToolConfig,
        _log_sink: &dyn ProviderLogSink,
    ) -> AgentResult<ReviewOutput> {
        self.review_with_tools(session, input, tools)
    }

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String>;

    fn attach_session(&self, _session: &AgentSession) -> AgentResult<()> {
        Err(AgentError::provider(
            "agent provider does not support session resume",
        ))
    }
}

pub trait ProviderLogSink: Send + Sync {
    fn append_stdout(&self, bytes: &[u8]) -> AgentResult<()>;

    fn append_stderr(&self, bytes: &[u8]) -> AgentResult<()>;
}
