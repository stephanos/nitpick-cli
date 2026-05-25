use std::path::PathBuf;

use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactContent, ArtifactId, ArtifactSyncOutcome,
    ArtifactSyncState, first_changed_file_for_diff,
};

use crate::{GitHubCliReviewSyncDestination, PullRequestRef};

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
        let pending_artifacts = artifacts
            .iter()
            .filter(|artifact| {
                matches!(
                    artifact.sync_state,
                    ArtifactSyncState::Pending {
                        ref destination,
                        remote_id: Some(_),
                        ..
                    } if destination == "github-review"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if pending_artifacts.is_empty() {
            return Ok(None);
        }
        let review_id = match &pending_artifacts[0].sync_state {
            ArtifactSyncState::Pending {
                remote_id: Some(review_id),
                ..
            } => review_id.clone(),
            _ => return Ok(None),
        };
        let review = match self.destination.fetch_review(&review_id) {
            Ok(review) => review,
            Err(_) => {
                return Ok(Some(
                    artifacts
                        .iter()
                        .map(|artifact| {
                            let next_state = match &artifact.sync_state {
                                ArtifactSyncState::Pending {
                                    destination,
                                    remote_id: Some(current_review_id),
                                    ..
                                } if destination == "github-review"
                                    && current_review_id == &review_id =>
                                {
                                    ArtifactSyncState::LocalOnly
                                }
                                _ => artifact.sync_state.clone(),
                            };
                            (artifact.id.clone(), next_state)
                        })
                        .collect(),
                ));
            }
        };
        if review.state == "PENDING" {
            let has_new_inline_comments = artifacts.iter().any(|artifact| {
                artifact.sync_state == ArtifactSyncState::LocalOnly
                    && matches!(artifact.content, ArtifactContent::ReviewComment(_))
            });
            if has_new_inline_comments {
                return Err(AgentError::invalid_input(
                    "pending GitHub draft review already exists; submit or clear the draft review before staging new inline comments",
                ));
            }

            let remote_url = review.html_url.clone().or_else(|| {
                pending_artifacts
                    .iter()
                    .find_map(|artifact| match &artifact.sync_state {
                        ArtifactSyncState::Pending { remote_url, .. } => remote_url.clone(),
                        _ => None,
                    })
            });
            let local_summary = artifacts.iter().find_map(|artifact| {
                if artifact.sync_state != ArtifactSyncState::LocalOnly {
                    return None;
                }
                match &artifact.content {
                    ArtifactContent::ReviewSummary(summary) => Some(summary.clone()),
                    _ => None,
                }
            });
            if let Some(summary) = local_summary {
                self.destination
                    .update_pending_review_body(&review_id, &summary)?;
            }
            return Ok(Some(
                artifacts
                    .iter()
                    .map(|artifact| {
                        let next_state = if artifact.sync_state == ArtifactSyncState::LocalOnly {
                            match &artifact.content {
                                ArtifactContent::ReviewSummary(_) => ArtifactSyncState::Pending {
                                    destination: "github-review".into(),
                                    remote_id: Some(review_id.clone()),
                                    remote_url: remote_url.clone(),
                                },
                                _ => artifact.sync_state.clone(),
                            }
                        } else {
                            artifact.sync_state.clone()
                        };
                        (artifact.id.clone(), next_state)
                    })
                    .collect(),
            ));
        }
        let remote_id = review.html_url.or_else(|| {
            pending_artifacts
                .iter()
                .find_map(|artifact| match &artifact.sync_state {
                    ArtifactSyncState::Pending { remote_url, .. } => remote_url.clone(),
                    _ => None,
                })
        });
        Ok(Some(
            artifacts
                .iter()
                .map(|artifact| {
                    let next_state = match &artifact.sync_state {
                        ArtifactSyncState::Pending {
                            destination,
                            remote_id: Some(current_review_id),
                            ..
                        } if destination == "github-review" && current_review_id == &review_id => {
                            ArtifactSyncState::Synced {
                                destination: "github-review".into(),
                                remote_id: remote_id.clone(),
                            }
                        }
                        _ => artifact.sync_state.clone(),
                    };
                    (artifact.id.clone(), next_state)
                })
                .collect(),
        ))
    }

    pub fn create_no_findings_draft_file_comment(
        &self,
        diff: &str,
    ) -> AgentResult<Option<ArtifactSyncOutcome>> {
        let Some(path) = first_changed_file_for_diff(diff)? else {
            return Ok(None);
        };
        self.destination
            .create_pending_file_comment(&path, NO_FINDINGS_REVIEW_COMMENT)
            .map(Some)
    }
}
