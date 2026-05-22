use std::{
    collections::HashSet,
    env,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    str::FromStr,
    time::Instant,
};

use fs_err as fs;
use nitpick_agent_core::{
    AgentError, AgentResult, Artifact, ArtifactContent, ArtifactSyncDestination,
    ArtifactSyncOutcome, ArtifactSyncState, ReviewComment, ReviewInput, ReviewRequest,
    ReviewSource, ReviewSubject, checkout_root_from_env_values, parse_json_bytes,
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
                remote_id: None,
                remote_url: None,
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

pub struct GitHubCliReviewSyncDestination {
    target: PullRequestRef,
    command: PathBuf,
}

impl GitHubCliReviewSyncDestination {
    pub fn new(target: PullRequestRef, command: impl Into<PathBuf>) -> Self {
        Self {
            target,
            command: command.into(),
        }
    }

    pub fn create_pending_review_batch(
        &self,
        artifacts: &[Artifact],
    ) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        sync_review_batch_with_github_cli(&self.command, &self.target, artifacts, self.name())
    }

    pub fn fetch_review(&self, review_id: &str) -> AgentResult<GitHubReviewResponse> {
        github_review_from_cli(
            &self.command,
            &[&format!(
                "repos/{}/{}/pulls/{}/reviews/{}",
                self.target.owner, self.target.repo, self.target.number, review_id
            )],
        )
    }

    pub fn update_pending_review_body(
        &self,
        review_id: &str,
        body: &str,
    ) -> AgentResult<GitHubReviewResponse> {
        github_review_from_cli_with_input(
            &self.command,
            &[
                &format!(
                    "repos/{}/{}/pulls/{}/reviews/{}",
                    self.target.owner, self.target.repo, self.target.number, review_id
                ),
                "--method",
                "PUT",
                "--input",
                "-",
            ],
            &serde_json::json!({ "body": body }).to_string(),
        )
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

impl From<&PullRequestRef> for DiscoveredPullRequest {
    fn from(pull_request: &PullRequestRef) -> Self {
        Self {
            owner: pull_request.owner.clone(),
            repo: pull_request.repo.clone(),
            number: pull_request.number,
            head_sha: String::new(),
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
    review_request_scopes: Vec<ReviewRequestScope>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ReviewRequestScope {
    Owner(String),
    Repo(String),
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
            review_request_scopes: Vec::new(),
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
            review_request_scopes: Vec::new(),
        }
    }

    pub fn with_allowlist(mut self, allowlist: &[String]) -> Self {
        self.review_request_scopes = review_request_scopes(allowlist);
        self
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

    pub fn checkout_path_for(&self, pull_request: &PullRequestRef) -> PathBuf {
        checkout_path(
            &self.checkout_root,
            &DiscoveredPullRequest::from(pull_request),
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

        fs::remove_dir_all(&checkout_dir)
            .map_err(|error| AgentError::io_path("remove checkout", &checkout_dir, error))?;
        Ok(true)
    }

    pub fn list_checkouts(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        let mut checkouts = Vec::new();
        if !self.checkout_root.exists() {
            return Ok(checkouts);
        }

        for owner_entry in fs::read_dir(&self.checkout_root).map_err(|error| {
            AgentError::io_path("read checkout root", &self.checkout_root, error)
        })? {
            let owner_entry =
                owner_entry.map_err(|error| AgentError::io("read owner dir", error))?;
            if !owner_entry
                .file_type()
                .map_err(|error| AgentError::io("read owner file type", error))?
                .is_dir()
            {
                continue;
            }
            let owner = owner_entry.file_name().to_string_lossy().to_string();

            for repo_entry in fs::read_dir(owner_entry.path())
                .map_err(|error| AgentError::io("read repo dir", error))?
            {
                let repo_entry =
                    repo_entry.map_err(|error| AgentError::io("read repo entry", error))?;
                if !repo_entry
                    .file_type()
                    .map_err(|error| AgentError::io("read repo file type", error))?
                    .is_dir()
                {
                    continue;
                }
                let repo = repo_entry.file_name().to_string_lossy().to_string();

                for pr_entry in fs::read_dir(repo_entry.path())
                    .map_err(|error| AgentError::io("read PR checkout dir", error))?
                {
                    let pr_entry =
                        pr_entry.map_err(|error| AgentError::io("read PR entry", error))?;
                    if !pr_entry
                        .file_type()
                        .map_err(|error| AgentError::io("read PR checkout file type", error))?
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
            return Err(AgentError::invalid_input(format!(
                "GitHub review request `{}` is missing a pull request number",
                request.display_reference()
            )));
        };
        let (owner, repo) = request.repository.split_once('/').ok_or_else(|| {
            AgentError::invalid_input(format!(
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

    fn already_reviewed(&self, request: &ReviewRequest) -> AgentResult<bool> {
        let Some(number) = request.number else {
            return Ok(false);
        };
        let (owner, repo) = request.repository.split_once('/').ok_or_else(|| {
            AgentError::invalid_input(format!(
                "invalid GitHub repository name `{}`",
                request.repository
            ))
        })?;
        pull_request_has_nitpick_review(&self.command, owner, repo, number, &request.head_sha)
    }
}

impl ReviewRequestDiscovery for GitHubCliDiscovery {
    #[tracing::instrument(skip_all, fields(source = "github"))]
    fn requested_reviews(&self) -> AgentResult<Vec<DiscoveredPullRequest>> {
        let records = if self.review_request_scopes.is_empty() {
            search_pull_requests(&self.command, None)?
        } else {
            let mut records = Vec::new();
            let mut seen = HashSet::new();
            for scope in &self.review_request_scopes {
                for record in search_pull_requests(&self.command, Some(scope))? {
                    let key = (record.repository.name_with_owner.clone(), record.number);
                    if seen.insert(key) {
                        records.push(record);
                    }
                }
            }
            records
        };
        records
            .into_iter()
            .map(|record| self.discover_with_head_sha(record))
            .collect()
    }

    #[tracing::instrument(skip_all, fields(repository = %pull_request.repository(), number = pull_request.number))]
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
            ..ReviewInput::default()
        })
    }
}

fn search_pull_requests(
    command: &Path,
    scope: Option<&ReviewRequestScope>,
) -> AgentResult<Vec<SearchPullRequest>> {
    let mut args = vec![
        "search".to_owned(),
        "prs".to_owned(),
        "user-review-requested:@me".to_owned(),
        "--state".to_owned(),
        "open".to_owned(),
    ];
    if let Some(scope) = scope {
        match scope {
            ReviewRequestScope::Owner(owner) => {
                args.push("--owner".to_owned());
                args.push(owner.clone());
            }
            ReviewRequestScope::Repo(repo) => {
                args.push("--repo".to_owned());
                args.push(repo.clone());
            }
        }
    }
    args.extend([
        "--limit".to_owned(),
        "100".to_owned(),
        "--json".to_owned(),
        "repository,number".to_owned(),
    ]);
    tracing::debug!(command = %command.display(), args = ?args, "running GitHub CLI");
    let started = Instant::now();
    let output = Command::new(command)
        .args(&args)
        .output()
        .map_err(|error| {
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub CLI finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }

    parse_github_json(&output.stdout, "GitHub review request response")
}

fn review_request_scopes(allowlist: &[String]) -> Vec<ReviewRequestScope> {
    let mut scopes = Vec::new();
    let mut seen = HashSet::new();
    for pattern in allowlist {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        let Some((owner, repo)) = pattern.split_once('/') else {
            continue;
        };
        let scope = if repo == "*" && !owner.contains('*') {
            Some(ReviewRequestScope::Owner(owner.to_owned()))
        } else if !owner.contains('*') && !repo.contains('*') {
            Some(ReviewRequestScope::Repo(format!("{owner}/{repo}")))
        } else {
            None
        };
        if let Some(scope) = scope
            && seen.insert(scope.clone())
        {
            scopes.push(scope);
        }
    }
    scopes
}

fn default_checkout_root() -> PathBuf {
    checkout_root_from_env_values(
        env::var_os("NITPICK_AGENT_CHECKOUT_DIR"),
        env::var_os("NITPICK_AGENT_DATA_DIR"),
    )
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
            AgentError::invalid_input(format!(
                "checkout path has no parent: {}",
                repo_dir.display()
            ))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| AgentError::io_path("create checkout parent", parent, error))?;
        tracing::debug!(
            command = %command.display(),
            repository = %pull_request.repository(),
            "cloning GitHub checkout"
        );
        let started = Instant::now();
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
                AgentError::github_cli(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    command.display()
                ))
            })?;
        tracing::debug!(
            command = %command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "GitHub checkout clone finished"
        );
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
    tracing::debug!(command = %command.display(), args = ?args, "running git command");
    let started = Instant::now();
    let output = Command::new(command).args(args).output().map_err(|error| {
        AgentError::provider(format!(
            "failed to start git command `{}`: {error}",
            command.display()
        ))
    })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "git command finished"
    );
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
    fn repository_parts(&self) -> AgentResult<(&str, &str)> {
        self.repository
            .name_with_owner
            .split_once('/')
            .ok_or_else(|| {
                AgentError::invalid_input(format!(
                    "invalid GitHub repository name `{}`",
                    self.repository.name_with_owner
                ))
            })
    }

    fn into_discovered(self) -> AgentResult<DiscoveredPullRequest> {
        let (owner, repo) = self.repository_parts()?;
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
    tracing::debug!(
        command = %command.display(),
        repository = %format!("{owner}/{repo}"),
        number,
        "reading GitHub PR head SHA"
    );
    let started = Instant::now();
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
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub PR head SHA command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    let response: PullRequestHeadResponse =
        parse_github_json(&output.stdout, "GitHub PR response")?;
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
    tracing::debug!(
        command = %command.display(),
        repository = %format!("{owner}/{repo}"),
        number,
        "reading GitHub PR details"
    );
    let started = Instant::now();
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
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub PR details command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    let response: PullRequestDetailsResponse =
        parse_github_json(&output.stdout, "GitHub PR response")?;
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
    tracing::debug!(
        command = %command.display(),
        repository = %format!("{owner}/{repo}"),
        number,
        "reading GitHub PR diff"
    );
    let started = Instant::now();
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
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub PR diff command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn pull_request_has_nitpick_review(
    command: &Path,
    owner: &str,
    repo: &str,
    number: u64,
    head_sha: &str,
) -> AgentResult<bool> {
    tracing::debug!(
        command = %command.display(),
        repository = %format!("{owner}/{repo}"),
        number,
        "listing GitHub PR reviews"
    );
    let started = Instant::now();
    let output = Command::new(command)
        .args([
            "api",
            &format!("repos/{owner}/{repo}/pulls/{number}/reviews"),
        ])
        .output()
        .map_err(|error| {
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub PR reviews command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    let reviews: Vec<PullRequestReviewResponse> =
        parse_github_json(&output.stdout, "GitHub PR reviews response")?;
    Ok(reviews.into_iter().any(|review| {
        review.commit_id == head_sha && review.body.is_some_and(|body| has_nitpick_marker(&body))
    }))
}

#[derive(Deserialize)]
struct PullRequestReviewResponse {
    commit_id: String,
    body: Option<String>,
}

fn has_nitpick_marker(body: &str) -> bool {
    body.contains("<!-- nitpick-agent:") || body.contains("<!-- nitpick:")
}

fn github_cli_status_error(output: &std::process::Output) -> AgentError {
    command_status_error("GitHub CLI", output)
}

fn command_status_error(command: &str, output: &std::process::Output) -> AgentError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let message = format!(
        "{command} failed with status {}{}",
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    );
    if command == "GitHub CLI" {
        if is_github_rate_limit_error(&stderr) {
            let retry_after_seconds = parse_retry_after_seconds(&stderr);
            let retry_hint = retry_after_seconds
                .map(|seconds| format!(" retry after {seconds} seconds."))
                .unwrap_or_else(|| " retry after the rate limit resets.".to_owned());
            return AgentError::github_rate_limited(
                format!("GitHub rate limited the request;{retry_hint} {message}"),
                retry_after_seconds,
            );
        }
        AgentError::github_cli(message)
    } else {
        AgentError::provider(message)
    }
}

fn is_github_rate_limit_error(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("http 429")
        || stderr.contains("status 429")
        || stderr.contains(" 429")
        || stderr.contains("api rate limit exceeded")
        || stderr.contains("secondary rate limit")
}

fn parse_retry_after_seconds(stderr: &str) -> Option<u64> {
    let lower = stderr.to_ascii_lowercase();
    for marker in ["retry-after:", "retry after"] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let rest = &lower[start + marker.len()..];
        if let Some(seconds) = first_number(rest) {
            return Some(seconds);
        }
    }
    None
}

fn first_number(value: &str) -> Option<u64> {
    let start = value.find(|character: char| character.is_ascii_digit())?;
    let digits = value[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn parse_github_json<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    context: &str,
) -> AgentResult<T> {
    parse_json_bytes(bytes, &format!("invalid {context}"))
}

impl ArtifactSyncDestination for GitHubCliSyncDestination {
    fn name(&self) -> &'static str {
        "github"
    }

    fn sync(&self, artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome> {
        tracing::debug!(
            command = %self.command.display(),
            repository = %format!("{}/{}", self.target.owner, self.target.repo),
            number = self.target.number,
            "posting GitHub PR comment"
        );
        let started = Instant::now();
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
                AgentError::github_cli(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    self.command.display()
                ))
            })?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::github_cli("GitHub CLI stdin unavailable"))?
            .write_all(github_comment_body(artifact).as_bytes())
            .map_err(|error| {
                AgentError::github_cli(format!("write GitHub comment body: {error}"))
            })?;

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::github_cli(format!("wait for GitHub CLI: {error}")))?;
        tracing::debug!(
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "GitHub PR comment command finished"
        );
        if !output.status.success() {
            return Err(github_cli_status_error(&output));
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

impl ArtifactSyncDestination for GitHubCliReviewSyncDestination {
    fn name(&self) -> &'static str {
        "github-review"
    }

    fn sync(&self, artifact: &Artifact) -> AgentResult<ArtifactSyncOutcome> {
        match &artifact.content {
            ArtifactContent::ReviewSummary(_) => sync_with_github_cli(
                &self.command,
                &[
                    "pr",
                    "review",
                    &self.target.number.to_string(),
                    "--repo",
                    &format!("{}/{}", self.target.owner, self.target.repo),
                    "--comment",
                    "--body-file",
                    "-",
                ],
                &github_comment_body(artifact),
                self.name(),
            ),
            ArtifactContent::ReviewComment(comment) => {
                let head_sha = pull_request_head_sha(
                    &self.command,
                    &self.target.owner,
                    &self.target.repo,
                    self.target.number,
                )?;
                let payload = serde_json::json!({
                    "commit_id": head_sha,
                    "event": "COMMENT",
                    "comments": [{
                        "path": comment.path,
                        "line": comment.line,
                        "side": "RIGHT",
                        "body": robot_prefixed_body(&comment.body),
                    }],
                });
                sync_with_github_cli(
                    &self.command,
                    &[
                        "api",
                        &format!(
                            "repos/{}/{}/pulls/{}/reviews",
                            self.target.owner, self.target.repo, self.target.number
                        ),
                        "--method",
                        "POST",
                        "--input",
                        "-",
                    ],
                    &payload.to_string(),
                    self.name(),
                )
            }
            ArtifactContent::ChatResponse(_) => Err(AgentError::invalid_input(
                "github-review sync only supports review artifacts",
            )),
        }
    }

    fn sync_batch(&self, artifacts: &[Artifact]) -> AgentResult<Vec<ArtifactSyncOutcome>> {
        sync_review_batch_with_github_cli(&self.command, &self.target, artifacts, self.name())
    }
}

fn sync_review_batch_with_github_cli(
    command: &Path,
    target: &PullRequestRef,
    artifacts: &[Artifact],
    destination: &str,
) -> AgentResult<Vec<ArtifactSyncOutcome>> {
    let mut body = None;
    let mut comments = Vec::new();
    for artifact in artifacts {
        match &artifact.content {
            ArtifactContent::ReviewSummary(summary) => {
                body = Some(summary.clone());
            }
            ArtifactContent::ReviewComment(comment) => {
                comments.push(comment.clone());
            }
            ArtifactContent::ChatResponse(_) => {
                return Err(AgentError::invalid_input(
                    "github-review sync only supports review artifacts",
                ));
            }
        }
    }
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }
    if body.is_none() && comments.is_empty() {
        return Err(AgentError::invalid_input(
            "github-review sync requires at least one review summary or comment",
        ));
    }

    let head_sha = pull_request_head_sha(command, &target.owner, &target.repo, target.number)?;
    let payload_comments = comments
        .into_iter()
        .map(review_comment_payload)
        .collect::<Vec<_>>();
    let mut payload = serde_json::json!({
        "commit_id": head_sha,
        "comments": payload_comments,
    });
    if let Some(body) = body {
        payload["body"] = serde_json::Value::String(body);
    }
    let outcome = sync_pending_review_with_github_cli(
        command,
        &[
            "api",
            &format!(
                "repos/{}/{}/pulls/{}/reviews",
                target.owner, target.repo, target.number
            ),
            "--method",
            "POST",
            "--input",
            "-",
        ],
        &payload.to_string(),
        destination,
    )?;
    Ok(artifacts.iter().map(|_| outcome.clone()).collect())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct GitHubReviewResponse {
    pub id: u64,
    pub html_url: Option<String>,
    pub state: String,
    pub commit_id: Option<String>,
}

fn review_comment_payload(comment: ReviewComment) -> serde_json::Value {
    serde_json::json!({
        "path": comment.path,
        "line": comment.line,
        "side": "RIGHT",
        "body": robot_prefixed_body(&comment.body),
    })
}

fn sync_with_github_cli(
    command: &Path,
    args: &[&str],
    body: &str,
    destination: &str,
) -> AgentResult<ArtifactSyncOutcome> {
    tracing::debug!(
        command = %command.display(),
        args = ?args,
        destination,
        "syncing artifact with GitHub CLI"
    );
    let started = Instant::now();
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| AgentError::github_cli("GitHub CLI stdin unavailable"))?
        .write_all(body.as_bytes())
        .map_err(|error| AgentError::github_cli(format!("write GitHub body: {error}")))?;
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|error| AgentError::github_cli(format!("wait for GitHub CLI: {error}")))?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub artifact sync command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    let remote_id = github_remote_id_from_stdout(&output.stdout);

    Ok(ArtifactSyncOutcome {
        sync_state: ArtifactSyncState::Synced {
            destination: destination.into(),
            remote_id: (!remote_id.is_empty()).then_some(remote_id.clone()),
        },
        remote_id: (!remote_id.is_empty()).then_some(remote_id),
    })
}

fn sync_pending_review_with_github_cli(
    command: &Path,
    args: &[&str],
    body: &str,
    destination: &str,
) -> AgentResult<ArtifactSyncOutcome> {
    let output = run_github_cli_with_input(command, args, body)?;
    let review: GitHubReviewResponse = parse_github_json(&output.stdout, "GitHub review response")?;
    Ok(ArtifactSyncOutcome {
        sync_state: ArtifactSyncState::Pending {
            destination: destination.into(),
            remote_id: Some(review.id.to_string()),
            remote_url: review.html_url.clone(),
        },
        remote_id: review.html_url,
    })
}

fn github_review_from_cli(
    command: &Path,
    endpoint_args: &[&str],
) -> AgentResult<GitHubReviewResponse> {
    let mut args = vec!["api"];
    args.extend_from_slice(endpoint_args);
    tracing::debug!(command = %command.display(), args = ?args, "running GitHub review API command");
    let started = Instant::now();
    let output = Command::new(command)
        .args(&args)
        .output()
        .map_err(|error| {
            AgentError::github_cli(format!("run GitHub CLI `{}`: {error}", command.display()))
        })?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub review API command finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    parse_github_json(&output.stdout, "GitHub review response")
}

fn github_review_from_cli_with_input(
    command: &Path,
    endpoint_args: &[&str],
    body: &str,
) -> AgentResult<GitHubReviewResponse> {
    let mut args = vec!["api"];
    args.extend_from_slice(endpoint_args);
    let output = run_github_cli_with_input(command, &args, body)?;
    parse_github_json(&output.stdout, "GitHub review response")
}

fn run_github_cli_with_input(command: &Path, args: &[&str], body: &str) -> AgentResult<Output> {
    tracing::debug!(
        command = %command.display(),
        args = ?args,
        body_bytes = body.len(),
        "running GitHub CLI with stdin"
    );
    let started = Instant::now();
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            AgentError::github_cli(format!(
                "failed to start GitHub CLI `{}`: {error}",
                command.display()
            ))
        })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| AgentError::github_cli("GitHub CLI stdin unavailable"))?
        .write_all(body.as_bytes())
        .map_err(|error| AgentError::github_cli(format!("write GitHub body: {error}")))?;
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|error| AgentError::github_cli(format!("wait for GitHub CLI: {error}")))?;
    tracing::debug!(
        command = %command.display(),
        status = %output.status,
        duration_ms = started.elapsed().as_millis(),
        "GitHub CLI with stdin finished"
    );
    if !output.status.success() {
        return Err(github_cli_status_error(&output));
    }
    Ok(output)
}

fn github_remote_id_from_stdout(stdout: &[u8]) -> String {
    let output = String::from_utf8_lossy(stdout).trim().to_owned();
    if output.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&output)
        && let Some(html_url) = value.get("html_url").and_then(|value| value.as_str())
    {
        return html_url.to_owned();
    }
    output
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
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return parse_github_pull_url(trimmed);
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

fn parse_github_pull_url(value: &str) -> Result<PullRequestRef, ParsePullRequestRefError> {
    let url = url::Url::parse(value).map_err(|_| ParsePullRequestRefError::new(value))?;
    if url.domain() != Some("github.com") {
        return Err(ParsePullRequestRefError::new(value));
    }

    let mut segments = url
        .path_segments()
        .ok_or_else(|| ParsePullRequestRefError::new(value))?;
    let owner = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(value))?;
    let repo = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(value))?;
    let kind = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(value))?;
    let number = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ParsePullRequestRefError::new(value))?;

    if kind != "pull" {
        return Err(ParsePullRequestRefError::new(value));
    }

    Ok(PullRequestRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number: number
            .parse::<u64>()
            .map_err(|_| ParsePullRequestRefError::new(value))?,
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

fn robot_prefixed_body(body: &str) -> String {
    if body.starts_with("🤖 ") {
        body.to_owned()
    } else {
        format!("🤖 {body}")
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;

    use super::{command_status_error, is_github_rate_limit_error, parse_retry_after_seconds};

    #[test]
    fn detects_github_rate_limit_errors() {
        assert!(is_github_rate_limit_error("HTTP 429: secondary rate limit"));
        assert!(is_github_rate_limit_error("API rate limit exceeded"));
        assert!(!is_github_rate_limit_error("HTTP 404: Not Found"));
    }

    #[test]
    fn parses_retry_after_seconds() {
        assert_eq!(parse_retry_after_seconds("Retry-After: 60"), Some(60));
        assert_eq!(
            parse_retry_after_seconds("retry after 120 seconds"),
            Some(120)
        );
        assert_eq!(parse_retry_after_seconds("HTTP 429"), None);
    }

    #[cfg(unix)]
    #[test]
    fn github_cli_status_error_uses_rate_limit_variant_for_429s() {
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"HTTP 429: API rate limit exceeded\nRetry-After: 60".to_vec(),
        };

        let error = command_status_error("GitHub CLI", &output);

        assert!(matches!(
            error,
            nitpick_agent_core::AgentError::GitHubRateLimited {
                retry_after_seconds: Some(60),
                ..
            }
        ));
    }
}
