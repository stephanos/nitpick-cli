use std::path::PathBuf;

use crate::{AgentError, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewToolConfig {
    pub mcp_config_path: PathBuf,
    pub instructions: String,
}

pub trait AgentProvider: Send + Sync {
    fn review(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput>;

    fn supports_review_tools(&self) -> bool {
        false
    }

    fn chat(
        &self,
        session: &mut AgentSession,
        input: &ChatInput,
        context: ProviderRunContext<'_>,
    ) -> AgentResult<String>;

    fn attach_session(&self, _session: &AgentSession) -> AgentResult<()> {
        Err(AgentError::provider(
            "agent provider does not support session resume",
        ))
    }
}

#[derive(Clone, Copy)]
pub struct ProviderReviewContext<'a> {
    pub tools: Option<&'a ReviewToolConfig>,
    pub run_sink: &'a dyn ProviderRunSink,
}

impl<'a> ProviderReviewContext<'a> {
    pub fn new(run_sink: &'a dyn ProviderRunSink) -> Self {
        Self {
            tools: None,
            run_sink,
        }
    }

    pub fn with_tools(mut self, tools: &'a ReviewToolConfig) -> Self {
        self.tools = Some(tools);
        self
    }
}

#[derive(Clone, Copy)]
pub struct ProviderRunContext<'a> {
    pub run_sink: &'a dyn ProviderRunSink,
}

impl<'a> ProviderRunContext<'a> {
    pub fn new(run_sink: &'a dyn ProviderRunSink) -> Self {
        Self { run_sink }
    }
}

pub trait ProviderRunSink: Send + Sync {
    fn append_stdout(&self, bytes: &[u8]) -> AgentResult<()>;

    fn append_stderr(&self, bytes: &[u8]) -> AgentResult<()>;

    fn set_run_diagnostic(&self, _content: &str) -> AgentResult<()> {
        Ok(())
    }

    fn flush(&self) -> AgentResult<()> {
        Ok(())
    }

    fn is_cancelled(&self) -> AgentResult<bool> {
        Ok(false)
    }
}

pub struct NoopProviderRunSink;

impl ProviderRunSink for NoopProviderRunSink {
    fn append_stdout(&self, _bytes: &[u8]) -> AgentResult<()> {
        Ok(())
    }

    fn append_stderr(&self, _bytes: &[u8]) -> AgentResult<()> {
        Ok(())
    }
}
