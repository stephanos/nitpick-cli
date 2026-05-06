use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactContent, ArtifactSyncDestination,
    ArtifactSyncOutcome, ArtifactSyncState,
};
use serde::{Deserialize, Serialize};

pub struct GitHubDryRunSyncDestination;

impl ArtifactSyncDestination for GitHubDryRunSyncDestination {
    fn name(&self) -> &'static str {
        "github"
    }

    fn sync(&self, _artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome> {
        Ok(ArtifactSyncOutcome {
            sync_state: ArtifactSyncState::Pending {
                destination: self.name().into(),
            },
            remote_id: None,
        })
    }
}

pub struct GitHubCliSyncDestination {
    target: PullRequestRef,
    command: PathBuf,
}

impl GitHubCliSyncDestination {
    pub fn new(target: PullRequestRef, command: impl Into<PathBuf>) -> Self {
        Self {
            target,
            command: command.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPullRequest {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub head_sha: String,
}

pub struct GitHubCliDiscovery {
    command: PathBuf,
}

pub trait ReviewRequestDiscovery: Send + Sync {
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>>;
}

impl GitHubCliDiscovery {
    pub fn new(command: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        <Self as ReviewRequestDiscovery>::requested_reviews(self)
    }
}

impl ReviewRequestDiscovery for GitHubCliDiscovery {
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        let output = Command::new(&self.command)
            .args([
                "search",
                "prs",
                "user-review-requested:@me",
                "--state=open",
                "--limit",
                "100",
                "--json",
                "repository,number",
            ])
            .output()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    self.command.display()
                ))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::new(format!(
                "GitHub CLI failed with status {}{}",
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }

        let records: Vec<SearchPullRequest> =
            serde_json::from_slice(&output.stdout).map_err(|error| {
                AgentError::new(format!("invalid GitHub review request response: {error}"))
            })?;
        records
            .into_iter()
            .map(|record| self.discover_with_head_sha(record))
            .collect()
    }
}

impl GitHubCliDiscovery {
    fn discover_with_head_sha(
        &self,
        record: SearchPullRequest,
    ) -> AgentResult<DiscoveredPullRequest> {
        let mut discovered = record.into_discovered()?;
        discovered.head_sha = pull_request_head_sha(
            &self.command,
            &discovered.owner,
            &discovered.repo,
            discovered.number,
        )?;
        Ok(discovered)
    }
}

#[derive(Deserialize)]
struct SearchPullRequest {
    repository: SearchRepository,
    number: u64,
}

impl SearchPullRequest {
    fn into_discovered(self) -> AgentResult<DiscoveredPullRequest> {
        let (owner, repo) = self
            .repository
            .name_with_owner
            .split_once('/')
            .ok_or_else(|| {
                AgentError::new(format!(
                    "invalid GitHub repository name `{}`",
                    self.repository.name_with_owner
                ))
            })?;
        Ok(DiscoveredPullRequest {
            owner: owner.into(),
            repo: repo.into(),
            number: self.number,
            head_sha: String::new(),
        })
    }
}

#[derive(Deserialize)]
struct SearchRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

fn pull_request_head_sha(
    command: &Path,
    owner: &str,
    repo: &str,
    number: u64,
) -> AgentResult<String> {
    let output = Command::new(command)
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &format!("{owner}/{repo}"),
            "--json",
            "headRefOid",
        ])
        .output()
        .map_err(|error| {
            AgentError::new(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(AgentError::new(format!(
            "GitHub CLI failed with status {}{}",
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        )));
    }
    let response: PullRequestHeadResponse = serde_json::from_slice(&output.stdout)
        .map_err(|error| AgentError::new(format!("invalid GitHub PR response: {error}")))?;
    Ok(response.head_ref_oid)
}

#[derive(Deserialize)]
struct PullRequestHeadResponse {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessedReview {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub head_sha: String,
    pub activity_id: Option<String>,
    pub reviewed_at_unix: u64,
}

impl ProcessedReview {
    pub fn from_pull_request(
        pull_request: &DiscoveredPullRequest,
        activity_id: Option<String>,
    ) -> Self {
        Self::from_pull_request_at(pull_request, activity_id, unix_now())
    }

    pub fn from_pull_request_at(
        pull_request: &DiscoveredPullRequest,
        activity_id: Option<String>,
        reviewed_at_unix: u64,
    ) -> Self {
        Self {
            owner: pull_request.owner.clone(),
            repo: pull_request.repo.clone(),
            number: pull_request.number,
            head_sha: pull_request.head_sha.clone(),
            activity_id,
            reviewed_at_unix,
        }
    }
}

pub trait ProcessedReviewStore: Send + Sync {
    fn get_processed(
        &self,
        pull_request: &DiscoveredPullRequest,
    ) -> AgentResult<Option<ProcessedReview>>;

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()>;

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>>;

    fn needs_review(&self, pull_request: &DiscoveredPullRequest) -> AgentResult<bool> {
        Ok(self
            .get_processed(pull_request)?
            .is_none_or(|processed| processed.head_sha != pull_request.head_sha))
    }

    fn mark_processed(
        &self,
        pull_request: &DiscoveredPullRequest,
        activity_id: Option<String>,
    ) -> AgentResult<()> {
        self.mark_processed_at(pull_request, activity_id, unix_now())
    }

    fn mark_processed_at(
        &self,
        pull_request: &DiscoveredPullRequest,
        activity_id: Option<String>,
        reviewed_at_unix: u64,
    ) -> AgentResult<()> {
        self.save_processed(&ProcessedReview::from_pull_request_at(
            pull_request,
            activity_id,
            reviewed_at_unix,
        ))
    }
}

#[derive(Default)]
pub struct MemoryProcessedReviewStore {
    reviews: Mutex<BTreeMap<String, ProcessedReview>>,
}

impl ProcessedReviewStore for MemoryProcessedReviewStore {
    fn get_processed(
        &self,
        pull_request: &DiscoveredPullRequest,
    ) -> AgentResult<Option<ProcessedReview>> {
        let reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        Ok(reviews.get(&processed_key(pull_request)).cloned())
    }

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()> {
        let mut reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        reviews.insert(processed_review_key(review), review.clone());
        Ok(())
    }

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>> {
        let reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        Ok(reviews.values().cloned().collect())
    }
}

pub struct FsProcessedReviewStore {
    base: PathBuf,
}

impl FsProcessedReviewStore {
    pub fn new(base: impl AsRef<Path>) -> AgentResult<Self> {
        let base = base.as_ref().to_path_buf();
        fs::create_dir_all(&base)
            .map_err(|error| AgentError::new(format!("create processed review dir: {error}")))?;
        Ok(Self { base })
    }
}

impl ProcessedReviewStore for FsProcessedReviewStore {
    fn get_processed(
        &self,
        pull_request: &DiscoveredPullRequest,
    ) -> AgentResult<Option<ProcessedReview>> {
        let path = self
            .base
            .join(format!("{}.json", processed_key(pull_request)));
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_processed_review(&path)?))
    }

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()> {
        write_processed_review(
            &self
                .base
                .join(format!("{}.json", processed_review_key(review))),
            review,
        )
    }

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>> {
        let mut paths = fs::read_dir(&self.base)
            .map_err(|error| AgentError::new(format!("read processed review dir: {error}")))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AgentError::new(format!("read processed review dir entry: {error}"))
            })?;
        paths.sort();

        let mut reviews = Vec::new();
        for path in paths {
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                reviews.push(read_processed_review(&path)?);
            }
        }
        Ok(reviews)
    }
}

fn processed_key(pull_request: &DiscoveredPullRequest) -> String {
    sanitize_key(&format!(
        "{}__{}__{}",
        pull_request.owner, pull_request.repo, pull_request.number
    ))
}

fn processed_review_key(review: &ProcessedReview) -> String {
    sanitize_key(&format!(
        "{}__{}__{}",
        review.owner, review.repo, review.number
    ))
}

fn sanitize_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn write_processed_review(path: &Path, review: &ProcessedReview) -> AgentResult<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(review)
        .map_err(|error| AgentError::new(format!("serialize processed review: {error}")))?;
    fs::write(&tmp, bytes)
        .map_err(|error| AgentError::new(format!("write processed review temp file: {error}")))?;
    fs::rename(&tmp, path)
        .map_err(|error| AgentError::new(format!("replace processed review: {error}")))
}

fn read_processed_review(path: &Path) -> AgentResult<ProcessedReview> {
    let bytes = fs::read(path)
        .map_err(|error| AgentError::new(format!("read processed review: {error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AgentError::new(format!("parse {}: {error}", path.display())))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

impl ArtifactSyncDestination for GitHubCliSyncDestination {
    fn name(&self) -> &'static str {
        "github"
    }

    fn sync(&self, artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome> {
        let mut child = Command::new(&self.command)
            .args([
                "pr",
                "comment",
                &self.target.number.to_string(),
                "--repo",
                &format!("{}/{}", self.target.owner, self.target.repo),
                "--body-file",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    self.command.display()
                ))
            })?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::new("GitHub CLI stdin unavailable"))?
            .write_all(github_comment_body(artifact).as_bytes())
            .map_err(|error| AgentError::new(format!("write GitHub comment body: {error}")))?;

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::new(format!("wait for GitHub CLI: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::new(format!(
                "GitHub CLI failed with status {}{}",
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }
        let remote_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();

        Ok(ArtifactSyncOutcome {
            sync_state: ArtifactSyncState::Synced {
                destination: self.name().into(),
                remote_id: (!remote_id.is_empty()).then_some(remote_id.clone()),
            },
            remote_id: (!remote_id.is_empty()).then_some(remote_id),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl FromStr for PullRequestRef {
    type Err = ParsePullRequestRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if let Some(path) = trimmed.strip_prefix("https://github.com/") {
            return parse_github_pull_path(path);
        }
        if let Some(path) = trimmed.strip_prefix("http://github.com/") {
            return parse_github_pull_path(path);
        }

        let (repo, number) = trimmed
            .rsplit_once('#')
            .ok_or_else(|| ParsePullRequestRefError::new(trimmed))?;
        let (owner, repo) = repo
            .split_once('/')
            .ok_or_else(|| ParsePullRequestRefError::new(trimmed))?;
        let number = number
            .parse::<u64>()
            .map_err(|_| ParsePullRequestRefError::new(trimmed))?;

        Ok(PullRequestRef {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsePullRequestRefError {
    value: String,
}

impl ParsePullRequestRefError {
    fn new(value: &str) -> Self {
        Self {
            value: value.to_owned(),
        }
    }
}

impl std::fmt::Display for ParsePullRequestRefError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid GitHub pull request reference `{}`",
            self.value
        )
    }
}

impl std::error::Error for ParsePullRequestRefError {}

fn parse_github_pull_path(path: &str) -> Result<PullRequestRef, ParsePullRequestRefError> {
    let mut segments = path.trim_matches('/').split('/');
    let owner = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(path))?;
    let repo = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(path))?;
    let kind = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(path))?;
    let number = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(path))?;

    if kind != "pull" {
        return Err(ParsePullRequestRefError::new(path));
    }

    Ok(PullRequestRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number: number
            .parse::<u64>()
            .map_err(|_| ParsePullRequestRefError::new(path))?,
    })
}

fn github_comment_body(artifact: &Artifact) -> String {
    match &artifact.content {
        ArtifactContent::ReviewSummary(summary) => {
            format!("<!-- nitpick-agent:{} -->\n\n{summary}\n", artifact.id)
        }
        ArtifactContent::ReviewComment(comment) => format!(
            "<!-- nitpick-agent:{} -->\n\n{}:{}\n\n{}\n",
            artifact.id, comment.path, comment.line, comment.body
        ),
        ArtifactContent::ChatResponse(response) => {
            format!("<!-- nitpick-agent:{} -->\n\n{response}\n", artifact.id)
        }
    }
}
