use clap::{Args, Subcommand};
use nitpick_agent_client::{CleanupCheckoutsResult, HostClient};
use serde::Deserialize;

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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostStatus {
    pub activity_count: usize,
    pub running_activity_count: usize,
    pub completed_activity_count: usize,
    pub error_activity_count: usize,
    pub artifact_count: usize,
    pub local_only_artifact_count: usize,
    pub pending_sync_artifact_count: usize,
    pub provider: String,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

pub fn run(
    command: SystemCommand,
    context: CliRunContext,
    _options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        SystemCommand::Status => match client.status() {
            Ok(status) => Ok(format_host_status(&host_status(status))),
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
    format!(
        "nitpick-agent-host: connected\nactivities: {} ({} running, {} completed, {} error)\nartifacts: {}\nlocal-only artifacts: {}\npending-sync artifacts: {}\nagent: {}\nmodel: {}",
        status.activity_count,
        status.running_activity_count,
        status.completed_activity_count,
        status.error_activity_count,
        status.artifact_count,
        status.local_only_artifact_count,
        status.pending_sync_artifact_count,
        status.provider,
        status.model.as_deref().unwrap_or("(default)")
    )
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

fn host_status(status: nitpick_agent_client::HostStatus) -> HostStatus {
    HostStatus {
        activity_count: status.activity_count,
        running_activity_count: status.running_activity_count,
        completed_activity_count: status.completed_activity_count,
        error_activity_count: status.error_activity_count,
        artifact_count: status.artifact_count,
        local_only_artifact_count: status.local_only_artifact_count,
        pending_sync_artifact_count: status.pending_sync_artifact_count,
        provider: status.provider,
        model: status.model,
        review_source_name: status.review_source_name,
        review_source_enabled: status.review_source_enabled,
        review_source_last_poll_unix: status.review_source_last_poll_unix,
        review_source_last_poll_summary: status.review_source_last_poll_summary,
    }
}

#[cfg(test)]
mod tests {
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
            super::format_cleanup_checkouts(&nitpick_agent_client::CleanupCheckoutsResult {
                removed_count: 1,
                cleaned: vec!["acme/platform#42".into()],
            }),
            "cleaned up 1 checkout(s)\nacme/platform#42"
        );
        assert_eq!(
            super::format_cleanup_checkouts(&nitpick_agent_client::CleanupCheckoutsResult {
                removed_count: 0,
                cleaned: Vec::new(),
            }),
            "no checkouts cleaned up"
        );
    }

    #[test]
    fn formats_host_status() {
        let status = HostStatus {
            activity_count: 2,
            running_activity_count: 1,
            completed_activity_count: 1,
            error_activity_count: 0,
            artifact_count: 5,
            local_only_artifact_count: 3,
            pending_sync_artifact_count: 1,
            provider: "claude".into(),
            model: Some("sonnet".into()),
            review_source_name: "github".into(),
            review_source_enabled: true,
            review_source_last_poll_unix: Some(1_000),
            review_source_last_poll_summary: Some("reviewed 1 of 1 PRs".into()),
        };

        assert_eq!(
            format_host_status(&status),
            "nitpick-agent-host: connected\nactivities: 2 (1 running, 1 completed, 0 error)\nartifacts: 5\nlocal-only artifacts: 3\npending-sync artifacts: 1\nagent: claude\nmodel: sonnet"
        );
    }

    #[test]
    fn parses_host_status_json() {
        let status = super::parse_host_status_json(
            r#"{"activity_count":2,"running_activity_count":1,"completed_activity_count":1,"error_activity_count":0,"artifact_count":5,"local_only_artifact_count":3,"pending_sync_artifact_count":1,"provider":"claude","model":null,"review_source_name":"github","review_source_enabled":true,"review_source_last_poll_unix":1000,"review_source_last_poll_summary":"reviewed 1 of 1 PRs"}"#,
        )
        .expect("status parses");

        assert_eq!(
            status,
            HostStatus {
                activity_count: 2,
                running_activity_count: 1,
                completed_activity_count: 1,
                error_activity_count: 0,
                artifact_count: 5,
                local_only_artifact_count: 3,
                pending_sync_artifact_count: 1,
                provider: "claude".into(),
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
