use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::{AgentError, AgentProviderKind, AgentResult, nono_sandbox::NonoSandboxSpec};

const DEFAULT_REGISTRY_URL: &str = "https://registry.nono.sh";
const LATEST_VERSION: &str = "latest";
pub(crate) const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NonoProviderProfile {
    namespace: &'static str,
    name: &'static str,
}

impl NonoProviderProfile {
    fn for_provider(provider: &AgentProviderKind) -> Self {
        match provider {
            AgentProviderKind::Claude => Self {
                namespace: "always-further",
                name: "claude",
            },
            AgentProviderKind::Codex => Self {
                namespace: "always-further",
                name: "codex",
            },
        }
    }

    fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NonoProfileManager<C = HttpNonoRegistryClient> {
    cache_dir: PathBuf,
    client: C,
    refresh_interval: Duration,
}

impl NonoProfileManager<HttpNonoRegistryClient> {
    pub(crate) fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            client: HttpNonoRegistryClient::new(DEFAULT_REGISTRY_URL),
            refresh_interval: PROFILE_REFRESH_INTERVAL,
        }
    }
}

impl<C: NonoRegistryClient> NonoProfileManager<C> {
    #[cfg(test)]
    fn with_client(cache_dir: impl Into<PathBuf>, client: C) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            client,
            refresh_interval: PROFILE_REFRESH_INTERVAL,
        }
    }

    pub(crate) fn resolve_for_provider(
        &self,
        provider: &AgentProviderKind,
        repo_dir: &Path,
        now: SystemTime,
    ) -> AgentResult<NonoSandboxSpec> {
        let package = NonoProviderProfile::for_provider(provider);
        let cached = self.read_cached_profile(&package)?;
        let profile = match cached {
            Some(cached)
                if !is_stale(
                    cached.metadata.last_checked_unix,
                    now,
                    self.refresh_interval,
                ) =>
            {
                cached.policy_json
            }
            Some(mut cached) => match self.pull_and_store(&package, now) {
                Ok(profile) => profile.policy_json,
                Err(error) => {
                    tracing::warn!(
                        package = %package.key(),
                        error = %error,
                        "using stale nono provider profile after refresh failed"
                    );
                    cached.metadata.last_checked_unix = unix_seconds(now)?;
                    self.write_cached_profile(&package, &cached.policy_json, &cached.metadata)?;
                    cached.policy_json
                }
            },
            None => self.pull_and_store(&package, now)?.policy_json,
        };

        resolve_profile_spec(&profile, repo_dir)
    }

    fn pull_and_store(
        &self,
        package: &NonoProviderProfile,
        now: SystemTime,
    ) -> AgentResult<CachedProfile> {
        let profile = self.client.pull_profile(package)?;
        let checked_at = unix_seconds(now)?;
        let metadata = CachedProfileMetadata {
            namespace: package.namespace.to_owned(),
            name: package.name.to_owned(),
            version: profile.version.clone(),
            policy_sha256: profile.policy_sha256.clone(),
            downloaded_unix: checked_at,
            last_checked_unix: checked_at,
        };
        self.write_cached_profile(package, &profile.policy_json, &metadata)?;
        Ok(CachedProfile {
            policy_json: profile.policy_json,
            metadata,
        })
    }

    fn package_dir(&self, package: &NonoProviderProfile) -> PathBuf {
        self.cache_dir
            .join("packages")
            .join(package.namespace)
            .join(package.name)
    }

    fn read_cached_profile(
        &self,
        package: &NonoProviderProfile,
    ) -> AgentResult<Option<CachedProfile>> {
        let package_dir = self.package_dir(package);
        let policy_path = package_dir.join("policy.json");
        let metadata_path = package_dir.join("metadata.json");
        if !policy_path.exists() || !metadata_path.exists() {
            return Ok(None);
        }
        let policy_json = fs::read_to_string(&policy_path)
            .map_err(|error| AgentError::io_path("read nono profile", &policy_path, error))?;
        let metadata_json = fs::read_to_string(&metadata_path).map_err(|error| {
            AgentError::io_path("read nono profile metadata", &metadata_path, error)
        })?;
        let metadata = serde_json::from_str(&metadata_json).map_err(|error| {
            AgentError::json(
                "parse nono profile metadata",
                metadata_path.display(),
                error,
            )
        })?;
        Ok(Some(CachedProfile {
            policy_json,
            metadata,
        }))
    }

    fn write_cached_profile(
        &self,
        package: &NonoProviderProfile,
        policy_json: &str,
        metadata: &CachedProfileMetadata,
    ) -> AgentResult<()> {
        let package_dir = self.package_dir(package);
        fs::create_dir_all(&package_dir).map_err(|error| {
            AgentError::io_path("create nono profile cache", &package_dir, error)
        })?;
        let policy_path = package_dir.join("policy.json");
        let metadata_path = package_dir.join("metadata.json");
        fs::write(&policy_path, policy_json)
            .map_err(|error| AgentError::io_path("write nono profile", &policy_path, error))?;
        let metadata_json = serde_json::to_vec_pretty(metadata).map_err(|error| {
            AgentError::sandbox(format!("serialize nono profile metadata: {error}"))
        })?;
        fs::write(&metadata_path, metadata_json).map_err(|error| {
            AgentError::io_path("write nono profile metadata", &metadata_path, error)
        })?;
        Ok(())
    }
}

fn is_stale(last_checked_unix: u64, now: SystemTime, refresh_interval: Duration) -> bool {
    let Ok(now_unix) = unix_seconds(now) else {
        return false;
    };
    now_unix.saturating_sub(last_checked_unix) >= refresh_interval.as_secs()
}

fn unix_seconds(time: SystemTime) -> AgentResult<u64> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            AgentError::sandbox(format!(
                "resolve current time for nono profile cache: {error}"
            ))
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedProfile {
    policy_json: String,
    metadata: CachedProfileMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CachedProfileMetadata {
    namespace: String,
    name: String,
    version: String,
    policy_sha256: String,
    downloaded_unix: u64,
    last_checked_unix: u64,
}

pub(crate) trait NonoRegistryClient {
    fn pull_profile(&self, package: &NonoProviderProfile) -> AgentResult<PulledProfile>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PulledProfile {
    version: String,
    policy_json: String,
    policy_sha256: String,
}

#[derive(Clone, Debug)]
pub(crate) struct HttpNonoRegistryClient {
    base_url: String,
    http: ureq::Agent,
}

impl HttpNonoRegistryClient {
    fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            http: ureq::Agent::new_with_defaults(),
        }
    }

    fn pull_response(&self, package: &NonoProviderProfile) -> AgentResult<PullResponse> {
        let path = format!(
            "/api/v1/packages/{}/{}/versions/{LATEST_VERSION}/pull",
            package.namespace, package.name
        );
        self.get_json(&path)
    }

    fn download_text(&self, url: &str) -> AgentResult<String> {
        let resolved_url = self.resolve_url(url);
        let mut response = self.http.get(&resolved_url).call().map_err(|error| {
            AgentError::sandbox(format!(
                "download nono registry artifact {resolved_url}: {error}"
            ))
        })?;
        response.body_mut().read_to_string().map_err(|error| {
            AgentError::sandbox(format!(
                "read nono registry artifact {resolved_url}: {error}"
            ))
        })
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> AgentResult<T> {
        let url = self.resolve_url(path);
        let mut response = self.http.get(&url).call().map_err(|error| {
            AgentError::sandbox(format!("fetch nono registry metadata {url}: {error}"))
        })?;
        let body = response.body_mut().read_to_string().map_err(|error| {
            AgentError::sandbox(format!("read nono registry metadata {url}: {error}"))
        })?;
        serde_json::from_str(&body).map_err(|error| {
            AgentError::sandbox(format!("parse nono registry metadata {url}: {error}"))
        })
    }

    fn resolve_url(&self, url: &str) -> String {
        if url.starts_with("http://") || url.starts_with("https://") {
            url.to_owned()
        } else {
            format!("{}{}", self.base_url, url)
        }
    }
}

impl NonoRegistryClient for HttpNonoRegistryClient {
    fn pull_profile(&self, package: &NonoProviderProfile) -> AgentResult<PulledProfile> {
        let pull = self.pull_response(package)?;
        validate_pull_response(package, &pull)?;
        let policy_artifact = pull
            .artifacts
            .iter()
            .find(|artifact| artifact.filename == "policy.json")
            .ok_or_else(|| {
                AgentError::sandbox(format!(
                    "nono package {} has no policy.json artifact",
                    package.key()
                ))
            })?;
        let bundle_json = self.download_text(&pull.bundle_url)?;
        verify_bundle_subjects(package, &bundle_json, &pull.artifacts)?;
        let policy_json = self.download_text(&policy_artifact.download_url)?;
        let digest = sha256_hex(policy_json.as_bytes());
        if digest != policy_artifact.sha256_digest {
            return Err(AgentError::sandbox(format!(
                "nono package {} policy.json digest mismatch: registry={}, local={digest}",
                package.key(),
                policy_artifact.sha256_digest
            )));
        }
        Ok(PulledProfile {
            version: pull.version,
            policy_json,
            policy_sha256: digest,
        })
    }
}

fn validate_pull_response(package: &NonoProviderProfile, pull: &PullResponse) -> AgentResult<()> {
    if pull.namespace != package.namespace || pull.name != package.name {
        return Err(AgentError::sandbox(format!(
            "registry returned {}/{} for requested nono package {}",
            pull.namespace,
            pull.name,
            package.key()
        )));
    }
    if !pull.scan_passed {
        return Err(AgentError::sandbox(format!(
            "nono package {} did not pass registry scan",
            package.key()
        )));
    }
    let mut filenames = BTreeSet::new();
    for artifact in &pull.artifacts {
        validate_relative_registry_path(&artifact.filename)?;
        if !filenames.insert(&artifact.filename) {
            return Err(AgentError::sandbox(format!(
                "nono package {} has duplicate artifact {}",
                package.key(),
                artifact.filename
            )));
        }
    }
    Ok(())
}

fn validate_relative_registry_path(path: &str) -> AgentResult<()> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AgentError::sandbox(format!(
            "nono registry artifact path must be relative and contained: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_bundle_subjects(
    package: &NonoProviderProfile,
    bundle_json: &str,
    artifacts: &[PullArtifact],
) -> AgentResult<()> {
    let bundle_path = Path::new(".nono-trust.bundle");
    let bundle = nono::trust::load_bundle_from_str(bundle_json, bundle_path)
        .map_err(|error| AgentError::sandbox(format!("load nono package trust bundle: {error}")))?;
    let subjects = nono::trust::extract_all_subjects(&bundle, bundle_path).map_err(|error| {
        AgentError::sandbox(format!("read nono package trust bundle subjects: {error}"))
    })?;
    let Some((_, first_digest)) = subjects.first() else {
        return Err(AgentError::sandbox(format!(
            "nono package {} trust bundle has no subjects",
            package.key()
        )));
    };
    let trusted_root = nono::trust::load_production_trusted_root()
        .map_err(|error| AgentError::sandbox(format!("load nono trust root: {error}")))?;
    nono::trust::verify_bundle_with_digest(
        first_digest,
        &bundle,
        &trusted_root,
        &nono::trust::VerificationPolicy::default(),
        bundle_path,
    )
    .map_err(|error| AgentError::sandbox(format!("verify nono package trust bundle: {error}")))?;
    let subject_digests = subjects
        .into_iter()
        .map(|(_, digest)| digest)
        .collect::<BTreeSet<_>>();
    for artifact in artifacts {
        if !subject_digests.contains(&artifact.sha256_digest) {
            return Err(AgentError::sandbox(format!(
                "nono package {} artifact {} digest not present in trust bundle",
                package.key(),
                artifact.filename
            )));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, Deserialize)]
struct PullResponse {
    namespace: String,
    name: String,
    version: String,
    artifacts: Vec<PullArtifact>,
    scan_passed: bool,
    bundle_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PullArtifact {
    filename: String,
    sha256_digest: String,
    download_url: String,
}

fn resolve_profile_spec(profile_json: &str, repo_dir: &Path) -> AgentResult<NonoSandboxSpec> {
    let profile = parse_profile(profile_json)?;
    let mut read_paths = Vec::new();
    let mut read_write_paths = Vec::new();
    let mut platform_rules = Vec::new();

    for group_name in &profile.groups.include {
        if let Some(group) = builtin_group(group_name) {
            read_paths.extend(expand_existing_static_paths(group.read)?);
            read_write_paths.extend(expand_existing_static_paths(group.read_write)?);
            platform_rules.extend(group.platform_rules.iter().map(|rule| (*rule).to_owned()));
        } else {
            tracing::warn!(group = group_name, "ignoring unknown nono profile group");
        }
    }

    read_write_paths.extend(expand_existing_paths(&profile.filesystem.allow)?);
    read_paths.extend(expand_existing_paths(&profile.filesystem.read)?);
    read_write_paths.extend(expand_profile_paths(&profile.filesystem.allow_file)?);
    read_paths.extend(expand_profile_paths(&profile.filesystem.read_file)?);

    match profile.workdir.access {
        WorkdirAccess::Read => read_paths.push(repo_dir.to_path_buf()),
        WorkdirAccess::ReadWrite => read_write_paths.push(repo_dir.to_path_buf()),
        WorkdirAccess::None | WorkdirAccess::Write => {}
    }

    dedup_paths(&mut read_paths);
    dedup_paths(&mut read_write_paths);
    dedup_strings(&mut platform_rules);

    Ok(NonoSandboxSpec::new(
        read_paths,
        read_write_paths,
        platform_rules,
    ))
}

fn parse_profile(profile_json: &str) -> AgentResult<RegistryProfile> {
    serde_json::from_str(profile_json)
        .map_err(|error| AgentError::sandbox(format!("parse nono provider profile: {error}")))
}

fn expand_existing_paths(paths: &[String]) -> AgentResult<Vec<PathBuf>> {
    Ok(expand_profile_paths(paths)?
        .into_iter()
        .filter(|path| path.exists())
        .collect())
}

fn expand_existing_static_paths(paths: &[&str]) -> AgentResult<Vec<PathBuf>> {
    Ok(paths
        .iter()
        .map(|path| expand_profile_path(path))
        .collect::<AgentResult<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.exists())
        .collect())
}

fn expand_profile_paths(paths: &[String]) -> AgentResult<Vec<PathBuf>> {
    paths.iter().map(|path| expand_profile_path(path)).collect()
}

fn expand_profile_path(path: &str) -> AgentResult<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_owned());
    let uid = std::env::var("UID").unwrap_or_else(|_| current_uid().unwrap_or_default());
    let xdg_config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
    let xdg_data =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let xdg_state =
        std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{home}/.local/state"));
    let xdg_cache = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{home}/.cache"));

    let expanded = path
        .strip_prefix("~/")
        .map(|rest| format!("{home}/{rest}"))
        .unwrap_or_else(|| path.to_owned())
        .replace("$HOME", &home)
        .replace("$TMPDIR", tmpdir.trim_end_matches('/'))
        .replace("$UID", &uid)
        .replace("$XDG_CONFIG_HOME", &xdg_config)
        .replace("$XDG_DATA_HOME", &xdg_data)
        .replace("$XDG_STATE_HOME", &xdg_state)
        .replace("$XDG_CACHE_HOME", &xdg_cache);
    Ok(PathBuf::from(expanded))
}

#[cfg(unix)]
fn current_uid() -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let home = std::env::var("HOME").ok()?;
    Some(fs::metadata(home).ok()?.uid().to_string())
}

#[cfg(not(unix))]
fn current_uid() -> Option<String> {
    None
}

fn dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn dedup_strings(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryProfile {
    #[serde(default)]
    groups: ProfileGroups,
    #[serde(default)]
    filesystem: ProfileFilesystem,
    #[serde(default)]
    workdir: ProfileWorkdir,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileGroups {
    #[serde(default, deserialize_with = "deserialize_conditional_name_vec")]
    include: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileFilesystem {
    #[serde(default, deserialize_with = "deserialize_conditional_path_vec")]
    allow: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_conditional_path_vec")]
    read: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_conditional_path_vec")]
    allow_file: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_conditional_path_vec")]
    read_file: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileWorkdir {
    #[serde(default)]
    access: WorkdirAccess,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WorkdirAccess {
    #[default]
    None,
    Read,
    #[serde(rename = "readwrite")]
    ReadWrite,
    Write,
}

fn deserialize_conditional_name_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_conditional_string_vec(deserializer, "name")
}

fn deserialize_conditional_path_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_conditional_string_vec(deserializer, "path")
}

fn deserialize_conditional_string_vec<'de, D>(
    deserializer: D,
    value_key: &'static str,
) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut result = Vec::new();
    for value in values {
        match value {
            serde_json::Value::String(item) => result.push(item),
            serde_json::Value::Object(mut object) => {
                let Some(item) = object
                    .remove(value_key)
                    .and_then(|value| value.as_str().map(ToOwned::to_owned))
                else {
                    return Err(serde::de::Error::custom(format!(
                        "conditional entry is missing string field {value_key}"
                    )));
                };
                let when = object
                    .remove("when")
                    .and_then(|value| value.as_str().map(ToOwned::to_owned));
                if when.as_deref().is_none_or(when_matches_current) {
                    result.push(item);
                }
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "conditional entry must be a string or object",
                ));
            }
        }
    }
    Ok(result)
}

fn when_matches_current(when: &str) -> bool {
    match when {
        "macos" => cfg!(target_os = "macos"),
        "linux" => cfg!(target_os = "linux"),
        "unix" => cfg!(unix),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BuiltinGroup {
    read: &'static [&'static str],
    read_write: &'static [&'static str],
    platform_rules: &'static [&'static str],
}

fn builtin_group(name: &str) -> Option<BuiltinGroup> {
    let groups = BTreeMap::from([
        (
            "codex_macos",
            BuiltinGroup {
                read: &[],
                read_write: &[
                    "$HOME/Library/Keychains/login.keychain-db",
                    "$HOME/Library/Keychains/metadata.keychain-db",
                ],
                platform_rules: &[],
            },
        ),
        (
            "claude_code_macos",
            BuiltinGroup {
                read: &[
                    "$HOME/.local/share/claude",
                    "$HOME/Applications/Claude Code URL Handler.app",
                ],
                read_write: &[
                    "$HOME/Library/Keychains",
                    "$HOME/Library/Keychains/login.keychain-db",
                    "$HOME/Library/Keychains/metadata.keychain-db",
                ],
                platform_rules: &[],
            },
        ),
        (
            "user_caches_macos",
            BuiltinGroup {
                read: &["~/Library/Preferences"],
                read_write: &["~/Library/Caches", "~/Library/Logs"],
                platform_rules: &[],
            },
        ),
        (
            "node_runtime",
            BuiltinGroup {
                read: &[
                    "~/.nvm",
                    "~/.fnm",
                    "~/.npm",
                    "~/.node",
                    "~/.local/share/fnm",
                    "/usr/local/lib/node_modules",
                    "~/Library/pnpm",
                    "~/.local/share/pnpm",
                ],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "rust_runtime",
            BuiltinGroup {
                read: &["~/.cargo", "~/.rustup"],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "python_runtime",
            BuiltinGroup {
                read: &["~/.pyenv", "~/.local/lib", "~/.local/share/uv", "~/.conda"],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "linux_sysfs_read",
            BuiltinGroup {
                read: &["/sys"],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "nix_runtime",
            BuiltinGroup {
                read: &[
                    "~/.nix-profile",
                    "~/.local/state/nix/profile",
                    "~/.local/state/nix/profiles",
                    "~/.nix-defexpr",
                    "~/.local/state/nix/defexpr",
                    "/run/current-system/sw",
                    "/etc/profiles/per-user",
                    "/nix/var/nix/profiles",
                    "/nix/store",
                ],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "git_config",
            BuiltinGroup {
                read: &[
                    "$HOME/.gitconfig",
                    "$HOME/.gitignore_global",
                    "$HOME/.config/git/config",
                    "$HOME/.config/git/ignore",
                    "$HOME/.config/git/attributes",
                ],
                read_write: &[],
                platform_rules: &[],
            },
        ),
        (
            "vscode_macos",
            BuiltinGroup {
                read: &[],
                read_write: &["$HOME/.vscode", "$HOME/Library/Application Support/Code"],
                platform_rules: &[],
            },
        ),
        (
            "unlink_protection",
            BuiltinGroup {
                read: &[],
                read_write: &[],
                platform_rules: &["(deny file-write-unlink)"],
            },
        ),
    ]);
    let group = groups.get(name).copied()?;
    if name.ends_with("_macos") && !cfg!(target_os = "macos") {
        return None;
    }
    if name.ends_with("_linux") && !cfg!(target_os = "linux") {
        return None;
    }
    Some(group)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[derive(Clone, Debug)]
    struct FakeRegistryClient {
        calls: Rc<RefCell<usize>>,
        result: AgentResult<PulledProfile>,
    }

    impl NonoRegistryClient for FakeRegistryClient {
        fn pull_profile(&self, _package: &NonoProviderProfile) -> AgentResult<PulledProfile> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    fn profile_json() -> String {
        serde_json::json!({
            "groups": { "include": ["git_config"] },
            "filesystem": {
                "allow": ["$HOME/.codex"],
                "read": [{ "path": "/bin", "when": if cfg!(target_os = "macos") { "macos" } else { "linux" } }]
            },
            "workdir": { "access": "readwrite" }
        })
        .to_string()
    }

    fn pulled_profile(version: &str) -> PulledProfile {
        let policy_json = profile_json();
        PulledProfile {
            version: version.to_owned(),
            policy_sha256: sha256_hex(policy_json.as_bytes()),
            policy_json,
        }
    }

    #[test]
    fn missing_cache_pulls_and_writes_profile() {
        let dir = tempfile::tempdir().expect("temp dir");
        let calls = Rc::new(RefCell::new(0));
        let client = FakeRegistryClient {
            calls: calls.clone(),
            result: Ok(pulled_profile("1.0.0")),
        };
        let manager = NonoProfileManager::with_client(dir.path(), client);

        let spec = manager
            .resolve_for_provider(
                &AgentProviderKind::Codex,
                dir.path(),
                UNIX_EPOCH + Duration::from_secs(100),
            )
            .expect("resolve profile");

        assert_eq!(*calls.borrow(), 1);
        assert!(
            dir.path()
                .join("packages/always-further/codex/policy.json")
                .is_file()
        );
        assert!(spec.read_write_paths.contains(&dir.path().to_path_buf()));
    }

    #[test]
    fn fresh_cache_does_not_pull() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manager = NonoProfileManager::with_client(
            dir.path(),
            FakeRegistryClient {
                calls: Rc::new(RefCell::new(0)),
                result: Ok(pulled_profile("1.0.0")),
            },
        );
        manager
            .pull_and_store(
                &NonoProviderProfile::for_provider(&AgentProviderKind::Claude),
                UNIX_EPOCH + Duration::from_secs(100),
            )
            .expect("seed cache");
        let calls = Rc::new(RefCell::new(0));
        let manager = NonoProfileManager::with_client(
            dir.path(),
            FakeRegistryClient {
                calls: calls.clone(),
                result: Err(AgentError::sandbox("network down")),
            },
        );

        manager
            .resolve_for_provider(
                &AgentProviderKind::Claude,
                dir.path(),
                UNIX_EPOCH + Duration::from_secs(100 + 60),
            )
            .expect("resolve profile");

        assert_eq!(*calls.borrow(), 0);
    }

    #[test]
    fn stale_cache_uses_cached_profile_when_refresh_fails() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manager = NonoProfileManager::with_client(
            dir.path(),
            FakeRegistryClient {
                calls: Rc::new(RefCell::new(0)),
                result: Ok(pulled_profile("1.0.0")),
            },
        );
        manager
            .pull_and_store(
                &NonoProviderProfile::for_provider(&AgentProviderKind::Codex),
                UNIX_EPOCH + Duration::from_secs(100),
            )
            .expect("seed cache");
        let calls = Rc::new(RefCell::new(0));
        let manager = NonoProfileManager::with_client(
            dir.path(),
            FakeRegistryClient {
                calls: calls.clone(),
                result: Err(AgentError::sandbox("network down")),
            },
        );

        let spec = manager
            .resolve_for_provider(
                &AgentProviderKind::Codex,
                dir.path(),
                UNIX_EPOCH + Duration::from_secs(100 + PROFILE_REFRESH_INTERVAL.as_secs() + 1),
            )
            .expect("resolve stale profile");

        assert_eq!(*calls.borrow(), 1);
        assert!(spec.read_write_paths.contains(&dir.path().to_path_buf()));
        let metadata_json = fs::read_to_string(
            dir.path()
                .join("packages/always-further/codex/metadata.json"),
        )
        .expect("metadata");
        let metadata: CachedProfileMetadata =
            serde_json::from_str(&metadata_json).expect("metadata json");
        assert_eq!(
            metadata.last_checked_unix,
            100 + PROFILE_REFRESH_INTERVAL.as_secs() + 1
        );
    }

    #[test]
    fn provider_profiles_map_to_registry_packages() {
        assert_eq!(
            NonoProviderProfile::for_provider(&AgentProviderKind::Codex).key(),
            "always-further/codex"
        );
        assert_eq!(
            NonoProviderProfile::for_provider(&AgentProviderKind::Claude).key(),
            "always-further/claude"
        );
    }
}
