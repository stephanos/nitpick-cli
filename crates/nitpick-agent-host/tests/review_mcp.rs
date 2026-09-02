use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};

use nitpick_agent_core::{
    AgentProvider, AgentResult, AgentRuntime, MemoryActivityStore, ProviderReviewContext,
    ProviderRunContext,
};
use nitpick_agent_host::{
    HostDaemon, HostReviewProvider,
    review_mcp::{
        ActiveReviewSession, AddReviewCommentInput, DeleteDraftCommentInput, ExistingReviewComment,
        PullRequestContext, PullRequestConversationComment, ReviewMcpChatState,
        ReviewMcpServerHandle, ReviewMcpSessionState, ReviewMcpTools,
        load_review_mcp_session_state, write_review_mcp_session_state_for_test,
    },
};
use nitpick_agent_model::{
    ActivityId, ActivityStatus, AgentSession, ChatInput, ReviewChatSessionSnapshot,
    ReviewCommentBatch, ReviewInput, ReviewOutput, ReviewSubject,
};

#[test]
fn active_review_session_records_review_comments() {
    let fixture = ReviewFixture::new();
    let session = ActiveReviewSession::new(&fixture.repo_dir, DIFF).expect("review session");

    let comment = session
        .add_review_comment("src.rs", 1, "use a clearer entry point")
        .expect("review comment");

    assert_eq!(comment.path, "src.rs");
    assert_eq!(comment.line, 1);
    assert_eq!(comment.body, "use a clearer entry point");
    assert!(!session.is_finished().expect("finished state"));
    assert_eq!(session.comments().expect("comments"), vec![comment]);
}

#[test]
fn active_review_session_finish_returns_comment_count_and_marks_finished() {
    let fixture = ReviewFixture::new();
    let session = ActiveReviewSession::new(&fixture.repo_dir, DIFF).expect("review session");
    session
        .add_review_comment("src.rs", 1, "use a clearer entry point")
        .expect("first comment");
    session
        .add_review_comment("src.rs", 2, "avoid debug output")
        .expect("second comment");

    let result = session.finish_review().expect("finish review");

    assert_eq!(result.status, "completed");
    assert_eq!(result.comment_count, 2);
    assert!(session.is_finished().expect("finished state"));
}

#[test]
fn active_review_session_rejects_review_comments_after_finish() {
    let fixture = ReviewFixture::new();
    let session = ActiveReviewSession::new(&fixture.repo_dir, DIFF).expect("review session");
    session.finish_review().expect("finish review");

    let error = session
        .add_review_comment("src.rs", 1, "too late")
        .expect_err("finished session rejected");

    assert_eq!(error.to_string(), "review session is already finished");
    assert_eq!(session.comments().expect("comments"), vec![]);
}

#[test]
fn review_mcp_tools_add_review_comment_records_comment() {
    let fixture = ReviewFixture::new();
    let session = ActiveReviewSession::new(&fixture.repo_dir, DIFF).expect("review session");
    let tools = ReviewMcpTools::new(session.clone());

    let result = tools
        .add_review_comment(AddReviewCommentInput {
            path: "src.rs".to_owned(),
            line: 1,
            body: "use a clearer entry point".to_owned(),
        })
        .expect("review comment");

    assert!(result.accepted);
    assert_eq!(session.comments().expect("comments").len(), 1);
    assert!(!session.is_finished().expect("finished state"));
}

#[test]
fn review_chat_addition_is_staged_only_in_the_current_batch() {
    let fixture = ReviewFixture::new();
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: fixture.repo_dir,
            diff: DIFF.into(),
            comments: Vec::new(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: Vec::new(),
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: Some(ReviewMcpChatState {
                host_addr: "127.0.0.1:1".into(),
                activity_id: "activity-1".into(),
                repository: "acme/platform".into(),
                number: 42,
                pinned_head_sha: "abc123".into(),
                batch: ReviewCommentBatch {
                    id: "batch-1".into(),
                    additions: Vec::new(),
                    deletion_ids: Vec::new(),
                },
                last_result: None,
            }),
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    tools
        .add_review_comment(AddReviewCommentInput {
            path: "src.rs".into(),
            line: 1,
            body: "use a clearer entry point".into(),
        })
        .expect("stage comment");

    let state = load_review_mcp_session_state(state.path()).expect("state");
    assert!(state.comments.is_empty());
    assert_eq!(state.chat.expect("chat").batch.additions.len(), 1);
}

#[test]
fn review_chat_deletion_is_staged_only_in_the_current_batch() {
    let fixture = ReviewFixture::new();
    let state = write_chat_state(&fixture, "127.0.0.1:1");
    let mut session_state = load_review_mcp_session_state(state.path()).expect("state");
    session_state.existing_comments = vec![existing_comment(
        "11",
        "nitpick",
        "🤖 Old automated note.",
        true,
    )];
    write_review_mcp_session_state_for_test(state.path(), &session_state).expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    tools
        .delete_draft_comment(DeleteDraftCommentInput { id: "11".into() })
        .expect("stage deletion");

    let state = load_review_mcp_session_state(state.path()).expect("state");
    assert!(state.deleted_comment_ids.is_empty());
    assert_eq!(state.chat.expect("chat").batch.deletion_ids, ["11"]);
}

#[test]
fn review_chat_empty_finish_is_a_local_no_op() {
    let fixture = ReviewFixture::new();
    let state = write_chat_state(&fixture, "127.0.0.1:1");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools.finish_review().expect("empty finish");

    assert_eq!(result.status, "completed");
    assert_eq!(result.comment_count, 0);
    let state = load_review_mcp_session_state(state.path()).expect("state");
    assert!(!state.finished);
    assert_eq!(state.chat.expect("chat").batch.id, "batch-1");
}

#[test]
fn review_chat_successful_finish_rotates_the_batch_and_allows_another_finish() {
    let fixture = ReviewFixture::new();
    let (host_addr, server) = serve_finish_results(2);
    let state = write_chat_state(&fixture, &host_addr);
    let tools = ReviewMcpTools::from_state_path(state.path());

    for body in ["first finding", "second finding"] {
        tools
            .add_review_comment(AddReviewCommentInput {
                path: "src.rs".into(),
                line: 1,
                body: body.into(),
            })
            .expect("stage comment");
        let result = tools.finish_review().expect("finish batch");
        assert_eq!(result.comment_count, 1);
        let state_after_finish =
            load_review_mcp_session_state(state.path()).expect("state after finish");
        let chat = state_after_finish.chat.expect("chat");
        assert!(chat.batch.is_empty());
        assert_ne!(chat.batch.id, "batch-1");
        assert!(!state_after_finish.finished);
    }
    server.join().expect("server");

    let final_state = load_review_mcp_session_state(state.path()).expect("final state");
    assert_eq!(final_state.existing_comments.len(), 2);
    assert_eq!(final_state.existing_comments[0].id, "comment-1");
    assert_eq!(final_state.existing_comments[1].id, "comment-2");
    assert!(
        final_state
            .existing_comments
            .iter()
            .all(|comment| comment.draft)
    );
}

#[test]
fn review_chat_failed_finish_retains_the_batch_for_retry() {
    let fixture = ReviewFixture::new();
    let state = write_chat_state(&fixture, "127.0.0.1:1");
    let tools = ReviewMcpTools::from_state_path(state.path());
    tools
        .add_review_comment(AddReviewCommentInput {
            path: "src.rs".into(),
            line: 1,
            body: "keep this finding".into(),
        })
        .expect("stage comment");

    tools.finish_review().expect_err("host unavailable");

    let state = load_review_mcp_session_state(state.path()).expect("state");
    let chat = state.chat.expect("chat");
    assert_eq!(chat.batch.id, "batch-1");
    assert_eq!(chat.batch.additions.len(), 1);
    assert!(chat.last_result.is_none());
}

#[test]
fn review_chat_can_delete_a_canonical_comment_from_an_earlier_batch() {
    let fixture = ReviewFixture::new();
    let (host_addr, server) = serve_finish_results(4);
    let state = write_chat_state(&fixture, &host_addr);
    let tools = ReviewMcpTools::from_state_path(state.path());
    tools
        .add_review_comment(AddReviewCommentInput {
            path: "src.rs".into(),
            line: 1,
            body: "temporary finding".into(),
        })
        .expect("stage comment");
    tools.finish_review().expect("finish addition");

    let comments = tools.existing_review_comments().expect("existing comments");
    assert_eq!(comments.comments[0].id, "comment-1");
    assert!(comments.comments[0].draft);
    tools
        .delete_draft_comment(DeleteDraftCommentInput {
            id: "comment-1".into(),
        })
        .expect("stage canonical deletion");
    tools.finish_review().expect("finish deletion");

    assert!(
        tools
            .existing_review_comments()
            .expect("comments after deletion")
            .comments
            .is_empty()
    );
    server.join().expect("server");
}

#[test]
fn review_chat_handle_uses_the_host_snapshot_executable_and_state() {
    let fixture = ReviewFixture::new();
    let host_executable = fixture._dir.path().join("nitpick-agent-host");
    fs::write(&host_executable, "host").expect("host executable");
    let handle = ReviewMcpServerHandle::start_chat(
        "127.0.0.1:19783",
        ReviewChatSessionSnapshot {
            activity_id: ActivityId::new("activity-1"),
            repository: "acme/platform".into(),
            number: 42,
            repo_dir: fixture.repo_dir.clone(),
            head_sha: "abc123".into(),
            diff: DIFF.into(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: Vec::new(),
            mcp_server_command: host_executable.clone(),
        },
    )
    .expect("chat handle");

    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(&handle.tool_config().mcp_config_path).expect("config"))
            .expect("config json");
    assert_eq!(
        config["mcpServers"]["nitpick-review"]["command"],
        host_executable.display().to_string()
    );
    let state = handle.session_state().expect("state");
    assert_eq!(state.repo_dir, fixture.repo_dir);
    assert!(state.chat.expect("chat").batch.is_empty());
}

#[test]
fn review_mcp_tools_finish_review_marks_session_finished() {
    let fixture = ReviewFixture::new();
    let session = ActiveReviewSession::new(&fixture.repo_dir, DIFF).expect("review session");
    let tools = ReviewMcpTools::new(session.clone());
    tools
        .add_review_comment(AddReviewCommentInput {
            path: "src.rs".to_owned(),
            line: 1,
            body: "use a clearer entry point".to_owned(),
        })
        .expect("review comment");

    let result = tools.finish_review().expect("finish review");

    assert_eq!(result.status, "completed");
    assert_eq!(result.comment_count, 1);
    assert!(session.is_finished().expect("finished state"));
}

#[test]
fn review_mcp_tools_lists_existing_comments() {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: ".".into(),
            diff: String::new(),
            comments: Vec::new(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: vec![
                existing_comment("10", "alice", "Please adjust this.", false),
                existing_comment("11", "nitpick", "🤖 Old automated note.", true),
            ],
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: None,
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools.existing_review_comments().expect("existing comments");

    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.comments[0].id, "10");
    assert_eq!(result.comments[1].body, "🤖 Old automated note.");
}

#[test]
fn review_chat_existing_comments_refreshes_the_host_snapshot() {
    let fixture = ReviewFixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let addr = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).expect("read request");
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if request_body_is_complete(&request) {
                break;
            }
        }
        assert!(request.starts_with(b"POST /activities/activity-1/review-chat-session HTTP/1.1"));
        let response_body = serde_json::json!({
            "activity_id": "activity-1",
            "repository": "acme/platform",
            "number": 42,
            "repo_dir": ".",
            "head_sha": "abc123",
            "diff": DIFF,
            "pull_request_context": PullRequestContext::default(),
            "existing_comments": [{
                "id": "comment-1",
                "review_id": "99",
                "path": "src.rs",
                "line": 1,
                "body": "🤖 Remotely staged finding.",
                "author": "nitpick",
                "draft": true
            }],
            "mcp_server_command": "/tmp/nitpick-agent-host"
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        )
        .expect("write response");
    });
    let state = write_chat_state(&fixture, &addr.to_string());
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools.existing_review_comments().expect("existing comments");

    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].id, "comment-1");
    assert!(result.comments[0].draft);
    let state = load_review_mcp_session_state(state.path()).expect("state");
    assert_eq!(state.existing_comments, result.comments);
    server.join().expect("server");
}

#[test]
fn review_mcp_tools_returns_pull_request_context() {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: ".".into(),
            diff: String::new(),
            comments: Vec::new(),
            pull_request_context: pull_request_context(),
            existing_comments: Vec::new(),
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: None,
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools.pull_request_context().expect("pull request context");

    assert_eq!(result.context.title, "Add watcher");
    assert_eq!(result.context.body, "Please review the watcher changes.");
    assert_eq!(result.context.conversation_comments.len(), 1);
    assert_eq!(
        result.context.conversation_comments[0].body,
        "Can you explain the retry behavior?"
    );
}

#[test]
fn review_mcp_tools_returns_pull_request_conversation_comments() {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: ".".into(),
            diff: String::new(),
            comments: Vec::new(),
            pull_request_context: pull_request_context(),
            existing_comments: Vec::new(),
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: None,
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools
        .pull_request_conversation_comments()
        .expect("pull request conversation comments");

    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].id, "100");
    assert_eq!(result.comments[0].author.as_deref(), Some("alice"));
}

#[test]
fn review_mcp_tools_records_robot_draft_comment_deletion() {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: ".".into(),
            diff: String::new(),
            comments: Vec::new(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: vec![existing_comment(
                "11",
                "nitpick",
                "🤖 Old automated note.",
                true,
            )],
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: None,
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let result = tools
        .delete_draft_comment(DeleteDraftCommentInput { id: "11".into() })
        .expect("delete draft comment");

    assert!(result.deleted);
    let state = load_review_mcp_session_state(state.path()).expect("state");
    assert_eq!(state.deleted_comment_ids, ["11"]);
}

#[test]
fn review_mcp_tools_refuses_to_delete_user_or_submitted_comments() {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: ".".into(),
            diff: String::new(),
            comments: Vec::new(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: vec![
                existing_comment("10", "alice", "Please adjust this.", true),
                existing_comment("11", "nitpick", "🤖 Submitted note.", false),
            ],
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: None,
            finished: false,
        },
    )
    .expect("write state");
    let tools = ReviewMcpTools::from_state_path(state.path());

    let user_error = tools
        .delete_draft_comment(DeleteDraftCommentInput { id: "10".into() })
        .expect_err("user comment deletion rejected");
    let submitted_error = tools
        .delete_draft_comment(DeleteDraftCommentInput { id: "11".into() })
        .expect_err("submitted comment deletion rejected");

    assert_eq!(
        user_error.to_string(),
        "can only delete robot-authored draft comments"
    );
    assert_eq!(
        submitted_error.to_string(),
        "can only delete robot-authored draft comments"
    );
}

#[test]
fn host_daemon_review_wrapper_returns_comments_from_finished_mcp_session() {
    let fixture = ReviewFixture::new();
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_provider(store, Arc::new(FinishingToolProvider));

    let activity = daemon
        .start_review(ReviewInput {
            repo_dir: fixture.repo_dir,
            diff: DIFF.into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review activity");

    assert_eq!(activity.status, ActivityStatus::Completed);
    let output = match activity.output.expect("review output") {
        nitpick_agent_core::ActivityOutput::Review(output) => output,
        nitpick_agent_core::ActivityOutput::Chat(_) => panic!("expected review output"),
    };
    assert_eq!(output.comments.len(), 1);
    assert_eq!(output.comments[0].path, "src.rs");
    assert_eq!(output.comments[0].line, 1);
    assert_eq!(output.comments[0].body, "use a clearer entry point");
}

#[test]
fn host_daemon_review_wrapper_errors_when_provider_does_not_finish_mcp_session() {
    let fixture = ReviewFixture::new();
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_provider(store, Arc::new(NonFinishingToolProvider));

    let activity = daemon
        .start_review(ReviewInput {
            repo_dir: fixture.repo_dir,
            diff: DIFF.into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review activity");

    assert_eq!(activity.status, ActivityStatus::Error);
    assert_eq!(
        activity.error.as_deref(),
        Some("provider exited before calling finish_review")
    );
}

#[test]
fn host_daemon_review_wrapper_uses_generated_config_and_file_backed_session() {
    let fixture = ReviewFixture::new();
    let store = Arc::new(MemoryActivityStore::default());
    let daemon = HostDaemon::with_provider(store, Arc::new(FileBackedToolProvider));

    let activity = daemon
        .start_review(ReviewInput {
            repo_dir: fixture.repo_dir,
            diff: DIFF.into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review activity");

    assert_eq!(activity.status, ActivityStatus::Completed);
    let output = match activity.output.expect("review output") {
        nitpick_agent_core::ActivityOutput::Review(output) => output,
        nitpick_agent_core::ActivityOutput::Chat(_) => panic!("expected review output"),
    };
    assert_eq!(output.comments.len(), 1);
    assert_eq!(output.comments[0].body, "recorded through file state");
}

#[test]
fn host_review_provider_exposes_existing_comments_and_deletes_robot_drafts() {
    let fixture = ReviewFixture::new();
    let dir = tempfile::tempdir().expect("temp dir");
    let gh = dir.path().join("gh");
    let commands = dir.path().join("commands");
    fs::write(
        &gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {commands}
if [ "$*" = "api repos/acme/platform/pulls/42/comments" ]; then
  printf '[{{"id":10,"pull_request_review_id":98,"path":"src.rs","line":1,"body":"Please adjust this.","user":{{"login":"alice"}},"state":"SUBMITTED"}},{{"id":11,"pull_request_review_id":99,"path":"src.rs","line":1,"body":"🤖 Old automated note.","user":{{"login":"nitpick"}},"state":"PENDING"}}]\n'
elif [ "$*" = "api repos/acme/platform/pulls/42/reviews" ]; then
  printf '[]\n'
elif [ "$*" = "pr view 42 --repo acme/platform --json title,author,url,body,baseRefOid,headRefOid,headRefName,state,mergedAt" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","body":"Please review the watcher changes.","baseRefOid":"base123","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}\n'
elif [ "$*" = "api repos/acme/platform/issues/42/comments" ]; then
  printf '[{{"id":100,"body":"Can you explain the retry behavior?","user":{{"login":"alice"}},"created_at":"2026-05-30T12:00:00Z","updated_at":"2026-05-30T12:30:00Z","html_url":"https://github.com/acme/platform/pull/42#issuecomment-100"}}]\n'
fi
"#,
            commands = commands.display(),
        ),
    )
    .expect("write gh");
    make_executable(&gh);
    let store = Arc::new(MemoryActivityStore::default());
    let provider = Arc::new(HostReviewProvider::new(
        Arc::new(DeletingToolProvider),
        Some(gh.display().to_string()),
    ));
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir: fixture.repo_dir,
            diff: DIFF.into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review activity");

    assert_eq!(activity.status, ActivityStatus::Completed);
    assert_eq!(
        fs::read_to_string(commands).expect("commands"),
        "api repos/acme/platform/pulls/42/comments\napi repos/acme/platform/pulls/42/reviews\npr view 42 --repo acme/platform --json title,author,url,body,baseRefOid,headRefOid,headRefName,state,mergedAt\napi repos/acme/platform/issues/42/comments\napi repos/acme/platform/pulls/comments/11 --method DELETE\n"
    );
}

struct FinishingToolProvider;

impl AgentProvider for FinishingToolProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        let tools = context.tools.expect("tool-aware provider should use tools");
        let tools = ReviewMcpTools::from_state_path(state_path_from_config(&tools.mcp_config_path));
        tools
            .add_review_comment(AddReviewCommentInput {
                path: "src.rs".into(),
                line: 1,
                body: "use a clearer entry point".into(),
            })
            .expect("add comment");
        tools.finish_review().expect("finish review");
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok(String::new())
    }
}

struct NonFinishingToolProvider;

impl AgentProvider for NonFinishingToolProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        assert!(
            context.tools.is_some(),
            "tool-aware provider should use tools"
        );
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok(String::new())
    }
}

struct FileBackedToolProvider;

impl AgentProvider for FileBackedToolProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        let tools = context.tools.expect("tool-aware provider should use tools");
        let state_path = state_path_from_config(&tools.mcp_config_path);
        let initial_state =
            load_review_mcp_session_state(&state_path).expect("initial session state");
        assert!(!initial_state.finished);
        let tools = ReviewMcpTools::from_state_path(state_path);
        tools
            .add_review_comment(AddReviewCommentInput {
                path: "src.rs".into(),
                line: 1,
                body: "recorded through file state".into(),
            })
            .expect("add comment");
        tools.finish_review().expect("finish review");
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok(String::new())
    }
}

struct DeletingToolProvider;

impl AgentProvider for DeletingToolProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        let tools = context.tools.expect("tool-aware provider should use tools");
        let tools = ReviewMcpTools::from_state_path(state_path_from_config(&tools.mcp_config_path));
        let existing = tools.existing_review_comments()?;
        assert_eq!(existing.comments.len(), 2);
        assert_eq!(existing.comments[0].author.as_deref(), Some("alice"));
        assert!(existing.comments[1].draft);
        let context = tools.pull_request_context()?;
        assert_eq!(context.context.body, "Please review the watcher changes.");
        let comments = tools.pull_request_conversation_comments()?;
        assert_eq!(comments.comments.len(), 1);
        assert_eq!(
            comments.comments[0].body,
            "Can you explain the retry behavior?"
        );
        tools.delete_draft_comment(DeleteDraftCommentInput { id: "11".into() })?;
        tools.finish_review()?;
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok(String::new())
    }
}

fn state_path_from_config(config_path: &std::path::Path) -> String {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(config_path).expect("mcp config bytes"))
            .expect("mcp config json");
    let args = config["mcpServers"]["nitpick-review"]["args"]
        .as_array()
        .expect("server args");
    assert_eq!(args[0], "review-mcp");
    args[1].as_str().expect("state path").to_owned()
}

struct ReviewFixture {
    _dir: tempfile::TempDir,
    repo_dir: std::path::PathBuf,
}

impl ReviewFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs::create_dir(&repo_dir).expect("repo dir");
        fs::write(
            repo_dir.join("src.rs"),
            "fn main() {}\neprintln!(\"debug\");\n",
        )
        .expect("repo file");

        Self {
            _dir: dir,
            repo_dir,
        }
    }
}

const DIFF: &str = "\
diff --git a/src.rs b/src.rs
--- a/src.rs
+++ b/src.rs
@@ -0,0 +1,2 @@
+fn main() {}
+eprintln!(\"debug\");
";

fn existing_comment(id: &str, author: &str, body: &str, draft: bool) -> ExistingReviewComment {
    ExistingReviewComment {
        id: id.into(),
        review_id: Some("99".into()),
        path: "src.rs".into(),
        line: Some(1),
        body: body.into(),
        author: Some(author.into()),
        draft,
    }
}

fn pull_request_context() -> PullRequestContext {
    PullRequestContext {
        title: "Add watcher".into(),
        author: "stephan".into(),
        url: "https://github.com/acme/platform/pull/42".into(),
        body: "Please review the watcher changes.".into(),
        head_sha: "abc123".into(),
        head_ref_name: "feature/watcher".into(),
        state: "open".into(),
        conversation_comments: vec![PullRequestConversationComment {
            id: "100".into(),
            body: "Can you explain the retry behavior?".into(),
            author: Some("alice".into()),
            created_at: Some("2026-05-30T12:00:00Z".into()),
            updated_at: Some("2026-05-30T12:30:00Z".into()),
            url: Some("https://github.com/acme/platform/pull/42#issuecomment-100".into()),
        }],
    }
}

fn write_chat_state(fixture: &ReviewFixture, host_addr: &str) -> tempfile::NamedTempFile {
    let state = tempfile::NamedTempFile::new().expect("state file");
    write_review_mcp_session_state_for_test(
        state.path(),
        &ReviewMcpSessionState {
            repo_dir: fixture.repo_dir.clone(),
            diff: DIFF.into(),
            comments: Vec::new(),
            pull_request_context: PullRequestContext::default(),
            existing_comments: Vec::new(),
            deleted_comment_ids: Vec::new(),
            github: None,
            chat: Some(ReviewMcpChatState {
                host_addr: host_addr.into(),
                activity_id: "activity-1".into(),
                repository: "acme/platform".into(),
                number: 42,
                pinned_head_sha: "abc123".into(),
                batch: ReviewCommentBatch {
                    id: "batch-1".into(),
                    additions: Vec::new(),
                    deletion_ids: Vec::new(),
                },
                last_result: None,
            }),
            finished: false,
        },
    )
    .expect("write state");
    state
}

fn serve_finish_results(request_count: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let addr = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let mut existing_comments: Vec<serde_json::Value> = Vec::new();
        let mut next_comment_id = 1;
        for _ in 0..request_count {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).expect("read request");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if request_body_is_complete(&request) {
                    break;
                }
            }
            let body_start = request
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .map(|index| index + 4)
                .expect("request headers");
            if request.starts_with(b"POST /activities/activity-1/review-chat-session HTTP/1.1") {
                let response_body = serde_json::json!({
                    "activity_id": "activity-1",
                    "repository": "acme/platform",
                    "number": 42,
                    "repo_dir": ".",
                    "head_sha": "abc123",
                    "diff": DIFF,
                    "pull_request_context": PullRequestContext::default(),
                    "existing_comments": existing_comments,
                    "mcp_server_command": "/tmp/nitpick-agent-host"
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .expect("write response");
                continue;
            }
            let input: serde_json::Value =
                serde_json::from_slice(&request[body_start..]).expect("request json");
            let batch_id = input["batch"]["id"].as_str().expect("batch id");
            let additions = input["batch"]["additions"].as_array().expect("additions");
            let deletion_ids = input["batch"]["deletion_ids"]
                .as_array()
                .expect("deletion IDs");
            existing_comments.retain(|comment| !deletion_ids.iter().any(|id| id == &comment["id"]));
            for addition in additions {
                existing_comments.push(serde_json::json!({
                    "id": format!("comment-{next_comment_id}"),
                    "review_id": "99",
                    "path": addition["path"],
                    "line": addition["line"],
                    "body": format!("🤖 {}", addition["body"].as_str().expect("body")),
                    "author": "nitpick",
                    "draft": true
                }));
                next_comment_id += 1;
            }
            let response_body = serde_json::json!({
                "batch_id": batch_id,
                "no_op": false,
                "committed_addition_count": additions.len(),
                "committed_deletion_count": deletion_ids.len(),
                "pending_review_id": "99",
                "pending_review_url": "https://example.test/review-99",
                "existing_comments": existing_comments
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .expect("write response");
        }
    });
    (addr.to_string(), server)
}

fn request_body_is_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|length| length.parse::<usize>().ok())
        })
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_length
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}
