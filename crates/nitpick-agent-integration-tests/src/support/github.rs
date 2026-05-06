use std::sync::Mutex;

use nitpick_agent_core::AgentResult;
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
}

impl StubDiscovery {
    pub fn new(pull_requests: Vec<DiscoveredPullRequest>) -> Self {
        Self {
            pull_requests: Mutex::new(pull_requests),
        }
    }

    pub fn set_pull_requests(&self, pull_requests: Vec<DiscoveredPullRequest>) {
        *self.pull_requests.lock().expect("lock") = pull_requests;
    }
}

impl ReviewRequestDiscovery for StubDiscovery {
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        Ok(self.pull_requests.lock().expect("lock").clone())
    }
}
