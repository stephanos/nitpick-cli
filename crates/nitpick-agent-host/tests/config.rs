use std::sync::Arc;

use nitpick_agent_core::{ActivityKind, ActivityStore, AgentProviderKind, MemoryActivityStore};
use nitpick_agent_host::{
    AgentConfig, AgentSandboxConfig, CONFIG_TEMPLATE, GitHubDiscoveryConfig, HostDaemon,
    HostStatus, REVIEW_PROMPT_TEMPLATE,
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
    assert_eq!(config.review_extra_instructions, "");
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
prompt_path = "prompts/review.md"
extra_instructions = "focus on correctness"

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
    assert_eq!(
        config.review_prompt_path,
        std::path::PathBuf::from("prompts/review.md")
    );
    assert_eq!(config.review_extra_instructions, "focus on correctness");
}

#[test]
fn load_resolves_relative_review_prompt_path_next_to_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("nested/config.toml");
    std::fs::create_dir_all(config_path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &config_path,
        r#"
[reviews]
prompt_path = "prompts/review.md"
"#,
    )
    .expect("write config");

    let config = AgentConfig::load(&config_path).expect("config");

    assert_eq!(
        config.review_prompt_path,
        dir.path().join("nested/prompts/review.md")
    );
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
            running_activity_count: 0,
            completed_activity_count: 0,
            error_activity_count: 0,
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
