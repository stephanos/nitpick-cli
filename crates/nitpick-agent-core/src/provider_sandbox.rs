use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    AgentError, AgentProviderKind, AgentResult, ReviewToolConfig, app_paths::default_data_dir,
    nono_profile::NonoProfileManager, nono_sandbox::NonoSandboxSpec,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSandboxConfig {
    pub enabled: bool,
    nono_helper_command: Option<PathBuf>,
    nono_profile_cache_dir: Option<PathBuf>,
    nono_profile_updates_enabled: bool,
    provider_runtime_dir: Option<PathBuf>,
    extra_read_paths: Vec<PathBuf>,
    extra_read_write_paths: Vec<PathBuf>,
}

impl CommandSandboxConfig {
    pub fn nono() -> Self {
        Self {
            enabled: true,
            nono_helper_command: None,
            nono_profile_cache_dir: None,
            nono_profile_updates_enabled: true,
            provider_runtime_dir: None,
            extra_read_paths: Vec::new(),
            extra_read_write_paths: Vec::new(),
        }
    }

    pub fn unsandboxed() -> Self {
        Self {
            enabled: false,
            nono_helper_command: None,
            nono_profile_cache_dir: None,
            nono_profile_updates_enabled: false,
            provider_runtime_dir: None,
            extra_read_paths: Vec::new(),
            extra_read_write_paths: Vec::new(),
        }
    }

    fn with_extra_read_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra_read_paths.extend(paths);
        self
    }

    pub fn with_read_paths(self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.with_extra_read_paths(paths)
    }

    pub fn with_helper_command(mut self, command: impl Into<PathBuf>) -> Self {
        self.nono_helper_command = Some(command.into());
        self
    }

    pub fn with_nono_profile_cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.nono_profile_cache_dir = Some(path.into());
        self
    }

    pub fn without_nono_profile_updates(mut self) -> Self {
        self.nono_profile_updates_enabled = false;
        self
    }

    pub(crate) fn with_extra_read_write_paths(
        mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.extra_read_write_paths.extend(paths);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_provider_runtime_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.provider_runtime_dir = Some(path.into());
        self
    }

    pub(crate) fn with_review_tool_paths(self, tools: &ReviewToolConfig) -> Self {
        let paths = review_tool_sandbox_paths(&tools.mcp_config_path);
        self.with_extra_read_paths(paths.read_paths)
            .with_extra_read_write_paths(paths.read_write_paths)
    }

    pub(crate) fn helper_command(&self) -> AgentResult<PathBuf> {
        match &self.nono_helper_command {
            Some(command) => Ok(command.clone()),
            None => std::env::current_exe().map_err(|error| {
                AgentError::sandbox(format!(
                    "resolve current executable for nono helper: {error}"
                ))
            }),
        }
    }
}

impl Default for CommandSandboxConfig {
    fn default() -> Self {
        Self::nono()
    }
}

pub(crate) struct ProviderSandboxPlan {
    pub(crate) env: Vec<(&'static str, PathBuf)>,
    pub(crate) spec: NonoSandboxSpec,
}

impl ProviderSandboxPlan {
    pub(crate) fn prepare(
        provider: &AgentProviderKind,
        repo_dir: &Path,
        provider_command: &Path,
        sandbox: &CommandSandboxConfig,
    ) -> AgentResult<Self> {
        let env = provider_runtime_env(provider, sandbox)?;
        let spec = nono_sandbox_spec(provider, repo_dir, provider_command, sandbox)?;
        Ok(Self { env, spec })
    }
}

fn provider_runtime_env(
    provider: &AgentProviderKind,
    sandbox: &CommandSandboxConfig,
) -> AgentResult<Vec<(&'static str, PathBuf)>> {
    let root = provider_runtime_root_dir_for_sandbox(sandbox);
    let tmp = root.join("tmp").join(provider.as_str());
    let env = match provider {
        AgentProviderKind::Claude => vec![("CLAUDE_CODE_TMPDIR", tmp.clone()), ("TMPDIR", tmp)],
        AgentProviderKind::Codex => vec![("TMPDIR", tmp)],
    };
    for (_, path) in &env {
        fs_err::create_dir_all(path).map_err(|error| {
            AgentError::sandbox(format!(
                "create provider runtime directory {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(env)
}

fn nono_sandbox_spec(
    provider: &AgentProviderKind,
    repo_dir: &Path,
    provider_command: &Path,
    sandbox: &CommandSandboxConfig,
) -> AgentResult<NonoSandboxSpec> {
    let mut read_paths = vec![repo_dir.to_path_buf(), provider_command.to_path_buf()];
    read_paths.extend(nono_system_read_paths());
    read_paths.extend(provider_dependency_read_paths(provider_command));
    read_paths.extend(provider_runtime_read_paths());
    read_paths.extend(provider_config_read_paths());
    read_paths.extend(sandbox.extra_read_paths.iter().cloned());

    let mut read_write_paths = provider_runtime_read_write_paths(sandbox);
    read_write_paths.extend(nono_system_read_write_paths());
    read_write_paths.extend(provider_config_read_write_paths());
    let provider_config_literal_read_write_paths = provider_config_literal_read_write_paths();
    read_write_paths.extend(provider_config_literal_read_write_paths.iter().cloned());
    read_write_paths.extend(sandbox.extra_read_write_paths.iter().cloned());

    let mut platform_rules =
        nono_literal_read_write_rules(&provider_config_literal_read_write_paths);
    if sandbox.nono_profile_updates_enabled {
        let profile_spec = NonoProfileManager::new(nono_profile_cache_dir(sandbox))
            .resolve_for_provider(provider, repo_dir, SystemTime::now())?;
        read_paths.extend(profile_spec.read_paths);
        read_write_paths.extend(profile_spec.read_write_paths);
        platform_rules.extend(profile_spec.platform_rules);
    }
    if platform_rules
        .iter()
        .any(|rule| rule == "(deny file-write-unlink)")
    {
        platform_rules.extend(nono_unlink_override_rules(&read_write_paths));
    }

    Ok(NonoSandboxSpec::new(
        read_paths,
        read_write_paths,
        platform_rules,
    ))
}

fn nono_system_read_paths() -> Vec<PathBuf> {
    [
        "/bin",
        "/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/share",
        "/lib",
        "/lib64",
        "/etc",
        "/private/etc",
        "/System",
        "/Library",
        "/Applications",
        "/dev",
        "/var",
        "/private/var",
        "/tmp",
        "/private/tmp",
        "/opt",
        "/run",
        "/nix",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.exists())
    .collect()
}

fn nono_system_read_write_paths() -> Vec<PathBuf> {
    ["/dev/null"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

fn provider_dependency_read_paths(provider_command: &Path) -> Vec<PathBuf> {
    provider_command
        .ancestors()
        .skip(1)
        .find(|ancestor| is_node_package_root(ancestor))
        .map(|path| vec![path.to_path_buf()])
        .unwrap_or_default()
}

fn is_node_package_root(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if parent.file_name().and_then(|name| name.to_str()) == Some("node_modules") {
        return true;
    }
    parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('@'))
        && parent.parent().and_then(|grandparent| {
            grandparent
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name == "node_modules")
        }) == Some(true)
}

fn nono_literal_read_write_rules(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            format!(
                r#"(allow file-read* file-write* (literal "{}"))"#,
                escape_nono_platform_rule_string(&path.to_string_lossy())
            )
        })
        .collect()
}

fn nono_unlink_override_rules(paths: &[PathBuf]) -> Vec<String> {
    let mut rules = paths
        .iter()
        .flat_map(|path| {
            let mut variants = vec![path.clone()];
            if let Ok(canonical) = path.canonicalize()
                && canonical != *path
            {
                variants.push(canonical);
            }
            variants.into_iter().map(|path| {
                let filter = if fs_err::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                    "subpath"
                } else {
                    "literal"
                };
                format!(
                    r#"(allow file-write-unlink ({} "{}"))"#,
                    filter,
                    escape_nono_platform_rule_string(&path.to_string_lossy())
                )
            })
        })
        .collect::<Vec<_>>();
    rules.sort();
    rules.dedup();
    rules
}

fn escape_nono_platform_rule_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

struct ReviewToolSandboxPaths {
    read_paths: Vec<PathBuf>,
    read_write_paths: Vec<PathBuf>,
}

fn review_tool_sandbox_paths(config_path: &Path) -> ReviewToolSandboxPaths {
    let config = fs_err::read(config_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    let server = &config["mcpServers"]["nitpick-review"];
    let mut read_paths = vec![config_path.to_path_buf()];
    let mut read_write_paths = Vec::new();
    if let Some(command) = server["command"].as_str() {
        read_paths.push(PathBuf::from(command));
    }
    if let Some(state_path) = server["args"]
        .as_array()
        .and_then(|args| args.iter().filter_map(|arg| arg.as_str()).nth(1))
        .map(PathBuf::from)
        && let Some(parent) = state_path.parent()
    {
        read_write_paths.push(parent.to_path_buf());
    }
    ReviewToolSandboxPaths {
        read_paths,
        read_write_paths,
    }
}

fn provider_runtime_read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [Path::new("/opt/homebrew"), Path::new("/usr/local")] {
        if path.exists() {
            paths.push(path.to_path_buf());
        }
    }
    paths
}

fn provider_runtime_read_write_paths(sandbox: &CommandSandboxConfig) -> Vec<PathBuf> {
    vec![provider_runtime_root_dir_for_sandbox(sandbox)]
}

fn provider_config_read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(home.join(".agents").join("skills"));
    }
    paths
}

fn provider_config_read_write_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.extend([
            home.join(".claude"),
            home.join(".codex"),
            home.join(".local").join("share").join("claude"),
            home.join(".local").join("state").join("claude"),
            home.join("Library")
                .join("Application Support")
                .join("Claude"),
            home.join("Library")
                .join("Application Support")
                .join("ClaudeCode"),
            home.join("Library").join("Caches").join("Claude"),
            home.join("Library")
                .join("Caches")
                .join("claude-cli-nodejs"),
        ]);
    }
    paths
}

fn provider_config_literal_read_write_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(home.join(".claude.json"));
        paths.push(home.join(".claude.lock"));
    }
    paths
}

fn provider_runtime_root_dir() -> PathBuf {
    default_data_dir().join("provider-runtime")
}

fn nono_profile_cache_dir(sandbox: &CommandSandboxConfig) -> PathBuf {
    sandbox
        .nono_profile_cache_dir
        .clone()
        .unwrap_or_else(|| default_data_dir().join("nono"))
}

fn provider_runtime_root_dir_for_sandbox(sandbox: &CommandSandboxConfig) -> PathBuf {
    sandbox
        .provider_runtime_dir
        .clone()
        .unwrap_or_else(provider_runtime_root_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nono_unlink_overrides_allow_read_write_dirs_to_remove_lock_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let read_write_dir = dir.path().join("nitpick-review-mcp");
        fs_err::create_dir(&read_write_dir).expect("read write dir");

        let rules = nono_unlink_override_rules(std::slice::from_ref(&read_write_dir));

        assert!(rules.contains(&format!(
            r#"(allow file-write-unlink (subpath "{}"))"#,
            read_write_dir.display()
        )));
        if let Ok(canonical) = read_write_dir.canonicalize()
            && canonical != read_write_dir
        {
            assert!(rules.contains(&format!(
                r#"(allow file-write-unlink (subpath "{}"))"#,
                canonical.display()
            )));
        }
    }

    #[test]
    fn nono_sandbox_spec_allows_dev_null_read_write() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("provider");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");

        let spec = nono_sandbox_spec(
            &AgentProviderKind::Codex,
            &repo_dir,
            &provider_command,
            &CommandSandboxConfig::nono().without_nono_profile_updates(),
        )
        .expect("spec");

        if Path::new("/dev/null").exists() {
            assert!(spec.read_write_paths.contains(&PathBuf::from("/dev/null")));
        }
    }
}
