use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

use nitpick_agent_core::{
    ActivityStore, AgentProvider, AgentProviderKind, AgentRuntime, AgentSession, ChatInput,
    CommandAgentProvider, CommandSandboxConfig, MemoryActivityStore, NoopProviderRunSink,
    ProviderReviewContext, ReviewInput, ReviewSubject, ReviewToolConfig,
    validate_review_output_file, validate_review_output_file_for_diff,
};

#[test]
fn command_provider_runs_chat_command_and_stores_output() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf provider-warning >&2\nprintf command-response\n",
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
            repo_dir: dir.path().to_path_buf(),
            prompt: "hello".into(),
            ..ChatInput::default()
        })
        .expect("chat runs");

    assert_eq!(activity.error, None);
    assert_eq!(
        activity.output.unwrap().chat_text(),
        Some("command-response")
    );
    assert_eq!(activity.session.messages.len(), 3);
    assert_eq!(activity.session.messages[0].role, "provider.stdout");
    assert_eq!(activity.session.messages[0].content, "command-response");
    assert_eq!(activity.session.messages[1].role, "provider.stderr");
    assert_eq!(activity.session.messages[1].content, "provider-warning");
    assert_eq!(activity.session.messages[2].role, "provider.run");
    assert!(
        activity.session.messages[2]
            .content
            .contains("provider claude command completed")
    );
    assert!(
        activity.session.messages[2]
            .content
            .contains("stdout: captured")
    );
    assert!(
        activity.session.messages[2]
            .content
            .contains("stderr: captured")
    );
}

#[test]
fn command_provider_times_out_chat_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(&command, "#!/bin/sh\ncat >/dev/null\nexec sleep 10\n").expect("write command");
    make_executable(&command);

    let provider = Arc::new(CommandAgentProvider::new(
        AgentProviderKind::Claude,
        Some("test-model".into()),
        &command,
    ));
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let started = Instant::now();
    let activity = runtime
        .start_chat(ChatInput {
            repo_dir: dir.path().to_path_buf(),
            prompt: "hello".into(),
            provider_timeout_ms: Some(50),
            ..ChatInput::default()
        })
        .expect("chat activity saved");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        activity.error.as_deref(),
        Some("claude provider command timed out after 50ms")
    );
    let run_log = provider_log(&activity.session, "provider.run").expect("provider run log");
    assert!(run_log.contains("timed_out: true"));
}

#[test]
fn codex_command_provider_runs_chat_with_exec() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nprintf command-response\n",
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

    let activity = runtime
        .start_chat(ChatInput {
            repo_dir: dir.path().to_path_buf(),
            prompt: "hello".into(),
            ..ChatInput::default()
        })
        .expect("chat runs");

    assert_eq!(activity.error, None);
    assert_eq!(
        activity.output.unwrap().chat_text(),
        Some("command-response")
    );
    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "--dangerously-bypass-approvals-and-sandbox exec\n"
    );
}

#[test]
fn claude_command_provider_passes_review_session_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nmkdir -p .nitpick\nprintf '{{\"comments\":[]}}' > .nitpick/review-output.json\n",
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
            repo_dir,
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    let args = fs::read_to_string(args_log).expect("args");
    let session_id = args
        .strip_prefix("--session-id ")
        .and_then(|value| value.strip_suffix('\n'))
        .expect("session id arg");
    assert!(is_uuid_like(session_id), "{session_id}");
}

#[test]
fn codex_command_provider_does_not_use_claude_session_flag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nmkdir -p .nitpick\nprintf '{{\"comments\":[]}}' > .nitpick/review-output.json\n",
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
            repo_dir,
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
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
        "--dangerously-bypass-approvals-and-sandbox exec\n"
    );
}

#[test]
fn command_provider_reads_review_output_from_validated_json_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nmkdir -p .nitpick\ncat > .nitpick/review-output.json <<'JSON'\n{\"comments\":[{\"path\":\"src.rs\",\"line\":1,\"body\":\"use a clearer name\"}]}\nJSON\nprintf ignored-stdout\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir,
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    let activity_output = activity.output.unwrap();
    let output = activity_output.review_output().expect("review output");
    assert_eq!(output.comments.len(), 1);
    assert_eq!(output.comments[0].path, "src.rs");
}

#[test]
fn command_provider_persists_run_diagnostic_for_quiet_review() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nmkdir -p .nitpick\nprintf '{\"comments\":[]}' > .nitpick/review-output.json\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir,
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    let run_log = provider_log(&activity.session, "provider.run").expect("provider run log");
    assert!(run_log.contains("provider claude command completed"));
    assert!(run_log.contains("sandbox: disabled"));
    assert!(run_log.contains("status: exit status: 0"));
    assert!(run_log.contains("stdout: empty"));
    assert!(run_log.contains("stderr: empty"));
}

#[test]
fn command_provider_persists_run_diagnostic_while_command_is_running() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nsleep 1\nprintf command-response\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(CommandAgentProvider::new(
        AgentProviderKind::Claude,
        Some("test-model".into()),
        &command,
    ));
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let input = ChatInput {
        repo_dir: dir.path().to_path_buf(),
        prompt: "hello".into(),
        provider_timeout_ms: Some(5_000),
        ..ChatInput::default()
    };
    let activity = runtime.create_chat_activity().expect("activity");
    let activity_id = activity.id.clone();
    let runtime_thread = std::thread::spawn(move || runtime.run_chat(activity, input));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let persisted = store.get(&activity_id).expect("persisted activity");
        if let Some(run_log) = provider_log(&persisted.session, "provider.run") {
            assert!(run_log.contains("provider claude command running"));
            assert!(run_log.contains("timeout: 5s"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "provider run diagnostic was not persisted before command exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let activity = runtime_thread
        .join()
        .expect("runtime thread")
        .expect("chat");
    assert_eq!(activity.error, None);
}

#[test]
fn command_provider_cancels_running_command_when_activity_is_cancelled() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf started\nsleep 30\nprintf done\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let input = ChatInput {
        repo_dir: dir.path().to_path_buf(),
        prompt: "hello".into(),
        provider_timeout_ms: Some(60_000),
        ..ChatInput::default()
    };
    let activity = runtime.create_chat_activity().expect("activity");
    let activity_id = activity.id.clone();
    let runtime_thread = std::thread::spawn(move || runtime.run_chat(activity, input));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let persisted = store.get(&activity_id).expect("persisted activity");
        if provider_log(&persisted.session, "provider.stdout") == Some("started") {
            let mut cancelled = persisted;
            cancelled.status = nitpick_agent_core::ActivityStatus::Error;
            cancelled.error = Some("cancelled by test".into());
            store.save(&cancelled).expect("save cancelled activity");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "provider did not start before cancellation"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let activity = runtime_thread
        .join()
        .expect("runtime thread")
        .expect("chat");
    assert_eq!(
        activity.error.as_deref(),
        Some("claude provider command cancelled")
    );
}

#[test]
fn command_provider_persists_review_logs_before_command_exits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf 'streamed stdout\\n'\nprintf 'streamed stderr\\n' >&2\nsleep 1\nmkdir -p .nitpick\nprintf '{\"comments\":[]}' > .nitpick/review-output.json\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store.clone());
    let input = ReviewInput {
        repo_dir,
        diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
        subject: ReviewSubject {
            repository: "acme/platform".into(),
            number: Some(42),
            ..ReviewSubject::default()
        },
        ..ReviewInput::default()
    };
    let activity = runtime.create_review_activity(&input).expect("activity");
    let activity_id = activity.id.clone();
    let runtime_thread = std::thread::spawn(move || runtime.run_review(activity, input));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let persisted = store.get(&activity_id).expect("persisted activity");
        let stdout = provider_log(&persisted.session, "provider.stdout");
        let stderr = provider_log(&persisted.session, "provider.stderr");
        if stdout == Some("streamed stdout") && stderr == Some("streamed stderr") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "provider logs were not persisted before command exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let activity = runtime_thread
        .join()
        .expect("runtime thread")
        .expect("review");
    assert_eq!(activity.error, None);
}

#[test]
fn command_provider_uses_configured_review_prompt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
    let command = dir.path().join("provider");
    let prompt_log = dir.path().join("prompt.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat > '{}'\nmkdir -p .nitpick\nprintf '{{\"comments\":[]}}' > .nitpick/review-output.json\n",
            prompt_log.display()
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
            repo_dir,
            review_prompt: "Custom prompt: write to {review_output_path}.".into(),
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("review runs");

    let prompt = fs::read_to_string(prompt_log).expect("prompt");
    assert!(prompt.starts_with("Custom prompt: write to .nitpick/review-output.json."));
}

#[test]
fn command_provider_rejects_review_without_diff_before_running_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let marker = dir.path().join("command-ran");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf ran > '{}'\nmkdir -p .nitpick\nprintf '{{\"comments\":[]}}' > .nitpick/review-output.json\n",
            marker.display()
        ),
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir,
            diff: " \n\t".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("activity saved");

    assert_eq!(
        activity.error.as_deref(),
        Some("review input missing diff; cannot run review")
    );
    assert!(!marker.exists(), "provider command should not run");
}

#[test]
fn command_provider_rejects_review_without_checkout_before_running_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("missing-repo");
    let command = dir.path().join("provider");
    let marker = dir.path().join("command-ran");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf ran > '{}'\nmkdir -p .nitpick\nprintf '{{\"comments\":[]}}' > .nitpick/review-output.json\n",
            marker.display()
        ),
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir: repo_dir.clone(),
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("activity saved");

    let expected_error = format!("review input checkout not found: {}", repo_dir.display());
    assert_eq!(activity.error.as_deref(), Some(expected_error.as_str()));
    assert!(!marker.exists(), "provider command should not run");
}

#[test]
fn command_provider_rejects_missing_review_output_json_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf old-style-summary\n",
    )
    .expect("write command");
    make_executable(&command);

    let provider = Arc::new(
        CommandAgentProvider::new(
            AgentProviderKind::Claude,
            Some("test-model".into()),
            &command,
        )
        .with_sandbox(CommandSandboxConfig::unsandboxed()),
    );
    let store = Arc::new(MemoryActivityStore::default());
    let runtime = AgentRuntime::new(provider, store);

    let activity = runtime
        .start_review(ReviewInput {
            repo_dir,
            diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
            subject: ReviewSubject {
                repository: "acme/platform".into(),
                number: Some(42),
                ..ReviewSubject::default()
            },
            ..ReviewInput::default()
        })
        .expect("activity saved");

    assert_eq!(
        activity.error.as_deref(),
        Some("review output file missing: .nitpick/review-output.json")
    );
}

#[test]
fn command_provider_review_with_tools_passes_mcp_config_and_skips_review_output_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    let prompt_log = dir.path().join("prompt.log");
    let mcp_config_path = dir.path().join("mcp.json");
    fs::write(&mcp_config_path, "{\"mcpServers\":{}}").expect("mcp config");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat > '{}'\n",
            args_log.display(),
            prompt_log.display()
        ),
    )
    .expect("write command");
    make_executable(&command);
    let provider = CommandAgentProvider::new(
        AgentProviderKind::Claude,
        Some("test-model".into()),
        &command,
    );
    let mut session = AgentSession {
        provider_session_id: Some("123e4567-e89b-12d3-a456-426614174000".into()),
        ..AgentSession::default()
    };

    let tools = ReviewToolConfig {
        mcp_config_path: mcp_config_path.clone(),
        instructions:
            "Use pull_request_context, pull_request_conversation_comments, existing_review_comments, add_review_comment, then finish_review."
                .into(),
    };
    let output = provider
        .review(
            &mut session,
            &ReviewInput {
                repo_dir: repo_dir.clone(),
                review_prompt:
                    "Custom prompt: use {review_output_path}.\n\nExtra configured guidance.".into(),
                instructions: "focus on correctness".into(),
                diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
                subject: ReviewSubject {
                    repository: "acme/platform".into(),
                    number: Some(42),
                    ..ReviewSubject::default()
                },
                ..ReviewInput::default()
            },
            ProviderReviewContext::new(&NoopProviderRunSink).with_tools(&tools),
        )
        .expect("review with tools");

    assert_eq!(output.comments, vec![]);
    assert!(!repo_dir.join(".nitpick/review-output.json").exists());
    assert_eq!(session.provider, Some(AgentProviderKind::Claude));
    let args = fs::read_to_string(args_log).expect("args");
    assert!(args.contains("--mcp-config"));
    assert!(args.contains(mcp_config_path.to_str().expect("utf-8 path")));
    let prompt = fs::read_to_string(prompt_log).expect("prompt");
    assert!(prompt.starts_with("Custom prompt: use the Nitpick review MCP tools."));
    assert!(prompt.contains("Extra configured guidance."));
    assert!(prompt.contains("pull_request_context"));
    assert!(prompt.contains("pull_request_conversation_comments"));
    assert!(prompt.contains("existing_review_comments"));
    assert!(prompt.contains("add_review_comment"));
    assert!(prompt.contains("finish_review"));
    assert!(prompt.contains("focus on correctness"));
    assert!(!prompt.contains(".nitpick/review-output.json"));
}

#[test]
fn codex_command_provider_review_with_tools_passes_mcp_server_config_overrides() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    let mcp_config_path = dir.path().join("mcp.json");
    fs::write(
        &mcp_config_path,
        serde_json::json!({
            "mcpServers": {
                "nitpick-review": {
                    "command": "/bin/nitpick-agent-host",
                    "args": ["review-mcp", "/tmp/nitpick-state.json"]
                }
            }
        })
        .to_string(),
    )
    .expect("mcp config");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\n",
            args_log.display(),
        ),
    )
    .expect("write command");
    make_executable(&command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Codex, None, &command);
    let mut session = AgentSession::default();

    let tools = ReviewToolConfig {
        mcp_config_path,
        instructions: "Use tools".into(),
    };
    provider
        .review(
            &mut session,
            &ReviewInput {
                repo_dir,
                diff: "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n".into(),
                ..ReviewInput::default()
            },
            ProviderReviewContext::new(&NoopProviderRunSink).with_tools(&tools),
        )
        .expect("review with tools");

    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "--dangerously-bypass-approvals-and-sandbox exec -c mcp_servers.nitpick-review.command=\"/bin/nitpick-agent-host\" -c mcp_servers.nitpick-review.args=[\"review-mcp\",\"/tmp/nitpick-state.json\"]\n"
    );
}

#[test]
fn command_provider_resolves_bare_provider_command_from_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let command = bin_dir.join("fake-provider");
    fs::write(&command, "#!/bin/sh\nexit 0\n").expect("write command");
    make_executable(&command);
    let old_path = env::var_os("PATH");
    let path = match old_path.as_deref() {
        Some(old_path) => {
            let mut paths = vec![bin_dir.clone()];
            paths.extend(env::split_paths(old_path));
            env::join_paths(paths).expect("join path")
        }
        None => bin_dir.clone().into_os_string(),
    };
    unsafe {
        env::set_var("PATH", &path);
    }

    let provider = CommandAgentProvider::new(AgentProviderKind::Claude, None, "fake-provider");
    let resolved = provider
        .resolved_command()
        .expect("provider command resolves from PATH");

    assert_eq!(resolved, command.canonicalize().expect("canonical command"));

    unsafe {
        match old_path {
            Some(old_path) => env::set_var("PATH", old_path),
            None => env::remove_var("PATH"),
        }
    }
}

#[test]
fn nono_sandboxed_provider_command_uses_current_executable_helper() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let provider_command = dir.path().join("provider");
    fs::write(&provider_command, "#!/bin/sh\n").expect("write provider command");
    make_executable(&provider_command);
    let helper_command = dir.path().join("nitpick");
    fs::write(&helper_command, "#!/bin/sh\n").expect("write helper command");
    make_executable(&helper_command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Claude, None, &provider_command)
        .with_sandbox(
            CommandSandboxConfig::nono()
                .with_helper_command(&helper_command)
                .without_nono_profile_updates(),
        );

    let command = provider
        .command_for_testing(Some(&repo_dir), &["--version".into()])
        .expect("command");

    assert_eq!(command.get_program(), helper_command.as_os_str());
    assert_eq!(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec![
            "__nitpick-nono-sandbox".to_owned(),
            "--".to_owned(),
            provider_command
                .canonicalize()
                .expect("canonical provider")
                .to_string_lossy()
                .into_owned(),
            "--version".to_owned(),
        ]
    );
    assert!(
        command
            .get_envs()
            .any(|(key, value)| { key == "NITPICK_NONO_SANDBOX_SPEC" && value.is_some() })
    );
}

#[test]
fn nono_sandbox_spec_allows_node_package_root_for_provider_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let package_root = dir.path().join("lib/node_modules/@openai/codex");
    let provider_bin = package_root.join("bin");
    fs::create_dir_all(&provider_bin).expect("provider bin dir");
    let provider_command = provider_bin.join("codex.js");
    fs::write(&provider_command, "#!/usr/bin/env node\n").expect("write provider command");
    make_executable(&provider_command);
    let helper_command = dir.path().join("nitpick");
    fs::write(&helper_command, "#!/bin/sh\n").expect("write helper command");
    make_executable(&helper_command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Codex, None, &provider_command)
        .with_sandbox(
            CommandSandboxConfig::nono()
                .with_helper_command(&helper_command)
                .without_nono_profile_updates(),
        );

    let command = provider
        .command_for_testing(Some(&repo_dir), &[])
        .expect("command");
    let spec = command
        .get_envs()
        .find_map(|(key, value)| {
            (key == "NITPICK_NONO_SANDBOX_SPEC").then(|| {
                value
                    .expect("spec env value")
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .expect("nono spec env");
    let spec: serde_json::Value = serde_json::from_str(&spec).expect("spec json");
    let read_paths = spec["read_paths"]
        .as_array()
        .expect("read paths")
        .iter()
        .map(|path| path.as_str().expect("path").to_owned())
        .collect::<Vec<_>>();

    assert!(
        read_paths.contains(
            &package_root
                .canonicalize()
                .expect("canonical package root")
                .to_string_lossy()
                .into_owned()
        )
    );
}

#[test]
fn validate_review_output_file_rejects_paths_that_escape_repo() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"comments\":[{\"path\":\"../outside.rs\",\"line\":1,\"body\":\"bad path\"}]}",
    )
    .expect("write output");

    let error = validate_review_output_file(&repo_dir, &output_path).expect_err("invalid path");

    assert_eq!(
        error.to_string(),
        "review comment path escapes repository: ../outside.rs"
    );
}

#[test]
fn validate_review_output_file_rejects_directory_comment_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::create_dir(repo_dir.join("src")).expect("source dir");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"comments\":[{\"path\":\"src\",\"line\":0,\"body\":\"directory note\"}]}",
    )
    .expect("write output");

    let error = validate_review_output_file(&repo_dir, &output_path).expect_err("directory path");

    assert_eq!(error.to_string(), "review comment path is not a file: src");
}

#[test]
fn validate_review_output_file_accepts_line_zero_for_file_in_diff_changeset() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\n").expect("repo file");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"comments\":[{\"path\":\"src.rs\",\"line\":0,\"body\":\"file-level note\"}]}",
    )
    .expect("write output");

    let output = validate_review_output_file_for_diff(
        &repo_dir,
        &output_path,
        "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -0,0 +1 @@\n+fn main() {}\n",
    )
    .expect("line zero is accepted for changed file");

    assert_eq!(output.comments[0].line, 0);
}

#[test]
fn validate_review_output_file_rejects_comment_line_outside_diff_changeset() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("src.rs"), "fn main() {}\nfn unchanged() {}\n").expect("repo file");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"comments\":[{\"path\":\"src.rs\",\"line\":2,\"body\":\"not changed\"}]}",
    )
    .expect("write output");

    let error = validate_review_output_file_for_diff(
        &repo_dir,
        &output_path,
        "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -1,2 +1,2 @@\n+fn main() {}\n fn unchanged() {}\n",
    )
    .expect_err("unchanged line rejected");

    assert_eq!(
        error.to_string(),
        "review comment line is outside the diff changeset: src.rs:2"
    );
}

#[test]
fn validate_review_output_file_uses_target_path_for_renamed_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    fs::write(repo_dir.join("new.rs"), "fn renamed() {}\n").expect("repo file");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"comments\":[{\"path\":\"new.rs\",\"line\":1,\"body\":\"renamed file note\"}]}",
    )
    .expect("write output");

    validate_review_output_file_for_diff(
        &repo_dir,
        &output_path,
        "diff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1 +1 @@\n+fn renamed() {}\n",
    )
    .expect("target path is accepted");
}

#[test]
fn claude_command_provider_resumes_existing_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            args_log.display()
        ),
    )
    .expect("write command");
    make_executable(&command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Claude, None, &command);
    let session = nitpick_agent_core::AgentSession {
        provider: Some(AgentProviderKind::Claude),
        provider_session_id: Some("github:acme/platform#42".into()),
        ..nitpick_agent_core::AgentSession::default()
    };

    provider.attach_session(&session).expect("attach");

    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "--resume github:acme/platform#42\n"
    );
}

#[test]
fn attach_requires_provider_session_id() {
    let provider = CommandAgentProvider::for_kind(AgentProviderKind::Claude, None);
    let error = provider
        .attach_session(&nitpick_agent_core::AgentSession::default())
        .expect_err("missing session id");

    assert_eq!(error.to_string(), "activity has no provider session id");
}

#[test]
fn codex_command_provider_resumes_existing_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            args_log.display()
        ),
    )
    .expect("write command");
    make_executable(&command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Codex, None, &command);
    let session = nitpick_agent_core::AgentSession {
        provider: Some(AgentProviderKind::Codex),
        provider_session_id: Some("github:acme/platform#42".into()),
        ..nitpick_agent_core::AgentSession::default()
    };

    provider.attach_session(&session).expect("attach");

    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "resume github:acme/platform#42\n"
    );
}

#[test]
fn attach_session_includes_stderr_from_failed_resume_command() {
    let dir = tempfile::tempdir().expect("temp dir");
    let command = dir.path().join("provider");
    fs::write(
        &command,
        "#!/bin/sh\nprintf 'session not found\\n' >&2\nexit 1\n",
    )
    .expect("write command");
    make_executable(&command);
    let provider = CommandAgentProvider::new(AgentProviderKind::Claude, None, &command);
    let session = nitpick_agent_core::AgentSession {
        provider: Some(AgentProviderKind::Claude),
        provider_session_id: Some("github:acme/platform#42".into()),
        ..nitpick_agent_core::AgentSession::default()
    };

    let error = provider.attach_session(&session).expect_err("resume fails");

    assert_eq!(
        error.to_string(),
        "claude provider command failed with status exit status: 1: session not found"
    );
}

fn make_executable(command: &std::path::Path) {
    let mut permissions = fs::metadata(command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(command, permissions).expect("chmod");
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .filter(|(index, _)| ![8, 13, 18, 23].contains(index))
            .all(|(_, byte)| byte.is_ascii_hexdigit())
}

fn provider_log<'a>(session: &'a nitpick_agent_core::AgentSession, role: &str) -> Option<&'a str> {
    session
        .messages
        .iter()
        .find(|message| message.role == role)
        .map(|message| message.content.as_str())
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

trait ReviewOutputExt {
    fn review_output(&self) -> Option<&nitpick_agent_core::ReviewOutput>;
}

impl ReviewOutputExt for nitpick_agent_core::ActivityOutput {
    fn review_output(&self) -> Option<&nitpick_agent_core::ReviewOutput> {
        match self {
            Self::Review(output) => Some(output),
            Self::Chat(_) => None,
        }
    }
}
