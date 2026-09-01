use std::{
    fs::OpenOptions,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nitpick_agent_client::HostClient;
use nitpick_agent_core::{AgentError, AgentResult, ReviewCommentValidator, ReviewToolConfig};
pub use nitpick_agent_model::{
    ExistingReviewComment, PullRequestContext, PullRequestConversationComment,
};
use nitpick_agent_model::{
    FinishReviewCommentBatchInput, FinishReviewCommentBatchResult, ReviewChatSessionInput,
    ReviewChatSessionSnapshot, ReviewComment, ReviewCommentBatch, ReviewInput,
};
use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::review_mcp_session_store::ReviewMcpSessionStore;

static NEXT_CONFIG_ID: AtomicU64 = AtomicU64::new(1);
const LOCK_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct ActiveReviewSession {
    state: Arc<Mutex<ActiveReviewSessionState>>,
}

#[derive(Debug)]
struct ActiveReviewSessionState {
    validator: ReviewCommentValidator,
    comments: Vec<ReviewComment>,
    pull_request_context: PullRequestContext,
    existing_comments: Vec<ExistingReviewComment>,
    deleted_comment_ids: Vec<String>,
    finished: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct FinishReviewResult {
    pub status: String,
    pub comment_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct AddReviewCommentInput {
    pub path: String,
    pub line: u32,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AddReviewCommentResult {
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ExistingReviewCommentsResult {
    pub comments: Vec<ExistingReviewComment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PullRequestContextResult {
    pub context: PullRequestContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PullRequestConversationCommentsResult {
    pub comments: Vec<PullRequestConversationComment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct DeleteDraftCommentInput {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DeleteDraftCommentResult {
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMcpGitHubTarget {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMcpSessionState {
    pub repo_dir: PathBuf,
    pub diff: String,
    pub comments: Vec<ReviewComment>,
    #[serde(default)]
    pub pull_request_context: PullRequestContext,
    #[serde(default)]
    pub existing_comments: Vec<ExistingReviewComment>,
    #[serde(default)]
    pub deleted_comment_ids: Vec<String>,
    #[serde(default)]
    pub github: Option<ReviewMcpGitHubTarget>,
    #[serde(default)]
    pub chat: Option<ReviewMcpChatState>,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMcpChatState {
    pub host_addr: String,
    pub activity_id: String,
    pub repository: String,
    pub number: u64,
    pub pinned_head_sha: String,
    pub batch: ReviewCommentBatch,
    pub last_result: Option<FinishReviewCommentBatchResult>,
}

#[derive(Debug)]
pub struct ReviewMcpServerHandle {
    config_path: PathBuf,
    state_path: PathBuf,
    temp_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ReviewMcpTools {
    session: ReviewMcpSessionStore,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ReviewMcpTools {}

#[tool_router(router = tool_router)]
impl ReviewMcpTools {
    pub fn new(session: ActiveReviewSession) -> Self {
        Self {
            session: ReviewMcpSessionStore::active(session),
            tool_router: Self::tool_router(),
        }
    }

    pub fn from_state_path(state_path: impl Into<PathBuf>) -> Self {
        Self {
            session: ReviewMcpSessionStore::file(state_path),
            tool_router: Self::tool_router(),
        }
    }

    pub fn add_review_comment(
        &self,
        input: AddReviewCommentInput,
    ) -> AgentResult<AddReviewCommentResult> {
        self.session.add_review_comment(input)
    }

    pub fn finish_review(&self) -> AgentResult<FinishReviewResult> {
        self.session.finish_review()
    }

    pub fn existing_review_comments(&self) -> AgentResult<ExistingReviewCommentsResult> {
        self.session.existing_review_comments()
    }

    pub fn pull_request_context(&self) -> AgentResult<PullRequestContextResult> {
        self.session.pull_request_context()
    }

    pub fn pull_request_conversation_comments(
        &self,
    ) -> AgentResult<PullRequestConversationCommentsResult> {
        self.session.pull_request_conversation_comments()
    }

    pub fn delete_draft_comment(
        &self,
        input: DeleteDraftCommentInput,
    ) -> AgentResult<DeleteDraftCommentResult> {
        self.session.delete_draft_comment(input)
    }

    #[tool(
        name = "add_review_comment",
        description = "Add a review comment to the active review session"
    )]
    async fn add_review_comment_tool(
        &self,
        Parameters(input): Parameters<AddReviewCommentInput>,
    ) -> Result<Json<AddReviewCommentResult>, String> {
        self.add_review_comment(input)
            .map(Json)
            .map_err(|error| error.to_string())
    }

    #[tool(
        name = "finish_review",
        description = "Commit the current review batch. Automated reviews finish once; review chat starts a new batch after each successful commit."
    )]
    async fn finish_review_tool(&self) -> Result<Json<FinishReviewResult>, String> {
        self.finish_review()
            .map(Json)
            .map_err(|error| error.to_string())
    }

    #[tool(
        name = "existing_review_comments",
        description = "List existing pull request review comments, including draft comments visible to Nitpick"
    )]
    async fn existing_review_comments_tool(
        &self,
    ) -> Result<Json<ExistingReviewCommentsResult>, String> {
        self.existing_review_comments()
            .map(Json)
            .map_err(|error| error.to_string())
    }

    #[tool(
        name = "pull_request_context",
        description = "Read pull request details, including title, author, URL, body, head commit, branch name, state, and conversation comments"
    )]
    async fn pull_request_context_tool(&self) -> Result<Json<PullRequestContextResult>, String> {
        self.pull_request_context()
            .map(Json)
            .map_err(|error| error.to_string())
    }

    #[tool(
        name = "pull_request_conversation_comments",
        description = "List general pull request conversation comments from the pull request timeline, excluding inline review comments"
    )]
    async fn pull_request_conversation_comments_tool(
        &self,
    ) -> Result<Json<PullRequestConversationCommentsResult>, String> {
        self.pull_request_conversation_comments()
            .map(Json)
            .map_err(|error| error.to_string())
    }

    #[tool(
        name = "delete_draft_comment",
        description = "Delete an outdated Nitpick draft review comment by id. Only draft comments whose body starts with the robot emoji can be deleted."
    )]
    async fn delete_draft_comment_tool(
        &self,
        Parameters(input): Parameters<DeleteDraftCommentInput>,
    ) -> Result<Json<DeleteDraftCommentResult>, String> {
        self.delete_draft_comment(input)
            .map(Json)
            .map_err(|error| error.to_string())
    }
}

impl ActiveReviewSession {
    pub fn new(repo_dir: impl AsRef<Path>, diff: &str) -> AgentResult<Self> {
        let validator = ReviewCommentValidator::for_diff(repo_dir, diff)?;
        Ok(Self {
            state: Arc::new(Mutex::new(ActiveReviewSessionState {
                validator,
                comments: Vec::new(),
                pull_request_context: PullRequestContext::default(),
                existing_comments: Vec::new(),
                deleted_comment_ids: Vec::new(),
                finished: false,
            })),
        })
    }

    pub fn add_review_comment(
        &self,
        path: &str,
        line: u32,
        body: impl Into<String>,
    ) -> AgentResult<ReviewComment> {
        let mut state = self.lock_state()?;
        if state.finished {
            return Err(AgentError::invalid_input(
                "review session is already finished",
            ));
        }

        let comment = state.validator.validate_comment(path, line, body)?;
        state.comments.push(comment.clone());
        Ok(comment)
    }

    pub fn finish_review(&self) -> AgentResult<FinishReviewResult> {
        let mut state = self.lock_state()?;
        state.finished = true;
        Ok(FinishReviewResult {
            status: "completed".to_owned(),
            comment_count: state.comments.len(),
        })
    }

    pub fn is_finished(&self) -> AgentResult<bool> {
        Ok(self.lock_state()?.finished)
    }

    pub fn comments(&self) -> AgentResult<Vec<ReviewComment>> {
        Ok(self.lock_state()?.comments.clone())
    }

    pub fn existing_review_comments(&self) -> AgentResult<ExistingReviewCommentsResult> {
        Ok(ExistingReviewCommentsResult {
            comments: self.lock_state()?.existing_comments.clone(),
        })
    }

    pub fn pull_request_context(&self) -> AgentResult<PullRequestContextResult> {
        Ok(PullRequestContextResult {
            context: self.lock_state()?.pull_request_context.clone(),
        })
    }

    pub fn pull_request_conversation_comments(
        &self,
    ) -> AgentResult<PullRequestConversationCommentsResult> {
        Ok(PullRequestConversationCommentsResult {
            comments: self
                .lock_state()?
                .pull_request_context
                .conversation_comments
                .clone(),
        })
    }

    pub fn delete_draft_comment(&self, id: &str) -> AgentResult<DeleteDraftCommentResult> {
        let mut state = self.lock_state()?;
        validate_deletable_comment(&state.existing_comments, id)?;
        if !state
            .deleted_comment_ids
            .iter()
            .any(|deleted_id| deleted_id == id)
        {
            state.deleted_comment_ids.push(id.to_owned());
        }
        Ok(DeleteDraftCommentResult { deleted: true })
    }

    fn lock_state(&self) -> AgentResult<MutexGuard<'_, ActiveReviewSessionState>> {
        self.state
            .lock()
            .map_err(|_| AgentError::new("active review session lock poisoned"))
    }
}

impl ReviewMcpServerHandle {
    pub fn start(
        input: &ReviewInput,
        existing_comments: Vec<ExistingReviewComment>,
        pull_request_context: PullRequestContext,
        github: Option<ReviewMcpGitHubTarget>,
    ) -> AgentResult<Self> {
        let temp_dir = new_temp_config_dir()?;
        let state_path = temp_dir.join("session.json");
        write_review_mcp_session_state(
            &state_path,
            &ReviewMcpSessionState {
                repo_dir: input.repo_dir.clone(),
                diff: input.diff.clone(),
                comments: Vec::new(),
                pull_request_context,
                existing_comments,
                deleted_comment_ids: Vec::new(),
                github,
                chat: None,
                finished: false,
            },
        )?;
        let config_path = temp_dir.join("mcp.json");
        fs_err::write(
            &config_path,
            review_mcp_config_json(&state_path, Path::new(&review_mcp_server_command())),
        )
        .map_err(|error| AgentError::io_path("write review MCP config", &config_path, error))?;
        Ok(Self {
            config_path,
            state_path,
            temp_dir,
        })
    }

    pub fn start_chat(
        host_addr: impl Into<String>,
        snapshot: ReviewChatSessionSnapshot,
    ) -> AgentResult<Self> {
        let temp_dir = new_temp_config_dir()?;
        let state_path = temp_dir.join("session.json");
        write_review_mcp_session_state(
            &state_path,
            &ReviewMcpSessionState {
                repo_dir: snapshot.repo_dir,
                diff: snapshot.diff,
                comments: Vec::new(),
                pull_request_context: snapshot.pull_request_context,
                existing_comments: snapshot.existing_comments,
                deleted_comment_ids: Vec::new(),
                github: None,
                chat: Some(ReviewMcpChatState {
                    host_addr: host_addr.into(),
                    activity_id: snapshot.activity_id.to_string(),
                    repository: snapshot.repository,
                    number: snapshot.number,
                    pinned_head_sha: snapshot.head_sha,
                    batch: ReviewCommentBatch {
                        id: uuid::Uuid::new_v4().to_string(),
                        additions: Vec::new(),
                        deletion_ids: Vec::new(),
                    },
                    last_result: None,
                }),
                finished: false,
            },
        )?;
        let config_path = temp_dir.join("mcp.json");
        fs_err::write(
            &config_path,
            review_mcp_config_json(&state_path, &snapshot.mcp_server_command),
        )
        .map_err(|error| AgentError::io_path("write review MCP config", &config_path, error))?;
        Ok(Self {
            config_path,
            state_path,
            temp_dir,
        })
    }

    pub fn tool_config(&self) -> ReviewToolConfig {
        ReviewToolConfig {
            mcp_config_path: self.config_path.clone(),
            instructions: review_tool_instructions(),
        }
    }

    pub fn session_state(&self) -> AgentResult<ReviewMcpSessionState> {
        load_review_mcp_session_state(&self.state_path)
    }

    pub fn uncommitted_counts(&self) -> AgentResult<(usize, usize)> {
        let state = self.session_state()?;
        Ok(state
            .chat
            .map(|chat| (chat.batch.additions.len(), chat.batch.deletion_ids.len()))
            .unwrap_or((state.comments.len(), state.deleted_comment_ids.len())))
    }
}

impl Drop for ReviewMcpServerHandle {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.config_path);
        let _ = fs_err::remove_file(&self.state_path);
        let _ = fs_err::remove_file(lock_path(&self.state_path));
        let _ = fs_err::remove_dir(&self.temp_dir);
    }
}

pub fn load_review_mcp_session_state(path: impl AsRef<Path>) -> AgentResult<ReviewMcpSessionState> {
    let path = path.as_ref();
    let bytes = fs_err::read(path)
        .map_err(|error| AgentError::io_path("read review MCP session state", path, error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AgentError::json("parse review MCP session state", path.display(), error))
}

pub async fn serve_review_mcp_stdio(state_path: PathBuf) -> AgentResult<()> {
    let service = ReviewMcpTools::from_state_path(state_path)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| AgentError::provider(format!("start review MCP stdio server: {error}")))?;
    service
        .waiting()
        .await
        .map_err(|error| AgentError::provider(format!("run review MCP stdio server: {error}")))?;
    Ok(())
}

pub(crate) fn add_review_comment_to_state_file(
    state_path: &Path,
    path: &str,
    line: u32,
    body: String,
) -> AgentResult<ReviewComment> {
    update_review_mcp_session_state(state_path, |state| {
        if state.finished {
            return Err(AgentError::invalid_input(
                "review session is already finished",
            ));
        }
        let validator = ReviewCommentValidator::for_diff(&state.repo_dir, &state.diff)?;
        let comment = validator.validate_comment(path, line, body)?;
        if let Some(chat) = &mut state.chat {
            chat.batch.additions.push(comment.clone());
        } else {
            state.comments.push(comment.clone());
        }
        Ok(comment)
    })
}

pub(crate) fn finish_review_in_state_file(state_path: &Path) -> AgentResult<FinishReviewResult> {
    update_review_mcp_session_state(state_path, |state| {
        let Some(chat) = state.chat.as_ref() else {
            state.finished = true;
            return Ok(FinishReviewResult {
                status: "completed".to_owned(),
                comment_count: state.comments.len(),
            });
        };
        if chat.batch.is_empty() {
            return Ok(FinishReviewResult {
                status: "completed".to_owned(),
                comment_count: 0,
            });
        }

        let host_addr = chat.host_addr.clone();
        let activity_id = chat.activity_id.clone();
        let pinned_head_sha = chat.pinned_head_sha.clone();
        let batch = chat.batch.clone();
        let result = HostClient::new(host_addr)
            .finish_review_comment_batch(
                &activity_id,
                &FinishReviewCommentBatchInput {
                    pinned_head_sha,
                    batch: batch.clone(),
                },
            )
            .map_err(|error| {
                AgentError::provider(format!("finish review comment batch: {error}"))
            })?;
        if result.batch_id != batch.id
            || result.committed_addition_count != batch.additions.len()
            || result.committed_deletion_count != batch.deletion_ids.len()
        {
            return Err(AgentError::provider(format!(
                "finish review comment batch returned an incomplete receipt for {}",
                batch.id
            )));
        }
        state.existing_comments = result.existing_comments.clone();
        let comment_count = result.committed_addition_count;
        let chat = state.chat.as_mut().expect("review chat state is present");
        chat.last_result = Some(result);
        chat.batch = ReviewCommentBatch {
            id: uuid::Uuid::new_v4().to_string(),
            additions: Vec::new(),
            deletion_ids: Vec::new(),
        };
        Ok(FinishReviewResult {
            status: "completed".to_owned(),
            comment_count,
        })
    })
}

pub(crate) fn existing_review_comments_in_state_file(
    state_path: &Path,
) -> AgentResult<ExistingReviewCommentsResult> {
    update_review_mcp_session_state(state_path, |state| {
        if let Some(chat) = state.chat.clone() {
            let snapshot = HostClient::new(chat.host_addr)
                .prepare_review_chat_session(
                    &chat.activity_id,
                    &ReviewChatSessionInput {
                        repository: chat.repository,
                        number: chat.number,
                    },
                )
                .map_err(|error| {
                    AgentError::provider(format!(
                        "refresh existing review comments from host: {error}"
                    ))
                })?;
            state.existing_comments = snapshot.existing_comments;
        }
        Ok(ExistingReviewCommentsResult {
            comments: state.existing_comments.clone(),
        })
    })
}

pub(crate) fn pull_request_context_in_state_file(
    state_path: &Path,
) -> AgentResult<PullRequestContextResult> {
    let state = load_review_mcp_session_state(state_path)?;
    Ok(PullRequestContextResult {
        context: state.pull_request_context,
    })
}

pub(crate) fn pull_request_conversation_comments_in_state_file(
    state_path: &Path,
) -> AgentResult<PullRequestConversationCommentsResult> {
    let state = load_review_mcp_session_state(state_path)?;
    Ok(PullRequestConversationCommentsResult {
        comments: state.pull_request_context.conversation_comments,
    })
}

pub(crate) fn delete_draft_comment_in_state_file(
    state_path: &Path,
    id: &str,
) -> AgentResult<DeleteDraftCommentResult> {
    update_review_mcp_session_state(state_path, |state| {
        validate_deletable_comment(&state.existing_comments, id)?;
        if let Some(chat) = &mut state.chat {
            if !chat
                .batch
                .deletion_ids
                .iter()
                .any(|deleted_id| deleted_id == id)
            {
                chat.batch.deletion_ids.push(id.to_owned());
            }
        } else if !state
            .deleted_comment_ids
            .iter()
            .any(|deleted_id| deleted_id == id)
        {
            state.deleted_comment_ids.push(id.to_owned());
        }
        Ok(DeleteDraftCommentResult { deleted: true })
    })
}

fn validate_deletable_comment(comments: &[ExistingReviewComment], id: &str) -> AgentResult<()> {
    let Some(comment) = comments.iter().find(|comment| comment.id == id) else {
        return Err(AgentError::invalid_input(format!(
            "review comment `{id}` is not available to this review session"
        )));
    };
    if !comment.draft || !comment.body.starts_with("🤖") {
        return Err(AgentError::invalid_input(
            "can only delete robot-authored draft comments",
        ));
    }
    Ok(())
}

fn update_review_mcp_session_state<T>(
    state_path: &Path,
    update: impl FnOnce(&mut ReviewMcpSessionState) -> AgentResult<T>,
) -> AgentResult<T> {
    let _lock = StateFileLock::acquire(state_path)?;
    let mut state = load_review_mcp_session_state(state_path)?;
    let result = update(&mut state)?;
    write_review_mcp_session_state(state_path, &state)?;
    Ok(result)
}

fn write_review_mcp_session_state(path: &Path, state: &ReviewMcpSessionState) -> AgentResult<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
        AgentError::json("serialize review MCP session state", path.display(), error)
    })?;
    let temp_path = path.with_extension("json.tmp");
    fs_err::write(&temp_path, bytes).map_err(|error| {
        AgentError::io_path("write review MCP session state", &temp_path, error)
    })?;
    fs_err::rename(&temp_path, path)
        .map_err(|error| AgentError::io_path("replace review MCP session state", path, error))
}

pub fn write_review_mcp_session_state_for_test(
    path: &Path,
    state: &ReviewMcpSessionState,
) -> AgentResult<()> {
    write_review_mcp_session_state(path, state)
}

struct StateFileLock {
    path: PathBuf,
}

impl StateFileLock {
    fn acquire(state_path: &Path) -> AgentResult<Self> {
        let path = lock_path(state_path);
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(AgentError::provider(format!(
                            "timed out waiting for review MCP session lock: {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(AgentError::io_path(
                        "create review MCP session lock",
                        &path,
                        error,
                    ));
                }
            }
        }
    }
}

impl Drop for StateFileLock {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.path);
    }
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}

fn new_temp_config_dir() -> AgentResult<PathBuf> {
    let id = NEXT_CONFIG_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nitpick-review-mcp-{}-{id}", std::process::id()));
    fs_err::create_dir_all(&dir)
        .map_err(|error| AgentError::io_path("create review MCP config directory", &dir, error))?;
    Ok(dir)
}

fn review_mcp_config_json(state_path: &Path, command: &Path) -> String {
    serde_json::json!({
        "mcpServers": {
            "nitpick-review": {
                "command": command,
                "args": ["review-mcp", state_path]
            }
        }
    })
    .to_string()
}

fn review_tool_instructions() -> String {
    include_str!("../../../examples/review-mcp-instructions.md").into()
}

fn review_mcp_server_command() -> String {
    std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "nitpick-agent-host".into())
}
