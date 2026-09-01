use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use nitpick_agent_core::{
    AgentError, AgentResult, ArtifactSyncOutcome, first_changed_file_for_diff,
};
use nitpick_agent_model::{
    Artifact, ArtifactContent, ArtifactId, ArtifactSyncState, ReviewComment,
};

use crate::{
    GitHubPullRequestClient, GitHubReviewComment, GitHubReviewResponse, PullRequestRef,
    pending_review_reconciler::remote_comment_matches, review_payload::marked_body,
};

pub const NO_FINDINGS_REVIEW_COMMENT: &str = "🤖 Review completed: no findings.";
const GITHUB_REVIEW_DESTINATION: &str = "github-review";

#[derive(Clone)]
pub struct GitHubReviewSyncCoordinator {
    command: PathBuf,
    targets: TargetSyncGate,
}

impl GitHubReviewSyncCoordinator {
    pub fn new(command: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            targets: TargetSyncGate::default(),
        }
    }

    pub fn synchronize(
        &self,
        target: &PullRequestRef,
        artifacts: &[Artifact],
    ) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        if artifacts.is_empty() {
            return Ok(Vec::new());
        }
        if artifacts
            .iter()
            .any(|artifact| matches!(artifact.content, ArtifactContent::ChatResponse(_)))
        {
            return Err(AgentError::invalid_input(
                "github-review sync only supports review artifacts",
            ));
        }
        let sync_artifacts = artifacts
            .iter()
            .filter(|artifact| artifact_needs_sync(artifact))
            .cloned()
            .collect::<Vec<_>>();
        if sync_artifacts.is_empty() {
            return Ok(artifacts.iter().map(preserved_outcome).collect());
        }

        let _guard = self.targets.acquire(target.clone())?;
        let client = GitHubPullRequestClient::new(target.clone(), &self.command);
        let mut sync_outcomes = self
            .synchronize_locked(&client, &sync_artifacts)
            .map_err(|error| error.with_context(format!("sync GitHub review for {target}")))?
            .into_iter();
        Ok(artifacts
            .iter()
            .map(|artifact| {
                if artifact_needs_sync(artifact) {
                    sync_outcomes
                        .next()
                        .expect("coordinator returned one outcome per sync artifact")
                } else {
                    preserved_outcome(artifact)
                }
            })
            .collect())
    }

    fn synchronize_locked(
        &self,
        client: &GitHubPullRequestClient,
        artifacts: &[Artifact],
    ) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        let pending_review = discover_pending_review(client)?;
        let mut working_artifacts = artifacts.to_vec();
        let mut outcomes = (0..artifacts.len()).map(|_| None).collect::<Vec<_>>();

        for review_id in pending_review_ids(artifacts) {
            if pending_review
                .as_ref()
                .is_some_and(|review| review.id.to_string() == review_id)
            {
                continue;
            }
            let referenced_artifact = artifacts
                .iter()
                .find(|artifact| artifact_references_review(artifact, review_id))
                .expect("remembered review ID came from an artifact");
            match client.fetch_review(review_id).map_err(|error| {
                error.with_context(format!(
                    "fetch review {review_id} for artifact {}",
                    referenced_artifact.id
                ))
            }) {
                Ok(review) if review.state.eq_ignore_ascii_case("PENDING") => {
                    return Err(AgentError::invalid_input(format!(
                        "artifact {} references pending review {review_id}, but it is not the authenticated user's unique current pending review",
                        referenced_artifact.id
                    )));
                }
                Ok(review) => {
                    for (index, artifact) in artifacts.iter().enumerate() {
                        if artifact_references_review(artifact, review_id) {
                            outcomes[index] = Some(submitted_outcome(artifact, review_id, &review));
                        }
                    }
                }
                Err(error) if error.github_http_status() == Some(404) => {
                    for artifact in &mut working_artifacts {
                        if artifact_references_review(artifact, review_id) {
                            artifact.sync_state = ArtifactSyncState::LocalOnly;
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        }

        let remaining = working_artifacts
            .iter()
            .enumerate()
            .filter(|(index, _)| outcomes[*index].is_none())
            .map(|(_, artifact)| artifact.clone())
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            return Ok(outcomes.into_iter().flatten().collect());
        }

        let mut remaining_outcomes = if let Some(review) = pending_review {
            self.synchronize_existing_review(client, &remaining, &review)?
        } else {
            let first_artifact = &remaining[0];
            let head_sha = client.head_sha().map_err(|error| {
                error.with_context(format!(
                    "resolve head SHA before creating pending review for artifact {}",
                    first_artifact.id
                ))
            })?;
            match client
                .create_pending_review(head_sha, &remaining)
                .map_err(|error| {
                    error.with_context(format!(
                        "create pending review for artifact {}",
                        first_artifact.id
                    ))
                }) {
                Ok(review) => remaining.iter().map(|_| pending_outcome(&review)).collect(),
                Err(create_error) if create_error.github_http_status() == Some(422) => {
                    let review = discover_pending_review(client)?.ok_or(create_error)?;
                    self.synchronize_existing_review(client, &remaining, &review)?
                }
                Err(error) => return Err(error),
            }
        }
        .into_iter();
        for outcome in &mut outcomes {
            if outcome.is_none() {
                *outcome = Some(
                    remaining_outcomes
                        .next()
                        .expect("coordinator returned one outcome per artifact"),
                );
            }
        }
        debug_assert!(remaining_outcomes.next().is_none());
        Ok(outcomes.into_iter().flatten().collect())
    }

    fn synchronize_existing_review(
        &self,
        client: &GitHubPullRequestClient,
        artifacts: &[Artifact],
        review: &GitHubReviewResponse,
    ) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        let review_id = review.id.to_string();
        let first_unstaged_comment = artifacts.iter().find(|artifact| {
            !artifact_references_review(artifact, &review_id)
                && matches!(artifact.content, ArtifactContent::ReviewComment(_))
        });
        let mut remote_comments = if let Some(artifact) = first_unstaged_comment {
            client.review_comments_for(&review_id).map_err(|error| {
                error.with_context(format!(
                    "list comments for pending review {review_id} before synchronizing artifact {}",
                    artifact.id
                ))
            })?
        } else {
            Vec::new()
        };
        let mut head_sha = None;
        let mut outcomes = Vec::with_capacity(artifacts.len());

        for artifact in artifacts {
            let sync_state = match &artifact.content {
                ArtifactContent::ReviewComment(comment) => {
                    if !artifact_references_review(artifact, &review_id)
                        && !remote_comment_matches(&remote_comments, artifact)
                    {
                        if head_sha.is_none() {
                            head_sha = Some(client.head_sha().map_err(|error| {
                                error.with_context(format!(
                                    "resolve head SHA for artifact {} in pending review {review_id}",
                                    artifact.id
                                ))
                            })?);
                        }
                        client
                            .append_review_comment(
                                head_sha
                                    .as_deref()
                                    .expect("local comments require head SHA"),
                                artifact,
                            )
                            .map_err(|error| {
                                error.with_context(format!(
                                    "append artifact {} to pending review {}",
                                    artifact.id, review_id
                                ))
                            })?;
                        remote_comments.push(GitHubReviewComment {
                            id: artifact.id.to_string(),
                            review_id: Some(review_id.clone()),
                            path: comment.path.clone(),
                            line: (comment.line != 0).then_some(comment.line),
                            body: comment.body.clone(),
                            author: None,
                            draft: true,
                        });
                    }
                    pending_state(review)
                }
                ArtifactContent::ReviewSummary(summary) => {
                    if artifact_references_review(artifact, &review_id) {
                        artifact.sync_state.clone()
                    } else {
                        let desired = marked_body(summary, &artifact.id);
                        let remote_body = review.body.as_deref().unwrap_or_default();
                        if remote_body == desired {
                            pending_state(review)
                        } else if remote_body.trim().is_empty()
                            || crate::has_nitpick_marker(remote_body)
                        {
                            client
                                .update_pending_review_body(&review_id, &desired)
                                .map_err(|error| {
                                    error.with_context(format!(
                                        "update artifact {} in pending review {} body",
                                        artifact.id, review_id
                                    ))
                                })?;
                            pending_state(review)
                        } else {
                            ArtifactSyncState::LocalOnly
                        }
                    }
                }
                ArtifactContent::ChatResponse(_) => unreachable!("validated review artifacts"),
            };
            outcomes.push(ArtifactSyncOutcome {
                remote_id: review.html_url.clone(),
                sync_state,
            });
        }
        Ok(outcomes)
    }
}

fn discover_pending_review(
    client: &GitHubPullRequestClient,
) -> AgentResult<Option<GitHubReviewResponse>> {
    let login = client.authenticated_login()?;
    let mut pending_reviews = client
        .reviews()?
        .into_iter()
        .filter(|review| {
            review.state.eq_ignore_ascii_case("PENDING")
                && review
                    .user
                    .as_ref()
                    .is_some_and(|user| user.login.eq_ignore_ascii_case(&login))
        })
        .collect::<Vec<_>>();
    match pending_reviews.len() {
        0 => Ok(None),
        1 => Ok(pending_reviews.pop()),
        count => Err(AgentError::invalid_input(format!(
            "authenticated GitHub user `{login}` has {count} pending reviews for {}",
            client.target()
        ))),
    }
}

#[derive(Clone, Default)]
struct TargetSyncGate {
    state: Arc<(Mutex<HashSet<PullRequestRef>>, Condvar)>,
}

impl TargetSyncGate {
    fn acquire(&self, target: PullRequestRef) -> AgentResult<TargetSyncGuard> {
        let (active, changed) = self.state.as_ref();
        let mut active = active
            .lock()
            .map_err(|_| AgentError::io("GitHub review sync target lock", "poisoned"))?;
        while active.contains(&target) {
            active = changed
                .wait(active)
                .map_err(|_| AgentError::io("GitHub review sync target lock", "poisoned"))?;
        }
        active.insert(target.clone());
        Ok(TargetSyncGuard {
            target,
            state: self.state.clone(),
        })
    }
}

struct TargetSyncGuard {
    target: PullRequestRef,
    state: Arc<(Mutex<HashSet<PullRequestRef>>, Condvar)>,
}

impl Drop for TargetSyncGuard {
    fn drop(&mut self) {
        let (active, changed) = self.state.as_ref();
        if let Ok(mut active) = active.lock() {
            active.remove(&self.target);
            changed.notify_all();
        }
    }
}

fn pending_review_ids(artifacts: &[Artifact]) -> Vec<&str> {
    let mut review_ids = Vec::new();
    for artifact in artifacts {
        if let ArtifactSyncState::Pending {
            destination,
            remote_id: Some(review_id),
            ..
        } = &artifact.sync_state
            && destination == GITHUB_REVIEW_DESTINATION
            && !review_ids.contains(&review_id.as_str())
        {
            review_ids.push(review_id.as_str());
        }
    }
    review_ids
}

fn artifact_needs_sync(artifact: &Artifact) -> bool {
    match &artifact.sync_state {
        ArtifactSyncState::LocalOnly => true,
        ArtifactSyncState::Pending { destination, .. }
        | ArtifactSyncState::Failed { destination, .. } => destination == GITHUB_REVIEW_DESTINATION,
        ArtifactSyncState::Synced { .. } => false,
    }
}

fn preserved_outcome(artifact: &Artifact) -> ArtifactSyncOutcome {
    ArtifactSyncOutcome {
        sync_state: artifact.sync_state.clone(),
        remote_id: match &artifact.sync_state {
            ArtifactSyncState::Synced { remote_id, .. } => remote_id.clone(),
            ArtifactSyncState::Pending { remote_url, .. } => remote_url.clone(),
            _ => None,
        },
    }
}

fn pending_state(review: &GitHubReviewResponse) -> ArtifactSyncState {
    ArtifactSyncState::Pending {
        destination: GITHUB_REVIEW_DESTINATION.into(),
        remote_id: Some(review.id.to_string()),
        remote_url: review.html_url.clone(),
    }
}

fn pending_outcome(review: &GitHubReviewResponse) -> ArtifactSyncOutcome {
    ArtifactSyncOutcome {
        sync_state: pending_state(review),
        remote_id: review.html_url.clone(),
    }
}

fn submitted_outcome(
    artifact: &Artifact,
    review_id: &str,
    review: &GitHubReviewResponse,
) -> ArtifactSyncOutcome {
    let sync_state = if artifact_references_review(artifact, review_id) {
        ArtifactSyncState::Synced {
            destination: GITHUB_REVIEW_DESTINATION.into(),
            remote_id: review.html_url.clone(),
        }
    } else {
        artifact.sync_state.clone()
    };
    ArtifactSyncOutcome {
        sync_state,
        remote_id: review.html_url.clone(),
    }
}

fn artifact_references_review(artifact: &Artifact, review_id: &str) -> bool {
    matches!(
        &artifact.sync_state,
        ArtifactSyncState::Pending {
            destination,
            remote_id: Some(artifact_review_id),
            ..
        } if destination == GITHUB_REVIEW_DESTINATION && artifact_review_id == review_id
    )
}

pub struct GitHubReviewWorkflowSync {
    target: PullRequestRef,
    coordinator: GitHubReviewSyncCoordinator,
}

impl GitHubReviewWorkflowSync {
    pub fn new(target: PullRequestRef, command: impl Into<PathBuf>) -> Self {
        let command = command.into();
        Self {
            target,
            coordinator: GitHubReviewSyncCoordinator::new(command),
        }
    }

    pub fn reconcile_pending_artifact_states(
        &self,
        artifacts: &[Artifact],
    ) -> AgentResult<Option<Vec<(ArtifactId, ArtifactSyncState)>>> {
        let outcomes = self.coordinator.synchronize(&self.target, artifacts)?;
        Ok(Some(
            artifacts
                .iter()
                .zip(outcomes)
                .map(|(artifact, outcome)| (artifact.id.clone(), outcome.sync_state))
                .collect(),
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
            .coordinator
            .synchronize(&self.target, std::slice::from_ref(artifact))?;
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

#[cfg(test)]
mod coordinator_tests {
    use std::{sync::mpsc, thread};

    use super::*;

    #[test]
    fn target_gate_serializes_the_same_pull_request() {
        let gate = TargetSyncGate::default();
        let target = pull_request(42);
        let first = gate.acquire(target.clone()).expect("first guard");
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let second_gate = gate.clone();
        let second = thread::spawn(move || {
            started_tx.send(()).expect("started");
            let _guard = second_gate.acquire(target).expect("second guard");
            acquired_tx.send(()).expect("acquired");
        });

        started_rx.recv().expect("second started");
        assert!(acquired_rx.try_recv().is_err());
        drop(first);
        acquired_rx.recv().expect("second acquired");
        second.join().expect("second thread");
    }

    #[test]
    fn target_gate_does_not_serialize_different_pull_requests() {
        let gate = TargetSyncGate::default();
        let _first = gate.acquire(pull_request(41)).expect("first guard");
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let second_gate = gate.clone();
        let second = thread::spawn(move || {
            let _guard = second_gate.acquire(pull_request(42)).expect("second guard");
            acquired_tx.send(()).expect("acquired");
        });

        acquired_rx.recv().expect("different target acquired");
        second.join().expect("second thread");
    }

    fn pull_request(number: u64) -> PullRequestRef {
        PullRequestRef {
            owner: "acme".into(),
            repo: "platform".into(),
            number,
        }
    }
}
