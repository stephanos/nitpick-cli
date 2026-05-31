use std::path::PathBuf;

use nitpick_agent_core::AgentResult;

use crate::{
    GitHubPullRequestContext, GitHubReviewComment, GitHubReviewResponse, PullRequestDetails,
    PullRequestRef, command::GitHubCommand, github_delete_from_cli,
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
