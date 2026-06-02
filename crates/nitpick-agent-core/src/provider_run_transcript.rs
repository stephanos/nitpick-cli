use std::{path::Path, time::Duration};

use crate::{AgentMessage, AgentProviderKind, AgentSession};

pub struct ProviderRunTranscriptContext<'a> {
    pub provider: &'a AgentProviderKind,
    pub model: Option<&'a str>,
    pub command: &'a Path,
    pub sandbox_enabled: bool,
    pub timeout: Option<Duration>,
    pub provider_debug_file: Option<&'a Path>,
}

pub(crate) struct ProviderRunTranscriptResult<'a> {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) duration_ms: u128,
    pub(crate) timed_out: bool,
    pub(crate) stdout: &'a [u8],
    pub(crate) stderr: &'a [u8],
}

impl ProviderRunTranscriptContext<'_> {
    pub fn start_diagnostic(&self) -> String {
        let mut lines = vec![
            format!("provider {} command running", self.provider),
            format!("model: {}", self.model.unwrap_or("(default)")),
            format!("command: {}", self.command.display()),
            format!(
                "sandbox: {}",
                if self.sandbox_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            format!(
                "timeout: {}",
                self.timeout
                    .map(format_timeout_duration)
                    .unwrap_or_else(|| "none".into())
            ),
            "status: running".into(),
        ];
        if let Some(provider_debug_file) = self.provider_debug_file {
            lines.push(format!("debug_file: {}", provider_debug_file.display()));
        }
        lines.join("\n")
    }

    pub(crate) fn completion_diagnostic(&self, result: ProviderRunTranscriptResult<'_>) -> String {
        let mut lines = vec![
            format!("provider {} command completed", self.provider),
            format!("model: {}", self.model.unwrap_or("(default)")),
            format!("command: {}", self.command.display()),
            format!(
                "sandbox: {}",
                if self.sandbox_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            format!(
                "timeout: {}",
                self.timeout
                    .map(format_timeout_duration)
                    .unwrap_or_else(|| "none".into())
            ),
            format!("status: {}", result.status),
            format!("duration_ms: {}", result.duration_ms),
            format!("timed_out: {}", result.timed_out),
            format!("stdout: {}", provider_stream_state(result.stdout)),
            format!("stderr: {}", provider_stream_state(result.stderr)),
        ];
        if let Some(provider_debug_file) = self.provider_debug_file {
            lines.push(format!("debug_file: {}", provider_debug_file.display()));
        }
        lines.join("\n")
    }
}

pub(crate) fn record_provider_sandbox_diagnostic(
    session: &mut AgentSession,
    status: std::process::ExitStatus,
    stderr: &str,
) {
    session.messages.push(AgentMessage {
        role: "provider.sandbox".into(),
        content: sandbox_diagnostic(status, stderr),
    });
}

pub(crate) fn provider_failure_hint(stderr: &str, sandbox_enabled: bool) -> String {
    if provider_session_already_in_use(stderr) {
        "; provider session is already in use, wait for the active Claude process to finish or stop the stale provider process before retrying".into()
    } else {
        sandbox_failure_hint(sandbox_enabled)
    }
}

pub(crate) fn provider_session_already_in_use(stderr: &str) -> bool {
    stderr.contains("Session ID") && stderr.contains("already in use")
}

fn provider_stream_state(bytes: &[u8]) -> &'static str {
    if bytes.is_empty() {
        "empty"
    } else {
        "captured"
    }
}

pub(crate) fn format_timeout_duration(timeout: Duration) -> String {
    if timeout.as_millis() < 1_000 {
        format!("{}ms", timeout.as_millis())
    } else {
        format!("{}s", timeout.as_secs())
    }
}

pub(crate) fn sandbox_failure_hint(sandbox_enabled: bool) -> String {
    if sandbox_enabled {
        "; sandbox was enabled, retry with --no-sandbox to determine whether the sandbox is involved"
            .into()
    } else {
        String::new()
    }
}

pub(crate) fn sandbox_diagnostic(status: std::process::ExitStatus, stderr: &str) -> String {
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
