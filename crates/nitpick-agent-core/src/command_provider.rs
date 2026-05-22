use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use crate::{
    AgentError, AgentProvider, AgentProviderKind, AgentResult, AgentSession, ChatInput,
    REVIEW_OUTPUT_RELATIVE_PATH, ReviewInput, ReviewOutput, ReviewToolConfig,
    validate_review_output_file_for_diff,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSandboxConfig {
    pub enabled: bool,
    extra_read_paths: Vec<PathBuf>,
    extra_read_write_paths: Vec<PathBuf>,
}

impl CommandSandboxConfig {
    pub fn macos_seatbelt() -> Self {
        Self {
            enabled: true,
            extra_read_paths: Vec::new(),
            extra_read_write_paths: Vec::new(),
        }
    }

    pub fn unsandboxed() -> Self {
        Self {
            enabled: false,
            extra_read_paths: Vec::new(),
            extra_read_write_paths: Vec::new(),
        }
    }

    fn with_extra_read_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra_read_paths.extend(paths);
        self
    }

    fn with_extra_read_write_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra_read_write_paths.extend(paths);
        self
    }
}

impl Default for CommandSandboxConfig {
    fn default() -> Self {
        Self::macos_seatbelt()
    }
}

impl CommandSandboxConfig {
    fn with_review_tool_paths(self, tools: &ReviewToolConfig) -> Self {
        let paths = review_tool_sandbox_paths(&tools.mcp_config_path);
        self.with_extra_read_paths(paths.read_paths)
            .with_extra_read_write_paths(paths.read_write_paths)
    }
}

pub struct CommandAgentProvider {
    kind: AgentProviderKind,
    model: Option<String>,
    command: PathBuf,
    sandbox: CommandSandboxConfig,
}

impl CommandAgentProvider {
    pub fn new(
        kind: AgentProviderKind,
        model: Option<String>,
        command: impl Into<PathBuf>,
    ) -> Self {
        Self {
            kind,
            model,
            command: command.into(),
            sandbox: CommandSandboxConfig::unsandboxed(),
        }
    }

    pub fn for_kind(kind: AgentProviderKind, model: Option<String>) -> Self {
        let command = kind.as_str();
        Self::new(kind, model, command)
    }

    pub fn kind(&self) -> &AgentProviderKind {
        &self.kind
    }

    pub fn command(&self) -> &std::path::Path {
        &self.command
    }

    pub fn resolved_command(&self) -> AgentResult<PathBuf> {
        resolve_command_path(&self.command)
    }

    pub fn with_sandbox(mut self, sandbox: CommandSandboxConfig) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn attach_session_in_repo(
        &self,
        session: &AgentSession,
        repo_dir: &Path,
    ) -> AgentResult<()> {
        let session_id = session
            .provider_session_id
            .as_deref()
            .ok_or_else(|| AgentError::invalid_input("activity has no provider session id"))?;
        let repo_dir = self.sandbox_repo_dir(repo_dir, &self.sandbox)?;
        match self.kind {
            AgentProviderKind::Claude => self.run_interactive_in_dir(
                &["--resume".into(), session_id.into()],
                repo_dir.as_deref(),
            ),
            AgentProviderKind::Codex => self
                .run_interactive_in_dir(&["resume".into(), session_id.into()], repo_dir.as_deref()),
        }
    }

    pub fn start_interactive_session_in_repo(&self, repo_dir: &Path) -> AgentResult<()> {
        let repo_dir = self.sandbox_repo_dir(repo_dir, &self.sandbox)?;
        self.run_interactive_in_dir(&[], repo_dir.as_deref())
    }

    fn run_prompt_in_dir_with_sandbox(
        &self,
        prompt: &str,
        args: &[String],
        current_dir: Option<&Path>,
        review_output_path: Option<&Path>,
        sandbox: &CommandSandboxConfig,
    ) -> AgentResult<String> {
        let mut command = self.command_for_with_sandbox(current_dir, args, sandbox)?;
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        if let Some(review_output_path) = review_output_path {
            command.env("NITPICK_REVIEW_OUTPUT", review_output_path);
        }
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            sandbox = sandbox.enabled,
            "running provider command"
        );
        let started = Instant::now();
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::provider(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?;

        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::provider("provider command stdin unavailable"))?
            .write_all(prompt.as_bytes())
            .map_err(|error| AgentError::provider(format!("write provider prompt: {error}")))?;

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::provider(format!("wait for provider command: {error}")))?;
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "provider command finished"
        );
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::provider(format!(
                "{} provider command failed with status {}{}",
                self.kind,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn command_for(&self, repo_dir: Option<&Path>, args: &[String]) -> AgentResult<Command> {
        self.command_for_with_sandbox(repo_dir, args, &self.sandbox)
    }

    fn command_for_with_sandbox(
        &self,
        repo_dir: Option<&Path>,
        args: &[String],
        sandbox: &CommandSandboxConfig,
    ) -> AgentResult<Command> {
        if !sandbox.enabled {
            let mut command = Command::new(self.resolved_command()?);
            command.args(args);
            return Ok(command);
        }

        #[cfg(target_os = "macos")]
        {
            let repo_dir = repo_dir.ok_or_else(|| {
                AgentError::sandbox("sandboxed provider execution requires a repository directory")
            })?;
            let provider_command = self.resolved_command()?;
            let mut command = Command::new("sandbox-exec");
            command
                .arg("-p")
                .arg(macos_sandbox_profile(repo_dir, &provider_command, sandbox)?);
            command.arg(&provider_command);
            command.args(args);
            Ok(command)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = repo_dir;
            let _ = args;
            Err(AgentError::sandbox(
                "sandboxed provider execution is only implemented on macOS",
            ))
        }
    }

    fn run_interactive(&self, args: &[String]) -> AgentResult<()> {
        self.run_interactive_in_dir(args, None)
    }

    fn run_interactive_in_dir(
        &self,
        args: &[String],
        current_dir: Option<&Path>,
    ) -> AgentResult<()> {
        let mut command = self.command_for(current_dir, args)?;
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            "running interactive provider command"
        );
        let started = Instant::now();
        let output = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::provider(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?
            .wait_with_output()
            .map_err(|error| AgentError::provider(format!("wait for provider command: {error}")))?;
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "interactive provider command finished"
        );
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::provider(format!(
                "{} provider command failed with status {}{}",
                self.kind,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }
        Ok(())
    }

    fn sandbox_repo_dir(
        &self,
        repo_dir: &Path,
        _sandbox: &CommandSandboxConfig,
    ) -> AgentResult<Option<PathBuf>> {
        repo_dir
            .canonicalize()
            .map_err(|error| {
                AgentError::sandbox(format!(
                    "canonicalize sandbox repository {}: {error}",
                    repo_dir.display()
                ))
            })
            .map(Some)
    }

    fn effective_sandbox(&self, disable_sandbox: bool) -> CommandSandboxConfig {
        if disable_sandbox {
            CommandSandboxConfig::unsandboxed()
        } else {
            self.sandbox.clone()
        }
    }

    fn effective_prompt_sandbox(&self, disable_sandbox: bool) -> CommandSandboxConfig {
        match self.kind {
            AgentProviderKind::Claude => self.effective_sandbox(disable_sandbox),
            AgentProviderKind::Codex => CommandSandboxConfig::unsandboxed(),
        }
    }
}

fn resolve_command_path(command: &Path) -> AgentResult<PathBuf> {
    if command.components().count() > 1 || command.is_absolute() {
        return command.canonicalize().map_err(|error| {
            AgentError::provider(format!(
                "resolve provider command `{}`: {error}",
                command.display()
            ))
        });
    }

    which::which(command)
        .map_err(|_| {
            AgentError::provider(format!(
                "provider command `{}` not found on PATH",
                command.display()
            ))
        })?
        .canonicalize()
        .map_err(|error| {
            AgentError::provider(format!(
                "resolve provider command `{}`: {error}",
                command.display()
            ))
        })
}

impl AgentProvider for CommandAgentProvider {
    #[tracing::instrument(skip_all, fields(provider = %self.kind, repository = %input.subject.repository))]
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_prompt_sandbox(input.disable_sandbox);
        let repo_dir = input.repo_dir.canonicalize().map_err(|error| {
            AgentError::provider(format!(
                "canonicalize review repository {}: {error}",
                input.repo_dir.display()
            ))
        })?;
        let output_path = repo_dir.join(REVIEW_OUTPUT_RELATIVE_PATH);
        fs_err::create_dir_all(output_path.parent().ok_or_else(|| {
            AgentError::provider(format!(
                "review output path has no parent: {}",
                output_path.display()
            ))
        })?)
        .map_err(|error| {
            AgentError::provider(format!("create review output directory: {error}"))
        })?;
        if output_path.exists() {
            fs_err::remove_file(&output_path).map_err(|error| {
                AgentError::provider(format!("remove stale review output: {error}"))
            })?;
        }
        let prompt = review_prompt(self.model.as_deref(), input, REVIEW_OUTPUT_RELATIVE_PATH);
        let args = self.review_args(session);
        self.run_prompt_in_dir_with_sandbox(
            &prompt,
            &args,
            Some(&repo_dir),
            Some(&output_path),
            &sandbox,
        )?;
        validate_review_output_file_for_diff(&repo_dir, &output_path, &input.diff)
    }

    fn supports_review_tools(&self) -> bool {
        true
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind, repository = %input.subject.repository))]
    fn review_with_tools(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        tools: &ReviewToolConfig,
    ) -> AgentResult<ReviewOutput> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_prompt_sandbox(input.disable_sandbox);
        let repo_dir = input.repo_dir.canonicalize().map_err(|error| {
            AgentError::provider(format!(
                "canonicalize review repository {}: {error}",
                input.repo_dir.display()
            ))
        })?;
        let prompt = review_tool_prompt(self.model.as_deref(), input, &tools.instructions);
        let args = self.review_tool_args(session, tools);
        let sandbox = sandbox.with_review_tool_paths(tools);
        self.run_prompt_in_dir_with_sandbox(&prompt, &args, Some(&repo_dir), None, &sandbox)?;
        Ok(ReviewOutput {
            comments: Vec::new(),
        })
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind, repo_dir = %input.repo_dir.display()))]
    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_prompt_sandbox(input.disable_sandbox);
        let repo_dir = self.sandbox_repo_dir(&input.repo_dir, &sandbox)?;
        let args = self.prompt_args();
        self.run_prompt_in_dir_with_sandbox(
            &chat_prompt(self.model.as_deref(), input),
            &args,
            repo_dir.as_deref(),
            None,
            &sandbox,
        )
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind))]
    fn attach_session(&self, session: &AgentSession) -> AgentResult<()> {
        let session_id = session
            .provider_session_id
            .as_deref()
            .ok_or_else(|| AgentError::invalid_input("activity has no provider session id"))?;
        match self.kind {
            AgentProviderKind::Claude => {
                self.run_interactive(&["--resume".into(), session_id.into()])
            }
            AgentProviderKind::Codex => self.run_interactive(&["resume".into(), session_id.into()]),
        }
    }
}

impl CommandAgentProvider {
    fn review_args(&self, session: &AgentSession) -> Vec<String> {
        match (&self.kind, session.provider_session_id.as_deref()) {
            (AgentProviderKind::Claude, Some(session_id)) => {
                vec!["--session-id".into(), session_id.into()]
            }
            (AgentProviderKind::Codex, _) => self.prompt_args(),
            _ => Vec::new(),
        }
    }

    fn prompt_args(&self) -> Vec<String> {
        match self.kind {
            AgentProviderKind::Claude => Vec::new(),
            AgentProviderKind::Codex => {
                vec![
                    "--dangerously-bypass-approvals-and-sandbox".into(),
                    "exec".into(),
                ]
            }
        }
    }

    fn review_tool_args(&self, session: &AgentSession, tools: &ReviewToolConfig) -> Vec<String> {
        let mut args = self.review_args(session);
        match self.kind {
            AgentProviderKind::Claude => {
                args.push("--mcp-config".into());
                args.push(to_command_path(&tools.mcp_config_path));
            }
            AgentProviderKind::Codex => {
                let server = codex_mcp_server_config(&tools.mcp_config_path);
                args.push("-c".into());
                args.push(format!(
                    "mcp_servers.nitpick-review.command={}",
                    server.command
                ));
                args.push("-c".into());
                args.push(format!("mcp_servers.nitpick-review.args={}", server.args));
            }
        }
        args
    }
}

fn review_prompt(model: Option<&str>, input: &ReviewInput, output_path: &str) -> String {
    format!(
        "{}\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ninstructions:\n{}\n\ndiff:\n{}\n",
        initial_review_prompt(input, output_path),
        model.unwrap_or("(default)"),
        input.subject.repository,
        input
            .subject
            .number
            .map(|number| number.to_string())
            .unwrap_or_else(|| "(none)".into()),
        input.subject.title,
        input.subject.author,
        input.repo_dir.display(),
        input.instructions,
        input.diff,
    )
}

fn initial_review_prompt(input: &ReviewInput, output_path: &str) -> String {
    let prompt = input.review_prompt.trim();
    let prompt = if prompt.is_empty() {
        include_str!("../../../examples/review-prompt.md")
    } else {
        prompt
    };
    prompt.replace("{review_output_path}", output_path)
}

fn review_tool_prompt(model: Option<&str>, input: &ReviewInput, tool_instructions: &str) -> String {
    format!(
        "{}\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ntool instructions:\n{}\n\ninstructions:\n{}\n\ndiff:\n{}\n",
        initial_review_tool_prompt(input),
        model.unwrap_or("(default)"),
        input.subject.repository,
        input
            .subject
            .number
            .map(|number| number.to_string())
            .unwrap_or_else(|| "(none)".into()),
        input.subject.title,
        input.subject.author,
        input.repo_dir.display(),
        tool_instructions,
        input.instructions,
        input.diff,
    )
}

fn initial_review_tool_prompt(input: &ReviewInput) -> String {
    let prompt = input.review_prompt.trim();
    let prompt = if prompt.is_empty() {
        include_str!("../../../examples/review-prompt.md")
    } else {
        prompt
    };
    let prompt = prompt.replace(
        "Write review annotations as JSON to `{review_output_path}` relative to the repository root. Do not return review annotations on stdout.",
        "Record review annotations with the Nitpick review MCP tools. Do not write review annotations to stdout or to a file.",
    );
    let prompt = prompt.replace(
        "The JSON object must contain `comments`. Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.",
        "Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.",
    );
    prompt.replace("{review_output_path}", "the Nitpick review MCP tools")
}

fn chat_prompt(model: Option<&str>, input: &ChatInput) -> String {
    format!(
        "You are answering a development question.\n\nmodel: {}\nrepo_dir: {}\ncontext:\n{}\n\nprompt:\n{}\n",
        model.unwrap_or("(default)"),
        input.repo_dir.display(),
        input.context,
        input.prompt,
    )
}

fn to_command_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

struct CodexMcpServerConfig {
    command: String,
    args: String,
}

struct ReviewToolSandboxPaths {
    read_paths: Vec<PathBuf>,
    read_write_paths: Vec<PathBuf>,
}

fn codex_mcp_server_config(config_path: &Path) -> CodexMcpServerConfig {
    let config = fs_err::read(config_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    let server = &config["mcpServers"]["nitpick-review"];
    let command = server["command"]
        .as_str()
        .map(serde_json::to_string)
        .and_then(Result::ok)
        .unwrap_or_else(|| "\"nitpick-agent-host\"".into());
    let args = server["args"]
        .as_array()
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .and_then(|args| serde_json::to_string(&args).ok())
        .unwrap_or_else(|| "[]".into());
    CodexMcpServerConfig { command, args }
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

#[cfg(target_os = "macos")]
fn macos_sandbox_profile(
    repo_dir: &Path,
    provider_command: &Path,
    sandbox: &CommandSandboxConfig,
) -> AgentResult<String> {
    let repo_dir = repo_dir
        .canonicalize()
        .map_err(|error| AgentError::sandbox(format!("canonicalize sandbox repo dir: {error}")))?;
    let repo = escape_sandbox_string(&repo_dir.to_string_lossy());
    let command = provider_command
        .canonicalize()
        .unwrap_or_else(|_| provider_command.to_path_buf());
    let command = escape_sandbox_string(&command.to_string_lossy());
    let extra_reads = sandbox
        .extra_read_paths
        .iter()
        .map(|path| sandbox_literal_rule("file-read*", path))
        .collect::<String>();
    let extra_read_writes = sandbox
        .extra_read_write_paths
        .iter()
        .map(|path| sandbox_subpath_rule("file-read* file-write*", path))
        .collect::<String>();
    Ok(format!(
        r#"(version 1)
(deny default)
(allow process*)
(allow network*)
(allow sysctl-read)
(allow file-read-metadata)
(allow file-read* file-write* (literal "/dev/null"))
(allow file-read* (subpath "/System") (subpath "/usr") (subpath "/bin") (subpath "/sbin") (literal "{command}"))
(allow file-read* file-write* (subpath "{repo}"))
{extra_reads}{extra_read_writes}
"#
    ))
}

#[cfg(target_os = "macos")]
fn sandbox_literal_rule(operation: &str, path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = escape_sandbox_string(&path.to_string_lossy());
    format!(r#"(allow {operation} (literal "{path}"))"#) + "\n"
}

#[cfg(target_os = "macos")]
fn sandbox_subpath_rule(operation: &str, path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = escape_sandbox_string(&path.to_string_lossy());
    format!(r#"(allow {operation} (subpath "{path}"))"#) + "\n"
}

#[cfg(target_os = "macos")]
fn escape_sandbox_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_sandbox_profile_includes_review_tool_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("provider");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let mcp_command = dir.path().join("nitpick-agent-host");
        fs_err::write(&mcp_command, "#!/bin/sh\n").expect("mcp command");
        let state_path = dir.path().join("session.json");
        let mcp_config_path = dir.path().join("mcp.json");
        fs_err::write(
            &mcp_config_path,
            serde_json::json!({
                "mcpServers": {
                    "nitpick-review": {
                        "command": mcp_command,
                        "args": ["review-mcp", state_path]
                    }
                }
            })
            .to_string(),
        )
        .expect("mcp config");
        let sandbox =
            CommandSandboxConfig::macos_seatbelt().with_review_tool_paths(&ReviewToolConfig {
                mcp_config_path: mcp_config_path.clone(),
                instructions: String::new(),
            });

        let profile =
            macos_sandbox_profile(&repo_dir, &provider_command, &sandbox).expect("profile");

        assert!(profile.contains(&format!(
            r#"(literal "{}")"#,
            mcp_config_path.canonicalize().expect("config path").display()
        )));
        assert!(profile.contains(&format!(
            r#"(literal "{}")"#,
            mcp_command.canonicalize().expect("mcp command").display()
        )));
        assert!(profile.contains(&format!(
            r#"(subpath "{}")"#,
            dir.path().canonicalize().expect("temp dir").display()
        )));
    }
}
