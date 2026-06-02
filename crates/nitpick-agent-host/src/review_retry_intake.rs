use nitpick_agent_core::{Activity, ActivityId, ReviewRetryMetadata};

pub struct ReviewRetryIntake;

impl ReviewRetryIntake {
    pub fn is_retryable_review_metadata(retry: &ReviewRetryMetadata) -> bool {
        retry.source == "github"
            && retry.number.is_some()
            && retry
                .repository
                .split_once('/')
                .is_some_and(|(owner, repo)| !owner.is_empty() && !repo.is_empty())
    }

    pub fn provider_failure_resolved(activity: &Activity) -> bool {
        activity
            .retry
            .as_ref()
            .and_then(|retry| retry.resolved_by_activity.as_ref())
            .is_some()
    }

    pub fn mark_resolved(activity: &mut Activity, resolved_by: ActivityId) {
        if let Some(retry) = activity.retry.as_mut() {
            retry.resolved_by_activity = Some(resolved_by);
        }
    }
}
