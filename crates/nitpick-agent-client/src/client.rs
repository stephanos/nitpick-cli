use std::time::Duration;

use nitpick_agent_core::{Activity, Artifact, ChatInput, ReviewInput, ReviewRequest};

use crate::{
    CleanupCheckoutsResult, HostClientError, HostClientResult, HostStatus,
    json::parse_json,
    transport::{ArtifactSyncInput, request_host},
};

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

    pub fn status(&self) -> HostClientResult<HostStatus> {
        self.get_json("/status")
    }

    pub fn activities(&self) -> HostClientResult<Vec<Activity>> {
        self.get_json("/activities")
    }

    pub fn activity_artifacts(&self, activity_id: &str) -> HostClientResult<Vec<Artifact>> {
        self.get_json(&format!("/activities/{activity_id}/artifacts"))
    }

    pub fn artifact(&self, artifact_id: &str) -> HostClientResult<Artifact> {
        self.get_json(&format!("/artifacts/{artifact_id}"))
    }

    pub fn pending_sync_artifacts(
        &self,
        destination: Option<&str>,
    ) -> HostClientResult<Vec<Artifact>> {
        match destination {
            Some(destination) => self.get_json(&format!("/sync/pending?destination={destination}")),
            None => self.get_json("/sync/pending"),
        }
    }

    pub fn review_requests(&self, only_new: bool) -> HostClientResult<Vec<ReviewRequest>> {
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
    ) -> HostClientResult<Artifact> {
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
    ) -> HostClientResult<Vec<Artifact>> {
        self.post_json(
            &format!("/activities/{activity_id}/artifact-sync"),
            &ArtifactSyncInput {
                destination,
                target,
            },
        )
    }

    pub fn review(&self, input: &ReviewInput) -> HostClientResult<Activity> {
        self.post_json("/reviews", input)
    }

    pub fn chat(&self, input: &ChatInput) -> HostClientResult<Activity> {
        self.post_json("/chats", input)
    }

    pub fn cleanup_checkouts(&self) -> HostClientResult<CleanupCheckoutsResult> {
        self.post_json("/maintenance/cleanup-checkouts", &serde_json::json!({}))
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> HostClientResult<T> {
        let body = request_host(&self.agent, &self.addr, "GET", path, None)?;
        parse_json(&body)
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        input: &impl serde::Serialize,
    ) -> HostClientResult<T> {
        let body =
            serde_json::to_vec(input).map_err(|error| HostClientError::SerializeRequest {
                message: error.to_string(),
            })?;
        let response = request_host(&self.agent, &self.addr, "POST", path, Some(&body))?;
        parse_json(&response)
    }
}
