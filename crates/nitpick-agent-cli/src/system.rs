use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{CleanupCheckoutsResult, HostStatus};

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    Status,
    SyncPending { destination: Option<String> },
    CleanupCheckouts,
}

#[derive(Args)]
pub struct SystemArgs {
    #[command(subcommand)]
    pub command: SystemSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum SystemSubcommand {
    Status,
    SyncPending { destination: Option<String> },
    CleanupCheckouts,
}

impl From<SystemSubcommand> for SystemCommand {
    fn from(command: SystemSubcommand) -> Self {
        match command {
            SystemSubcommand::Status => Self::Status,
            SystemSubcommand::SyncPending { destination } => Self::SyncPending { destination },
            SystemSubcommand::CleanupCheckouts => Self::CleanupCheckouts,
        }
    }
}

pub fn run(
    command: SystemCommand,
    context: CliRunContext,
    _options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        SystemCommand::Status => match client.status() {
            Ok(status) => Ok(format_host_status(&status)),
            Err(error) if error.is_unavailable() => Ok(format!(
                "nitpick-agent-host: not connected\naddress: {}",
                context.host_addr
            )),
            Err(error) => Err(error.into()),
        },
        SystemCommand::SyncPending { destination } => Ok(crate::artifact::format_artifacts(
            &client.pending_sync_artifacts(destination.as_deref())?,
        )),
        SystemCommand::CleanupCheckouts => {
            Ok(format_cleanup_checkouts(&client.cleanup_checkouts()?))
        }
    }
}

pub fn format_host_status(status: &HostStatus) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_host_status_at(status, now_unix)
}

pub(crate) fn format_host_status_at(status: &HostStatus, now_unix: u64) -> String {
    let reviews_line = format!(
        "open reviews: {}\nactive reviews: {} running, {} queued\nreviews completed: {}\nreviews errored: {}",
        status.open_review_count,
        status.running_review_count,
        status.queued_review_count,
        status.completed_review_count,
        status.error_review_count,
    );
    let review_source_line = if status.review_source_enabled {
        let poll_age = status
            .review_source_last_poll_unix
            .map(|t| format_poll_age(t, now_unix))
            .unwrap_or_else(|| "never".into());
        let summary = status
            .review_source_last_poll_summary
            .as_deref()
            .unwrap_or("—");
        format!(
            "review source: {} (last poll: {}, {})",
            status.review_source_name, poll_age, summary
        )
    } else {
        format!("review source: {} (disabled)", status.review_source_name)
    };
    format!(
        "nitpick-agent-host: connected\n{reviews_line}\nartifacts: {} ({} pending sync)\nagent: {} {}\n{review_source_line}",
        status.artifact_count,
        status.pending_sync_artifact_count,
        status.provider,
        status.model.as_deref().unwrap_or("(default)"),
    )
}

pub(crate) fn format_poll_age(last_poll_unix: u64, now_unix: u64) -> String {
    let age_secs = now_unix.saturating_sub(last_poll_unix);
    if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3600 {
        format!("{}m ago", age_secs / 60)
    } else {
        format!("{}h ago", age_secs / 3600)
    }
}

pub fn parse_host_status_json(body: &str) -> Result<HostStatus, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host status response: {error}"))
}

pub fn format_cleanup_checkouts(result: &CleanupCheckoutsResult) -> String {
    if result.removed_count == 0 {
        return "no checkouts cleaned up".into();
    }
    format!(
        "cleaned up {} checkout(s)\n{}",
        result.removed_count,
        result.cleaned.join("\n")
    )
}

pub fn host_status_url(addr: &str) -> String {
    format!("http://{addr}/status")
}

#[cfg(test)]
mod tests {
    use nitpick_agent_core::{AgentProviderKind, CleanupCheckoutsResult};

    use super::{HostStatus, SystemCommand, format_host_status, host_status_url};
    use crate::{CliCommand, parse_command};

    #[test]
    fn parses_status_command() {
        let command =
            parse_command(["system".to_owned(), "status".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::System(SystemCommand::Status));
    }

    #[test]
    fn parses_sync_pending_command_with_destination() {
        let command = parse_command([
            "system".to_owned(),
            "sync-pending".to_owned(),
            "github".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::System(SystemCommand::SyncPending {
                destination: Some("github".into()),
            })
        );
    }

    #[test]
    fn parses_sync_pending_command_without_destination() {
        let command =
            parse_command(["system".to_owned(), "sync-pending".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::System(SystemCommand::SyncPending { destination: None })
        );
    }

    #[test]
    fn parses_cleanup_checkouts_command() {
        let command =
            parse_command(["system".to_owned(), "cleanup-checkouts".to_owned()]).expect("command");

        assert_eq!(command, CliCommand::System(SystemCommand::CleanupCheckouts));
    }

    #[test]
    fn formats_cleanup_checkouts_result() {
        assert_eq!(
            super::format_cleanup_checkouts(&CleanupCheckoutsResult {
                removed_count: 1,
                cleaned: vec!["acme/platform#42".into()],
            }),
            "cleaned up 1 checkout(s)\nacme/platform#42"
        );
        assert_eq!(
            super::format_cleanup_checkouts(&CleanupCheckoutsResult {
                removed_count: 0,
                cleaned: Vec::new(),
            }),
            "no checkouts cleaned up"
        );
    }

    #[test]
    fn formats_host_status() {
        let status = HostStatus {
            activity_count: 4,
            queued_activity_count: 1,
            running_activity_count: 2,
            completed_activity_count: 1,
            error_activity_count: 0,
            open_review_count: 4,
            queued_review_count: 1,
            running_review_count: 2,
            completed_review_count: 3,
            error_review_count: 0,
            artifact_count: 5,
            local_only_artifact_count: 3,
            pending_sync_artifact_count: 1,
            provider: AgentProviderKind::Claude,
            model: Some("sonnet".into()),
            review_source_name: "github".into(),
            review_source_enabled: false,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
        };

        assert_eq!(
            format_host_status(&status),
            "nitpick-agent-host: connected\nopen reviews: 4\nactive reviews: 2 running, 1 queued\nreviews completed: 3\nreviews errored: 0\nartifacts: 5 (1 pending sync)\nagent: claude sonnet\nreview source: github (disabled)"
        );
    }

    #[test]
    fn formats_host_status_with_review_source_enabled() {
        let status = HostStatus {
            activity_count: 0,
            queued_activity_count: 0,
            running_activity_count: 0,
            completed_activity_count: 0,
            error_activity_count: 0,
            open_review_count: 0,
            queued_review_count: 0,
            running_review_count: 0,
            completed_review_count: 0,
            error_review_count: 0,
            artifact_count: 0,
            local_only_artifact_count: 0,
            pending_sync_artifact_count: 0,
            provider: AgentProviderKind::Claude,
            model: None,
            review_source_name: "github".into(),
            review_source_enabled: true,
            review_source_last_poll_unix: None,
            review_source_last_poll_summary: None,
        };

        let output = super::format_host_status_at(&status, 1_000);
        assert!(
            output.contains("review source: github (last poll: never"),
            "unexpected: {output}"
        );
    }

    #[test]
    fn formats_poll_age() {
        assert_eq!(super::format_poll_age(900, 1000), "1m ago");
        assert_eq!(super::format_poll_age(400, 1000), "10m ago");
        assert_eq!(super::format_poll_age(0, 3700), "1h ago");
        assert_eq!(super::format_poll_age(995, 1000), "5s ago");
    }

    #[test]
    fn parses_host_status_json() {
        let status = super::parse_host_status_json(
            r#"{"activity_count":4,"queued_activity_count":1,"running_activity_count":2,"completed_activity_count":1,"error_activity_count":0,"open_review_count":4,"queued_review_count":1,"running_review_count":2,"completed_review_count":3,"error_review_count":0,"artifact_count":5,"local_only_artifact_count":3,"pending_sync_artifact_count":1,"provider":"claude","model":null,"review_source_name":"github","review_source_enabled":true,"review_source_last_poll_unix":1000,"review_source_last_poll_summary":"reviewed 1 of 1 PRs"}"#,
        )
        .expect("status parses");

        assert_eq!(
            status,
            HostStatus {
                activity_count: 4,
                queued_activity_count: 1,
                running_activity_count: 2,
                completed_activity_count: 1,
                error_activity_count: 0,
                open_review_count: 4,
                queued_review_count: 1,
                running_review_count: 2,
                completed_review_count: 3,
                error_review_count: 0,
                artifact_count: 5,
                local_only_artifact_count: 3,
                pending_sync_artifact_count: 1,
                provider: AgentProviderKind::Claude,
                model: None,
                review_source_name: "github".into(),
                review_source_enabled: true,
                review_source_last_poll_unix: Some(1_000),
                review_source_last_poll_summary: Some("reviewed 1 of 1 PRs".into()),
            }
        );
    }

    #[test]
    fn builds_host_status_url() {
        assert_eq!(
            host_status_url("127.0.0.1:19783"),
            "http://127.0.0.1:19783/status"
        );
    }
}
