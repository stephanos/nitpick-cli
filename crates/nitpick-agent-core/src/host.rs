use serde::{Deserialize, Serialize};

use crate::AgentProviderKind;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub activity_count: usize,
    pub queued_activity_count: usize,
    pub running_activity_count: usize,
    pub completed_activity_count: usize,
    pub error_activity_count: usize,
    pub open_review_count: usize,
    pub queued_review_count: usize,
    pub running_review_count: usize,
    pub completed_review_count: usize,
    pub error_review_count: usize,
    pub artifact_count: usize,
    pub local_only_artifact_count: usize,
    pub pending_sync_artifact_count: usize,
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCheckoutsResult {
    pub removed_count: usize,
    pub cleaned: Vec<String>,
}
