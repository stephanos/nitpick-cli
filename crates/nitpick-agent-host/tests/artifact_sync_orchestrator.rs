use nitpick_agent_core::{
    ActivityId, Artifact, ArtifactContent, ArtifactId, ArtifactKind, ArtifactSyncOutcome,
    ArtifactSyncState, MemoryActivityStore,
};
use nitpick_agent_host::ArtifactSyncOrchestrator;
use std::sync::Arc;

#[test]
fn artifact_sync_orchestrator_applies_batch_outcomes_to_store() {
    let store = Arc::new(MemoryActivityStore::default());
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary".into()),
    );
    store
        .save_artifacts(std::slice::from_ref(&artifact))
        .expect("save");
    let outcome = ArtifactSyncOutcome {
        sync_state: ArtifactSyncState::Synced {
            destination: "github".into(),
            remote_id: Some("remote-1".into()),
        },
        remote_id: Some("remote-1".into()),
    };

    let updated = ArtifactSyncOrchestrator::new(store)
        .apply_batch_outcomes("github", std::slice::from_ref(&artifact), vec![outcome])
        .expect("apply outcomes");

    assert_eq!(updated.len(), 1);
    assert_eq!(
        updated[0].sync_state,
        ArtifactSyncState::Synced {
            destination: "github".into(),
            remote_id: Some("remote-1".into()),
        }
    );
}
