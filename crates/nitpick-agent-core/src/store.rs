use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    Activity, ActivityId, ActivityKind, AgentError, AgentResult, Artifact, ArtifactContent,
    ArtifactId, ArtifactKind, ArtifactSyncState,
};

const STORE_SCHEMA_VERSION: u32 = 1;

pub trait ArtifactStore: Send + Sync {
    fn create_artifact(
        &self,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> AgentResult<Artifact>;

    fn save_artifacts(&self, artifacts: &[Artifact]) -> AgentResult<()>;

    fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>>;

    fn list_artifacts(&self) -> AgentResult<Vec<Artifact>>;

    fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact>;

    fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact>;
}

pub trait ActivityStore: ArtifactStore + Send + Sync {
    fn create(&self, kind: ActivityKind) -> AgentResult<Activity>;

    fn save(&self, activity: &Activity) -> AgentResult<()>;

    fn get(&self, id: &ActivityId) -> AgentResult<Activity>;

    fn list(&self) -> AgentResult<Vec<Activity>>;
}

#[derive(Default)]
pub struct MemoryActivityStore {
    next_id: AtomicU64,
    next_artifact_id: AtomicU64,
    activities: Mutex<BTreeMap<ActivityId, Activity>>,
    artifacts: Mutex<BTreeMap<ArtifactId, Artifact>>,
}

impl MemoryActivityStore {
    pub fn get(&self, id: &ActivityId) -> AgentResult<Activity> {
        <Self as ActivityStore>::get(self, id)
    }

    pub fn list(&self) -> AgentResult<Vec<Activity>> {
        <Self as ActivityStore>::list(self)
    }

    pub fn create_artifact(
        &self,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> AgentResult<Artifact> {
        <Self as ArtifactStore>::create_artifact(self, activity_id, kind, content)
    }

    pub fn save_artifacts(&self, artifacts: &[Artifact]) -> AgentResult<()> {
        <Self as ArtifactStore>::save_artifacts(self, artifacts)
    }

    pub fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        <Self as ArtifactStore>::list_artifacts_for(self, activity_id)
    }

    pub fn list_artifacts(&self) -> AgentResult<Vec<Artifact>> {
        <Self as ArtifactStore>::list_artifacts(self)
    }

    pub fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact> {
        <Self as ArtifactStore>::get_artifact(self, id)
    }

    pub fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact> {
        <Self as ArtifactStore>::update_artifact_sync_state(self, id, sync_state)
    }
}

impl ActivityStore for MemoryActivityStore {
    fn create(&self, kind: ActivityKind) -> AgentResult<Activity> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let activity = Activity::new(ActivityId::new(format!("activity-{id}")), kind);
        self.save(&activity)?;
        Ok(activity)
    }

    fn save(&self, activity: &Activity) -> AgentResult<()> {
        let mut activities = self
            .activities
            .lock()
            .map_err(|_| AgentError::new("activity store lock poisoned"))?;
        activities.insert(activity.id.clone(), activity.clone());
        Ok(())
    }

    fn get(&self, id: &ActivityId) -> AgentResult<Activity> {
        let activities = self
            .activities
            .lock()
            .map_err(|_| AgentError::new("activity store lock poisoned"))?;
        activities
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::new(format!("activity not found: {id}")))
    }

    fn list(&self) -> AgentResult<Vec<Activity>> {
        let activities = self
            .activities
            .lock()
            .map_err(|_| AgentError::new("activity store lock poisoned"))?;
        Ok(activities.values().cloned().collect())
    }
}

impl ArtifactStore for MemoryActivityStore {
    fn create_artifact(
        &self,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> AgentResult<Artifact> {
        let id = self.next_artifact_id.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Artifact::local(
            ArtifactId::new(format!("artifact-{id}")),
            activity_id,
            kind,
            content,
        ))
    }

    fn save_artifacts(&self, artifacts: &[Artifact]) -> AgentResult<()> {
        let mut stored = self
            .artifacts
            .lock()
            .map_err(|_| AgentError::new("artifact store lock poisoned"))?;
        for artifact in artifacts {
            stored.insert(artifact.id.clone(), artifact.clone());
        }
        Ok(())
    }

    fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        let stored = self
            .artifacts
            .lock()
            .map_err(|_| AgentError::new("artifact store lock poisoned"))?;
        Ok(stored
            .values()
            .filter(|artifact| &artifact.activity_id == activity_id)
            .cloned()
            .collect())
    }

    fn list_artifacts(&self) -> AgentResult<Vec<Artifact>> {
        let stored = self
            .artifacts
            .lock()
            .map_err(|_| AgentError::new("artifact store lock poisoned"))?;
        Ok(stored.values().cloned().collect())
    }

    fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact> {
        let stored = self
            .artifacts
            .lock()
            .map_err(|_| AgentError::new("artifact store lock poisoned"))?;
        stored
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::new(format!("artifact not found: {id}")))
    }

    fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact> {
        let mut stored = self
            .artifacts
            .lock()
            .map_err(|_| AgentError::new("artifact store lock poisoned"))?;
        let artifact = stored
            .get_mut(id)
            .ok_or_else(|| AgentError::new(format!("artifact not found: {id}")))?;
        artifact.sync_state = sync_state;
        Ok(artifact.clone())
    }
}

pub struct FsActivityStore {
    base: PathBuf,
    next_id: AtomicU64,
    next_artifact_id: AtomicU64,
}

impl FsActivityStore {
    pub fn new(base: impl AsRef<Path>) -> AgentResult<Self> {
        let base = base.as_ref().to_path_buf();
        fs::create_dir_all(activity_dir(&base)).map_err(fs_error("create activity dir"))?;
        fs::create_dir_all(artifact_dir(&base)).map_err(fs_error("create artifact dir"))?;
        ensure_manifest(&base)?;
        let next_id = next_numeric_suffix(&activity_dir(&base), "activity-")?;
        let next_artifact_id = next_numeric_suffix(&artifact_dir(&base), "artifact-")?;

        Ok(Self {
            base,
            next_id: AtomicU64::new(next_id),
            next_artifact_id: AtomicU64::new(next_artifact_id),
        })
    }

    pub fn get(&self, id: &ActivityId) -> AgentResult<Activity> {
        <Self as ActivityStore>::get(self, id)
    }

    pub fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        <Self as ArtifactStore>::list_artifacts_for(self, activity_id)
    }

    pub fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact> {
        <Self as ArtifactStore>::get_artifact(self, id)
    }

    pub fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact> {
        <Self as ArtifactStore>::update_artifact_sync_state(self, id, sync_state)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoreManifest {
    schema_version: u32,
}

fn ensure_manifest(base: &Path) -> AgentResult<()> {
    let path = base.join("store.json");
    if path.exists() {
        let manifest: StoreManifest = read_json(&path)?;
        if manifest.schema_version != STORE_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "unsupported store schema version {}; expected {}",
                manifest.schema_version, STORE_SCHEMA_VERSION
            )));
        }
        return Ok(());
    }

    write_json(
        &path,
        &StoreManifest {
            schema_version: STORE_SCHEMA_VERSION,
        },
    )
}

impl ActivityStore for FsActivityStore {
    fn create(&self, kind: ActivityKind) -> AgentResult<Activity> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let activity = Activity::new(ActivityId::new(format!("activity-{id}")), kind);
        self.save(&activity)?;
        Ok(activity)
    }

    fn save(&self, activity: &Activity) -> AgentResult<()> {
        write_json(&activity_path(&self.base, &activity.id), activity)
    }

    fn get(&self, id: &ActivityId) -> AgentResult<Activity> {
        read_json(&activity_path(&self.base, id))
    }

    fn list(&self) -> AgentResult<Vec<Activity>> {
        read_json_dir(&activity_dir(&self.base))
    }
}

impl ArtifactStore for FsActivityStore {
    fn create_artifact(
        &self,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> AgentResult<Artifact> {
        let id = self.next_artifact_id.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Artifact::local(
            ArtifactId::new(format!("artifact-{id}")),
            activity_id,
            kind,
            content,
        ))
    }

    fn save_artifacts(&self, artifacts: &[Artifact]) -> AgentResult<()> {
        for artifact in artifacts {
            write_json(&artifact_path(&self.base, &artifact.id), artifact)?;
        }
        Ok(())
    }

    fn list_artifacts_for(&self, activity_id: &ActivityId) -> AgentResult<Vec<Artifact>> {
        Ok(self
            .list_artifacts()?
            .into_iter()
            .filter(|artifact| &artifact.activity_id == activity_id)
            .collect())
    }

    fn list_artifacts(&self) -> AgentResult<Vec<Artifact>> {
        read_json_dir(&artifact_dir(&self.base))
    }

    fn get_artifact(&self, id: &ArtifactId) -> AgentResult<Artifact> {
        read_json(&artifact_path(&self.base, id))
            .map_err(|error| AgentError::new(format!("artifact not found: {id}: {error}")))
    }

    fn update_artifact_sync_state(
        &self,
        id: &ArtifactId,
        sync_state: ArtifactSyncState,
    ) -> AgentResult<Artifact> {
        let mut artifact = self.get_artifact(id)?;
        artifact.sync_state = sync_state;
        write_json(&artifact_path(&self.base, id), &artifact)?;
        Ok(artifact)
    }
}

fn activity_dir(base: &Path) -> PathBuf {
    base.join("activities")
}

fn artifact_dir(base: &Path) -> PathBuf {
    base.join("artifacts")
}

fn activity_path(base: &Path, id: &ActivityId) -> PathBuf {
    activity_dir(base).join(format!("{id}.json"))
}

fn artifact_path(base: &Path, id: &ArtifactId) -> PathBuf {
    artifact_dir(base).join(format!("{id}.json"))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> AgentResult<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AgentError::new(format!("serialize {}: {error}", path.display())))?;
    fs::write(&tmp, bytes).map_err(fs_error("write temp json"))?;
    fs::rename(&tmp, path).map_err(fs_error("replace json"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AgentResult<T> {
    let bytes = fs::read(path).map_err(fs_error("read json"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AgentError::new(format!("parse {}: {error}", path.display())))
}

fn read_json_dir<T: serde::de::DeserializeOwned>(dir: &Path) -> AgentResult<Vec<T>> {
    let mut values = Vec::new();
    let mut paths = fs::read_dir(dir)
        .map_err(fs_error("read json dir"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(fs_error("read json dir entry"))?;
    paths.sort();

    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            values.push(read_json(&path)?);
        }
    }

    Ok(values)
}

fn next_numeric_suffix(dir: &Path, prefix: &str) -> AgentResult<u64> {
    let mut max = 0;
    for path in fs::read_dir(dir).map_err(fs_error("read id dir"))? {
        let path = path.map_err(fs_error("read id dir entry"))?.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some(value) = stem.strip_prefix(prefix) else {
            continue;
        };
        if let Ok(id) = value.parse::<u64>() {
            max = max.max(id);
        }
    }
    Ok(max)
}

fn fs_error(context: &'static str) -> impl FnOnce(std::io::Error) -> AgentError {
    move |error| AgentError::new(format!("{context}: {error}"))
}
