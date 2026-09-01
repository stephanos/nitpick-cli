use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

use nitpick_agent_core::{
    ActivityStore, AgentError, AgentResult, ReviewActivityIdentity, ReviewCommentValidator,
    read_json, write_json_atomic,
};
use nitpick_agent_github::{GitHubPullRequestClient, GitHubReviewSyncCoordinator, PullRequestRef};
use nitpick_agent_model::{
    ActivityId, Artifact, ArtifactContent, ArtifactId, ArtifactKind, ArtifactSyncState,
    ExistingReviewComment, FinishReviewCommentBatchInput, FinishReviewCommentBatchResult,
    ReviewChatSessionSnapshot,
};
use serde::{Deserialize, Serialize};

pub struct ReviewCommentBatchCommitter {
    data_dir: PathBuf,
    store: Arc<dyn ActivityStore>,
    github_command: PathBuf,
    github_review_sync: GitHubReviewSyncCoordinator,
}

impl ReviewCommentBatchCommitter {
    pub fn new(
        data_dir: impl AsRef<Path>,
        store: Arc<dyn ActivityStore>,
        github_command: impl Into<PathBuf>,
    ) -> Self {
        let github_command = github_command.into();
        let github_review_sync = GitHubReviewSyncCoordinator::new(&github_command);
        Self::with_sync_coordinator(data_dir, store, github_command, github_review_sync)
    }

    pub fn with_sync_coordinator(
        data_dir: impl AsRef<Path>,
        store: Arc<dyn ActivityStore>,
        github_command: impl Into<PathBuf>,
        github_review_sync: GitHubReviewSyncCoordinator,
    ) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            store,
            github_command: github_command.into(),
            github_review_sync,
        }
    }

    pub fn save_session_snapshot(&self, snapshot: &ReviewChatSessionSnapshot) -> AgentResult<()> {
        validate_path_key(&snapshot.head_sha, "review chat head SHA")?;
        let path = self.session_snapshot_path(&snapshot.activity_id, &snapshot.head_sha);
        let parent = path.parent().ok_or_else(|| {
            AgentError::invalid_input(format!(
                "review chat session snapshot has no parent: {}",
                path.display()
            ))
        })?;
        fs_err::create_dir_all(parent).map_err(|error| {
            AgentError::io_path("create review chat session directory", parent, error)
        })?;
        write_json_atomic(&path, snapshot)
    }

    pub fn commit(
        &self,
        activity_id: &ActivityId,
        target: &PullRequestRef,
        input: &FinishReviewCommentBatchInput,
    ) -> AgentResult<FinishReviewCommentBatchResult> {
        validate_batch_id(&input.batch.id)?;
        self.validate_activity_target(activity_id, target)?;
        let journal_path = self.journal_path(activity_id, &input.batch.id);
        let _batch_guard = BatchCommitGuard::acquire(journal_path.clone())?;
        let mut journal = if journal_path.exists() {
            let journal: ReviewCommentBatchJournal = read_json(&journal_path)?;
            if journal.request != *input {
                return Err(AgentError::invalid_input(format!(
                    "review comment batch `{}` was already used with different contents",
                    input.batch.id
                )));
            }
            if let Some(result) = journal.result {
                return Ok(result);
            }
            journal
        } else {
            let journal = if input.batch.is_empty() {
                self.prepare_journal(activity_id, Path::new("."), "", &[], input)?
            } else {
                let snapshot = self.load_session_snapshot(activity_id, target, input)?;
                self.prepare_journal(
                    activity_id,
                    &snapshot.repo_dir,
                    &snapshot.diff,
                    &snapshot.existing_comments,
                    input,
                )?
            };
            persist_journal(&journal_path, &journal)?;
            journal
        };

        if input.batch.is_empty() {
            let result = FinishReviewCommentBatchResult {
                batch_id: input.batch.id.clone(),
                no_op: true,
                committed_addition_count: 0,
                committed_deletion_count: 0,
                pending_review_id: None,
                pending_review_url: None,
                existing_comments: Vec::new(),
            };
            journal.result = Some(result.clone());
            persist_journal(&journal_path, &journal)?;
            return Ok(result);
        }

        self.ensure_current_head(target, &input.pinned_head_sha)?;
        let artifacts = self.ensure_artifacts(activity_id, input, &journal.artifact_ids)?;
        let mut pending_review_id = None;
        let mut pending_review_url = None;
        for (index, artifact) in artifacts.into_iter().enumerate() {
            let outcomes = self
                .github_review_sync
                .synchronize(target, std::slice::from_ref(&artifact))
                .map_err(|error| {
                    error.with_context(format!(
                        "commit review comment batch {} after {index} of {} addition(s)",
                        input.batch.id,
                        input.batch.additions.len()
                    ))
                })?;
            let outcome = outcomes.into_iter().next().ok_or_else(|| {
                AgentError::new("GitHub review synchronization returned no artifact outcome")
            })?;
            let updated = self
                .store
                .update_artifact_sync_state(&artifact.id, outcome.sync_state)?;
            if let ArtifactSyncState::Pending {
                remote_id,
                remote_url,
                ..
            } = updated.sync_state
            {
                pending_review_id = remote_id;
                pending_review_url = remote_url;
            }
        }

        let client = GitHubPullRequestClient::new(target.clone(), &self.github_command);
        for (index, deletion_id) in input.batch.deletion_ids.iter().enumerate() {
            if journal.completed_deletion_ids.contains(deletion_id) {
                continue;
            }
            match client.delete_review_comment(deletion_id) {
                Ok(()) => {}
                Err(error) if error.github_http_status() == Some(404) => {}
                Err(error) => {
                    return Err(error.with_context(format!(
                        "commit review comment batch {} after all {} addition(s) and {index} of {} deletion(s): delete draft review comment {deletion_id}",
                        input.batch.id,
                        input.batch.additions.len(),
                        input.batch.deletion_ids.len()
                    )));
                }
            }
            journal.completed_deletion_ids.insert(deletion_id.clone());
            persist_journal(&journal_path, &journal)?;
        }

        let existing_comments = client
            .review_comments()?
            .into_iter()
            .map(|comment| ExistingReviewComment {
                id: comment.id,
                review_id: comment.review_id,
                path: comment.path,
                line: comment.line,
                body: comment.body,
                author: comment.author,
                draft: comment.draft,
            })
            .collect::<Vec<_>>();
        self.update_session_comments(activity_id, target, input, &existing_comments)?;
        let result = FinishReviewCommentBatchResult {
            batch_id: input.batch.id.clone(),
            no_op: false,
            committed_addition_count: input.batch.additions.len(),
            committed_deletion_count: input.batch.deletion_ids.len(),
            pending_review_id,
            pending_review_url,
            existing_comments,
        };
        journal.result = Some(result.clone());
        persist_journal(&journal_path, &journal)?;
        Ok(result)
    }

    fn load_session_snapshot(
        &self,
        activity_id: &ActivityId,
        target: &PullRequestRef,
        input: &FinishReviewCommentBatchInput,
    ) -> AgentResult<ReviewChatSessionSnapshot> {
        validate_path_key(&input.pinned_head_sha, "review chat head SHA")?;
        let path = self.session_snapshot_path(activity_id, &input.pinned_head_sha);
        if !path.exists() {
            return Err(AgentError::invalid_input(
                "review chat session snapshot is unavailable; exit and rerun nitpick review chat",
            ));
        }
        let snapshot: ReviewChatSessionSnapshot = read_json(&path)?;
        let repository = format!("{}/{}", target.owner, target.repo);
        if snapshot.activity_id != *activity_id
            || snapshot.repository != repository
            || snapshot.number != target.number
            || snapshot.head_sha != input.pinned_head_sha
        {
            return Err(AgentError::invalid_input(format!(
                "review chat session snapshot does not match activity {activity_id} and {target}; exit and rerun nitpick review chat"
            )));
        }
        Ok(snapshot)
    }

    fn update_session_comments(
        &self,
        activity_id: &ActivityId,
        target: &PullRequestRef,
        input: &FinishReviewCommentBatchInput,
        existing_comments: &[ExistingReviewComment],
    ) -> AgentResult<()> {
        let mut snapshot = self.load_session_snapshot(activity_id, target, input)?;
        snapshot.existing_comments = existing_comments.to_vec();
        self.save_session_snapshot(&snapshot)
    }

    fn validate_activity_target(
        &self,
        activity_id: &ActivityId,
        target: &PullRequestRef,
    ) -> AgentResult<()> {
        let activity = self.store.get(activity_id)?;
        let resolved = ReviewActivityIdentity::new(&activity)
            .pull_request_target()
            .ok_or_else(|| {
                AgentError::invalid_input(format!(
                    "activity {activity_id} is not a pull request review"
                ))
            })?;
        let repository = format!("{}/{}", target.owner, target.repo);
        if resolved.repository != repository || resolved.number != target.number {
            return Err(AgentError::invalid_input(format!(
                "activity {activity_id} does not review {target}"
            )));
        }
        Ok(())
    }

    fn prepare_journal(
        &self,
        activity_id: &ActivityId,
        repo_dir: &Path,
        diff: &str,
        existing_comments: &[ExistingReviewComment],
        input: &FinishReviewCommentBatchInput,
    ) -> AgentResult<ReviewCommentBatchJournal> {
        if !input.batch.is_empty() {
            let validator = ReviewCommentValidator::for_diff(repo_dir, diff)?;
            for comment in &input.batch.additions {
                validator.validate_comment(&comment.path, comment.line, comment.body.clone())?;
            }
            for deletion_id in &input.batch.deletion_ids {
                validate_deletable_comment(existing_comments, deletion_id)?;
            }
        }
        Ok(ReviewCommentBatchJournal {
            request: input.clone(),
            artifact_ids: input
                .batch
                .additions
                .iter()
                .enumerate()
                .map(|(index, _)| deterministic_artifact_id(activity_id, &input.batch.id, index))
                .collect(),
            completed_deletion_ids: BTreeSet::new(),
            result: None,
        })
    }

    fn ensure_current_head(
        &self,
        target: &PullRequestRef,
        pinned_head_sha: &str,
    ) -> AgentResult<()> {
        let current_head =
            GitHubPullRequestClient::new(target.clone(), &self.github_command).head_sha()?;
        if current_head != pinned_head_sha {
            return Err(AgentError::invalid_input(format!(
                "pull request head changed from {pinned_head_sha} to {current_head}; exit and rerun nitpick review chat"
            )));
        }
        Ok(())
    }

    fn ensure_artifacts(
        &self,
        activity_id: &ActivityId,
        input: &FinishReviewCommentBatchInput,
        artifact_ids: &[ArtifactId],
    ) -> AgentResult<Vec<Artifact>> {
        let mut artifacts = Vec::with_capacity(input.batch.additions.len());
        for (comment, artifact_id) in input.batch.additions.iter().zip(artifact_ids) {
            let artifact = match self.store.get_artifact(artifact_id) {
                Ok(artifact) => artifact,
                Err(AgentError::NotFound { .. }) => Artifact::local(
                    artifact_id.clone(),
                    activity_id.clone(),
                    ArtifactKind::ReviewComment,
                    ArtifactContent::ReviewComment(comment.clone()),
                ),
                Err(error) => return Err(error),
            };
            artifacts.push(artifact);
        }
        self.store.save_artifacts(&artifacts)?;
        Ok(artifacts)
    }

    fn journal_path(&self, activity_id: &ActivityId, batch_id: &str) -> PathBuf {
        self.data_dir
            .join("review-comment-batches")
            .join(activity_id.as_str())
            .join(format!("{batch_id}.json"))
    }

    fn session_snapshot_path(&self, activity_id: &ActivityId, head_sha: &str) -> PathBuf {
        self.data_dir
            .join("review-chat-sessions")
            .join(activity_id.as_str())
            .join(format!("{head_sha}.json"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReviewCommentBatchJournal {
    request: FinishReviewCommentBatchInput,
    artifact_ids: Vec<ArtifactId>,
    #[serde(default)]
    completed_deletion_ids: BTreeSet<String>,
    result: Option<FinishReviewCommentBatchResult>,
}

fn persist_journal(path: &Path, journal: &ReviewCommentBatchJournal) -> AgentResult<()> {
    let parent = path.parent().ok_or_else(|| {
        AgentError::invalid_input(format!(
            "review comment batch journal has no parent: {}",
            path.display()
        ))
    })?;
    fs_err::create_dir_all(parent).map_err(|error| {
        AgentError::io_path(
            "create review comment batch journal directory",
            parent,
            error,
        )
    })?;
    write_json_atomic(path, journal)
}

fn deterministic_artifact_id(activity_id: &ActivityId, batch_id: &str, index: usize) -> ArtifactId {
    ArtifactId::new(format!(
        "{}-review-chat-{batch_id}-{index}",
        activity_id.as_str()
    ))
}

fn validate_batch_id(batch_id: &str) -> AgentResult<()> {
    validate_path_key(batch_id, "review comment batch ID")
}

fn validate_path_key(value: &str, label: &str) -> AgentResult<()> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Ok(());
    }
    Err(AgentError::invalid_input(format!(
        "invalid {label} `{value}`"
    )))
}

fn validate_deletable_comment(comments: &[ExistingReviewComment], id: &str) -> AgentResult<()> {
    let Some(comment) = comments.iter().find(|comment| comment.id == id) else {
        return Err(AgentError::invalid_input(format!(
            "review comment `{id}` is not available to this review session"
        )));
    };
    if !comment.draft || !comment.body.starts_with("🤖") {
        return Err(AgentError::invalid_input(
            "can only delete robot-authored draft comments",
        ));
    }
    Ok(())
}

struct BatchCommitGuard {
    key: PathBuf,
}

impl BatchCommitGuard {
    fn acquire(key: PathBuf) -> AgentResult<Self> {
        let (active, changed) = batch_commit_gate();
        let mut active = active
            .lock()
            .map_err(|_| AgentError::io("review comment batch lock", "poisoned"))?;
        while active.contains(&key) {
            active = changed
                .wait(active)
                .map_err(|_| AgentError::io("review comment batch lock", "poisoned"))?;
        }
        active.insert(key.clone());
        Ok(Self { key })
    }
}

impl Drop for BatchCommitGuard {
    fn drop(&mut self) {
        let (active, changed) = batch_commit_gate();
        if let Ok(mut active) = active.lock() {
            active.remove(&self.key);
            changed.notify_all();
        }
    }
}

fn batch_commit_gate() -> &'static (Mutex<HashSet<PathBuf>>, Condvar) {
    static GATE: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();
    GATE.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}
