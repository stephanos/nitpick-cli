use std::sync::Mutex;

use nitpick_agent_core::{
    AgentError, AgentProvider, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput,
};

#[derive(Default)]
pub struct RecordingProvider {
    reviewed_subjects: Mutex<Vec<String>>,
    review_error: Mutex<Option<String>>,
}

impl RecordingProvider {
    pub fn reviewed_subjects(&self) -> Vec<String> {
        self.reviewed_subjects.lock().expect("lock").clone()
    }

    pub fn fail_reviews(&self, error: impl Into<String>) {
        *self.review_error.lock().expect("lock") = Some(error.into());
    }
}

impl AgentProvider for RecordingProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        if let Some(error) = self.review_error.lock().expect("lock").clone() {
            return Err(AgentError::new(error));
        }
        self.reviewed_subjects.lock().expect("lock").push(format!(
            "{}#{}",
            input.subject.repository,
            input.subject.number.expect("pr number")
        ));
        Ok(ReviewOutput {
            summary: "review complete".into(),
            ..ReviewOutput::default()
        })
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok("chat complete".into())
    }
}
