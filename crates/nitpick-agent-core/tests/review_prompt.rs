use nitpick_agent_core::{ReviewInput, ReviewPromptOutput, ReviewSubject, render_review_prompt};

#[test]
fn review_prompt_render_target_switches_file_output_to_tools() {
    let input = ReviewInput {
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            title: "Improve thing".into(),
            author: "alice".into(),
        },
        repo_dir: "/tmp/repo".into(),
        instructions: "Be precise.".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs".into(),
        ..ReviewInput::default()
    };

    let prompt = render_review_prompt(
        Some("claude-opus"),
        &input,
        ReviewPromptOutput::Tools {
            tool_instructions: "Use add_review_comment.".into(),
        },
    );

    assert!(prompt.contains("model: claude-opus"));
    assert!(prompt.contains("tool instructions:\nUse add_review_comment."));
    assert!(prompt.contains("Use the Nitpick MCP tools to record inline review comments."));
    assert!(!prompt.contains("{review_output_path}"));
}
