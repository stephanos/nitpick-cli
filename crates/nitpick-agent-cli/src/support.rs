use std::path::Path;

use nitpick_agent_core::{Activity, ActivityStore, FsActivityStore};
use nitpick_agent_github::{GitHubCliDiscovery, PullRequestRef};

use crate::CliOptions;

pub(crate) fn handle_resume_error(activity: &Activity, data_dir: &Path, error: String) -> String {
    if !provider_session_missing(&error) {
        return error;
    }
    let Some(session_id) = activity.session.provider_session_id.as_deref() else {
        return error;
    };
    let message = format!(
        "activity {} can no longer be resumed because provider session {} was not found; cleared the stored session id",
        activity.id, session_id
    );
    match clear_provider_session_id(data_dir, activity) {
        Ok(()) => message,
        Err(clear_error) => format!("{message} (failed to persist recovery: {clear_error})"),
    }
}

pub(crate) fn apply_sandbox_option(
    config: &mut nitpick_agent_host::AgentConfig,
    options: &CliOptions,
) {
    if options.disable_sandbox {
        config.sandbox = nitpick_agent_host::AgentSandboxConfig {
            mode: "none".into(),
        };
    }
}

pub(crate) fn require_cached_checkout(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
) -> Result<std::path::PathBuf, String> {
    let pull_request = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
    let checkout = configured_github_discovery(config).checkout_path_for(&pull_request);
    if !checkout.join(".git").is_dir() {
        return Err(format!("checkout not found for {target}"));
    }
    Ok(checkout)
}

fn configured_github_discovery(config: &nitpick_agent_host::AgentConfig) -> GitHubCliDiscovery {
    match &config.checkout_dir {
        Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            "git",
            checkout_dir,
        ),
        None => GitHubCliDiscovery::new(config.github_command.as_deref().unwrap_or("gh")),
    }
}

fn provider_session_missing(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("session not found")
        || error.contains("session does not exist")
        || error.contains("conversation not found")
}

fn clear_provider_session_id(data_dir: &Path, activity: &Activity) -> Result<(), String> {
    let store = FsActivityStore::new(data_dir).map_err(|error| error.to_string())?;
    let mut stored = store.get(&activity.id).map_err(|error| error.to_string())?;
    stored.session.provider_session_id = None;
    stored.touch();
    store.save(&stored).map_err(|error| error.to_string())
}
