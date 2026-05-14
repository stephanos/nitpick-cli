use std::time::Duration;

use nitpick_agent_core::{Activity, Artifact, ChatInput, ReviewInput, ReviewRequest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostStatus {
    pub activity_count: usize,
    pub running_activity_count: usize,
    pub completed_activity_count: usize,
    pub error_activity_count: usize,
    pub artifact_count: usize,
    pub local_only_artifact_count: usize,
    pub pending_sync_artifact_count: usize,
    pub provider: String,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HostClient {
    addr: String,
    agent: ureq::Agent,
}

impl HostClient {
    pub fn new(addr: impl Into<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(2)))
            .build();
        Self {
            addr: addr.into(),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    pub fn localhost() -> Self {
        Self::new("127.0.0.1:19783")
    }

    pub fn status(&self) -> Result<HostStatus, String> {
        self.get_json("/status")
    }

    pub fn activities(&self) -> Result<Vec<Activity>, String> {
        self.get_json("/activities")
    }

    pub fn activity_artifacts(&self, activity_id: &str) -> Result<Vec<Artifact>, String> {
        self.get_json(&format!("/activities/{activity_id}/artifacts"))
    }

    pub fn artifact(&self, artifact_id: &str) -> Result<Artifact, String> {
        self.get_json(&format!("/artifacts/{artifact_id}"))
    }

    pub fn pending_sync_artifacts(
        &self,
        destination: Option<&str>,
    ) -> Result<Vec<Artifact>, String> {
        match destination {
            Some(destination) => self.get_json(&format!("/sync/pending?destination={destination}")),
            None => self.get_json("/sync/pending"),
        }
    }

    pub fn review_requests(&self, only_new: bool) -> Result<Vec<ReviewRequest>, String> {
        if only_new {
            self.get_json("/review-requests?filter=new")
        } else {
            self.get_json("/review-requests")
        }
    }

    pub fn sync_artifact(
        &self,
        artifact_id: &str,
        destination: &str,
        target: Option<&str>,
    ) -> Result<Artifact, String> {
        self.post_json(
            &format!("/artifacts/{artifact_id}/sync"),
            &ArtifactSyncInput {
                destination,
                target,
            },
        )
    }

    pub fn sync_activity_artifacts(
        &self,
        activity_id: &str,
        destination: &str,
        target: Option<&str>,
    ) -> Result<Vec<Artifact>, String> {
        self.post_json(
            &format!("/activities/{activity_id}/artifact-sync"),
            &ArtifactSyncInput {
                destination,
                target,
            },
        )
    }

    pub fn review(&self, input: &ReviewInput) -> Result<Activity, String> {
        self.post_json("/reviews", input)
    }

    pub fn chat(&self, input: &ChatInput) -> Result<Activity, String> {
        self.post_json("/chats", input)
    }

    pub fn cleanup_checkouts(&self) -> Result<CleanupCheckoutsResult, String> {
        self.post_json("/maintenance/cleanup-checkouts", &serde_json::json!({}))
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let body = request_host(&self.agent, &self.addr, "GET", path, None)?;
        parse_json(&body)
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        input: &impl serde::Serialize,
    ) -> Result<T, String> {
        let body = serde_json::to_vec(input).map_err(|error| error.to_string())?;
        let response = request_host(&self.agent, &self.addr, "POST", path, Some(&body))?;
        parse_json(&response)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CleanupCheckoutsResult {
    pub removed_count: usize,
    pub cleaned: Vec<String>,
}

#[derive(serde::Serialize)]
struct ArtifactSyncInput<'a> {
    destination: &'a str,
    target: Option<&'a str>,
}

fn request_host(
    agent: &ureq::Agent,
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<String, String> {
    let url = format!("http://{addr}{path}");
    let result = match (method, body) {
        ("GET", None) => agent.get(&url).call(),
        ("POST", Some(body)) => agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(body),
        ("POST", None) => agent.post(&url).send(&[]),
        ("GET", Some(_)) => return Err("GET host request cannot include a body".to_owned()),
        _ => return Err(format!("unsupported host request method: {method}")),
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) => {
            return Err(format!("nitpick-agent-host unavailable at {addr}: {error}"));
        }
    };
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| {
            format!(
                "read nitpick-agent-host response from {addr}{path}: {}",
                error
            )
        })
        .and_then(|body| {
            let status = response.status();
            if status.is_success() {
                Ok(body)
            } else {
                let details = body.trim();
                if details.is_empty() {
                    Err(format!("unexpected host status: {status}"))
                } else {
                    Err(format!("unexpected host status: {status}: {details}"))
                }
            }
        })
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, String> {
    let mut deserializer = serde_json::Deserializer::from_str(body);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        format!(
            "invalid host response at {}: {}",
            error.path(),
            error.inner()
        )
    })
}
