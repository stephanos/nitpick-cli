use std::{fs, sync::Arc};

use nitpick_agent_core::{
    ActivityStatus, AgentProvider, AgentResult, AgentSession, ChatInput, MemoryActivityStore,
    ReviewInput, ReviewOutput, ReviewSubject, ReviewToolConfig,
};
use nitpick_agent_host::{
    HostDaemon,
    review_mcp::{
        ActiveReviewSession, AddReviewCommentInput, ReviewMcpTools, load_review_mcp_session_state,
    },
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

struct FinishingToolProvider;

impl AgentProvider for FinishingToolProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        panic!("tool-aware provider should use review_with_tools");
    }

    fn review_with_tools(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        tools: &ReviewToolConfig,
    ) -> AgentResult<ReviewOutput> {
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

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
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
    ) -> AgentResult<ReviewOutput> {
        panic!("tool-aware provider should use review_with_tools");
    }

    fn review_with_tools(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        _tools: &ReviewToolConfig,
    ) -> AgentResult<ReviewOutput> {
        Ok(ReviewOutput::default())
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
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
    ) -> AgentResult<ReviewOutput> {
        panic!("tool-aware provider should use review_with_tools");
    }

    fn review_with_tools(
        &self,
        _session: &mut AgentSession,
        _input: &ReviewInput,
        tools: &ReviewToolConfig,
    ) -> AgentResult<ReviewOutput> {
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

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
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
