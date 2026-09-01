use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ActivityId, RemotePullRequestRef};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewInput {
    pub repo_dir: PathBuf,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub review_prompt: String,
    #[serde(default)]
    pub review_mode: ReviewMode,
    pub instructions: String,
    pub subject: ReviewSubject,
    #[serde(default)]
    pub head_sha: String,
    pub diff: String,
    #[serde(default)]
    pub disable_sandbox: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewMode {
    #[default]
    Requested,
    #[serde(rename = "self")]
    SelfReview,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSubject {
    pub repository: String,
    pub number: Option<u64>,
    pub title: String,
    pub author: String,
}

impl ReviewSubject {
    pub fn display_reference(&self) -> String {
        match self.number {
            Some(number) => format!("{}#{}", self.repository, number),
            None => self.repository.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub source: String,
    pub repository: String,
    pub number: Option<u64>,
    pub id: String,
    pub head_sha: String,
}

impl ReviewRequest {
    pub fn display_reference(&self) -> String {
        match self.number {
            Some(number) => format!("{}#{}", self.repository, number),
            None if self.id.is_empty() => self.repository.clone(),
            None => format!("{}#{}", self.repository, self.id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartReviewRequest {
    RemotePullRequest {
        reference: RemotePullRequestRef,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        disable_sandbox: bool,
    },
    Resolved {
        input: ReviewInput,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub comments: Vec<ReviewComment>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u32,
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewChatSessionInput {
    pub repository: String,
    pub number: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewChatSessionSnapshot {
    pub activity_id: ActivityId,
    pub repository: String,
    pub number: u64,
    pub repo_dir: PathBuf,
    pub head_sha: String,
    pub diff: String,
    pub pull_request_context: PullRequestContext,
    pub existing_comments: Vec<ExistingReviewComment>,
    pub mcp_server_command: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCommentBatch {
    pub id: String,
    #[serde(default)]
    pub additions: Vec<ReviewComment>,
    #[serde(default)]
    pub deletion_ids: Vec<String>,
}

impl ReviewCommentBatch {
    pub fn is_empty(&self) -> bool {
        self.additions.is_empty() && self.deletion_ids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReviewCommentBatchInput {
    pub pinned_head_sha: String,
    pub batch: ReviewCommentBatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReviewCommentBatchResult {
    pub batch_id: String,
    pub no_op: bool,
    pub committed_addition_count: usize,
    pub committed_deletion_count: usize,
    pub pending_review_id: Option<String>,
    pub pending_review_url: Option<String>,
    #[serde(default)]
    pub existing_comments: Vec<ExistingReviewComment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ExistingReviewComment {
    pub id: String,
    pub review_id: Option<String>,
    pub path: String,
    pub line: Option<u32>,
    pub body: String,
    pub author: Option<String>,
    pub draft: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PullRequestContext {
    pub title: String,
    pub author: String,
    pub url: String,
    pub body: String,
    pub head_sha: String,
    pub head_ref_name: String,
    pub state: String,
    #[serde(default)]
    pub conversation_comments: Vec<PullRequestConversationComment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PullRequestConversationComment {
    pub id: String,
    pub body: String,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatInput {
    pub repo_dir: PathBuf,
    pub prompt: String,
    pub context: String,
    #[serde(default)]
    pub disable_sandbox: bool,
    #[serde(default)]
    pub provider_timeout_ms: Option<u64>,
    #[serde(default)]
    pub provider_debug_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiagnosticInput {
    pub repo_dir: PathBuf,
    pub provider: Option<crate::AgentProviderKind>,
    pub model: Option<String>,
    #[serde(default)]
    pub disable_sandbox: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        FinishReviewCommentBatchInput, PullRequestContext, ReviewComment, ReviewCommentBatch,
        ReviewInput, ReviewSubject, StartReviewRequest,
    };
    use crate::RemotePullRequestRef;

    #[test]
    fn remote_pull_request_start_request_uses_explicit_kind() {
        let request = StartReviewRequest::RemotePullRequest {
            reference: RemotePullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            },
            force: true,
            disable_sandbox: true,
        };

        let json = serde_json::to_value(&request).expect("serialize request");

        assert_eq!(json["kind"], "remote_pull_request");
        assert_eq!(json["reference"]["owner"], "acme");
        assert_eq!(json["reference"]["repo"], "platform");
        assert_eq!(json["reference"]["number"], 42);
        assert_eq!(json["force"], true);
        assert_eq!(json["disable_sandbox"], true);
    }

    #[test]
    fn resolved_start_request_carries_review_input() {
        let input = ReviewInput {
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            diff: "diff --git a/src/lib.rs b/src/lib.rs\n".into(),
            ..ReviewInput::default()
        };
        let request = StartReviewRequest::Resolved {
            input: input.clone(),
        };

        let json = serde_json::to_string(&request).expect("serialize request");
        let decoded: StartReviewRequest = serde_json::from_str(&json).expect("deserialize request");

        assert_eq!(decoded, StartReviewRequest::Resolved { input });
    }

    #[test]
    fn review_chat_batch_contract_round_trips() {
        let input = FinishReviewCommentBatchInput {
            pinned_head_sha: "abc123".into(),
            batch: ReviewCommentBatch {
                id: "batch-1".into(),
                additions: vec![ReviewComment {
                    path: "src/lib.rs".into(),
                    line: 12,
                    body: "Prefer this.".into(),
                }],
                deletion_ids: vec!["99".into()],
            },
        };

        let json = serde_json::to_string(&input).expect("serialize batch");
        let decoded: FinishReviewCommentBatchInput =
            serde_json::from_str(&json).expect("deserialize batch");

        assert_eq!(decoded, input);
    }

    #[test]
    fn pull_request_context_defaults_conversation_comments() {
        let context: PullRequestContext = serde_json::from_str(
            r#"{"title":"Add watcher","author":"alice","url":"https://example.test/pr/1","body":"Body","head_sha":"abc123","head_ref_name":"feature","state":"open"}"#,
        )
        .expect("deserialize context");

        assert!(context.conversation_comments.is_empty());
    }
}
