use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use nitpick_agent_core::{
    AgentProviderKind, AgentRuntime, ChatInput, CommandAgentProvider, MemoryActivityStore,
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
