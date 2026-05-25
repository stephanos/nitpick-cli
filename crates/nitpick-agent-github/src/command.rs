use std::{
    ffi::OsStr,
    fmt::Debug,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Instant,
};

use nitpick_agent_core::{AgentError, AgentResult, parse_json_bytes};
use serde::de::DeserializeOwned;

pub(crate) struct GitHubCommand {
    command: PathBuf,
}

impl GitHubCommand {
    pub(crate) fn new(command: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.command
    }

    pub(crate) fn output<S>(&self, args: &[S]) -> AgentResult<Output>
    where
        S: AsRef<OsStr> + Debug,
    {
        self.output_with_start_error(args, "failed to start GitHub CLI")
    }

    pub(crate) fn output_with_start_error<S>(
        &self,
        args: &[S],
        start_error: &str,
    ) -> AgentResult<Output>
    where
        S: AsRef<OsStr> + Debug,
    {
        tracing::debug!(command = %self.command.display(), args = ?args, "running GitHub CLI");
        let started = Instant::now();
        let output = Command::new(&self.command)
            .args(args)
            .output()
            .map_err(|error| {
                AgentError::github_cli(format!(
                    "{start_error} `{}`: {error}",
                    self.command.display()
                ))
            })?;
        tracing::debug!(
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "GitHub CLI finished"
        );
        if !output.status.success() {
            return Err(github_cli_status_error(&output));
        }
        Ok(output)
    }

    pub(crate) fn json<T, S>(&self, args: &[S], context: &str) -> AgentResult<T>
    where
        T: DeserializeOwned,
        S: AsRef<OsStr> + Debug,
    {
        let output = self.output(args)?;
        parse_github_json(&output.stdout, context)
    }

    pub(crate) fn json_with_start_error<T, S>(
        &self,
        args: &[S],
        context: &str,
        start_error: &str,
    ) -> AgentResult<T>
    where
        T: DeserializeOwned,
        S: AsRef<OsStr> + Debug,
    {
        let output = self.output_with_start_error(args, start_error)?;
        parse_github_json(&output.stdout, context)
    }

    pub(crate) fn output_with_input<S>(&self, args: &[S], body: &str) -> AgentResult<Output>
    where
        S: AsRef<OsStr> + Debug,
    {
        tracing::debug!(
            command = %self.command.display(),
            args = ?args,
            body_bytes = body.len(),
            "running GitHub CLI with stdin"
        );
        let started = Instant::now();
        let mut child = Command::new(&self.command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::github_cli(format!(
                    "failed to start GitHub CLI `{}`: {error}",
                    self.command.display()
                ))
            })?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::github_cli("GitHub CLI stdin unavailable"))?
            .write_all(body.as_bytes())
            .map_err(|error| AgentError::github_cli(format!("write GitHub body: {error}")))?;
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::github_cli(format!("wait for GitHub CLI: {error}")))?;
        tracing::debug!(
            command = %self.command.display(),
            status = %output.status,
            duration_ms = started.elapsed().as_millis(),
            "GitHub CLI with stdin finished"
        );
        if !output.status.success() {
            return Err(github_cli_status_error(&output));
        }
        Ok(output)
    }

    pub(crate) fn json_with_input<T, S>(
        &self,
        args: &[S],
        body: &str,
        context: &str,
    ) -> AgentResult<T>
    where
        T: DeserializeOwned,
        S: AsRef<OsStr> + Debug,
    {
        let output = self.output_with_input(args, body)?;
        parse_github_json(&output.stdout, context)
    }
}

fn github_cli_status_error(output: &std::process::Output) -> AgentError {
    command_status_error("GitHub CLI", output)
}

pub(crate) fn command_status_error(command: &str, output: &std::process::Output) -> AgentError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let message = format!(
        "{command} failed with status {}{}",
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    );
    if command == "GitHub CLI" {
        if is_github_rate_limit_error(&stderr) {
            let retry_after_seconds = parse_retry_after_seconds(&stderr);
            let retry_hint = retry_after_seconds
                .map(|seconds| format!(" retry after {seconds} seconds."))
                .unwrap_or_else(|| " retry after the rate limit resets.".to_owned());
            return AgentError::github_rate_limited(
                format!("GitHub rate limited the request;{retry_hint} {message}"),
                retry_after_seconds,
            );
        }
        AgentError::github_cli(message)
    } else {
        AgentError::provider(message)
    }
}

pub(crate) fn is_github_rate_limit_error(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("http 429")
        || stderr.contains("status 429")
        || stderr.contains(" 429")
        || stderr.contains("api rate limit exceeded")
        || stderr.contains("secondary rate limit")
}

pub(crate) fn parse_retry_after_seconds(stderr: &str) -> Option<u64> {
    let lower = stderr.to_ascii_lowercase();
    for marker in ["retry-after:", "retry after"] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let rest = &lower[start + marker.len()..];
        if let Some(seconds) = first_number(rest) {
            return Some(seconds);
        }
    }
    None
}

fn first_number(value: &str) -> Option<u64> {
    let start = value.find(|character: char| character.is_ascii_digit())?;
    let digits = value[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

pub(crate) fn parse_github_json<T: DeserializeOwned>(
    bytes: &[u8],
    context: &str,
) -> AgentResult<T> {
    parse_json_bytes(bytes, &format!("invalid {context}"))
}
