use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use nitpick_agent_core::{
    AgentProviderKind, AgentRuntime, ChatInput, CommandAgentProvider, MemoryActivityStore,
    ReviewInput, ReviewSubject,
};

#[test]
fn command_provider_runs_chat_command_and_stores_output() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf command-response\n",
    )
    .expect("write command");
    let mut permissions = fs::metadata(&command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&command, permissions).expect("chmod");

    let provider = Arc::new(CommandAgentProvider::new(
        AgentProviderKind::Claude,
        Some("test-model".into()),
        &command,
    ));
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_chat(ChatInput {
            prompt: "hello".into(),
            ..ChatInput::default()
        })
        .expect("chat runs");

    assert_eq!(
        activity.output.unwrap().chat_text(),
        Some("command-response")
    );
}

#[test]
fn claude_command_provider_passes_review_session_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nprintf review-response\n",
            args_log.display()
        ),
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(CommandAgentProvider::new(
        AgentProviderKind::Claude,
        Some("test-model".into()),
        &command,
    ));
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    runtime
        .start_review(ReviewInput {
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "--session-id github:acme/platform#42\n"
    );
}

#[test]
fn codex_command_provider_does_not_use_claude_session_flag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nprintf review-response\n",
            args_log.display()
        ),
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(CommandAgentProvider::new(
        AgentProviderKind::Codex,
        Some("test-model".into()),
        &command,
    ));
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    runtime
        .start_review(ReviewInput {
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    assert_eq!(fs::read_to_string(args_log).expect("args"), "\n");
}

fn make_executable(command: &std::path::Path) {
    let mut permissions = fs::metadata(command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(command, permissions).expect("chmod");
}

trait ActivityOutputExt {
    fn chat_text(&self) -> Option<&str>;
}

impl ActivityOutputExt for nitpick_agent_core::ActivityOutput {
    fn chat_text(&self) -> Option<&str> {
        match self {
            Self::Chat(output) => Some(output),
            Self::Review(_) => None,
        }
    }
}
