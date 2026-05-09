use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactContent, ArtifactSyncDestination,
    ArtifactSyncOutcome, ArtifactSyncState, ReviewInput, ReviewRequest, ReviewSource,
    ReviewSubject,
};
use serde::{Deserialize, Serialize};

pub use nitpick_agent_core::{FsProcessedReviewStore, MemoryProcessedReviewStore};

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

impl DiscoveredPullRequest {
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl From<DiscoveredPullRequest> for ReviewRequest {
    fn from(pull_request: DiscoveredPullRequest) -> Self {
        Self {
            source: "github".into(),
            repository: pull_request.repository(),
            number: Some(pull_request.number),
            id: pull_request.number.to_string(),
            head_sha: pull_request.head_sha,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDetails {
    pub title: String,
    pub author: String,
    pub url: String,
    pub head_sha: String,
    pub head_ref_name: String,
    pub state: PullRequestState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

impl PullRequestState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
}

pub struct GitHubCliDiscovery {
    command: PathBuf,
    git_command: PathBuf,
    checkout_root: PathBuf,
}

pub trait ReviewRequestDiscovery: Send + Sync {
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>>;

    fn review_input(&self, pull_request: &DiscoveredPullRequest) -> AgentResult<ReviewInput>;
}

impl GitHubCliDiscovery {
    pub fn new(command: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            git_command: PathBuf::from("git"),
            checkout_root: default_checkout_root(),
        }
    }

    pub fn with_checkout_commands(
        command: impl Into<PathBuf>,
        git_command: impl Into<PathBuf>,
        checkout_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            command: command.into(),
            git_command: git_command.into(),
            checkout_root: checkout_root.into(),
        }
    }

    pub fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        <Self as ReviewRequestDiscovery>::requested_reviews(self)
    }

    pub fn review_input(&self, pull_request: &DiscoveredPullRequest) -> AgentResult<ReviewInput> {
        <Self as ReviewRequestDiscovery>::review_input(self, pull_request)
    }

    pub fn pull_request_details(
        &self,
        pull_request: &DiscoveredPullRequest,
    ) -> AgentResult<PullRequestDetails> {
        pull_request_details(
            &self.command,
            &pull_request.owner,
            &pull_request.repo,
            pull_request.number,
        )
    }

    pub fn cleanup_checkout_for(
        &self,
        pull_request: &DiscoveredPullRequest,
        details: &PullRequestDetails,
    ) -> AgentResult<bool> {
        if details.state == PullRequestState::Open {
            return Ok(false);
        }

        let checkout_dir = checkout_path(&self.checkout_root, pull_request);
        if !checkout_dir.exists() {
            return Ok(false);
        }

        fs::remove_dir_all(&checkout_dir).map_err(|error| {
            AgentError::new(format!(
                "remove checkout {}: {error}",
                checkout_dir.display()
            ))
        })?;
        Ok(true)
    }

    pub fn list_checkouts(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        let mut checkouts = Vec::new();
        if !self.checkout_root.exists() {
            return Ok(checkouts);
        }

        for owner_entry in fs::read_dir(&self.checkout_root)
            .map_err(|error| AgentError::new(format!("read checkout root: {error}")))?
        {
            let owner_entry =
                owner_entry.map_err(|error| AgentError::new(format!("read owner dir: {error}")))?;
            if !owner_entry
                .file_type()
                .map_err(|error| AgentError::new(format!("read owner file type: {error}")))?
                .is_dir()
            {
                continue;
            }
            let owner = owner_entry.file_name().to_string_lossy().to_string();

            for repo_entry in fs::read_dir(owner_entry.path())
                .map_err(|error| AgentError::new(format!("read repo dir: {error}")))?
            {
                let repo_entry = repo_entry
                    .map_err(|error| AgentError::new(format!("read repo entry: {error}")))?;
                if !repo_entry
                    .file_type()
                    .map_err(|error| AgentError::new(format!("read repo file type: {error}")))?
                    .is_dir()
                {
                    continue;
                }
                let repo = repo_entry.file_name().to_string_lossy().to_string();

                for pr_entry in fs::read_dir(repo_entry.path())
                    .map_err(|error| AgentError::new(format!("read PR checkout dir: {error}")))?
                {
                    let pr_entry = pr_entry
                        .map_err(|error| AgentError::new(format!("read PR entry: {error}")))?;
                    if !pr_entry
                        .file_type()
                        .map_err(|error| {
                            AgentError::new(format!("read PR checkout file type: {error}"))
                        })?
                        .is_dir()
                    {
                        continue;
                    }
                    let name = pr_entry.file_name().to_string_lossy().to_string();
                    let Some(number) = name
                        .strip_prefix("pr-")
                        .and_then(|value| value.parse::<u64>().ok())
                    else {
                        continue;
                    };
                    if !pr_entry.path().join(".git").is_dir() {
                        continue;
                    }
                    checkouts.push(DiscoveredPullRequest {
                        owner: owner.clone(),
                        repo: repo.clone(),
                        number,
                        head_sha: String::new(),
                    });
                }
            }
        }

        checkouts.sort_by(|lhs, rhs| {
            lhs.owner
                .cmp(&rhs.owner)
                .then(lhs.repo.cmp(&rhs.repo))
                .then(lhs.number.cmp(&rhs.number))
        });
        Ok(checkouts)
    }
}

impl ReviewSource for GitHubCliDiscovery {
    fn name(&self) -> &'static str {
        "github"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        <Self as ReviewRequestDiscovery>::requested_reviews(self)
            .map(|requests| requests.into_iter().map(ReviewRequest::from).collect())
    }

    fn review_input(&self, request: &ReviewRequest) -> AgentResult<ReviewInput> {
        let Some(number) = request.number else {
            return Err(AgentError::new(format!(
                "GitHub review request `{}` is missing a pull request number",
                request.display_reference()
            )));
        };
        let (owner, repo) = request.repository.split_once('/').ok_or_else(|| {
            AgentError::new(format!(
                "invalid GitHub repository name `{}`",
                request.repository
            ))
        })?;
        let pull_request = DiscoveredPullRequest {
            owner: owner.into(),
            repo: repo.into(),
            number,
            head_sha: request.head_sha.clone(),
        };
        <Self as ReviewRequestDiscovery>::review_input(self, &pull_request)
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

    fn review_input(&self, pull_request: &DiscoveredPullRequest) -> AgentResult<ReviewInput> {
        let details = self.pull_request_details(pull_request)?;
        let diff = pull_request_diff(
            &self.command,
            &pull_request.owner,
            &pull_request.repo,
            pull_request.number,
        )?;
        let repo_dir = ensure_checkout(
            &self.command,
            &self.git_command,
            &self.checkout_root,
            pull_request,
            &details.head_ref_name,
        )?;
        let repository = format!("{}/{}", pull_request.owner, pull_request.repo);
        Ok(ReviewInput {
            repo_dir,
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
            diff,
        })
    }
}

fn default_checkout_root() -> PathBuf {
    if let Some(path) = env::var_os("NITPICK_AGENT_CHECKOUT_DIR") {
        return PathBuf::from(path);
    }

    if let Some(data_dir) = env::var_os("NITPICK_AGENT_DATA_DIR") {
        return PathBuf::from(data_dir).join("checkouts");
    }

    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home)
            .join("nitpick-agent")
            .join("checkouts");
    }

    PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into()))
        .join(".local")
        .join("share")
        .join("nitpick-agent")
        .join("checkouts")
}

fn ensure_checkout(
    command: &Path,
    git_command: &Path,
    checkout_root: &Path,
    pull_request: &DiscoveredPullRequest,
    head_ref: &str,
) -> AgentResult<PathBuf> {
    let repo_dir = checkout_path(checkout_root, pull_request);

    if !repo_dir.join(".git").is_dir() {
        let parent = repo_dir.parent().ok_or_else(|| {
            AgentError::new(format!(
                "checkout path has no parent: {}",
                repo_dir.display()
            ))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| AgentError::new(format!("create checkout parent: {error}")))?;
        let output = Command::new(command)
            .args([
                "repo",
                "clone",
                &pull_request.repository(),
                repo_dir.to_string_lossy().as_ref(),
                "--",
                "--quiet",
            ])
            .output()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    command.display()
                ))
            })?;
        if !output.status.success() {
            return Err(github_cli_status_error(&output));
        }
    }

    run_git(
        git_command,
        &[
            "-C",
            repo_dir.to_string_lossy().as_ref(),
            "fetch",
            "origin",
            head_ref,
            "--quiet",
        ],
    )?;
    run_git(
        git_command,
        &[
            "-C",
            repo_dir.to_string_lossy().as_ref(),
            "checkout",
            "-B",
            head_ref,
            &format!("origin/{head_ref}"),
            "--quiet",
        ],
    )?;

    Ok(repo_dir)
}

fn checkout_path(checkout_root: &Path, pull_request: &DiscoveredPullRequest) -> PathBuf {
    checkout_root
        .join(&pull_request.owner)
        .join(&pull_request.repo)
        .join(format!("pr-{}", pull_request.number))
}

fn run_git(command: &Path, args: &[&str]) -> AgentResult<()> {
    let output = Command::new(command).args(args).output().map_err(|error| {
        AgentError::new(format!(
            "failed to start git command `{}`: {error}",
            command.display()
        ))
    })?;
    if !output.status.success() {
        return Err(command_status_error("git", &output));
    }
    Ok(())
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

fn pull_request_details(
    command: &Path,
    owner: &str,
    repo: &str,
    number: u64,
) -> AgentResult<PullRequestDetails> {
    let output = Command::new(command)
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &format!("{owner}/{repo}"),
            "--json",
            "title,author,url,headRefOid,headRefName,state,mergedAt",
        ])
        .output()
        .map_err(|error| {
            AgentError::new(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    let response: PullRequestDetailsResponse = serde_json::from_slice(&output.stdout)
        .map_err(|error| AgentError::new(format!("invalid GitHub PR response: {error}")))?;
    Ok(response.into_details())
}

#[derive(Deserialize)]
struct PullRequestDetailsResponse {
    title: String,
    author: PullRequestAuthor,
    url: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    state: String,
    #[serde(rename = "mergedAt")]
    merged_at: Option<String>,
}

impl PullRequestDetailsResponse {
    fn into_details(self) -> PullRequestDetails {
        PullRequestDetails {
            title: self.title,
            author: self.author.login,
            url: self.url,
            head_sha: self.head_ref_oid,
            head_ref_name: self.head_ref_name,
            state: if self.merged_at.is_some() {
                PullRequestState::Merged
            } else if self.state.eq_ignore_ascii_case("closed") {
                PullRequestState::Closed
            } else {
                PullRequestState::Open
            },
        }
    }
}

#[derive(Deserialize)]
struct PullRequestAuthor {
    login: String,
}

fn pull_request_diff(command: &Path, owner: &str, repo: &str, number: u64) -> AgentResult<String> {
    let output = Command::new(command)
        .args([
            "pr",
            "diff",
            &number.to_string(),
            "--repo",
            &format!("{owner}/{repo}"),
        ])
        .output()
        .map_err(|error| {
            AgentError::new(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn github_cli_status_error(output: &std::process::Output) -> AgentError {
    command_status_error("GitHub CLI", output)
}

fn command_status_error(command: &str, output: &std::process::Output) -> AgentError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    AgentError::new(format!(
        "{command} failed with status {}{}",
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
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
