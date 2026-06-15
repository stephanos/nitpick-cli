use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePullRequest {
    pub reference: RemotePullRequestRef,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub state: RemotePullRequestState,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub author_display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub reviewers: Vec<RemotePullRequestReviewer>,
    #[serde(default)]
    pub requested_team_names: Vec<String>,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub checks: Option<RemotePullRequestChecks>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePullRequestReviewer {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub state: RemotePullRequestReviewerState,
    #[serde(default)]
    pub discussing: bool,
    #[serde(default)]
    pub last_commented_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePullRequestReviewerState {
    #[default]
    None,
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePullRequestChecks {
    #[serde(default)]
    pub state: RemotePullRequestChecksState,
    #[serde(default)]
    pub failed_jobs: Vec<String>,
    #[serde(default)]
    pub rerunnable: bool,
    #[serde(default)]
    pub started_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePullRequestChecksState {
    #[default]
    None,
    Pending,
    Success,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPullRequestState {
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub last_opened_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemotePullRequestRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl RemotePullRequestRef {
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl std::fmt::Display for RemotePullRequestRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

impl FromStr for RemotePullRequestRef {
    type Err = ParseRemotePullRequestRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return parse_github_pull_url(trimmed);
        }

        let (repo, number) = trimmed
            .rsplit_once('#')
            .ok_or_else(|| ParseRemotePullRequestRefError::new(trimmed))?;
        let (owner, repo) = repo
            .split_once('/')
            .ok_or_else(|| ParseRemotePullRequestRefError::new(trimmed))?;
        let number = number
            .parse::<u64>()
            .map_err(|_| ParseRemotePullRequestRefError::new(trimmed))?;

        Ok(Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePullRequestState {
    #[default]
    Open,
    Closed,
    Merged,
}

impl RemotePullRequestState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseRemotePullRequestRefError {
    value: String,
}

impl ParseRemotePullRequestRefError {
    fn new(value: &str) -> Self {
        Self {
            value: value.to_owned(),
        }
    }
}

impl std::fmt::Display for ParseRemotePullRequestRefError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid GitHub pull request reference `{}`",
            self.value
        )
    }
}

impl std::error::Error for ParseRemotePullRequestRefError {}

fn parse_github_pull_url(
    value: &str,
) -> Result<RemotePullRequestRef, ParseRemotePullRequestRefError> {
    let url = url::Url::parse(value).map_err(|_| ParseRemotePullRequestRefError::new(value))?;
    if url.domain() != Some("github.com") {
        return Err(ParseRemotePullRequestRefError::new(value));
    }

    let mut segments = url
        .path_segments()
        .ok_or_else(|| ParseRemotePullRequestRefError::new(value))?;
    let owner = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| ParseRemotePullRequestRefError::new(value))?;
    let repo = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| ParseRemotePullRequestRefError::new(value))?;
    let marker = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| ParseRemotePullRequestRefError::new(value))?;
    let number = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| ParseRemotePullRequestRefError::new(value))?;
    if marker != "pull" {
        return Err(ParseRemotePullRequestRefError::new(value));
    }

    Ok(RemotePullRequestRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number: number
            .parse::<u64>()
            .map_err(|_| ParseRemotePullRequestRefError::new(value))?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LocalPullRequestState, RemotePullRequest, RemotePullRequestChecks,
        RemotePullRequestChecksState, RemotePullRequestRef, RemotePullRequestReviewer,
        RemotePullRequestReviewerState, RemotePullRequestState,
    };

    #[test]
    fn parses_owner_repo_number_reference() {
        let reference = "acme/platform#42"
            .parse::<RemotePullRequestRef>()
            .expect("reference parses");

        assert_eq!(reference.owner, "acme");
        assert_eq!(reference.repo, "platform");
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn parses_github_pull_request_url() {
        let reference = "https://github.com/acme/platform/pull/42"
            .parse::<RemotePullRequestRef>()
            .expect("url parses");

        assert_eq!(reference.owner, "acme");
        assert_eq!(reference.repo, "platform");
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn remote_pull_request_round_trips_through_serde() {
        let pull_request = RemotePullRequest {
            reference: RemotePullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            title: "Add dashboard API".into(),
            body: "Implements the first slice.".into(),
            url: "https://github.com/acme/platform/pull/42".into(),
            state: RemotePullRequestState::Open,
            draft: false,
            author: "stephanos".into(),
            author_display_name: Some("Stephanos".into()),
            created_at: Some("2026-06-13T00:00:00Z".into()),
            updated_at: Some("2026-06-13T01:00:00Z".into()),
            head_sha: "abc123".into(),
            head_ref_name: "feature/dashboard".into(),
            base_ref_name: "main".into(),
            labels: vec!["dashboard".into()],
            reviewers: vec![RemotePullRequestReviewer {
                login: "alice".into(),
                display_name: Some("Alice".into()),
                state: RemotePullRequestReviewerState::Approved,
                discussing: false,
                last_commented_at: Some("2026-06-13T00:30:00Z".into()),
            }],
            requested_team_names: vec!["platform".into()],
            additions: 10,
            deletions: 4,
            changed_files: 2,
            checks: Some(RemotePullRequestChecks {
                state: RemotePullRequestChecksState::Pending,
                failed_jobs: Vec::new(),
                rerunnable: false,
                started_at: Some("2026-06-13T00:45:00Z".into()),
            }),
        };

        let json = serde_json::to_string(&pull_request).expect("serialize pull request");
        let decoded: RemotePullRequest =
            serde_json::from_str(&json).expect("deserialize pull request");

        assert_eq!(decoded, pull_request);
    }

    #[test]
    fn local_pull_request_state_round_trips_through_serde() {
        let state = LocalPullRequestState {
            starred: true,
            ignored: false,
            last_opened_at: Some("2026-06-13T00:00:00Z".into()),
        };

        let json = serde_json::to_string(&state).expect("serialize local state");
        let decoded: LocalPullRequestState =
            serde_json::from_str(&json).expect("deserialize local state");

        assert_eq!(decoded, state);
    }
}
