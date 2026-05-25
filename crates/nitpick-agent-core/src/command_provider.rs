use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
    time::Duration,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    AgentError, AgentMessage, AgentProvider, AgentProviderKind, AgentResult, AgentSession,
    ChatInput, ProviderLogSink, REVIEW_OUTPUT_RELATIVE_PATH, ReviewInput, ReviewOutput,
    ReviewToolConfig, macos_sandbox::SandboxProfileBuilder, validate_review_output_file_for_diff,
};

const MAX_PROVIDER_LOG_BYTES: usize = 64 * 1024;

struct ProviderCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Copy)]
enum ProviderStream {
    Stdout,
    Stderr,
}

struct ProviderStreamChunk {
    stream: ProviderStream,
    bytes: Vec<u8>,
}

struct NoopProviderLogSink;

impl ProviderLogSink for NoopProviderLogSink {
    fn append_stdout(&self, _bytes: &[u8]) -> AgentResult<()> {
        Ok(())
    }

    fn append_stderr(&self, _bytes: &[u8]) -> AgentResult<()> {
        Ok(())
    }
}

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

    pub fn with_read_paths(self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.with_extra_read_paths(paths)
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
        session: &mut AgentSession,
        log_sink: Option<&dyn ProviderLogSink>,
        prompt: &str,
        args: &[String],
        current_dir: Option<&Path>,
        review_output_path: Option<&Path>,
        sandbox: &CommandSandboxConfig,
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

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::provider("provider command stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentError::provider("provider command stderr unavailable"))?;
        let (stream_tx, stream_rx) = mpsc::channel();
        let stdout_reader =
            spawn_provider_stream_reader(ProviderStream::Stdout, stdout, stream_tx.clone());
        let stderr_reader = spawn_provider_stream_reader(ProviderStream::Stderr, stderr, stream_tx);

        child
            .stdin
            .take()
            .ok_or_else(|| AgentError::provider("provider command stdin unavailable"))?
            .write_all(prompt.as_bytes())
            .map_err(|error| AgentError::provider(format!("write provider prompt: {error}")))?;

        let output = collect_provider_output(child, stream_rx, log_sink)?;
        join_provider_stream_reader(stdout_reader)?;
        join_provider_stream_reader(stderr_reader)?;
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "provider command finished"
        );
        record_provider_logs(session, &output.stdout, &output.stderr);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let sandbox_violations = if sandbox.enabled {
                sandbox_log_tag
                    .as_deref()
                    .map(recent_sandbox_violations)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if sandbox.enabled {
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
                sandbox_failure_hint(sandbox.enabled)
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
    push_provider_log(session, "provider.stdout", stdout);
    push_provider_log(session, "provider.stderr", stderr);
}

fn spawn_provider_stream_reader<R>(
    stream: ProviderStream,
    mut reader: R,
    tx: Sender<AgentResult<ProviderStreamChunk>>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = ProviderStreamChunk {
                        stream,
                        bytes: buffer[..count].to_vec(),
                    };
                    if tx.send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(AgentError::provider(format!(
                        "read provider output: {error}"
                    ))));
                    break;
                }
            }
        }
    })
}

fn collect_provider_output(
    mut child: std::process::Child,
    rx: Receiver<AgentResult<ProviderStreamChunk>>,
    log_sink: Option<&dyn ProviderLogSink>,
) -> AgentResult<ProviderCommandOutput> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, log_sink)?;
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| AgentError::provider(format!("wait for provider command: {error}")))?
        {
            break status;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, log_sink)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    };
    loop {
        match rx.recv() {
            Ok(chunk) => append_provider_stream_chunk(chunk?, &mut stdout, &mut stderr, log_sink)?,
            Err(_) => break,
        }
    }
    Ok(ProviderCommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn append_provider_stream_chunk(
    chunk: ProviderStreamChunk,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    log_sink: Option<&dyn ProviderLogSink>,
) -> AgentResult<()> {
    match chunk.stream {
        ProviderStream::Stdout => {
            stdout.extend_from_slice(&chunk.bytes);
            if let Some(log_sink) = log_sink {
                log_sink.append_stdout(&chunk.bytes)?;
            }
        }
        ProviderStream::Stderr => {
            stderr.extend_from_slice(&chunk.bytes);
            if let Some(log_sink) = log_sink {
                log_sink.append_stderr(&chunk.bytes)?;
            }
        }
    }
    Ok(())
}

fn join_provider_stream_reader(handle: JoinHandle<()>) -> AgentResult<()> {
    handle
        .join()
        .map_err(|_| AgentError::provider("provider output reader thread panicked"))
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

fn push_provider_log(session: &mut AgentSession, role: &str, bytes: &[u8]) {
    let content = bounded_provider_log(bytes);
    if content.is_empty() {
        return;
    }
    session.messages.push(AgentMessage {
        role: role.into(),
        content,
    });
}

fn bounded_provider_log(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_PROVIDER_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_PROVIDER_LOG_BYTES);
    let mut value = String::from_utf8_lossy(&bytes[start..]).trim().to_owned();
    if truncated {
        value = format!("[truncated to last {MAX_PROVIDER_LOG_BYTES} bytes]\n{value}");
    }
    value
}

fn sandbox_failure_hint(sandbox_enabled: bool) -> String {
    if sandbox_enabled {
        "; sandbox was enabled, retry with --no-sandbox to determine whether the sandbox is involved"
            .into()
    } else {
        String::new()
    }
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
    sandbox_violation_lines(&bounded_provider_log(&output.stdout))
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
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput> {
        self.review_with_log_sink(session, input, &NoopProviderLogSink)
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind, repository = %input.subject.repository))]
    fn review_with_log_sink(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        log_sink: &dyn ProviderLogSink,
    ) -> AgentResult<ReviewOutput> {
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
            session,
            Some(log_sink),
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
        self.review_with_tools_and_log_sink(session, input, tools, &NoopProviderLogSink)
    }

    #[tracing::instrument(skip_all, fields(provider = %self.kind, repository = %input.subject.repository))]
    fn review_with_tools_and_log_sink(
        &self,
        session: &mut AgentSession,
        input: &ReviewInput,
        tools: &ReviewToolConfig,
        log_sink: &dyn ProviderLogSink,
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
        self.run_prompt_in_dir_with_sandbox(
            session,
            Some(log_sink),
            &prompt,
            &args,
            Some(&repo_dir),
            None,
            &sandbox,
        )?;
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
            session,
            None,
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
        .allow_read_write(&repo_dir)
        .allow_reads(&provider_runtime_read_paths())
        .allow_read_writes(&provider_runtime_read_write_paths())
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
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.extend([home.join("Library").join("Keychains")]);
    }
    paths
}

#[cfg(target_os = "macos")]
fn provider_runtime_read_write_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.extend([
            home.join(".claude"),
            home.join(".cache"),
            home.join(".config").join("claude"),
            home.join(".local").join("share").join("claude"),
            home.join("Library")
                .join("Application Support")
                .join("Claude"),
            home.join(".npm"),
            home.join("Library").join("Caches").join("Claude"),
        ]);
    }
    paths.push(std::env::temp_dir());
    paths
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
    fn macos_sandbox_profile_includes_claude_runtime_paths() {
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
        assert!(profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{home}/.local/share/claude"))"#
        )));
        assert!(profile.contains(&format!(
            r#"(allow file-read* (subpath "{home}/Library/Keychains"))"#
        )));
        assert!(profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{home}/.claude"))"#
        )));
        assert!(profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{home}/.cache"))"#
        )));
        assert!(profile.contains(&format!(
            r#"(allow file-read* file-write* (subpath "{home}/.config/claude"))"#
        )));
        assert!(profile.contains(&format!(
            r#"(subpath "{}")"#,
            std::env::temp_dir().canonicalize().expect("temp dir").display()
        )));
        assert!(profile.contains(r#"(allow mach-lookup)"#));
        assert!(profile.contains(r#"(deny default (with message "NITPICK_TEST"))"#));
        assert!(profile.contains(r#"(allow process-exec)"#));
        assert!(profile.contains(r#"(allow ipc-posix-sem)"#));
        assert!(profile.contains(r#"(allow pseudo-tty)"#));
        assert!(profile.contains(r#"(allow file-map-executable"#));
        assert!(profile.contains(r#"(allow system-mac-syscall (mac-policy-name "vnguard"))"#));
        assert!(profile.contains(r#"(global-name "com.apple.trustd.agent")"#));
        assert!(profile.contains(r#"(literal "/dev/tty")"#));
        assert!(profile.contains(r#"(literal "/dev/urandom")"#));
        assert!(profile.contains(r#"(subpath "/private/etc")"#));
        assert!(profile.contains(r#"(subpath "/opt/homebrew")"#));
        assert!(profile.contains(r#"(subpath "/usr/local")"#));
    }
}
