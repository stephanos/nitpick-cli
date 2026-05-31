use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use crate::{
    AgentError, AgentMessage, AgentProvider, AgentProviderKind, AgentResult, AgentSession,
    ChatInput, ProviderReviewContext, ProviderRunContext, ProviderRunSink,
    REVIEW_OUTPUT_RELATIVE_PATH, ReviewInput, ReviewOutput, ReviewToolConfig,
    nono_sandbox::{NONO_SANDBOX_HELPER_ARG, NONO_SANDBOX_SPEC_ENV},
    provider_command_runner::ProviderCommandRunner,
    provider_log,
    provider_sandbox::{CommandSandboxConfig, ProviderSandboxPlan},
    validate_review_output_file_for_diff,
};

pub struct CommandAgentProvider {
    kind: AgentProviderKind,
    model: Option<String>,
    command: PathBuf,
    sandbox: CommandSandboxConfig,
}

struct PromptRunRequest<'a> {
    session: &'a mut AgentSession,
    run_sink: &'a dyn ProviderRunSink,
    prompt: &'a str,
    args: &'a [String],
    current_dir: Option<&'a Path>,
    review_output_path: Option<&'a Path>,
    sandbox: &'a CommandSandboxConfig,
    timeout: Option<std::time::Duration>,
    provider_debug_file: Option<&'a Path>,
}

struct ProviderRunDiagnosticContext<'a> {
    provider: &'a AgentProviderKind,
    model: Option<&'a str>,
    command: &'a Path,
    sandbox_enabled: bool,
    timeout: Option<std::time::Duration>,
    provider_debug_file: Option<&'a Path>,
}

struct ProviderRunDiagnosticResult<'a> {
    status: std::process::ExitStatus,
    duration_ms: u128,
    timed_out: bool,
    stdout: &'a [u8],
    stderr: &'a [u8],
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

    fn run_prompt_in_dir_with_sandbox(&self, request: PromptRunRequest<'_>) -> AgentResult<String> {
        let mut command =
            self.command_for_with_sandbox(request.current_dir, request.args, request.sandbox)?;
        if let Some(current_dir) = request.current_dir {
            command.current_dir(current_dir);
        }
        if let Some(review_output_path) = request.review_output_path {
            command.env("NITPICK_REVIEW_OUTPUT", review_output_path);
        }
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            sandbox = request.sandbox.enabled,
            "running provider command"
        );
        let diagnostic_context = ProviderRunDiagnosticContext {
            provider: &self.kind,
            model: self.model.as_deref(),
            command: &self.command,
            sandbox_enabled: request.sandbox.enabled,
            timeout: request.timeout,
            provider_debug_file: request.provider_debug_file,
        };
        request
            .run_sink
            .set_run_diagnostic(&provider_run_start_diagnostic(&diagnostic_context))?;
        let command_display = self.command.display().to_string();
        let output = ProviderCommandRunner::new(self.kind.as_str(), &command_display).run(
            command,
            request.prompt,
            request.run_sink,
            request.timeout,
        )?;
        tracing::debug!(
            provider = %self.kind,
            command = %self.command.display(),
            status = %output.status,
            duration_ms = output.duration_ms,
            "provider command finished"
        );
        record_provider_logs(request.session, &output.stdout, &output.stderr);
        let run_diagnostic = provider_run_diagnostic(
            &diagnostic_context,
            ProviderRunDiagnosticResult {
                status: output.status,
                duration_ms: output.duration_ms,
                timed_out: output.timed_out,
                stdout: &output.stdout,
                stderr: &output.stderr,
            },
        );
        request.run_sink.set_run_diagnostic(&run_diagnostic)?;
        record_provider_run_diagnostic(request.session, &run_diagnostic);
        if output.timed_out {
            return Err(AgentError::provider(format!(
                "{} provider command timed out after {}",
                self.kind,
                request
                    .timeout
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
            let failure_hint = provider_failure_hint(&stderr, request.sandbox.enabled);
            if request.sandbox.enabled && !session_already_in_use {
                record_provider_sandbox_diagnostic(request.session, output.status, &stderr);
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
        self.command_for_with_sandbox(repo_dir, args, &self.sandbox)
    }

    pub fn command_for_testing(
        &self,
        repo_dir: Option<&Path>,
        args: &[String],
    ) -> AgentResult<Command> {
        self.command_for(repo_dir, args)
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
        let repo_dir = repo_dir.ok_or_else(|| {
            AgentError::sandbox("sandboxed provider execution requires a repository directory")
        })?;
        let provider_command = self.resolved_command()?;
        let plan = ProviderSandboxPlan::prepare(&self.kind, repo_dir, &provider_command, sandbox)?;
        let mut command = Command::new(sandbox.helper_command()?);
        command.arg(NONO_SANDBOX_HELPER_ARG);
        command.arg("--");
        command.arg(&provider_command);
        command.args(args);
        command.envs(plan.env);
        command.env(NONO_SANDBOX_SPEC_ENV, plan.spec.to_env_value()?);
        Ok(command)
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

    pub fn nono_sandbox_spec_for_testing(
        &self,
        repo_dir: &Path,
        provider_command: &Path,
    ) -> AgentResult<String> {
        ProviderSandboxPlan::prepare(
            &self.kind,
            repo_dir,
            provider_command,
            &self.sandbox.clone().without_nono_profile_updates(),
        )?
        .spec
        .to_env_value()
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

fn provider_run_start_diagnostic(context: &ProviderRunDiagnosticContext<'_>) -> String {
    let mut lines = vec![
        format!("provider {} command running", context.provider),
        format!("model: {}", context.model.unwrap_or("(default)")),
        format!("command: {}", context.command.display()),
        format!(
            "sandbox: {}",
            if context.sandbox_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!(
            "timeout: {}",
            context
                .timeout
                .map(format_timeout_duration)
                .unwrap_or_else(|| "none".into())
        ),
        "status: running".into(),
    ];
    if let Some(provider_debug_file) = context.provider_debug_file {
        lines.push(format!("debug_file: {}", provider_debug_file.display()));
    }
    lines.join("\n")
}

fn provider_run_diagnostic(
    context: &ProviderRunDiagnosticContext<'_>,
    result: ProviderRunDiagnosticResult<'_>,
) -> String {
    let mut lines = vec![
        format!("provider {} command completed", context.provider),
        format!("model: {}", context.model.unwrap_or("(default)")),
        format!("command: {}", context.command.display()),
        format!(
            "sandbox: {}",
            if context.sandbox_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!(
            "timeout: {}",
            context
                .timeout
                .map(format_timeout_duration)
                .unwrap_or_else(|| "none".into())
        ),
        format!("status: {}", result.status),
        format!("duration_ms: {}", result.duration_ms),
        format!("timed_out: {}", result.timed_out),
        format!("stdout: {}", provider_stream_state(result.stdout)),
        format!("stderr: {}", provider_stream_state(result.stderr)),
    ];
    if let Some(provider_debug_file) = context.provider_debug_file {
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
) {
    session.messages.push(AgentMessage {
        role: "provider.sandbox".into(),
        content: sandbox_diagnostic(status, stderr),
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

fn sandbox_diagnostic(status: std::process::ExitStatus, stderr: &str) -> String {
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
    lines.join("\n")
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
                self.run_prompt_in_dir_with_sandbox(PromptRunRequest {
                    session,
                    run_sink: context.run_sink,
                    prompt: &prompt,
                    args: &args,
                    current_dir: Some(&repo_dir),
                    review_output_path: None,
                    sandbox: &sandbox,
                    timeout: None,
                    provider_debug_file: None,
                })?;
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
                self.run_prompt_in_dir_with_sandbox(PromptRunRequest {
                    session,
                    run_sink: context.run_sink,
                    prompt: &prompt,
                    args: &args,
                    current_dir: Some(&repo_dir),
                    review_output_path: Some(&output_path),
                    sandbox: &sandbox,
                    timeout: None,
                    provider_debug_file: None,
                })?;
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
        self.run_prompt_in_dir_with_sandbox(PromptRunRequest {
            session,
            run_sink: context.run_sink,
            prompt: &chat_prompt(self.model.as_deref(), input),
            args: &args,
            current_dir: repo_dir.as_deref(),
            review_output_path: None,
            sandbox: &sandbox,
            timeout: input
                .provider_timeout_ms
                .map(std::time::Duration::from_millis),
            provider_debug_file: input.provider_debug_file.as_deref(),
        })
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

#[cfg(test)]
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
        assert!(sandbox_diagnostic(status, "").contains("provider stderr was empty"));
        assert!(sandbox_diagnostic(status, "").contains("--no-sandbox"));
    }

    #[test]
    fn provider_sandbox_plan_prepares_runtime_env_and_capabilities() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("provider");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let runtime_dir = dir.path().join("runtime");

        let plan = crate::provider_sandbox::ProviderSandboxPlan::prepare(
            &AgentProviderKind::Claude,
            &repo_dir,
            &provider_command,
            &CommandSandboxConfig::nono()
                .with_provider_runtime_dir(&runtime_dir)
                .without_nono_profile_updates(),
        )
        .expect("plan");

        let tmp = runtime_dir.join("tmp").join("claude");
        assert_eq!(
            plan.env,
            vec![("CLAUDE_CODE_TMPDIR", tmp.clone()), ("TMPDIR", tmp.clone())]
        );
        assert!(tmp.is_dir());
        assert!(plan.spec.read_paths.contains(&repo_dir));
        assert!(plan.spec.read_paths.contains(&provider_command));
        assert!(plan.spec.read_write_paths.contains(&runtime_dir));
    }

    #[test]
    fn sandboxed_provider_command_sets_owned_runtime_env() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo_dir = dir.path().join("repo");
        fs_err::create_dir(&repo_dir).expect("repo dir");
        let provider_command = dir.path().join("claude");
        fs_err::write(&provider_command, "#!/bin/sh\n").expect("provider command");
        let runtime_dir = dir.path().join("runtime");
        let sandbox = CommandSandboxConfig::nono()
            .with_provider_runtime_dir(&runtime_dir)
            .without_nono_profile_updates();
        let provider =
            CommandAgentProvider::new(AgentProviderKind::Claude, None, &provider_command)
                .with_sandbox(sandbox);

        let command = provider
            .command_for_with_sandbox(Some(repo_dir.as_path()), &[], &provider.sandbox)
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
        let tmp = runtime_dir.join("tmp").join("claude");
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
        let runtime_dir = dir.path().join("runtime");
        let sandbox = CommandSandboxConfig::nono()
            .with_provider_runtime_dir(&runtime_dir)
            .without_nono_profile_updates();
        let provider = CommandAgentProvider::new(AgentProviderKind::Codex, None, &provider_command)
            .with_sandbox(sandbox);

        let command = provider
            .command_for_with_sandbox(Some(repo_dir.as_path()), &[], &provider.sandbox)
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
        let tmp = runtime_dir.join("tmp").join("codex");
        assert!(!envs.contains_key("CODEX_HOME"));
        assert_eq!(
            envs.get("TMPDIR"),
            Some(&tmp.to_string_lossy().into_owned())
        );
        assert!(tmp.is_dir());
    }
}
