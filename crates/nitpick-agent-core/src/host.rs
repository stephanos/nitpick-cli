use serde::{Deserialize, Serialize};

use crate::{ActivityId, AgentProviderKind, ProviderFailureKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAttention {
    pub kind: ProviderFailureKind,
    pub title: String,
    pub detail: String,
    pub retryable_activity_count: usize,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<HostAttention>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryFailedActivitiesInput {
    pub kind: ProviderFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryFailedActivitiesResult {
    pub queued: usize,
    pub skipped: usize,
    pub activities: Vec<ActivityId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCheckoutsResult {
    pub removed_count: usize,
    pub cleaned: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalStateResetResult {
    pub removed_activity_count: usize,
    pub removed_artifact_count: usize,
    pub removed_processed_review_count: usize,
    pub removed_checkout_count: usize,
    pub truncated_log: bool,
}
