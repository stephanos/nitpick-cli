use nitpick_agent_core::ProviderFailureClassification;
use nitpick_agent_model::{
    Activity, ActivityKind, ActivityStatus, AgentProviderKind, Artifact, ArtifactSyncState,
    HostAttention, HostStatus, ProviderFailureKind,
};

use crate::ReviewRetryIntake;

pub struct HostStatusProjection<'a> {
    pub activities: &'a [Activity],
    pub artifacts: &'a [Artifact],
    pub open_review_count: usize,
    pub provider: AgentProviderKind,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

impl HostStatusProjection<'_> {
    pub fn project(&self) -> HostStatus {
        let reviews: Vec<_> = self
            .activities
            .iter()
            .filter(|activity| activity.kind == ActivityKind::Review)
            .collect();
        HostStatus {
            activity_count: self.activities.len(),
            queued_activity_count: self
                .activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Queued)
                .count(),
            running_activity_count: self
                .activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Running)
                .count(),
            completed_activity_count: self
                .activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Completed)
                .count(),
            error_activity_count: self
                .activities
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Error)
                .count(),
            open_review_count: self.open_review_count,
            queued_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Queued)
                .count(),
            running_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Running)
                .count(),
            completed_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Completed)
                .count(),
            error_review_count: reviews
                .iter()
                .filter(|activity| activity.status == ActivityStatus::Error)
                .count(),
            artifact_count: self.artifacts.len(),
            local_only_artifact_count: self
                .artifacts
                .iter()
                .filter(|artifact| artifact.sync_state == ArtifactSyncState::LocalOnly)
                .count(),
            pending_sync_artifact_count: self
                .artifacts
                .iter()
                .filter(|artifact| matches!(artifact.sync_state, ArtifactSyncState::Pending { .. }))
                .count(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            review_source_name: self.review_source_name.clone(),
            review_source_enabled: self.review_source_enabled,
            review_source_last_poll_unix: self.review_source_last_poll_unix,
            review_source_last_poll_summary: self.review_source_last_poll_summary.clone(),
            attention: provider_attention(self.activities, &self.provider),
        }
    }
}

fn provider_attention(
    activities: &[Activity],
    provider: &AgentProviderKind,
) -> Option<HostAttention> {
    let mut classified = activities
        .iter()
        .filter_map(|activity| {
            let mut classified_activity = activity.clone();
            classified_activity
                .session
                .provider
                .get_or_insert_with(|| provider.clone());
            if ReviewRetryIntake::provider_failure_resolved(&classified_activity) {
                return None;
            }
            nitpick_agent_core::classify_provider_failure(&classified_activity)
                .map(|classification| (activity.updated_at_unix, classification))
        })
        .collect::<Vec<_>>();
    classified.sort_by_key(|(updated_at_unix, classification)| {
        (
            provider_failure_priority(&classification.kind),
            std::cmp::Reverse(*updated_at_unix),
        )
    });
    let (_, classification) = classified.first()?;
    let retryable_activity_count = activities
        .iter()
        .filter(|activity| {
            activity.kind == ActivityKind::Review
                && activity.status == ActivityStatus::Error
                && activity
                    .retry
                    .as_ref()
                    .and_then(|retry| retry.review.as_ref())
                    .is_some_and(ReviewRetryIntake::is_retryable_review_metadata)
                && !ReviewRetryIntake::provider_failure_resolved(activity)
                && nitpick_agent_core::classify_provider_failure(activity)
                    .is_some_and(|candidate| candidate.kind == classification.kind)
        })
        .count();
    Some(HostAttention {
        kind: classification.kind.clone(),
        title: "provider needs attention".into(),
        detail: provider_attention_detail(classification),
        retryable_activity_count,
    })
}

fn provider_attention_detail(classification: &ProviderFailureClassification) -> String {
    match classification.suggested_action.as_deref() {
        Some(action) => format!(
            "{}: {} {}",
            classification.title, classification.detail, action
        ),
        None => format!("{}: {}", classification.title, classification.detail),
    }
}

fn provider_failure_priority(kind: &ProviderFailureKind) -> u8 {
    match kind {
        ProviderFailureKind::AuthInvalidCredentials => 0,
        ProviderFailureKind::SandboxPermissionDenied => 1,
        ProviderFailureKind::ProviderUnavailable => 2,
        ProviderFailureKind::UnknownProviderFailure => 3,
    }
}
