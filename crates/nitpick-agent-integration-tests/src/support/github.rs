use std::sync::Mutex;

use nitpick_agent_core::{AgentError, AgentResult, ReviewInput, ReviewSubject};
use nitpick_agent_github::{DiscoveredPullRequest, ReviewRequestDiscovery};

pub fn pull_request(head_sha: &str) -> DiscoveredPullRequest {
    DiscoveredPullRequest {
        owner: "stephanos".into(),
        repo: "nitpick-agent".into(),
        number: 42,
        head_sha: head_sha.into(),
    }
}

pub struct StubDiscovery {
    pull_requests: Mutex<Vec<DiscoveredPullRequest>>,
    calls: Mutex<usize>,
    error: Mutex<Option<String>>,
}

impl StubDiscovery {
    pub fn new(pull_requests: Vec<DiscoveredPullRequest>) -> Self {
        Self {
            pull_requests: Mutex::new(pull_requests),
            calls: Mutex::new(0),
            error: Mutex::new(None),
        }
    }

    pub fn set_pull_requests(&self, pull_requests: Vec<DiscoveredPullRequest>) {
        *self.pull_requests.lock().expect("lock") = pull_requests;
    }

    pub fn set_error(&self, error: impl Into<String>) {
        *self.error.lock().expect("lock") = Some(error.into());
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().expect("lock")
    }
}

impl ReviewRequestDiscovery for StubDiscovery {
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        *self.calls.lock().expect("lock") += 1;
        if let Some(error) = self.error.lock().expect("lock").clone() {
            return Err(AgentError::new(error));
        }
        Ok(self.pull_requests.lock().expect("lock").clone())
    }

    fn review_input(&self, pull_request: &DiscoveredPullRequest) -> AgentResult<ReviewInput> {
        let repository = format!("{}/{}", pull_request.owner, pull_request.repo);
        Ok(ReviewInput {
            repo_dir: ".".into(),
            instructions: format!(
                "Review GitHub pull request {repository}#{} at head {}.",
                pull_request.number, pull_request.head_sha
            ),
            subject: ReviewSubject {
                repository,
                number: Some(pull_request.number),
                title: "Stub PR".into(),
                author: "stub-author".into(),
            },
            diff: format!("diff for {}", pull_request.head_sha),
        })
    }
}
