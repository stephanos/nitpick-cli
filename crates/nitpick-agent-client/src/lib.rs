use std::time::Duration;

use nitpick_agent_core::{
    Activity, Artifact, ChatInput, ReviewInput, ReviewRequest, parse_json_str,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostClientError {
    #[error("nitpick-agent-host unavailable at {addr}: {message}")]
    Unavailable { addr: String, message: String },
    #[error("unexpected host status: {status}")]
    HttpStatus { status: String },
    #[error("unexpected host status: {status}: {body}")]
    HttpStatusWithBody { status: String, body: String },
    #[error("read nitpick-agent-host response from {addr}{path}: {message}")]
    ReadResponse {
        addr: String,
        path: String,
        message: String,
    },
    #[error("invalid host response at {path}: {message}")]
    InvalidJson { path: String, message: String },
    #[error("serialize host request: {message}")]
    SerializeRequest { message: String },
    #[error("GET host request cannot include a body")]
    GetWithBody,
    #[error("unsupported host request method: {method}")]
    UnsupportedMethod { method: String },
}

impl HostClientError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}

impl From<HostClientError> for String {
    fn from(error: HostClientError) -> Self {
        error.to_string()
    }
}

pub type HostClientResult<T> = Result<T, HostClientError>;

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
) -> HostClientResult<String> {
    let url = format!("http://{addr}{path}");
    let result = match (method, body) {
        ("GET", None) => agent.get(&url).call(),
        ("POST", Some(body)) => agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(body),
        ("POST", None) => agent.post(&url).send(&[]),
        ("GET", Some(_)) => return Err(HostClientError::GetWithBody),
        _ => {
            return Err(HostClientError::UnsupportedMethod {
                method: method.to_owned(),
            });
        }
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) => {
            return Err(HostClientError::Unavailable {
                addr: addr.to_owned(),
                message: error.to_string(),
            });
        }
    };
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| HostClientError::ReadResponse {
            addr: addr.to_owned(),
            path: path.to_owned(),
            message: error.to_string(),
        })
        .and_then(|body| {
            let status = response.status();
            if status.is_success() {
                Ok(body)
            } else {
                let details = body.trim();
                if details.is_empty() {
                    Err(HostClientError::HttpStatus {
                        status: status.to_string(),
                    })
                } else {
                    Err(HostClientError::HttpStatusWithBody {
                        status: status.to_string(),
                        body: details.to_owned(),
                    })
                }
            }
        })
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> HostClientResult<T> {
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
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::{HostClientError, HostStatus, parse_json, request_host};

    #[test]
    fn parse_json_reports_field_path() {
        let error = parse_json::<HostStatus>(r#"{"activity_count":"wrong"}"#)
            .expect_err("invalid field type");

        assert!(matches!(
            error,
            HostClientError::InvalidJson { path, .. } if path == "activity_count"
        ));
    }

    #[test]
    fn request_host_includes_error_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).expect("read request");
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 19\r\nConnection: close\r\n\r\nbad request details",
                )
                .expect("write response");
        });

        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let error = request_host(&agent, &addr.to_string(), "GET", "/status", None)
            .expect_err("status error");
        handle.join().expect("server thread");

        assert!(matches!(
            error,
            HostClientError::HttpStatusWithBody { body, .. } if body == "bad request details"
        ));
    }
}
