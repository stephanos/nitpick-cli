use std::{path::Path, process::Command};

use nitpick_agent_core::{Activity, ActivityStore, FsActivityStore, ReviewInput};
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

pub(crate) fn ensure_cached_checkout(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
) -> Result<std::path::PathBuf, String> {
    ensure_cached_checkout_with_git_command(target, config, data_dir, Path::new("git"))
}

fn ensure_cached_checkout_with_git_command(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    git_command: &Path,
) -> Result<std::path::PathBuf, String> {
    let pull_request = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
    let discovery = configured_github_discovery_with_git_command(config, data_dir, git_command);
    let checkout = discovery.checkout_path_for(&pull_request);
    if checkout.join(".git").is_dir() {
        return Ok(checkout);
    }
    let review_input = discovery
        .review_input(&(&pull_request).into())
        .map_err(|error| error.to_string())?;
    Ok(review_input.repo_dir)
}

pub(crate) fn github_review_input(
    target: &str,
    config_path: &Path,
    data_dir: &Path,
) -> Result<ReviewInput, String> {
    let pull_request = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
    let config =
        nitpick_agent_host::AgentConfig::load_or_default(config_path).map_err(|error| {
            format!(
                "failed to load config {}: {error}",
                config_path.display()
            )
        })?;
    github_review_input_with_git_command(&pull_request, &config, data_dir, Path::new("git"))
}

fn github_review_input_with_git_command(
    pull_request: &PullRequestRef,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    git_command: &Path,
) -> Result<ReviewInput, String> {
    configured_github_discovery_with_git_command(config, data_dir, git_command)
        .review_input(&pull_request.into())
        .map_err(|error| error.to_string())
}

pub(crate) fn open_cached_checkout(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    editor: Option<&Path>,
) -> Result<String, String> {
    let checkout = ensure_cached_checkout(target, config, data_dir)?;
    open_checkout_with_editor(&checkout, editor)
}

fn open_checkout_with_editor(checkout: &Path, editor: Option<&Path>) -> Result<String, String> {
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

fn configured_github_discovery_with_git_command(
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    git_command: &Path,
) -> GitHubCliDiscovery {
    match &config.checkout_dir {
        Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            git_command,
            checkout_dir,
        ),
        None => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            git_command,
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
    fn open_cached_checkout_fetches_missing_checkout_before_opening_editor() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let gh = dir.path().join("gh");
        let git = dir.path().join("git");
        let editor = dir.path().join("editor");
        let log = dir.path().join("commands.log");
        std::fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
printf 'gh %s\n' "$*" >> '{}'
if [ "$1 $2" = "pr view" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
  exit 0
fi
if [ "$1 $2" = "pr diff" ]; then
  printf 'diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n'
  exit 0
fi
if [ "$1 $2" = "repo clone" ]; then
  mkdir -p "$4/.git"
  exit 0
fi
exit 1
"#,
                log.display()
            ),
        )
        .expect("gh");
        std::fs::write(
            &git,
            format!(
                "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> '{}'\nexit 0\n",
                log.display()
            ),
        )
        .expect("git");
        std::fs::write(
            &editor,
            format!(
                "#!/bin/sh\nprintf 'editor %s\\n' \"$1\" >> '{}'\n",
                log.display()
            ),
        )
        .expect("editor");
        make_executable(&gh);
        make_executable(&git);
        make_executable(&editor);
        let config = nitpick_agent_host::AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..nitpick_agent_host::AgentConfig::default()
        };
        let checkout = super::ensure_cached_checkout_with_git_command(
            "acme/platform#42",
            &config,
            &data_dir,
            &git,
        )
        .expect("ensure");
        let output =
            super::open_checkout_with_editor(&checkout, Some(editor.as_path())).expect("open");

        assert_eq!(output, format!("opened {}", checkout.display()));
        assert_eq!(
            std::fs::read_to_string(log).expect("log"),
            format!(
                "gh pr view 42 --repo acme/platform --json title,author,url,headRefOid,headRefName,state,mergedAt\n\
gh pr diff 42 --repo acme/platform\n\
gh repo clone acme/platform {} -- --quiet\n\
git -C {} fetch origin refs/pull/42/head --quiet\n\
git -C {} checkout -B feature/watcher FETCH_HEAD --quiet\n\
editor {}\n",
                checkout.display(),
                checkout.display(),
                checkout.display(),
                checkout.display()
            )
        );
    }

    #[test]
    fn ensure_cached_checkout_fetches_missing_checkout() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout = data_dir.join("checkouts/acme/platform/pr-42");
        let gh = dir.path().join("gh");
        let git = dir.path().join("git");
        let log = dir.path().join("commands.log");
        std::fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
printf 'gh %s\n' "$*" >> '{}'
if [ "$1 $2" = "pr view" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
  exit 0
fi
if [ "$1 $2" = "pr diff" ]; then
  printf 'diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n'
  exit 0
fi
if [ "$1 $2" = "repo clone" ]; then
  mkdir -p "$4/.git"
  exit 0
fi
exit 1
"#,
                log.display()
            ),
        )
        .expect("gh");
        std::fs::write(
            &git,
            format!(
                "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> '{}'\nexit 0\n",
                log.display()
            ),
        )
        .expect("git");
        make_executable(&gh);
        make_executable(&git);
        let config = nitpick_agent_host::AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..nitpick_agent_host::AgentConfig::default()
        };
        let ensured = super::ensure_cached_checkout_with_git_command(
            "acme/platform#42",
            &config,
            &data_dir,
            &git,
        )
        .expect("ensure");

        assert_eq!(ensured, checkout);
        assert_eq!(
            std::fs::read_to_string(log).expect("log"),
            format!(
                "gh pr view 42 --repo acme/platform --json title,author,url,headRefOid,headRefName,state,mergedAt\n\
gh pr diff 42 --repo acme/platform\n\
gh repo clone acme/platform {} -- --quiet\n\
git -C {} fetch origin refs/pull/42/head --quiet\n\
git -C {} checkout -B feature/watcher FETCH_HEAD --quiet\n",
                checkout.display(),
                checkout.display(),
                checkout.display()
            )
        );
    }

    #[test]
    fn github_review_input_uses_discovery_metadata_diff_and_checkout() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout = data_dir.join("checkouts/acme/platform/pr-42");
        let gh = dir.path().join("gh");
        let git = dir.path().join("git");
        let log = dir.path().join("commands.log");
        std::fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
printf 'gh %s\n' "$*" >> '{}'
if [ "$1 $2" = "pr view" ]; then
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
  exit 0
fi
if [ "$1 $2" = "pr diff" ]; then
  printf 'diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n'
  exit 0
fi
if [ "$1 $2" = "repo clone" ]; then
  mkdir -p "$4/.git"
  exit 0
fi
exit 1
"#,
                log.display()
            ),
        )
        .expect("gh");
        std::fs::write(
            &git,
            format!(
                "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> '{}'\nexit 0\n",
                log.display()
            ),
        )
        .expect("git");
        make_executable(&gh);
        make_executable(&git);
        let config = nitpick_agent_host::AgentConfig {
            github_command: Some(gh.display().to_string()),
            ..nitpick_agent_host::AgentConfig::default()
        };
        let pull_request = "https://github.com/acme/platform/pull/42"
            .parse()
            .expect("pull request ref");

        let input =
            super::github_review_input_with_git_command(&pull_request, &config, &data_dir, &git)
                .expect("review input");

        assert_eq!(input.repo_dir, checkout);
        assert_eq!(input.review_mode, nitpick_agent_core::ReviewMode::Requested);
        assert_eq!(input.subject.repository, "acme/platform");
        assert_eq!(input.subject.number, Some(42));
        assert_eq!(input.subject.title, "Add watcher");
        assert_eq!(input.subject.author, "stephan");
        assert_eq!(input.head_sha, "abc123");
        assert_eq!(input.diff, "diff --git a/src/lib.rs b/src/lib.rs\n+watcher\n");
    }

    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
}
