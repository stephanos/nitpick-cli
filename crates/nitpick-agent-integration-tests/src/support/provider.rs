use std::sync::Mutex;

use nitpick_agent_core::{
    AgentProvider, AgentResult, AgentSession, ChatInput, ReviewInput, ReviewOutput,
};

#[derive(Default)]
pub struct RecordingProvider {
    reviewed_subjects: Mutex<Vec<String>>,
}

impl RecordingProvider {
    pub fn reviewed_subjects(&self) -> Vec<String> {
        self.reviewed_subjects.lock().expect("lock").clone()
    }
}

impl AgentProvider for RecordingProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
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
