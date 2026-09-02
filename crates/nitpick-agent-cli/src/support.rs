use std::{ffi::OsString, path::Path, process::Command};

use nitpick_agent_core::{Activity, ActivityStore, FsActivityStore};
use nitpick_agent_github::{GitHubCliDiscovery, PullRequestRef};

use crate::CliOptions;

pub(crate) struct ReviewWorkspace<'a> {
    config: nitpick_agent_host::AgentConfig,
    data_dir: &'a Path,
    git_command: &'a Path,
    checkout_dir_env: Option<OsString>,
}

impl<'a> ReviewWorkspace<'a> {
    pub(crate) fn new(config: nitpick_agent_host::AgentConfig, data_dir: &'a Path) -> Self {
        Self::with_dependencies(
            config,
            data_dir,
            Path::new("git"),
            std::env::var_os("NITPICK_AGENT_CHECKOUT_DIR"),
        )
    }

    fn with_dependencies(
        config: nitpick_agent_host::AgentConfig,
        data_dir: &'a Path,
        git_command: &'a Path,
        checkout_dir_env: Option<OsString>,
    ) -> Self {
        Self {
            config,
            data_dir,
            git_command,
            checkout_dir_env,
        }
    }

    #[cfg(test)]
    fn with_git_command(
        config: nitpick_agent_host::AgentConfig,
        data_dir: &'a Path,
        git_command: &'a Path,
    ) -> Self {
        Self::with_dependencies(config, data_dir, git_command, None)
    }

    #[cfg(test)]
    fn with_checkout_dir_env(
        config: nitpick_agent_host::AgentConfig,
        data_dir: &'a Path,
        checkout_dir_env: Option<OsString>,
    ) -> Self {
        Self::with_dependencies(config, data_dir, Path::new("git"), checkout_dir_env)
    }

    pub(crate) fn checkout_path_for(&self, pull_request: &PullRequestRef) -> std::path::PathBuf {
        self.discovery().checkout_path_for(pull_request)
    }

    pub(crate) fn pull_request_for_path(
        &self,
        path: &Path,
    ) -> Result<Option<PullRequestRef>, String> {
        self.discovery()
            .pull_request_for_checkout_path(path)
            .map_err(|error| error.to_string())
    }

    fn ensure_checkout(&self, target: &str) -> Result<std::path::PathBuf, String> {
        let pull_request = target
            .parse::<PullRequestRef>()
            .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
        let checkout = self.checkout_path_for(&pull_request);
        if checkout.join(".git").is_dir() {
            return Ok(checkout);
        }
        self.discovery()
            .ensure_checkout_for(&pull_request)
            .map_err(|error| error.to_string())
    }

    fn discovery(&self) -> GitHubCliDiscovery {
        match &self.config.checkout_dir {
            Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
                self.config.github_command.as_deref().unwrap_or("gh"),
                self.git_command,
                checkout_dir,
            ),
            None => {
                let checkout_root = nitpick_agent_core::checkout_root_from_env_values(
                    self.checkout_dir_env.clone(),
                    Some(self.data_dir.as_os_str().to_os_string()),
                );
                GitHubCliDiscovery::with_checkout_commands(
                    self.config.github_command.as_deref().unwrap_or("gh"),
                    self.git_command,
                    checkout_root,
                )
            }
        }
    }
}

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
    ReviewWorkspace::new(config.clone(), data_dir).ensure_checkout(target)
}

#[cfg(test)]
fn ensure_cached_checkout_with_git_command(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
    data_dir: &Path,
    git_command: &Path,
) -> Result<std::path::PathBuf, String> {
    ReviewWorkspace::with_git_command(config.clone(), data_dir, git_command).ensure_checkout(target)
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
        .arg(checkout)
        .status()
        .map_err(|error| format!("failed to start editor `{}`: {error}", editor.display()))?;
    if !status.success() {
        return Err(format!("editor `{}` failed: {status}", editor.display()));
    }
    Ok(format!("opened {}", checkout.display()))
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
    fn review_workspace_uses_configured_checkout_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout_root = dir.path().join("configured-checkouts");
        let workspace = super::ReviewWorkspace::new(
            nitpick_agent_host::AgentConfig {
                checkout_dir: Some(checkout_root.display().to_string()),
                ..nitpick_agent_host::AgentConfig::default()
            },
            &data_dir,
        );
        let reference = "acme/platform#42".parse().expect("pull request ref");

        assert_eq!(
            workspace.checkout_path_for(&reference),
            checkout_root.join("acme/platform/pr-42")
        );
    }

    #[test]
    fn review_workspace_resolves_nested_configured_checkout_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout_root = dir.path().join("configured-checkouts");
        let checkout = checkout_root.join("acme/platform/pr-42");
        let nested = checkout.join("src/review");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        std::fs::create_dir_all(&nested).expect("nested checkout path");
        let workspace = super::ReviewWorkspace::with_checkout_dir_env(
            nitpick_agent_host::AgentConfig {
                checkout_dir: Some(checkout_root.display().to_string()),
                ..nitpick_agent_host::AgentConfig::default()
            },
            &data_dir,
            Some(dir.path().join("ignored-env-checkouts").into_os_string()),
        );

        assert_eq!(
            workspace
                .pull_request_for_path(&nested)
                .expect("pull request"),
            Some(nitpick_agent_github::PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            })
        );
    }

    #[test]
    fn review_workspace_resolves_registered_checkout_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout = data_dir.join("checkouts/acme/platform/pr-42");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        let workspace = super::ReviewWorkspace::with_checkout_dir_env(
            nitpick_agent_host::AgentConfig::default(),
            &data_dir,
            None,
        );

        assert_eq!(
            workspace
                .pull_request_for_path(&checkout)
                .expect("pull request"),
            Some(nitpick_agent_github::PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            })
        );
    }

    #[test]
    fn review_workspace_uses_checkout_dir_environment_value() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let checkout_root = dir.path().join("env-checkouts");
        let checkout = checkout_root.join("acme/platform/pr-42");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        let workspace = super::ReviewWorkspace::with_checkout_dir_env(
            nitpick_agent_host::AgentConfig::default(),
            &data_dir,
            Some(checkout_root.into_os_string()),
        );

        assert_eq!(
            workspace
                .pull_request_for_path(&checkout)
                .expect("pull request"),
            Some(nitpick_agent_github::PullRequestRef {
                owner: "acme".into(),
                repo: "platform".into(),
                number: 42,
            })
        );
    }

    #[test]
    fn review_workspace_ignores_paths_outside_registered_checkouts() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let lookalike = data_dir.join("checkouts/acme/platform/pr-42/src");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&lookalike).expect("lookalike");
        std::fs::create_dir_all(&outside).expect("outside");
        let workspace = super::ReviewWorkspace::with_checkout_dir_env(
            nitpick_agent_host::AgentConfig::default(),
            &data_dir,
            None,
        );

        assert_eq!(
            workspace
                .pull_request_for_path(&lookalike)
                .expect("lookalike result"),
            None
        );
        assert_eq!(
            workspace
                .pull_request_for_path(&outside)
                .expect("outside result"),
            None
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
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","body":"Please review the watcher changes.","baseRefOid":"base123","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
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
                "gh pr view 42 --repo acme/platform --json title,author,url,body,baseRefOid,headRefOid,headRefName,state,mergedAt\n\
gh repo clone acme/platform {} -- --quiet\n\
git -C {} fetch origin base123 refs/pull/42/head --quiet\n\
git -C {} checkout -B feature/watcher abc123 --quiet\n\
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
  printf '{{"title":"Add watcher","author":{{"login":"stephan"}},"url":"https://github.com/acme/platform/pull/42","body":"Please review the watcher changes.","baseRefOid":"base123","headRefOid":"abc123","headRefName":"feature/watcher","state":"OPEN","mergedAt":null}}'
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
                "gh pr view 42 --repo acme/platform --json title,author,url,body,baseRefOid,headRefOid,headRefName,state,mergedAt\n\
gh repo clone acme/platform {} -- --quiet\n\
git -C {} fetch origin base123 refs/pull/42/head --quiet\n\
git -C {} checkout -B feature/watcher abc123 --quiet\n",
                checkout.display(),
                checkout.display(),
                checkout.display()
            )
        );
    }

    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
}
