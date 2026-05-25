use std::time::Duration;

use nitpick_agent_core::{
    Activity, Artifact, ChatInput, CleanupCheckoutsResult, HostStatus, ReviewInput, ReviewRequest,
    parse_json_str,
};

use crate::{
    HostClientError, HostClientResult,
    transport::{ArtifactSyncInput, request_host},
};

const HOST_CLIENT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct HostClient {
    addr: String,
    agent: ureq::Agent,
}

impl HostClient {
    pub fn new(addr: impl Into<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(HOST_CLIENT_TIMEOUT))
            .build();
        Self {
            addr: addr.into(),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    pub fn localhost() -> Self {
        Self::new("127.0.0.1:19783")
    }

    // Query operations (read-only): observe state without modification
    pub fn status(&self) -> HostClientResult<HostStatus> {
        self.get_json("/status")
    }

    pub fn activities(&self) -> HostClientResult<Vec<Activity>> {
        self.get_json("/activities")
    }

    pub fn filtered_activities(
        &self,
        kind: Option<&str>,
        status: Option<&str>,
        limit: Option<usize>,
    ) -> HostClientResult<Vec<Activity>> {
        let mut query = Vec::new();
        if let Some(kind) = kind {
            query.push(format!("kind={kind}"));
        }
        if let Some(status) = status {
            query.push(format!("status={status}"));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if query.is_empty() {
            self.activities()
        } else {
            self.get_json(&format!("/activities?{}", query.join("&")))
        }
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

    // Action operations (write): modify host state
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
        decode_response_json(&body)
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
        decode_response_json(&response)
    }
}

fn decode_response_json<T: serde::de::DeserializeOwned>(body: &str) -> HostClientResult<T> {
    parse_json_str(body, "invalid host response").map_err(|error| match error {
        nitpick_agent_core::AgentError::Json { path, error, .. } => HostClientError::InvalidJson {
            path,
            message: error,
        },
        error => HostClientError::InvalidJson {
            path: "$".to_owned(),
            message: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use nitpick_agent_core::HostStatus;

    use crate::HostClientError;

    use super::decode_response_json;

    #[test]
    fn decode_response_json_reports_field_path() {
        let error = decode_response_json::<HostStatus>(r#"{"activity_count":"wrong"}"#)
            .expect_err("invalid field type");

        assert!(matches!(
            error,
            HostClientError::InvalidJson { path, .. } if path == "activity_count"
        ));
    }
}
