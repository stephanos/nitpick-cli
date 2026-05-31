use std::path::PathBuf;

use nitpick_agent_core::AgentResult;

use crate::review_mcp::{
    ActiveReviewSession, AddReviewCommentInput, AddReviewCommentResult, DeleteDraftCommentInput,
    DeleteDraftCommentResult, ExistingReviewCommentsResult, FinishReviewResult,
    PullRequestContextResult, PullRequestConversationCommentsResult,
    add_review_comment_to_state_file, delete_draft_comment_in_state_file,
    existing_review_comments_in_state_file, finish_review_in_state_file,
    pull_request_context_in_state_file, pull_request_conversation_comments_in_state_file,
};

#[derive(Clone, Debug)]
pub(crate) enum ReviewMcpSessionStore {
    Active(ActiveReviewSession),
    File(PathBuf),
}

impl ReviewMcpSessionStore {
    pub(crate) fn active(session: ActiveReviewSession) -> Self {
        Self::Active(session)
    }

    pub(crate) fn file(state_path: impl Into<PathBuf>) -> Self {
        Self::File(state_path.into())
    }

    pub(crate) fn add_review_comment(
        &self,
        input: AddReviewCommentInput,
    ) -> AgentResult<AddReviewCommentResult> {
        match self {
            Self::Active(session) => {
                session.add_review_comment(&input.path, input.line, input.body)?;
            }
            Self::File(state_path) => {
                add_review_comment_to_state_file(state_path, &input.path, input.line, input.body)?;
            }
        }
        Ok(AddReviewCommentResult { accepted: true })
    }

    pub(crate) fn finish_review(&self) -> AgentResult<FinishReviewResult> {
        match self {
            Self::Active(session) => session.finish_review(),
            Self::File(state_path) => finish_review_in_state_file(state_path),
        }
    }

    pub(crate) fn existing_review_comments(&self) -> AgentResult<ExistingReviewCommentsResult> {
        match self {
            Self::Active(session) => session.existing_review_comments(),
            Self::File(state_path) => existing_review_comments_in_state_file(state_path),
        }
    }

    pub(crate) fn pull_request_context(&self) -> AgentResult<PullRequestContextResult> {
        match self {
            Self::Active(session) => session.pull_request_context(),
            Self::File(state_path) => pull_request_context_in_state_file(state_path),
        }
    }

    pub(crate) fn pull_request_conversation_comments(
        &self,
    ) -> AgentResult<PullRequestConversationCommentsResult> {
        match self {
            Self::Active(session) => session.pull_request_conversation_comments(),
            Self::File(state_path) => pull_request_conversation_comments_in_state_file(state_path),
        }
    }

    pub(crate) fn delete_draft_comment(
        &self,
        input: DeleteDraftCommentInput,
    ) -> AgentResult<DeleteDraftCommentResult> {
        match self {
            Self::Active(session) => session.delete_draft_comment(&input.id),
            Self::File(state_path) => delete_draft_comment_in_state_file(state_path, &input.id),
        }
    }
}
