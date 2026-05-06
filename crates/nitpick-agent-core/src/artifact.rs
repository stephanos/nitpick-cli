use crate::{ActivityId, ReviewComment};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub activity_id: ActivityId,
    pub kind: ArtifactKind,
    pub content: ArtifactContent,
    pub sync_state: ArtifactSyncState,
}

impl Artifact {
    pub fn local(
        id: ArtifactId,
        activity_id: ActivityId,
        kind: ArtifactKind,
        content: ArtifactContent,
    ) -> Self {
        Self {
            id,
            activity_id,
            kind,
            content,
            sync_state: ArtifactSyncState::LocalOnly,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    ReviewSummary,
    ReviewComment,
    ChatResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactContent {
    ReviewSummary(String),
    ReviewComment(ReviewComment),
    ChatResponse(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactSyncState {
    LocalOnly,
    Pending {
        destination: String,
    },
    Synced {
        destination: String,
        remote_id: Option<String>,
    },
    Failed {
        destination: String,
        error: String,
    },
}
