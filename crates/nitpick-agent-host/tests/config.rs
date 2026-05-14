use std::sync::Arc;

use nitpick_agent_core::{ActivityKind, ActivityStore, AgentProviderKind, MemoryActivityStore};
use nitpick_agent_host::{
    AgentConfig, AgentSandboxConfig, GitHubDiscoveryConfig, HostDaemon, HostStatus,
};

#[test]
fn default_config_uses_claude_without_model_pin() {
    let config = AgentConfig::default();

    assert_eq!(config.provider, AgentProviderKind::Claude);
    assert_eq!(config.model, None);
}

#[test]
fn parses_agent_provider_and_model_from_toml() {
    let config = AgentConfig::from_toml(
        r#"
[agent]
provider = "codex"
model = "gpt-5.3-codex"
command = "/opt/bin/codex"
github_command = "/opt/bin/gh"
checkout_dir = "/var/tmp/nitpick-checkouts"
"#,
    )
    .expect("config parses");

    assert_eq!(config.provider, AgentProviderKind::Codex);
    assert_eq!(config.model.as_deref(), Some("gpt-5.3-codex"));
    assert_eq!(config.command.as_deref(), Some("/opt/bin/codex"));
    assert_eq!(config.github_command.as_deref(), Some("/opt/bin/gh"));
    assert_eq!(
        config.checkout_dir.as_deref(),
        Some("/var/tmp/nitpick-checkouts")
    );
    assert_eq!(config.sandbox, AgentSandboxConfig::default());
}

#[test]
fn parses_agent_sandbox_config_from_toml() {
    let config = AgentConfig::from_toml(
        r#"
[agent.sandbox]
mode = "none"
"#,
    )
    .expect("config parses");

    assert_eq!(
        config.sandbox,
        AgentSandboxConfig {
            mode: "none".into(),
        }
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
[github.discovery]
enabled = true
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
fn parses_github_discovery_config_from_sources_toml() {
    let config = AgentConfig::from_toml(
        r#"
[sources.github.discovery]
enabled = true
auto_review = true
interval_seconds = 120
allowlist = ["stephanos/*"]
denylist = ["*/archive-*"]
"#,
    )
    .expect("config parses");

    assert_eq!(
        config.github_discovery,
        GitHubDiscoveryConfig {
            enabled: true,
            auto_review: true,
            interval_seconds: 120,
            allowlist: vec!["stephanos/*".into()],
            denylist: vec!["*/archive-*".into()],
        }
    );
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
