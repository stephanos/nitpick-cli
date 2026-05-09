use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use nitpick_agent_core::{Activity, Artifact, ChatInput, ReviewInput};
use nitpick_agent_github::DiscoveredPullRequest;
use serde::Deserialize;

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
    pub github_discovery_enabled: bool,
    pub github_last_poll_unix: Option<u64>,
    pub github_last_poll_summary: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HostClient {
    addr: String,
}

impl HostClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
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

    pub fn github_review_requests(
        &self,
        only_new: bool,
    ) -> Result<Vec<DiscoveredPullRequest>, String> {
        if only_new {
            self.get_json("/github/review-requests?filter=new")
        } else {
            self.get_json("/github/review-requests")
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

    pub fn review(&self, input: &ReviewInput) -> Result<Activity, String> {
        self.post_json("/reviews", input)
    }

    pub fn chat(&self, input: &ChatInput) -> Result<Activity, String> {
        self.post_json("/chats", input)
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let body = request_host(&self.addr, "GET", path, None)?;
        serde_json::from_str(&body).map_err(|error| error.to_string())
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        input: &impl serde::Serialize,
    ) -> Result<T, String> {
        let body = serde_json::to_vec(input).map_err(|error| error.to_string())?;
        let response = request_host(&self.addr, "POST", path, Some(&body))?;
        serde_json::from_str(&response).map_err(|error| error.to_string())
    }
}

#[derive(serde::Serialize)]
struct ArtifactSyncInput<'a> {
    destination: &'a str,
    target: Option<&'a str>,
}

fn request_host(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr)
        .map_err(|error| format!("nitpick-agent-host unavailable at {addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let content_length = body.map_or(0, <[u8]>::len);
    let content_type = if body.is_some() {
        "Content-Type: application/json\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: nitpick-agent-host\r\nConnection: close\r\n{content_type}Content-Length: {content_length}\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    if let Some(body) = body {
        stream.write_all(body).map_err(|error| error.to_string())?;
    }

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    let (_, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "missing HTTP response body".to_owned())?;
    if !response.starts_with("HTTP/1.1 200 ") {
        return Err(format!(
            "unexpected host status line: {}",
            response.lines().next().unwrap_or("(empty)")
        ));
    }
    Ok(body.to_owned())
}
