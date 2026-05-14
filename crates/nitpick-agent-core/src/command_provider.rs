use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    AgentError, AgentProvider, AgentProviderKind, AgentResult, AgentSession, ChatInput,
    REVIEW_OUTPUT_RELATIVE_PATH, ReviewInput, ReviewOutput, validate_review_output_file_for_diff,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSandboxConfig {
    pub enabled: bool,
}

impl CommandSandboxConfig {
    pub fn macos_seatbelt() -> Self {
        Self { enabled: true }
    }

    pub fn unsandboxed() -> Self {
        Self { enabled: false }
    }
}

impl Default for CommandSandboxConfig {
    fn default() -> Self {
        Self::macos_seatbelt()
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
            .ok_or_else(|| AgentError::new("activity has no provider session id"))?;
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

    fn run_prompt_in_dir(
        &self,
        prompt: &str,
        args: &[String],
        current_dir: Option<&Path>,
        review_output_path: Option<&Path>,
    ) -> AgentResult<String> {
        self.run_prompt_in_dir_with_sandbox(
            prompt,
            args,
            current_dir,
            review_output_path,
            &self.sandbox,
        )
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
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?;

        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::new("provider command stdin unavailable"))?
            .write_all(prompt.as_bytes())
            .map_err(|error| AgentError::new(format!("write provider prompt: {error}")))?;

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::new(format!("wait for provider command: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::new(format!(
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
                AgentError::new("sandboxed provider execution requires a repository directory")
            })?;
            let provider_command = self.resolved_command()?;
            let mut command = Command::new("sandbox-exec");
            command
                .arg("-p")
                .arg(macos_sandbox_profile(repo_dir, &provider_command)?);
            command.arg(&provider_command);
            command.args(args);
            Ok(command)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = repo_dir;
            let _ = args;
            Err(AgentError::new(
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
        let output = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?
            .wait_with_output()
            .map_err(|error| AgentError::new(format!("wait for provider command: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::new(format!(
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
        sandbox: &CommandSandboxConfig,
    ) -> AgentResult<Option<PathBuf>> {
        if !sandbox.enabled {
            return Ok(None);
        }
        repo_dir
            .canonicalize()
            .map_err(|error| {
                AgentError::new(format!(
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
}

fn resolve_command_path(command: &Path) -> AgentResult<PathBuf> {
    if command.components().count() > 1 || command.is_absolute() {
        return command.canonicalize().map_err(|error| {
            AgentError::new(format!(
                "resolve provider command `{}`: {error}",
                command.display()
            ))
        });
    }

    let path = env::var_os("PATH")
        .ok_or_else(|| AgentError::new("resolve provider command: PATH is not set"))?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(command);
        if is_executable_file(&candidate) {
            return candidate.canonicalize().map_err(|error| {
                AgentError::new(format!(
                    "resolve provider command `{}`: {error}",
                    candidate.display()
                ))
            });
        }
    }

    Err(AgentError::new(format!(
        "provider command `{}` not found on PATH",
        command.display()
    )))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

impl AgentProvider for CommandAgentProvider {
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_sandbox(input.disable_sandbox);
        let repo_dir = input.repo_dir.canonicalize().map_err(|error| {
            AgentError::new(format!(
                "canonicalize review repository {}: {error}",
                input.repo_dir.display()
            ))
        })?;
        let output_path = repo_dir.join(REVIEW_OUTPUT_RELATIVE_PATH);
        std::fs::create_dir_all(output_path.parent().ok_or_else(|| {
            AgentError::new(format!(
                "review output path has no parent: {}",
                output_path.display()
            ))
        })?)
        .map_err(|error| AgentError::new(format!("create review output directory: {error}")))?;
        if output_path.exists() {
            std::fs::remove_file(&output_path)
                .map_err(|error| AgentError::new(format!("remove stale review output: {error}")))?;
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

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String> {
        session.provider = Some(self.kind.clone());
        let sandbox = self.effective_sandbox(input.disable_sandbox);
        let repo_dir = self.sandbox_repo_dir(&input.repo_dir, &sandbox)?;
        self.run_prompt_in_dir_with_sandbox(
            &chat_prompt(self.model.as_deref(), input),
            &[],
            repo_dir.as_deref(),
            None,
            &sandbox,
        )
    }

    fn attach_session(&self, session: &AgentSession) -> AgentResult<()> {
        let session_id = session
            .provider_session_id
            .as_deref()
            .ok_or_else(|| AgentError::new("activity has no provider session id"))?;
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
            _ => Vec::new(),
        }
    }
}

fn review_prompt(model: Option<&str>, input: &ReviewInput, output_path: &str) -> String {
    format!(
        "You are reviewing code. Write review annotations as JSON to `{}` relative to the repository root. Do not return review annotations on stdout.\n\nThe JSON object must contain `summary`, `comments`, and `journey`. Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ninstructions:\n{}\n\ndiff:\n{}\n",
        output_path,
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

fn chat_prompt(model: Option<&str>, input: &ChatInput) -> String {
    format!(
        "You are answering a development question.\n\nmodel: {}\nrepo_dir: {}\ncontext:\n{}\n\nprompt:\n{}\n",
        model.unwrap_or("(default)"),
        input.repo_dir.display(),
        input.context,
        input.prompt,
    )
}

#[cfg(target_os = "macos")]
fn macos_sandbox_profile(repo_dir: &Path, provider_command: &Path) -> AgentResult<String> {
    let repo_dir = repo_dir
        .canonicalize()
        .map_err(|error| AgentError::new(format!("canonicalize sandbox repo dir: {error}")))?;
    let repo = escape_sandbox_string(&repo_dir.to_string_lossy());
    let command = provider_command
        .canonicalize()
        .unwrap_or_else(|_| provider_command.to_path_buf());
    let command = escape_sandbox_string(&command.to_string_lossy());
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
"#
    ))
}

#[cfg(target_os = "macos")]
fn escape_sandbox_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
