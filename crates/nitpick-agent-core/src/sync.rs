use crate::{AgentResult, Artifact, ArtifactSyncState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactSyncOutcome {
    pub sync_state: ArtifactSyncState,
    pub remote_id: Option<String>,
}

pub trait ArtifactSyncDestination {
    fn name(&self) -> &'static str;

    fn sync(&self, artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome>;
}
