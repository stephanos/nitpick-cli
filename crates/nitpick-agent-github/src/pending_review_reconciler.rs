use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactContent, ArtifactId, ArtifactSyncState,
};

const GITHUB_REVIEW_DESTINATION: &str = "github-review";

pub(crate) enum PendingReviewRemote {
    Missing,
    Pending { remote_url: Option<String> },
    Submitted { remote_url: Option<String> },
}

pub(crate) struct PendingReviewReconciliation {
    pub(crate) state_updates: Vec<(ArtifactId, ArtifactSyncState)>,
    pub(crate) pending_body_update: Option<String>,
}

pub(crate) struct PendingReviewReconciler<'a> {
    artifacts: &'a [Artifact],
    review_id: Option<String>,
}

impl<'a> PendingReviewReconciler<'a> {
    pub(crate) fn new(artifacts: &'a [Artifact]) -> Self {
        Self {
            artifacts,
            review_id: artifacts
                .iter()
                .find_map(pending_github_review_id)
                .map(str::to_owned),
        }
    }

    pub(crate) fn review_id(&self) -> Option<&str> {
        self.review_id.as_deref()
    }

    pub(crate) fn reconcile(
        &self,
        remote: PendingReviewRemote,
    ) -> AgentResult<PendingReviewReconciliation> {
        let Some(review_id) = &self.review_id else {
            return Ok(PendingReviewReconciliation {
                state_updates: Vec::new(),
                pending_body_update: None,
            });
        };
        match remote {
            PendingReviewRemote::Missing => Ok(PendingReviewReconciliation {
                state_updates: self
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        let next_state = if pending_review_id_matches(artifact, review_id) {
                            ArtifactSyncState::LocalOnly
                        } else {
                            artifact.sync_state.clone()
                        };
                        (artifact.id.clone(), next_state)
                    })
                    .collect(),
                pending_body_update: None,
            }),
            PendingReviewRemote::Pending { remote_url } => {
                if self.artifacts.iter().any(is_local_inline_comment) {
                    return Err(AgentError::invalid_input(
                        "pending GitHub draft review already exists; submit or clear the draft review before staging new inline comments",
                    ));
                }

                let remote_url = self.remote_url_for(review_id, remote_url);
                let pending_body_update = self.local_summary();
                Ok(PendingReviewReconciliation {
                    state_updates: self
                        .artifacts
                        .iter()
                        .map(|artifact| {
                            let next_state = if artifact.sync_state == ArtifactSyncState::LocalOnly
                            {
                                match &artifact.content {
                                    ArtifactContent::ReviewSummary(_) => {
                                        ArtifactSyncState::Pending {
                                            destination: GITHUB_REVIEW_DESTINATION.into(),
                                            remote_id: Some(review_id.clone()),
                                            remote_url: remote_url.clone(),
                                        }
                                    }
                                    _ => artifact.sync_state.clone(),
                                }
                            } else {
                                artifact.sync_state.clone()
                            };
                            (artifact.id.clone(), next_state)
                        })
                        .collect(),
                    pending_body_update,
                })
            }
            PendingReviewRemote::Submitted { remote_url } => {
                let remote_id = self.remote_url_for(review_id, remote_url);
                Ok(PendingReviewReconciliation {
                    state_updates: self
                        .artifacts
                        .iter()
                        .map(|artifact| {
                            let next_state = if pending_review_id_matches(artifact, review_id) {
                                ArtifactSyncState::Synced {
                                    destination: GITHUB_REVIEW_DESTINATION.into(),
                                    remote_id: remote_id.clone(),
                                }
                            } else {
                                artifact.sync_state.clone()
                            };
                            (artifact.id.clone(), next_state)
                        })
                        .collect(),
                    pending_body_update: None,
                })
            }
        }
    }

    fn local_summary(&self) -> Option<String> {
        self.artifacts.iter().find_map(|artifact| {
            if artifact.sync_state != ArtifactSyncState::LocalOnly {
                return None;
            }
            match &artifact.content {
                ArtifactContent::ReviewSummary(summary) => Some(summary.clone()),
                _ => None,
            }
        })
    }

    fn remote_url_for(&self, review_id: &str, remote_url: Option<String>) -> Option<String> {
        remote_url.or_else(|| {
            self.artifacts
                .iter()
                .find_map(|artifact| match &artifact.sync_state {
                    ArtifactSyncState::Pending {
                        destination,
                        remote_id: Some(current_review_id),
                        remote_url,
                    } if destination == GITHUB_REVIEW_DESTINATION
                        && current_review_id == review_id =>
                    {
                        remote_url.clone()
                    }
                    _ => None,
                })
        })
    }
}

fn pending_github_review_id(artifact: &Artifact) -> Option<&str> {
    match &artifact.sync_state {
        ArtifactSyncState::Pending {
            destination,
            remote_id: Some(review_id),
            ..
        } if destination == GITHUB_REVIEW_DESTINATION => Some(review_id),
        _ => None,
    }
}

fn pending_review_id_matches(artifact: &Artifact, review_id: &str) -> bool {
    pending_github_review_id(artifact) == Some(review_id)
}

fn is_local_inline_comment(artifact: &Artifact) -> bool {
    artifact.sync_state == ArtifactSyncState::LocalOnly
        && matches!(artifact.content, ArtifactContent::ReviewComment(_))
}

#[cfg(test)]
mod tests {
    use nitpick_agent_core::{ActivityId, ArtifactKind, ReviewComment};

    use super::*;

    #[test]
    fn pending_remote_promotes_local_summary_and_requests_body_update() {
        let mut pending_comment = review_comment_artifact("comment");
        pending_comment.sync_state = ArtifactSyncState::Pending {
            destination: GITHUB_REVIEW_DESTINATION.into(),
            remote_id: Some("99".into()),
            remote_url: Some("https://example.test/review-99".into()),
        };
        let summary = Artifact::local(
            ArtifactId::new("summary"),
            ActivityId::new("activity"),
            ArtifactKind::ReviewSummary,
            ArtifactContent::ReviewSummary("updated summary".into()),
        );
        let artifacts = [pending_comment.clone(), summary.clone()];
        let reconciler = PendingReviewReconciler::new(&artifacts);

        let reconciliation = reconciler
            .reconcile(PendingReviewRemote::Pending { remote_url: None })
            .expect("reconcile");

        assert_eq!(reconciler.review_id(), Some("99"));
        assert_eq!(
            reconciliation.pending_body_update.as_deref(),
            Some("updated summary")
        );
        assert_eq!(
            reconciliation.state_updates,
            vec![
                (pending_comment.id, pending_comment.sync_state),
                (
                    summary.id,
                    ArtifactSyncState::Pending {
                        destination: GITHUB_REVIEW_DESTINATION.into(),
                        remote_id: Some("99".into()),
                        remote_url: Some("https://example.test/review-99".into()),
                    },
                ),
            ]
        );
    }

    #[test]
    fn submitted_remote_marks_pending_review_artifacts_synced() {
        let mut pending_comment = review_comment_artifact("comment");
        pending_comment.sync_state = ArtifactSyncState::Pending {
            destination: GITHUB_REVIEW_DESTINATION.into(),
            remote_id: Some("99".into()),
            remote_url: None,
        };
        let reconciler = PendingReviewReconciler::new(std::slice::from_ref(&pending_comment));

        let reconciliation = reconciler
            .reconcile(PendingReviewRemote::Submitted {
                remote_url: Some("https://example.test/review-99".into()),
            })
            .expect("reconcile");

        assert_eq!(
            reconciliation.state_updates,
            vec![(
                pending_comment.id,
                ArtifactSyncState::Synced {
                    destination: GITHUB_REVIEW_DESTINATION.into(),
                    remote_id: Some("https://example.test/review-99".into()),
                },
            )]
        );
    }

    fn review_comment_artifact(id: &str) -> Artifact {
        Artifact::local(
            ArtifactId::new(id),
            ActivityId::new("activity"),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: "body".into(),
            }),
        )
    }
}
