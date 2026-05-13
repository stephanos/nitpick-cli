use crate::{AgentResult, Artifact, ArtifactSyncState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactSyncOutcome {
    pub sync_state: ArtifactSyncState,
    pub remote_id: Option<String>,
}

pub trait ArtifactSyncDestination: Send + Sync {
    fn name(&self) -> &'static str;

    fn sync(&self, artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome>;

    fn sync_batch(&self, artifacts: &[Artifact]) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        artifacts
            .iter()
            .map(|artifact| self.sync(artifact))
            .collect()
    }
}
