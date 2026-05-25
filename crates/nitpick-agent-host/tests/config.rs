use std::sync::{Arc, Mutex};

use nitpick_agent_core::{
    ActivityKind, ActivityStore, AgentProvider, AgentProviderKind, AgentResult, AgentSession,
    ChatInput, HostStatus, MemoryActivityStore, ReviewInput, ReviewMode, ReviewOutput,
    ReviewRequest, ReviewSource,
};
use nitpick_agent_host::{
    AgentConfig, AgentSandboxConfig, CONFIG_TEMPLATE, GitHubDiscoveryConfig, HostDaemon,
    REVIEW_PROMPT_TEMPLATE,
};

#[test]
fn default_config_uses_claude_without_model_pin() {
    let config = AgentConfig::default();

    assert_eq!(config.provider, AgentProviderKind::Claude);
    assert_eq!(config.model, None);
    assert_eq!(config.github_discovery.interval_seconds, 60);
    assert_eq!(config.max_concurrent_reviews, 3);
    assert_eq!(
        config.review_prompt_path,
        std::path::PathBuf::from("review-prompt.md")
    );
    assert_eq!(config.review_extra_prompt_path, None);
    assert_eq!(config.review_self_extra_prompt_path, None);
    assert_eq!(config.review_requested_extra_prompt_path, None);
}

#[test]
fn config_template_parses() {
    let config = AgentConfig::from_toml(CONFIG_TEMPLATE).expect("template parses");

    assert_eq!(config.provider, AgentProviderKind::Claude);
    assert_eq!(config.github_discovery.interval_seconds, 60);
    assert_eq!(config.max_concurrent_reviews, 3);
}

#[test]
fn init_review_prompt_file_overwrites_with_template() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let prompt_path = dir.path().join("review-prompt.md");
    std::fs::write(&prompt_path, "old prompt").expect("write old prompt");

    let initialized_path =
        AgentConfig::init_review_prompt_file(&config_path).expect("init review prompt");

    assert_eq!(initialized_path, prompt_path);
    assert_eq!(
        std::fs::read_to_string(prompt_path).expect("review prompt"),
        REVIEW_PROMPT_TEMPLATE
    );
}

#[test]
fn init_template_file_creates_missing_config_with_template() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("nested/config.toml");

    AgentConfig::init_template_file(&path).expect("init template");

    assert_eq!(
        std::fs::read_to_string(path).expect("config"),
        CONFIG_TEMPLATE
    );
}

#[test]
fn init_template_file_does_not_overwrite_existing_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[agent]\nprovider = \"codex\"\n").expect("write config");

    AgentConfig::init_template_file(&path).expect("init template");

    assert_eq!(
        std::fs::read_to_string(path).expect("config"),
        "[agent]\nprovider = \"codex\"\n"
    );
}

#[test]
fn init_template_file_replaces_empty_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").expect("write empty config");

    AgentConfig::init_template_file(&path).expect("init template");

    assert_eq!(
        std::fs::read_to_string(path).expect("config"),
        CONFIG_TEMPLATE
    );
}

#[test]
fn write_config_example_file_creates_example_next_to_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");

    AgentConfig::write_config_example_file(&config_path).expect("write example");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("config.example.toml")).expect("example"),
        CONFIG_TEMPLATE
    );
}

#[test]
fn write_config_example_file_overwrites_existing_example() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let example_path = dir.path().join("config.example.toml");
    std::fs::write(&example_path, "old content").expect("write old example");

    AgentConfig::write_config_example_file(&config_path).expect("write example");

    assert_eq!(
        std::fs::read_to_string(example_path).expect("example"),
        CONFIG_TEMPLATE
    );
}

#[test]
fn write_config_example_file_does_not_touch_actual_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[agent]\nprovider = \"codex\"\n").expect("write config");

    AgentConfig::write_config_example_file(&config_path).expect("write example");

    assert_eq!(
        std::fs::read_to_string(&config_path).expect("config"),
        "[agent]\nprovider = \"codex\"\n"
    );
}

#[test]
fn parses_agent_provider_and_model_from_toml() {
    let config = AgentConfig::from_toml(
        r#"
[agent]
provider = "codex"
model = "gpt-5.3-codex"
command = "/opt/bin/codex"
sandbox = "none"

[reviews]
max_concurrent = 5

[github]
command = "/opt/bin/gh"
"#,
    )
    .expect("config parses");

    assert_eq!(config.provider, AgentProviderKind::Codex);
    assert_eq!(config.model.as_deref(), Some("gpt-5.3-codex"));
    assert_eq!(config.command.as_deref(), Some("/opt/bin/codex"));
    assert_eq!(config.github_command.as_deref(), Some("/opt/bin/gh"));
    assert_eq!(config.checkout_dir, None);
    assert_eq!(
        config.sandbox,
        AgentSandboxConfig {
            mode: "none".into(),
        }
    );
    assert_eq!(config.max_concurrent_reviews, 5);
    assert_eq!(config.review_extra_prompt_path, None);
    assert_eq!(config.review_self_extra_prompt_path, None);
    assert_eq!(config.review_requested_extra_prompt_path, None);
}

#[test]
fn load_rejects_relative_review_extra_prompt_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[reviews]
extra_prompt_path = "extra.md"
"#,
    )
    .expect("write config");

    let error = AgentConfig::load(&config_path).expect_err("config fails");

    assert!(
        error
            .to_string()
            .contains("review extra prompt path must be absolute")
    );
}

#[test]
fn load_accepts_absolute_review_extra_prompt_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let extra_prompt_path = dir.path().join("extra.md");
    std::fs::write(&extra_prompt_path, "Prefer correctness.").expect("write extra prompt");
    std::fs::write(
        &config_path,
        format!(
            r#"
[reviews]
extra_prompt_path = "{}"
"#,
            extra_prompt_path.display()
        ),
    )
    .expect("write config");

    let config = AgentConfig::load(&config_path).expect("config loads");

    assert_eq!(config.review_extra_prompt_path, Some(extra_prompt_path));
}

#[test]
fn load_accepts_absolute_review_mode_extra_prompt_paths() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let self_prompt_path = dir.path().join("self.md");
    let requested_prompt_path = dir.path().join("requested.md");
    std::fs::write(&self_prompt_path, "Self-review focus.").expect("write self prompt");
    std::fs::write(&requested_prompt_path, "Requested-review focus.")
        .expect("write requested prompt");
    std::fs::write(
        &config_path,
        format!(
            r#"
[reviews]
self_review_extra_prompt_path = "{}"
requested_review_extra_prompt_path = "{}"
"#,
            self_prompt_path.display(),
            requested_prompt_path.display()
        ),
    )
    .expect("write config");

    let config = AgentConfig::load(&config_path).expect("config loads");

    assert_eq!(config.review_self_extra_prompt_path, Some(self_prompt_path));
    assert_eq!(
        config.review_requested_extra_prompt_path,
        Some(requested_prompt_path)
    );
}

#[test]
fn load_rejects_relative_review_mode_extra_prompt_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[reviews]
self_review_extra_prompt_path = "self.md"
"#,
    )
    .expect("write config");

    let error = AgentConfig::load(&config_path).expect_err("config fails");

    assert!(
        error
            .to_string()
            .contains("self-review extra prompt path must be absolute")
    );
}

#[test]
fn load_rejects_missing_review_extra_prompt_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let extra_prompt_path = dir.path().join("missing.md");
    std::fs::write(
        &config_path,
        format!(
            r#"
[reviews]
extra_prompt_path = "{}"
"#,
            extra_prompt_path.display()
        ),
    )
    .expect("write config");

    let error = AgentConfig::load(&config_path).expect_err("config fails");

    assert!(
        error
            .to_string()
            .contains("review extra prompt path is not a file")
    );
}

#[test]
fn rejects_review_prompt_path_config() {
    let error = AgentConfig::from_toml(
        r#"
[reviews]
prompt_path = "review-prompt.md"
"#,
    )
    .expect_err("prompt_path is rejected");

    assert!(error.to_string().contains("prompt_path"));
}

#[test]
fn configured_review_extra_prompt_file_is_appended_to_review_prompt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_path = dir.path().join("review.md");
    let extra_prompt_path = dir.path().join("extra.md");
    std::fs::write(&prompt_path, "Base review prompt.").expect("write prompt");
    std::fs::write(&extra_prompt_path, "Prefer correctness over style.")
        .expect("write extra prompt");
    let config = AgentConfig {
        review_prompt_path: prompt_path,
        review_extra_prompt_path: Some(extra_prompt_path),
        ..AgentConfig::default()
    };
    let provider = Arc::new(RecordingReviewProvider::default());
    let daemon = HostDaemon::with_dependencies(
        Arc::new(MemoryActivityStore::default()),
        config,
        Arc::new(nitpick_agent_core::MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(EmptyReviewSource),
        Arc::new(nitpick_agent_core::SystemClock),
    );

    daemon.start_review(ReviewInput::default()).expect("review");

    let prompt = provider.review_prompt();
    assert!(prompt.contains("Base review prompt."));
    assert!(prompt.contains("Configured extra review prompt:"));
    assert!(prompt.contains("Prefer correctness over style."));
}

#[test]
fn configured_review_mode_extra_prompt_file_is_appended_for_matching_mode() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prompt_path = dir.path().join("review.md");
    let self_prompt_path = dir.path().join("self.md");
    let requested_prompt_path = dir.path().join("requested.md");
    std::fs::write(&prompt_path, "Base review prompt.").expect("write prompt");
    std::fs::write(&self_prompt_path, "Self-review focus.").expect("write self prompt");
    std::fs::write(&requested_prompt_path, "Requested-review focus.")
        .expect("write requested prompt");
    let provider = Arc::new(RecordingReviewProvider::default());
    let daemon = HostDaemon::with_dependencies(
        Arc::new(MemoryActivityStore::default()),
        AgentConfig {
            review_prompt_path: prompt_path,
            review_self_extra_prompt_path: Some(self_prompt_path),
            review_requested_extra_prompt_path: Some(requested_prompt_path),
            ..AgentConfig::default()
        },
        Arc::new(nitpick_agent_core::MemoryProcessedReviewStore::default()),
        provider.clone(),
        Arc::new(EmptyReviewSource),
        Arc::new(nitpick_agent_core::SystemClock),
    );

    daemon
        .start_review(ReviewInput {
            review_mode: ReviewMode::SelfReview,
            ..ReviewInput::default()
        })
        .expect("review");

    let prompt = provider.review_prompt();
    assert!(prompt.contains("Review mode: self-review."));
    assert!(prompt.contains("Configured self-review extra prompt:"));
    assert!(prompt.contains("Self-review focus."));
    assert!(!prompt.contains("Requested-review focus."));
}

#[test]
fn config_builds_command_provider_for_cli_session_resume() {
    let config = AgentConfig::from_toml(
        r#"
[agent]
provider = "claude"
model = "sonnet"
command = "/tmp/fake-claude"
"#,
    )
    .expect("config");

    let provider = config.command_provider();

    assert_eq!(provider.kind(), &AgentProviderKind::Claude);
    assert_eq!(provider.command().to_string_lossy(), "/tmp/fake-claude");
}

#[cfg(target_os = "macos")]
#[test]
fn command_provider_sandbox_includes_configured_prompt_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let prompt_path = dir.path().join("review-prompt.md");
    let extra_prompt_path = dir.path().join("extra.md");
    let self_prompt_path = dir.path().join("self.md");
    let requested_prompt_path = dir.path().join("requested.md");
    for path in [
        &prompt_path,
        &extra_prompt_path,
        &self_prompt_path,
        &requested_prompt_path,
    ] {
        std::fs::write(path, "prompt").expect("write prompt");
    }
    std::fs::write(
        &config_path,
        format!(
            r#"
[agent]
provider = "claude"
command = "/bin/sh"

[reviews]
extra_prompt_path = "{}"
self_review_extra_prompt_path = "{}"
requested_review_extra_prompt_path = "{}"
"#,
            extra_prompt_path.display(),
            self_prompt_path.display(),
            requested_prompt_path.display()
        ),
    )
    .expect("write config");
    let config = AgentConfig::load(&config_path).expect("config");

    let provider = config.command_provider();
    let profile = provider
        .macos_sandbox_profile_for_testing(dir.path(), std::path::Path::new("/bin/sh"))
        .expect("profile");

    for path in [
        &prompt_path,
        &extra_prompt_path,
        &self_prompt_path,
        &requested_prompt_path,
    ] {
        assert!(profile.contains(&format!(
            r#"(allow file-read* (literal "{}"))"#,
            path.canonicalize().expect("canonical prompt").display()
        )));
    }
}

#[test]
fn parses_github_discovery_config_from_toml() {
    let config = AgentConfig::from_toml(
        r#"
[github]
discovery = true
auto_review = true
interval_seconds = 60
allowlist = ["stephanos/*", "acme/platform"]
denylist = ["*/archive-*", "evil/*"]
"#,
    )
    .expect("config parses");

    assert_eq!(
        config.github_discovery,
        GitHubDiscoveryConfig {
            enabled: true,
            auto_review: true,
            interval_seconds: 60,
            allowlist: vec!["stephanos/*".into(), "acme/platform".into()],
            denylist: vec!["*/archive-*".into(), "evil/*".into()],
        }
    );
}

#[test]
fn rejects_double_star_github_patterns() {
    let error = AgentConfig::from_toml(
        r#"
[github]
allowlist = ["acme/**"]
"#,
    )
    .expect_err("double-star patterns are rejected");

    assert!(error.to_string().contains("use `*`, not `**`"));
}

#[test]
fn rejects_github_checkout_dir_config() {
    let error = AgentConfig::from_toml(
        r#"
[github]
checkout_dir = "/var/tmp/nitpick-checkouts"
"#,
    )
    .expect_err("checkout_dir is rejected");

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_legacy_nested_config() {
    let error = AgentConfig::from_toml(
        r#"
[sources.github.discovery]
enabled = true
"#,
    )
    .expect_err("legacy config is rejected");

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_legacy_agent_sandbox_table() {
    let error = AgentConfig::from_toml(
        r#"
[agent.sandbox]
mode = "none"
"#,
    )
    .expect_err("legacy config is rejected");

    assert!(error.to_string().contains("invalid type"));
}

#[test]
fn github_discovery_config_matches_allowlist_and_denylist_patterns() {
    let config = GitHubDiscoveryConfig {
        enabled: true,
        auto_review: true,
        interval_seconds: 300,
        allowlist: vec!["stephanos/*".into(), "acme/platform".into()],
        denylist: vec!["*/archive-*".into(), "evil/*".into()],
    };

    assert!(config.allows_repository("stephanos/nitpick-agent"));
    assert!(config.allows_repository("acme/platform"));
    assert!(!config.allows_repository("acme/other"));
    assert!(!config.allows_repository("stephanos/archive-old"));
    assert!(!config.allows_repository("evil/platform"));
}

#[test]
fn host_status_reports_configured_agent() {
    let store = Arc::new(MemoryActivityStore::default());
    store.create(ActivityKind::Review).expect("activity");
    let daemon = HostDaemon::with_config(
        store,
        AgentConfig {
            provider: AgentProviderKind::Codex,
            model: Some("gpt-5.3-codex".into()),
            command: None,
            github_command: None,
            ..AgentConfig::default()
        },
    );

    assert_eq!(
        daemon.status().expect("status"),
        HostStatus {
            activity_count: 1,
            queued_activity_count: 1,
            running_activity_count: 0,
            completed_activity_count: 0,
            error_activity_count: 0,
            open_review_count: 0,
            queued_review_count: 1,
            running_review_count: 0,
            completed_review_count: 0,
            error_review_count: 0,
            artifact_count: 0,
            local_only_artifact_count: 0,
            pending_sync_artifact_count: 0,
            provider: AgentProviderKind::Codex,
            model: Some("gpt-5.3-codex".into()),
            review_source_name: "github".into(),
            review_source_enabled: false,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
        }
    );
}

#[derive(Default)]
struct RecordingReviewProvider {
    review_prompt: Mutex<String>,
}

impl RecordingReviewProvider {
    fn review_prompt(&self) -> String {
        self.review_prompt.lock().expect("prompt lock").clone()
    }
}

impl AgentProvider for RecordingReviewProvider {
    fn review(
        &self,
        _session: &mut AgentSession,
        input: &ReviewInput,
    ) -> AgentResult<ReviewOutput> {
        *self.review_prompt.lock().expect("prompt lock") = input.review_prompt.clone();
        Ok(ReviewOutput::default())
    }

    fn chat(&self, _session: &mut AgentSession, _input: &ChatInput) -> AgentResult<String> {
        Ok(String::new())
    }
}

struct EmptyReviewSource;

impl ReviewSource for EmptyReviewSource {
    fn name(&self) -> &'static str {
        "empty"
    }

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>> {
        Ok(Vec::new())
    }

    fn review_input(&self, _request: &ReviewRequest) -> AgentResult<ReviewInput> {
        Ok(ReviewInput::default())
    }
}
