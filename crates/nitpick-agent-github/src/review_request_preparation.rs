use std::path::PathBuf;

use nitpick_agent_core::{ReviewInput, ReviewMode, ReviewSubject};

use crate::{DiscoveredPullRequest, PullRequestDetails};

pub fn prepare_github_review_input(
    pull_request: &DiscoveredPullRequest,
    details: PullRequestDetails,
    diff: String,
    repo_dir: PathBuf,
) -> ReviewInput {
    let repository = format!("{}/{}", pull_request.owner, pull_request.repo);
    ReviewInput {
        repo_dir,
        source: "github".into(),
        review_mode: ReviewMode::Requested,
        instructions: format!(
            "Review GitHub pull request {repository}#{}.\n\nURL: {}\nState: {}\nHead SHA: {}\nHead ref: {}.",
            pull_request.number,
            details.url,
            details.state.as_str(),
            details.head_sha,
            details.head_ref_name
        ),
        subject: ReviewSubject {
            repository,
            number: Some(pull_request.number),
            title: details.title,
            author: details.author,
        },
        head_sha: details.head_sha,
        diff,
        ..ReviewInput::default()
    }
}
