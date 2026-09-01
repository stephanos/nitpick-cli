use std::path::PathBuf;

use nitpick_agent_core::AgentResult;
use nitpick_agent_model::Artifact;

use crate::{
    GitHubPullRequestContext, GitHubReviewComment, GitHubReviewPayload, GitHubReviewResponse,
    GitHubUser, PullRequestDetails, PullRequestRef, command::GitHubCommand, github_delete_from_cli,
    github_pull_request_conversation_comments_from_cli, github_review_comments_from_cli,
    github_review_from_cli, github_review_from_cli_with_input, github_reviews_from_cli,
    pull_request_details, pull_request_diff, pull_request_head_sha,
};

pub struct GitHubPullRequestClient {
    target: PullRequestRef,
    command: GitHubCommand,
}

impl GitHubPullRequestClient {
    pub fn new(target: PullRequestRef, command: impl Into<PathBuf>) -> Self {
        Self {
            target,
            command: GitHubCommand::new(command),
        }
    }

    pub(crate) fn command(&self) -> &GitHubCommand {
        &self.command
    }

    pub(crate) fn target(&self) -> &PullRequestRef {
        &self.target
    }

    pub fn head_sha(&self) -> AgentResult<String> {
        pull_request_head_sha(
            &self.command,
            &self.target.owner,
            &self.target.repo,
            self.target.number,
        )
    }

    pub fn details(&self) -> AgentResult<PullRequestDetails> {
        pull_request_details(
            &self.command,
            &self.target.owner,
            &self.target.repo,
            self.target.number,
        )
    }

    pub fn diff(&self) -> AgentResult<String> {
        pull_request_diff(
            &self.command,
            &self.target.owner,
            &self.target.repo,
            self.target.number,
        )
    }

    pub fn fetch_review(&self, review_id: &str) -> AgentResult<GitHubReviewResponse> {
        github_review_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/reviews/{}",
                self.target.owner, self.target.repo, self.target.number, review_id
            )],
        )
    }

    pub(crate) fn authenticated_login(&self) -> AgentResult<String> {
        let user: GitHubUser = self
            .command
            .json(&["api", "user"], "GitHub authenticated user response")?;
        Ok(user.login)
    }

    pub(crate) fn reviews(&self) -> AgentResult<Vec<GitHubReviewResponse>> {
        github_reviews_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/reviews",
                self.target.owner, self.target.repo, self.target.number
            )],
        )
    }

    pub(crate) fn review_comments_for(
        &self,
        review_id: &str,
    ) -> AgentResult<Vec<GitHubReviewComment>> {
        let mut comments = github_review_comments_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/reviews/{}/comments",
                self.target.owner, self.target.repo, self.target.number, review_id
            )],
        )?;
        for comment in &mut comments {
            comment.draft = true;
        }
        Ok(comments)
    }

    pub(crate) fn create_pending_review(
        &self,
        head_sha: String,
        artifacts: &[Artifact],
    ) -> AgentResult<GitHubReviewResponse> {
        let payload = GitHubReviewPayload::batch(head_sha, artifacts)?;
        github_review_from_cli_with_input(
            &self.command,
            &[
                &format!(
                    "repos/{}/{}/pulls/{}/reviews",
                    self.target.owner, self.target.repo, self.target.number
                ),
                "--method",
                "POST",
                "--input",
                "-",
            ],
            &payload.to_string(),
        )
    }

    pub(crate) fn append_review_comment(
        &self,
        head_sha: &str,
        artifact: &Artifact,
    ) -> AgentResult<()> {
        let payload = GitHubReviewPayload::append_comment(head_sha.to_owned(), artifact)?;
        self.command.output_with_input(
            &[
                "api",
                &format!(
                    "repos/{}/{}/pulls/{}/comments",
                    self.target.owner, self.target.repo, self.target.number
                ),
                "--method",
                "POST",
                "--input",
                "-",
            ],
            &payload.to_string(),
        )?;
        Ok(())
    }

    pub fn update_pending_review_body(
        &self,
        review_id: &str,
        body: &str,
    ) -> AgentResult<GitHubReviewResponse> {
        github_review_from_cli_with_input(
            &self.command,
            &[
                &format!(
                    "repos/{}/{}/pulls/{}/reviews/{}",
                    self.target.owner, self.target.repo, self.target.number, review_id
                ),
                "--method",
                "PUT",
                "--input",
                "-",
            ],
            &serde_json::json!({ "body": body }).to_string(),
        )
    }

    pub fn review_comments(&self) -> AgentResult<Vec<GitHubReviewComment>> {
        let mut comments = github_review_comments_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/comments",
                self.target.owner, self.target.repo, self.target.number
            )],
        )?;
        let reviews = github_reviews_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/reviews",
                self.target.owner, self.target.repo, self.target.number
            )],
        )?;
        for review in reviews
            .into_iter()
            .filter(|review| review.state == "PENDING")
        {
            let pending_comments = github_review_comments_from_cli(
                &self.command,
                &[&format!(
                    "repos/{}/{}/pulls/{}/reviews/{}/comments",
                    self.target.owner, self.target.repo, self.target.number, review.id
                )],
            )?;
            comments.extend(pending_comments.into_iter().map(|mut comment| {
                comment.draft = true;
                comment
            }));
        }
        let mut seen = std::collections::HashSet::new();
        comments.retain(|comment| seen.insert(comment.id.clone()));
        Ok(comments)
    }

    pub fn pull_request_context(&self) -> AgentResult<GitHubPullRequestContext> {
        let details = self.details()?;
        let conversation_comments = github_pull_request_conversation_comments_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/issues/{}/comments",
                self.target.owner, self.target.repo, self.target.number
            )],
        )?;
        Ok(GitHubPullRequestContext {
            title: details.title,
            author: details.author,
            url: details.url,
            body: details.body,
            head_sha: details.head_sha,
            head_ref_name: details.head_ref_name,
            state: details.state.as_str().into(),
            conversation_comments,
        })
    }

    pub fn delete_review_comment(&self, comment_id: &str) -> AgentResult<()> {
        github_delete_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/comments/{}",
                self.target.owner, self.target.repo, comment_id
            )],
        )
    }
}
