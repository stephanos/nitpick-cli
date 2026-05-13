use nitpick_agent_core::{
    ActivityKind, ActivityStore, ArtifactContent, ArtifactKind, ArtifactStore, ArtifactSyncState,
    FsActivityStore,
};

#[test]
fn filesystem_store_keeps_activities_and_artifacts_after_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsActivityStore::new(dir.path()).expect("store");
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id.clone(),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local review result".into()),
        )
        .expect("artifact");
    store.save_artifacts(&[artifact]).expect("save artifact");

    let reopened = FsActivityStore::new(dir.path()).expect("reopen store");

    assert_eq!(
        reopened.get(&activity.id).expect("activity survives"),
        activity
    );
    assert_eq!(
        reopened
            .list_artifacts_for(&activity.id)
            .expect("artifacts survive")
            .len(),
        1
    );
}

#[test]
fn filesystem_store_updates_artifact_sync_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsActivityStore::new(dir.path()).expect("store");
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("local review result".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");

    let updated = store
        .update_artifact_sync_state(
            &artifact_id,
            ArtifactSyncState::Pending {
                destination: "github".into(),
                remote_id: None,
                remote_url: None,
            },
        )
        .expect("update sync state");

    assert_eq!(
        updated.sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
        }
    );

    let reopened = FsActivityStore::new(dir.path()).expect("reopen store");
    assert_eq!(
        reopened
            .get_artifact(&artifact_id)
            .expect("artifact survives")
            .sync_state,
        ArtifactSyncState::Pending {
            destination: "github".into(),
            remote_id: None,
            remote_url: None,
        }
    );
}

#[test]
fn filesystem_store_persists_pending_sync_remote_handle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = FsActivityStore::new(dir.path()).expect("store");
    let activity = store.create(ActivityKind::Review).expect("activity");
    let artifact = store
        .create_artifact(
            activity.id,
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("summary".into()),
        )
        .expect("artifact");
    let artifact_id = artifact.id.clone();
    store.save_artifacts(&[artifact]).expect("save artifact");

    store
        .update_artifact_sync_state(
            &artifact_id,
            ArtifactSyncState::Pending {
                destination: "github-review".into(),
                remote_id: Some("99".into()),
                remote_url: Some(
                    "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
                ),
            },
        )
        .expect("update sync state");

    let reopened = FsActivityStore::new(dir.path()).expect("reopen store");
    assert_eq!(
        reopened.get_artifact(&artifact_id).expect("artifact").sync_state,
        ArtifactSyncState::Pending {
            destination: "github-review".into(),
            remote_id: Some("99".into()),
            remote_url: Some(
                "https://github.com/acme/platform/pull/42#pullrequestreview-99".into(),
            ),
        }
    );
}

#[test]
fn filesystem_store_writes_schema_manifest() {
    let dir = tempfile::tempdir().expect("temp dir");

    FsActivityStore::new(dir.path()).expect("store");

    let manifest = std::fs::read_to_string(dir.path().join("store.json")).expect("manifest");
    assert!(manifest.contains(r#""schema_version": 1"#));
}
