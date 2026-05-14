use std::{env, fs, os::unix::fs::PermissionsExt, sync::Arc};

use nitpick_agent_core::{
    AgentProvider, AgentProviderKind, AgentRuntime, ChatInput, CommandAgentProvider,
    CommandSandboxConfig, MemoryActivityStore, ReviewInput, ReviewSubject,
    validate_review_output_file, validate_review_output_file_for_diff,
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
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let command = dir.path().join("provider");
    let args_log = dir.path().join("args.log");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nmkdir -p .nitpick\nprintf '{{\"summary\":\"review-response\",\"comments\":[],\"journey\":{{\"summary\":\"done\",\"steps\":[]}}}}' > .nitpick/review-output.json\n",
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

    assert_eq!(
        fs::read_to_string(args_log).expect("args"),
        "--session-id github:acme/platform#42\n"
    );
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
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncat >/dev/null\nmkdir -p .nitpick\nprintf '{{\"summary\":\"review-response\",\"comments\":[],\"journey\":{{\"summary\":\"done\",\"steps\":[]}}}}' > .nitpick/review-output.json\n",
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

    assert_eq!(fs::read_to_string(args_log).expect("args"), "\n");
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
        "#!/bin/sh\ncat >/dev/null\nmkdir -p .nitpick\ncat > .nitpick/review-output.json <<'JSON'\n{\"summary\":\"review summary\",\"comments\":[{\"path\":\"src.rs\",\"line\":1,\"body\":\"use a clearer name\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}\nJSON\nprintf ignored-stdout\n",
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
    assert_eq!(output.summary, "review summary");
    assert_eq!(output.comments.len(), 1);
    assert_eq!(output.comments[0].path, "src.rs");
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
fn validate_review_output_file_rejects_paths_that_escape_repo() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo_dir = dir.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    let output_path = repo_dir.join(".nitpick-review.json");
    fs::write(
        &output_path,
        "{\"summary\":\"review summary\",\"comments\":[{\"path\":\"../outside.rs\",\"line\":1,\"body\":\"bad path\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}",
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
        "{\"summary\":\"review summary\",\"comments\":[{\"path\":\"src\",\"line\":0,\"body\":\"directory note\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}",
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
        "{\"summary\":\"review summary\",\"comments\":[{\"path\":\"src.rs\",\"line\":0,\"body\":\"file-level note\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}",
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
        "{\"summary\":\"review summary\",\"comments\":[{\"path\":\"src.rs\",\"line\":2,\"body\":\"not changed\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}",
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
        "{\"summary\":\"review summary\",\"comments\":[{\"path\":\"new.rs\",\"line\":1,\"body\":\"renamed file note\"}],\"journey\":{\"summary\":\"checked diff\",\"steps\":[]}}",
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
