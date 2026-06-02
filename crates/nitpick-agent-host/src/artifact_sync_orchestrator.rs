use std::sync::Arc;

use nitpick_agent_core::{ActivityStore, AgentError, AgentResult, Artifact, ArtifactSyncOutcome};

pub struct ArtifactSyncOrchestrator {
    store: Arc<dyn ActivityStore>,
}

impl ArtifactSyncOrchestrator {
    pub fn new(store: Arc<dyn ActivityStore>) -> Self {
        Self { store }
    }

    pub fn apply_batch_outcomes(
        &self,
        destination: &str,
        artifacts: &[Artifact],
        outcomes: Vec<ArtifactSyncOutcome>,
    ) -> AgentResult<Vec<Artifact>> {
        if outcomes.len() != artifacts.len() {
            return Err(AgentError::invalid_input(format!(
                "sync destination `{destination}` returned {} outcome(s) for {} artifact(s)",
                outcomes.len(),
                artifacts.len()
            )));
        }

        let mut updated = Vec::with_capacity(artifacts.len());
        for (artifact, outcome) in artifacts.iter().zip(outcomes) {
            updated.push(
                self.store
                    .update_artifact_sync_state(&artifact.id, outcome.sync_state)?,
            );
        }
        Ok(updated)
    }
}
