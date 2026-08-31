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

#[test]
fn default_review_prompt_includes_feedback_format_contract() {
    let prompt = render_review_prompt(
        None,
        &ReviewInput::default(),
        ReviewPromptOutput::Tools {
            tool_instructions: "Use add_review_comment.".into(),
        },
    );

    assert!(
        prompt.contains(
            "<details>\n<summary><strong>SEVERITY</strong> — One-line summary.</summary>"
        )
    );
    let details_end = prompt.find("</details>").expect("details closing tag");
    let suggestion = prompt.find("**Suggestion:**").expect("suggestion guidance");
    assert!(suggestion > details_end);
    for severity in ["`nit`", "`small`", "`med`", "`high`"] {
        assert!(prompt.contains(severity), "missing severity {severity}");
    }
    assert!(prompt.contains("Preference-based. Non-blocking."));
    assert!(prompt.contains("Affects correctness or maintainability. Blocking."));
    assert!(prompt.contains("Prefer a small number of high-confidence findings."));
    assert!(prompt.contains("single comment that addresses the root issue."));
    assert!(prompt.contains("Reference specific codebase patterns and utilities"));
}
