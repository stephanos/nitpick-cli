use std::{path::Path, process::Command};

use nitpick_agent_core::{Activity, ActivityStore, FsActivityStore};
use nitpick_agent_github::{GitHubCliDiscovery, PullRequestRef};

use crate::CliOptions;

pub(crate) fn handle_resume_error(activity: &Activity, data_dir: &Path, error: String) -> String {
    if !provider_session_missing(&error) {
        return error;
    }
    if activity.session.provider_session_id.is_none() {
        return error;
    }
    let message = format!(
        "activity {} can no longer be resumed because its provider session was not found; cleared the stored session",
        activity.id
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
    data_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    let pull_request = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
    let checkout = configured_github_discovery(config, data_dir).checkout_path_for(&pull_request);
    if !checkout.join(".git").is_dir() {
        return Err(format!("checkout not found for {target}"));
    }
    Ok(checkout)
}

pub(crate) fn open_cached_checkout(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    editor: Option<&Path>,
) -> Result<String, String> {
    let checkout = require_cached_checkout(target, config, data_dir)?;
    let editor = editor
        .map(std::path::PathBuf::from)
        .or_else(editor_from_env)
        .ok_or_else(|| "set VISUAL or EDITOR to open review checkouts".to_owned())?;
    let status = Command::new(&editor)
        .arg(&checkout)
        .status()
        .map_err(|error| format!("failed to start editor `{}`: {error}", editor.display()))?;
    if !status.success() {
        return Err(format!("editor `{}` failed: {status}", editor.display()));
    }
    Ok(format!("opened {}", checkout.display()))
}

fn configured_github_discovery(
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
) -> GitHubCliDiscovery {
    match &config.checkout_dir {
        Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            "git",
            checkout_dir,
        ),
        None => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            "git",
            data_dir.join("checkouts"),
        ),
    }
}

fn editor_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("VISUAL")
        .or_else(|| std::env::var_os("EDITOR"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            if cfg!(target_os = "macos") {
                Some(std::path::PathBuf::from("open"))
            } else {
                None
            }
        })
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

#[cfg(test)]
mod tests {
    #[test]
    fn open_cached_checkout_opens_existing_checkout_with_editor() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout = data_dir.join("checkouts/acme/platform/pr-42");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        let editor = dir.path().join("editor");
        let log = dir.path().join("editor.log");
        std::fs::write(
            &editor,
            format!("#!/bin/sh\nprintf '%s\\n' \"$1\" > '{}'\n", log.display()),
        )
        .expect("editor");
        make_executable(&editor);

        let output = super::open_cached_checkout(
            "https://github.com/acme/platform/pull/42",
            &nitpick_agent_host::AgentConfig::default(),
            &data_dir,
            Some(editor.as_path()),
        )
        .expect("open");

        assert_eq!(output, format!("opened {}", checkout.display()));
        assert_eq!(
            std::fs::read_to_string(log).expect("log"),
            format!("{}\n", checkout.display())
        );
    }

    #[test]
    fn open_cached_checkout_reports_missing_checkout() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let editor = dir.path().join("editor");

        let error = super::open_cached_checkout(
            "acme/platform#42",
            &nitpick_agent_host::AgentConfig::default(),
            &data_dir,
            Some(&editor),
        )
        .expect_err("missing checkout");

        assert_eq!(error, "checkout not found for acme/platform#42");
    }

    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
}
