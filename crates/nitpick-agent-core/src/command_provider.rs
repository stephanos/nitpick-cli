use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    AgentError, AgentMessage, AgentProvider, AgentProviderKind, AgentResult, AgentSession,
    ChatInput, ProviderReviewContext, ProviderRunContext, ProviderRunSink,
    REVIEW_OUTPUT_RELATIVE_PATH, ReviewInput, ReviewOutput, ReviewToolConfig,
    app_paths::default_data_dir, macos_sandbox::SandboxProfileBuilder,
    provider_command_runner::ProviderCommandRunner, provider_log,
    validate_review_output_file_for_diff,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSandboxConfig {
    pub enabled: bool,
    provider_runtime_dir: Option<PathBuf>,
    extra_read_paths: Vec<PathBuf>,
    extra_read_write_paths: Vec<PathBuf>,
}

impl CommandSandboxConfig {
    pub fn macos_seatbelt() -> Self {
        Self {
            enabled: true,
            provider_runtime_dir: None,
            extra_read_paths: Vec::new(),
            extra_read_write_paths: Vec::new(),
        }
    }

    pub fn unsandboxed() -> Self {
        Self {
            enabled: false,
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

    fn with_extra_read_write_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra_read_write_paths.extend(paths);
        self
    }

    #[cfg(test)]
    fn with_provider_runtime_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.provider_runtime_dir = Some(path.into());
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
        session: &mut AgentSession,
        run_sink: &dyn ProviderRunSink,
        prompt: &str,
        args: &[String],
        current_dir: Option<&Path>,
        review_output_path: Option<&Path>,
        sandbox: &CommandSandboxConfig,
        timeout: Option<std::time::Duration>,
        provider_debug_file: Option<&Path>,
    ) -> AgentResult<String> {
        let sandbox_log_tag = sandbox.enabled.then(new_sandbox_log_tag);
        let mut command =
            self.command_for_with_sandbox(current_dir, args, sandbox, sandbox_log_tag.as_deref())?;
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
        run_sink.set_run_diagnostic(&provider_run_start_diagnostic(
            &self.kind,
            self.model.as_deref(),
            &self.command,
            sandbox.enabled,
            timeout,
            provider_debug_file,
        ))?;
        let command_display = self.command.display().to_string();
        let output = ProviderCommandRunner::new(self.kind.as_str(), &command_display)
            .run(command, prompt, run_sink, timeout)?;
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            status = %output.status,
            duration_ms = output.duration_ms,
            "provider command finished"
        );
        record_provider_logs(session, &output.stdout, &output.stderr);
        let run_diagnostic = provider_run_diagnostic(
            &self.kind,
            self.model.as_deref(),
            &self.command,
            sandbox.enabled,
            timeout,
            provider_debug_file,
            output.status,
            output.duration_ms,
            output.timed_out,
            &output.stdout,
            &output.stderr,
        );
        run_sink.set_run_diagnostic(&run_diagnostic)?;
        record_provider_run_diagnostic(session, &run_diagnostic);
        if output.timed_out {
            return Err(AgentError::provider(format!(
                "{} provider command timed out after {}",
                self.kind,
                timeout
                    .map(format_timeout_duration)
                    .unwrap_or_else(|| "unknown duration".into())
            )));
        }
        if output.cancelled {
            return Err(AgentError::provider(format!(
                "{} provider command cancelled",
                self.kind
            )));
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let session_already_in_use = provider_session_already_in_use(&stderr);
            let failure_hint = provider_failure_hint(&stderr, sandbox.enabled);
            let sandbox_violations = if sandbox.enabled {
                sandbox_log_tag
                    .as_deref()
                    .map(recent_sandbox_violations)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if sandbox.enabled && !session_already_in_use {
                record_provider_sandbox_diagnostic(
                    session,
                    output.status,
                    &stderr,
                    &sandbox_violations,
                );
            }
            return Err(AgentError::provider(format!(
                "{} provider command failed with status {}{}{}",
                self.kind,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                },
                failure_hint
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn command_for(&self, repo_dir: Option<&Path>, args: &[String]) -> AgentResult<Command> {
        self.command_for_with_sandbox(repo_dir, args, &self.sandbox, None)
    }

    fn command_for_with_sandbox(
        &self,
        repo_dir: Option<&Path>,
        args: &[String],
        sandbox: &CommandSandboxConfig,
        sandbox_log_tag: Option<&str>,
    ) -> AgentResult<Command> {
        if !sandbox.enabled {
            let mut command = Command::new(self.resolved_command()?);
            command.args(args);
            return Ok(command);
        }
        let provider_runtime_env = self.prepare_provider_runtime_env(sandbox)?;

        #[cfg(target_os = "macos")]
        {
            let repo_dir = repo_dir.ok_or_else(|| {
                AgentError::sandbox("sandboxed provider execution requires a repository directory")
            })?;
            let provider_command = self.resolved_command()?;
            let mut command = Command::new("sandbox-exec");
            command.arg("-p").arg(macos_sandbox_profile(
                repo_dir,
                &provider_command,
                sandbox,
                sandbox_log_tag,
            )?);
            command.arg(&provider_command);
            command.args(args);
            command.envs(provider_runtime_env);
            Ok(command)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = repo_dir;
            let _ = args;
            let _ = provider_runtime_env;
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
        self.effective_sandbox(disable_sandbox)
    }

    #[cfg(target_os = "macos")]
    pub fn macos_sandbox_profile_for_testing(
        &self,
        repo_dir: &Path,
        provider_command: &Path,
    ) -> AgentResult<String> {
        macos_sandbox_profile(repo_dir, provider_command, &self.sandbox, None)
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

fn record_provider_logs(session: &mut AgentSession, stdout: &[u8], stderr: &[u8]) {
    provider_log::push_provider_log(session, "provider.stdout", stdout);
    provider_log::push_provider_log(session, "provider.stderr", stderr);
}

fn record_provider_run_diagnostic(session: &mut AgentSession, content: &str) {
    provider_log::upsert_provider_log(session, "provider.run", content);
}

fn provider_run_start_diagnostic(
    provider: &AgentProviderKind,
    model: Option<&str>,
    command: &Path,
    sandbox_enabled: bool,
    timeout: Option<std::time::Duration>,
    provider_debug_file: Option<&Path>,
) -> String {
    let mut lines = vec![
        format!("provider {provider} command running"),
        format!("model: {}", model.unwrap_or("(default)")),
        format!("command: {}", command.display()),
        format!(
            "sandbox: {}",
            if sandbox_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!(
            "timeout: {}",
            timeout
                .map(format_timeout_duration)
                .unwrap_or_else(|| "none".into())
        ),
        "status: running".into(),
    ];
    if let Some(provider_debug_file) = provider_debug_file {
        lines.push(format!("debug_file: {}", provider_debug_file.display()));
    }
    lines.join("\n")
}

fn provider_run_diagnostic(
    provider: &AgentProviderKind,
    model: Option<&str>,
    command: &Path,
    sandbox_enabled: bool,
    timeout: Option<std::time::Duration>,
    provider_debug_file: Option<&Path>,
    status: std::process::ExitStatus,
    duration_ms: u128,
    timed_out: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut lines = vec![
        format!("provider {provider} command completed"),
        format!("model: {}", model.unwrap_or("(default)")),
        format!("command: {}", command.display()),
        format!(
            "sandbox: {}",
            if sandbox_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!(
            "timeout: {}",
            timeout
                .map(format_timeout_duration)
                .unwrap_or_else(|| "none".into())
        ),
        format!("status: {status}"),
        format!("duration_ms: {duration_ms}"),
        format!("timed_out: {timed_out}"),
        format!("stdout: {}", provider_stream_state(stdout)),
        format!("stderr: {}", provider_stream_state(stderr)),
    ];
    if let Some(provider_debug_file) = provider_debug_file {
        lines.push(format!("debug_file: {}", provider_debug_file.display()));
    }
    lines.join("\n")
}

fn provider_stream_state(bytes: &[u8]) -> &'static str {
    if bytes.is_empty() {
        "empty"
    } else {
        "captured"
    }
}

fn format_timeout_duration(timeout: std::time::Duration) -> String {
    if timeout.as_millis() < 1_000 {
        format!("{}ms", timeout.as_millis())
    } else {
        format!("{}s", timeout.as_secs())
    }
}

fn record_provider_sandbox_diagnostic(
    session: &mut AgentSession,
    status: std::process::ExitStatus,
    stderr: &str,
    violations: &[String],
) {
    session.messages.push(AgentMessage {
        role: "provider.sandbox".into(),
        content: sandbox_diagnostic(status, stderr, violations),
    });
}

fn sandbox_failure_hint(sandbox_enabled: bool) -> String {
    if sandbox_enabled {
        "; sandbox was enabled, retry with --no-sandbox to determine whether the sandbox is involved"
            .into()
    } else {
        String::new()
    }
}

fn provider_failure_hint(stderr: &str, sandbox_enabled: bool) -> String {
    if provider_session_already_in_use(stderr) {
        "; provider session is already in use, wait for the active Claude process to finish or stop the stale provider process before retrying".into()
    } else {
        sandbox_failure_hint(sandbox_enabled)
    }
}

fn provider_session_already_in_use(stderr: &str) -> bool {
    stderr.contains("Session ID") && stderr.contains("already in use")
}

fn sandbox_diagnostic(
    status: std::process::ExitStatus,
    stderr: &str,
    violations: &[String],
) -> String {
    let mut lines = vec![
        "sandbox was enabled for this provider run".into(),
        format!("provider exited with status {status}"),
        "retry with --no-sandbox to determine whether the sandbox is involved".into(),
    ];
    if stderr.trim().is_empty() {
        lines.push("provider stderr was empty".into());
    } else {
        lines.push("provider stderr:".into());
        lines.push(stderr.trim().into());
    }
    if violations.is_empty() {
        lines.push("no matching macOS sandbox violations were found in recent system logs".into());
    } else {
        lines.push("matching macOS sandbox violations:".into());
        lines.extend(violations.iter().cloned());
    }
    lines.join("\n")
}

fn new_sandbox_log_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("NITPICK_SANDBOX_{nanos}")
}

#[cfg(target_os = "macos")]
fn recent_sandbox_violations(log_tag: &str) -> Vec<String> {
    std::thread::sleep(std::time::Duration::from_millis(150));
    let predicate = format!(r#"eventMessage CONTAINS "{log_tag}" AND process != "log""#);
    let output = Command::new("/usr/bin/log")
        .args(["show", "--last", "2m", "--style", "compact", "--predicate"])
        .arg(predicate)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    sandbox_violation_lines(&provider_log::bounded_provider_log(&output.stdout))
}

#[cfg(target_os = "macos")]
fn sandbox_violation_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("Timestamp ")
                && !line.contains(" log[")
                && !line.contains("log run noninteractively")
                && (line.contains("Sandbox:")
                    || line.contains("deny(")
                    || line.contains(" deny ")
                    || line.contains("Violation:"))
        })
        .take(20)
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn recent_sandbox_violations(_log_tag: &str) -> Vec<String> {
    Vec::new()
}

impl AgentProvider for CommandAgentProvider {
    #[tracing::instrument(skip_all, fields(provider = %self.kind, repository = %input.subject.repository))]
    fn review(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        validate_review_input(input)?;
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_prompt_sandbox(input.disable_sandbox);
        let repo_dir = input.repo_dir.canonicalize().map_err(|error| {
            AgentError::provider(format!(
                "canonicalize review repository {}: {error}",
                input.repo_dir.display()
            ))
        })?;
        match context.tools {
            Some(tools) => {
                let prompt = review_tool_prompt(self.model.as_deref(), input, &tools.instructions);
                let args = self.review_tool_args(session, tools);
                let sandbox = sandbox.with_review_tool_paths(tools);
                self.run_prompt_in_dir_with_sandbox(
                    session,
                    context.run_sink,
                    &prompt,
                    &args,
                    Some(&repo_dir),
                    None,
                    &sandbox,
                    None,
                    None,
                )?;
                Ok(ReviewOutput {
                    comments: Vec::new(),
                })
            }
            None => {
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
                let prompt =
                    review_prompt(self.model.as_deref(), input, REVIEW_OUTPUT_RELATIVE_PATH);
                let args = self.review_args(session);
                self.run_prompt_in_dir_with_sandbox(
                    session,
                    context.run_sink,
                    &prompt,
                    &args,
                    Some(&repo_dir),
                    Some(&output_path),
                    &sandbox,
                    None,
                    None,
                )?;
                validate_review_output_file_for_diff(&repo_dir, &output_path, &input.diff)
            }
        }
    }

    fn supports_review_tools(&self) -> bool {
        true
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind, repo_dir = %input.repo_dir.display()))]
    fn chat(
        &self,
        session: &mut AgentSession,
        input: &ChatInput,
        context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_chat_sandbox(input);
        let repo_dir = self.sandbox_repo_dir(&input.repo_dir, &sandbox)?;
        let args = self.chat_args(input);
        self.run_prompt_in_dir_with_sandbox(
            session,
            context.run_sink,
            &chat_prompt(self.model.as_deref(), input),
            &args,
            repo_dir.as_deref(),
            None,
            &sandbox,
            input
                .provider_timeout_ms
                .map(std::time::Duration::from_millis),
            input.provider_debug_file.as_deref(),
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

fn validate_review_input(input: &ReviewInput) -> AgentResult<()> {
    if input.diff.trim().is_empty() {
        return Err(AgentError::invalid_input(
            "review input missing diff; cannot run review",
        ));
    }
    if !input.repo_dir.is_dir() {
        return Err(AgentError::invalid_input(format!(
            "review input checkout not found: {}",
            input.repo_dir.display()
        )));
    }
    Ok(())
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

    fn chat_args(&self, input: &ChatInput) -> Vec<String> {
        match (&self.kind, input.provider_debug_file.as_deref()) {
            (AgentProviderKind::Claude, Some(debug_file)) => {
                vec![
                    "-p".into(),
                    "--debug".into(),
                    "--debug-file".into(),
                    to_command_path(debug_file),
                ]
            }
            _ => self.prompt_args(),
        }
    }

    fn effective_chat_sandbox(&self, input: &ChatInput) -> CommandSandboxConfig {
        let sandbox = self.effective_prompt_sandbox(input.disable_sandbox);
        if !sandbox.enabled {
            return sandbox;
        }
        let Some(debug_file) = input.provider_debug_file.as_deref() else {
            return sandbox;
        };
        let Some(debug_dir) = debug_file.parent() else {
            return sandbox;
        };
        sandbox.with_extra_read_write_paths([debug_dir.to_path_buf()])
    }

    fn prepare_provider_runtime_env(
        &self,
        sandbox: &CommandSandboxConfig,
    ) -> AgentResult<Vec<(&'static str, PathBuf)>> {
        let root = provider_runtime_root_dir_for_sandbox(sandbox);
        let tmp = root.join("tmp").join(self.kind.as_str());
        let env = match self.kind {
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
    log_tag: Option<&str>,
) -> AgentResult<String> {
    let repo_dir = repo_dir
        .canonicalize()
        .map_err(|error| AgentError::sandbox(format!("canonicalize sandbox repo dir: {error}")))?;
    let command = provider_command
        .canonicalize()
        .unwrap_or_else(|_| provider_command.to_path_buf());
    let builder = SandboxProfileBuilder::new()
        .allow_processes()
        .allow_mach_lookup()
        .allow_network()
        .allow_sysctl_read()
        .allow_file_read_metadata()
        .allow_device_runtime()
        .allow_macos_runtime()
        .allow_literal_read(&command)
        .allow_read(&repo_dir)
        .allow_reads(&provider_runtime_read_paths())
        .allow_reads(&provider_config_read_paths())
        .allow_read_writes(&provider_runtime_read_write_paths(sandbox))
        .allow_read_writes(&provider_config_read_write_paths())
        .allow_literal_read_writes(&provider_config_literal_read_write_paths())
        .allow_literal_reads(&sandbox.extra_read_paths)
        .allow_read_writes(&sandbox.extra_read_write_paths);
    Ok(match log_tag {
        Some(log_tag) => builder.render_with_deny_message(log_tag),
        None => builder.render(),
    })
}

#[cfg(target_os = "macos")]
fn provider_runtime_read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in [Path::new("/opt/homebrew"), Path::new("/usr/local")] {
        if path.exists() {
            paths.push(path.to_path_buf());
        }
    }
    paths
}

#[cfg(target_os = "macos")]
fn provider_runtime_read_write_paths(sandbox: &CommandSandboxConfig) -> Vec<PathBuf> {
    vec![provider_runtime_root_dir_for_sandbox(sandbox)]
}

#[cfg(target_os = "macos")]
fn provider_config_read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(home.join(".agents").join("skills"));
    }
    paths
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

fn provider_runtime_root_dir_for_sandbox(sandbox: &CommandSandboxConfig) -> PathBuf {
    sandbox
        .provider_runtime_dir
        .clone()
        .unwrap_or_else(provider_runtime_root_dir)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn sandbox_diagnostic_mentions_no_sandbox_retry() {
        let command = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let status = std::process::Command::new(command)
            .arg(if cfg!(target_os = "windows") {
                "/C"
            } else {
                "-c"
            })
            .arg("exit 6")
            .status()
            .expect("status");

        assert!(sandbox_failure_hint(true).contains("--no-sandbox"));
        assert_eq!(sandbox_failure_hint(false), "");
        assert!(
            provider_failure_hint("Error: Session ID 65cc7ced is already in use.", true)
                .contains("provider session is already in use")
        );
        assert!(
            !provider_failure_hint("Error: Session ID 65cc7ced is already in use.", true)
                .contains("--no-sandbox")
        );
        assert!(sandbox_diagnostic(status, "", &[]).contains("provider stderr was empty"));
        assert!(sandbox_diagnostic(status, "", &[]).contains("--no-sandbox"));
        assert!(
            sandbox_diagnostic(status, "", &["Sandbox: deny file-read-data".into()])
                .contains("matching macOS sandbox violations")
        );
    }

    #[test]
    fn sandbox_violation_lines_ignores_log_query_output() {
        let output = r#"Timestamp               Ty Process[PID:TID]
2026-05-25 14:25:36.543 Df log[78270:22db5d] [com.apple.log:] log run noninteractively, parent: 75368 (nitpick-agent-host), args: '/usr/bin/log' 'show' '--last' '2m' '--style' 'compact' '--predicate' 'eventMessage CONTAINS "NITPICK_SANDBOX_1"'
2026-05-25 14:25:36.544 Df kernel[0:0] Sandbox: claude(123) deny(1) file-read-data /private/var/db/mds NITPICK_SANDBOX_1
"#;

        assert_eq!(
            sandbox_violation_lines(output),
            vec![
                "2026-05-25 14:25:36.544 Df kernel[0:0] Sandbox: claude(123) deny(1) file-read-data /private/var/db/mds NITPICK_SANDBOX_1"
            ]
        );
    }

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
            macos_sandbox_profile(&repo_dir, &provider_command, &sandbox, None).expect("profile");

        assert!(profile.contains(&format!(
            r#"(allow file-read* (subpath "{}"))"#,
            repo_dir.canonicalize().expect("repo dir").display()
        )));
        assert!(!profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{}"))"#,
            repo_dir.canonicalize().expect("repo dir").display()
        )));
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

    #[test]
    fn sandboxed_provider_command_sets_owned_runtime_env() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("claude");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let sandbox = CommandSandboxConfig::macos_seatbelt()
            .with_provider_runtime_dir(dir.path().join("runtime"));
        let provider =
            CommandAgentProvider::new(AgentProviderKind::Claude, None, &provider_command)
                .with_sandbox(sandbox);

        let command = provider
            .command_for_with_sandbox(Some(repo_dir.as_path()), &[], &provider.sandbox, None)
            .expect("command");

        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value
                        .map(|value| value.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let root = provider_runtime_root_dir_for_sandbox(&provider.sandbox);
        let tmp = root.join("tmp").join("claude");
        assert!(!envs.contains_key("CLAUDE_CONFIG_DIR"));
        assert_eq!(
            envs.get("CLAUDE_CODE_TMPDIR"),
            Some(&tmp.to_string_lossy().into_owned())
        );
        assert_eq!(
            envs.get("TMPDIR"),
            Some(&tmp.to_string_lossy().into_owned())
        );
        assert!(tmp.is_dir());
    }

    #[test]
    fn sandboxed_codex_provider_command_sets_owned_runtime_env() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("codex");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let sandbox = CommandSandboxConfig::macos_seatbelt()
            .with_provider_runtime_dir(dir.path().join("runtime"));
        let provider = CommandAgentProvider::new(AgentProviderKind::Codex, None, &provider_command)
            .with_sandbox(sandbox);

        let command = provider
            .command_for_with_sandbox(Some(repo_dir.as_path()), &[], &provider.sandbox, None)
            .expect("command");

        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value
                        .map(|value| value.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let root = provider_runtime_root_dir_for_sandbox(&provider.sandbox);
        let tmp = root.join("tmp").join("codex");
        assert!(!envs.contains_key("CODEX_HOME"));
        assert_eq!(
            envs.get("TMPDIR"),
            Some(&tmp.to_string_lossy().into_owned())
        );
        assert!(tmp.is_dir());
    }

    #[test]
    fn sandboxed_provider_can_read_repo_and_write_only_runtime_dir() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        fs_err::write(repo_dir.join("readable.txt"), "repo").expect("repo file");
        let blocked_file = dir.path().join("blocked.txt");
        fs_err::write(&blocked_file, "blocked").expect("blocked file");
        let provider_command = dir.path().join("provider");
        fs_err::write(
            &provider_command,
            r#"#!/bin/sh
set -eu
repo_dir="$1"
blocked_file="$2"
cat "$repo_dir/readable.txt" >/dev/null
if (echo blocked > "$repo_dir/write-blocked.txt") 2>/dev/null; then
  echo "repo write unexpectedly succeeded" >&2
  exit 10
fi
echo ok > "$TMPDIR/runtime-write.txt"
if cat "$blocked_file" >/dev/null 2>&1; then
  echo "outside read unexpectedly succeeded" >&2
  exit 11
fi
printf done
"#,
        )
        .expect("provider command");
        make_test_command_executable(&provider_command);
        let sandbox = CommandSandboxConfig::macos_seatbelt()
            .with_provider_runtime_dir(dir.path().join("runtime"));
        let provider =
            CommandAgentProvider::new(AgentProviderKind::Claude, None, &provider_command)
                .with_sandbox(sandbox);
        let args = vec![
            repo_dir.to_string_lossy().into_owned(),
            blocked_file.to_string_lossy().into_owned(),
        ];
        let mut command = provider
            .command_for_with_sandbox(Some(repo_dir.as_path()), &args, &provider.sandbox, None)
            .expect("command");

        let output = command.output().expect("provider output");

        assert!(output.status.success(), "stderr: {:?}", output.stderr);
        assert_eq!(String::from_utf8_lossy(&output.stdout), "done");
        assert!(!repo_dir.join("write-blocked.txt").exists());
        assert!(
            provider_runtime_root_dir_for_sandbox(&provider.sandbox)
                .join("tmp")
                .join("claude")
                .join("runtime-write.txt")
                .is_file()
        );
    }

    #[test]
    fn macos_sandbox_profile_uses_provider_config_and_owned_runtime_dirs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("claude");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let sandbox = CommandSandboxConfig::macos_seatbelt();

        let profile =
            macos_sandbox_profile(&repo_dir, &provider_command, &sandbox, Some("NITPICK_TEST"))
                .expect("profile");

        let home = std::env::var("HOME").expect("home");
        let provider_runtime_dir = provider_runtime_root_dir();
        assert!(profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{}"))"#,
            provider_runtime_dir
                .canonicalize()
                .unwrap_or(provider_runtime_dir)
                .display()
        )));
        assert!(profile.contains(&format!(r#"(subpath "{home}/.claude")"#)));
        assert!(profile.contains(&format!(r#"(literal "{home}/.claude.json")"#)));
        assert!(profile.contains(&format!(r#"(literal "{home}/.claude.lock")"#)));
        assert!(profile.contains(&format!(r#"(subpath "{home}/.codex")"#)));
        assert!(profile.contains(&format!(r#"(subpath "{home}/.agents/skills")"#)));
        assert!(!profile.contains(&format!(r#"(subpath "{home}/.cache")"#)));
        assert!(!profile.contains(&format!(r#"(subpath "{home}/Library/Keychains")"#)));
        assert!(!profile.contains(&format!(
            r#"(subpath "{}")"#,
            std::env::temp_dir().canonicalize().expect("temp dir").display()
        )));
        assert!(profile.contains(r#"(allow mach-lookup)"#));
        assert!(profile.contains(r#"(deny default (with message "NITPICK_TEST"))"#));
        assert!(profile.contains(r#"(allow process*)"#));
        assert!(profile.contains(r#"(allow ipc*)"#));
        assert!(profile.contains(r#"(allow pseudo-tty)"#));
        assert!(profile.contains(r#"(allow file-map-executable"#));
        assert!(profile.contains(r#"(allow system-mac-syscall (mac-policy-name "vnguard"))"#));
        assert!(profile.contains(r#"(literal "/dev/tty")"#));
        assert!(profile.contains(r#"(literal "/dev/urandom")"#));
        assert!(profile.contains(r#"(subpath "/private/etc")"#));
        assert!(profile.contains(r#"(subpath "/opt/homebrew")"#));
        assert!(profile.contains(r#"(subpath "/usr/local")"#));
    }

    fn make_test_command_executable(command: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs_err::metadata(command).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs_err::set_permissions(command, permissions).expect("chmod");
    }
}
