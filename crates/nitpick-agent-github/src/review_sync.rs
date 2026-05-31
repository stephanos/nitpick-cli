use std::path::PathBuf;

use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactId, ArtifactSyncState, ReviewComment,
    first_changed_file_for_diff,
};

use crate::{
    GitHubCliReviewSyncDestination, PullRequestRef,
    pending_review_reconciler::{PendingReviewReconciler, PendingReviewRemote},
};

pub const NO_FINDINGS_REVIEW_COMMENT: &str = "🤖 Review completed: no findings.";

pub struct GitHubReviewWorkflowSync {
    destination: GitHubCliReviewSyncDestination,
}

impl GitHubReviewWorkflowSync {
    pub fn new(target: PullRequestRef, command: impl Into<PathBuf>) -> Self {
        Self {
            destination: GitHubCliReviewSyncDestination::new(target, command),
        }
    }

    pub fn reconcile_pending_artifact_states(
        &self,
        artifacts: &[Artifact],
    ) -> AgentResult<Option<Vec<(ArtifactId, ArtifactSyncState)>>> {
        let reconciler = PendingReviewReconciler::new(artifacts);
        let Some(review_id) = reconciler.review_id() else {
            return Ok(None);
        };
        let review = match self.destination.fetch_review(review_id) {
            Ok(review) => review,
            Err(error) if is_missing_review_error(&error) => {
                return Ok(Some(
                    reconciler
                        .reconcile(PendingReviewRemote::Missing)?
                        .state_updates,
                ));
            }
            Err(error) => return Err(error),
        };
        if review.state == "PENDING" {
            let reconciliation = reconciler.reconcile(PendingReviewRemote::Pending {
                remote_url: review.html_url,
            })?;
            if let Some(summary) = reconciliation.pending_body_update {
                self.destination
                    .update_pending_review_body(review_id, &summary)?;
            }
            return Ok(Some(reconciliation.state_updates));
        }
        Ok(Some(
            reconciler
                .reconcile(PendingReviewRemote::Submitted {
                    remote_url: review.html_url,
                })?
                .state_updates,
        ))
    }

    pub fn no_findings_draft_file_comment(&self, diff: &str) -> AgentResult<Option<ReviewComment>> {
        no_findings_review_comment_for_diff(diff)
    }

    pub fn sync_no_findings_draft_file_comment(
        &self,
        artifact: &Artifact,
    ) -> AgentResult<(ArtifactId, ArtifactSyncState)> {
        let outcomes = self
            .destination
            .create_pending_review_batch(std::slice::from_ref(artifact))?;
        let outcome = outcomes.into_iter().next().ok_or_else(|| {
            AgentError::invalid_input("github-review sync returned no outcome for file comment")
        })?;
        Ok((artifact.id.clone(), outcome.sync_state))
    }
}

fn no_findings_review_comment_for_diff(diff: &str) -> AgentResult<Option<ReviewComment>> {
    let Some(path) = first_changed_file_for_diff(diff)? else {
        return Ok(None);
    };
    Ok(Some(ReviewComment {
        path,
        line: 0,
        body: NO_FINDINGS_REVIEW_COMMENT.into(),
    }))
}

fn is_missing_review_error(error: &AgentError) -> bool {
    matches!(error, AgentError::NotFound { .. })
}
