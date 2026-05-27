use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{CleanupCheckoutsResult, HostStatus, LocalStateResetResult};

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    SyncPending { destination: Option<String> },
    CleanupCheckouts,
    Reset { force: bool },
}

#[derive(Args)]
pub struct SystemArgs {
    #[command(subcommand)]
    pub command: SystemSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum SystemSubcommand {
    SyncPending {
        destination: Option<String>,
    },
    CleanupCheckouts,
    Reset {
        #[arg(long)]
        force: bool,
    },
}

impl From<SystemSubcommand> for SystemCommand {
    fn from(command: SystemSubcommand) -> Self {
        match command {
            SystemSubcommand::SyncPending { destination } => Self::SyncPending { destination },
            SystemSubcommand::CleanupCheckouts => Self::CleanupCheckouts,
            SystemSubcommand::Reset { force } => Self::Reset { force },
        }
    }
}

pub fn run(
    command: SystemCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        SystemCommand::SyncPending { destination } => Ok(crate::artifact::format_artifacts(
            &client.pending_sync_artifacts(destination.as_deref())?,
        )),
        SystemCommand::CleanupCheckouts => {
            Ok(format_cleanup_checkouts(&client.cleanup_checkouts()?))
        }
        SystemCommand::Reset { force } => {
            match options.reset_confirmation {
                Some(crate::Confirmation::Yes) => {}
                Some(crate::Confirmation::No) => return Ok("reset cancelled".into()),
                None => {
                    return Err(CliError::from(
                        "refusing to reset local state without interactive confirmation",
                    ));
                }
            }
            Ok(format_local_state_reset(&client.reset_local_state(force)?))
        }
    }
}

pub fn status(context: CliRunContext) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match client.status() {
        Ok(status) => Ok(format_host_status(&status)),
        Err(error) if error.is_unavailable() => Ok(format!(
            "{} {}\n  {} {}",
            crate::style::label("host"),
            crate::style::error("not connected"),
            crate::style::label("address"),
            context.host_addr
        )),
        Err(error) => Err(error.into()),
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
    let review_source_rows = if status.review_source_enabled {
        let poll_age = status
            .review_source_last_poll_unix
            .map(|t| format_poll_age(t, now_unix))
            .unwrap_or_else(|| "never".into());
        let summary = status
            .review_source_last_poll_summary
            .as_deref()
            .unwrap_or("—");
        vec![
            vec![
                crate::style::label("name"),
                status.review_source_name.clone(),
            ],
            vec![
                crate::style::label("status"),
                crate::style::success("enabled"),
            ],
            vec![crate::style::label("last poll"), poll_age],
            vec![crate::style::label("summary"), summary.into()],
        ]
    } else {
        vec![
            vec![
                crate::style::label("name"),
                status.review_source_name.clone(),
            ],
            vec![
                crate::style::label("status"),
                crate::style::label("disabled"),
            ],
        ]
    };
    [
        format_status_section(
            "Host",
            vec![vec![
                crate::style::label("status"),
                crate::style::success("connected"),
            ]],
        ),
        format_status_section(
            "Reviews",
            vec![
                vec![
                    crate::style::label("open"),
                    format_count(status.open_review_count, false),
                ],
                vec![
                    crate::style::label("running"),
                    format_count(status.running_review_count, status.running_review_count > 0),
                ],
                vec![
                    crate::style::label("queued"),
                    format_count(status.queued_review_count, status.queued_review_count > 0),
                ],
            ],
        ),
        format_status_section(
            "History",
            vec![
                vec![
                    crate::style::label("completed"),
                    format_count(status.completed_review_count, false),
                ],
                vec![
                    crate::style::label("errored"),
                    format_count(status.error_review_count, status.error_review_count > 0),
                ],
            ],
        ),
        format_status_section(
            "Artifacts",
            vec![
                vec![
                    crate::style::label("total"),
                    format_count(status.artifact_count, false),
                ],
                vec![
                    crate::style::label("pending"),
                    format_count(
                        status.pending_sync_artifact_count,
                        status.pending_sync_artifact_count > 0,
                    ),
                ],
            ],
        ),
        format_status_section(
            "Agent",
            vec![
                vec![crate::style::label("provider"), status.provider.to_string()],
                vec![
                    crate::style::label("model"),
                    status.model.as_deref().unwrap_or("(default)").into(),
                ],
            ],
        ),
        format_status_section("Review source", review_source_rows),
    ]
    .join("\n\n")
}

fn format_status_section(title: &str, rows: Vec<Vec<String>>) -> String {
    let body = crate::style::table(rows)
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}\n{body}")
}

fn format_count(count: usize, highlight: bool) -> String {
    if highlight {
        crate::style::warn(count)
    } else {
        count.to_string()
    }
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
        "cleaned up {} checkout(s)\n  {}",
        result.removed_count,
        result.cleaned.join("\n  ")
    )
}

pub fn format_local_state_reset(result: &LocalStateResetResult) -> String {
    let rows = vec![
        vec![
            crate::style::label("activities"),
            result.removed_activity_count.to_string(),
        ],
        vec![
            crate::style::label("artifacts"),
            result.removed_artifact_count.to_string(),
        ],
        vec![
            crate::style::label("processed reviews"),
            result.removed_processed_review_count.to_string(),
        ],
        vec![
            crate::style::label("checkouts"),
            result.removed_checkout_count.to_string(),
        ],
        vec![
            crate::style::label("daemon log"),
            if result.truncated_log {
                "truncated".into()
            } else {
                "unchanged".into()
            },
        ],
    ];
    format!(
        "{}\n{}",
        crate::style::success("local state reset"),
        crate::style::table(rows)
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub fn host_status_url(addr: &str) -> String {
    format!("http://{addr}/status")
}

#[cfg(test)]
mod tests {
    use nitpick_agent_core::{AgentProviderKind, CleanupCheckoutsResult};

    use super::{
        HostStatus, SystemCommand, format_host_status, format_local_state_reset, host_status_url,
    };
    use crate::{CliCommand, CliOptions, Confirmation, parse_command};

    #[test]
    fn rejects_system_status_command() {
        let error =
            parse_command(["system".to_owned(), "status".to_owned()]).expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'status'"));
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
    fn parses_reset_command() {
        let command = parse_command([
            "system".to_owned(),
            "reset".to_owned(),
            "--force".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::System(SystemCommand::Reset { force: true })
        );
    }

    #[test]
    fn rejects_reset_confirm_flag() {
        let error = parse_command([
            "system".to_owned(),
            "reset".to_owned(),
            "--confirm".to_owned(),
        ])
        .expect_err("command fails");

        assert!(error.contains("unexpected argument '--confirm'"));
    }

    #[test]
    fn reset_cancelled_by_confirmation_does_not_contact_host() {
        let output = super::run(
            SystemCommand::Reset { force: false },
            crate::CliRunContext {
                host_addr: "127.0.0.1:1".into(),
                repo_dir: std::path::PathBuf::new(),
                diff: String::new(),
                context: String::new(),
                config_path: std::path::PathBuf::new(),
                data_dir: std::path::PathBuf::new(),
            },
            CliOptions {
                reset_confirmation: Some(Confirmation::No),
                ..CliOptions::default()
            },
        )
        .expect("cancelled reset");

        assert_eq!(output, "reset cancelled");
    }

    #[test]
    fn formats_cleanup_checkouts_result() {
        assert_eq!(
            super::format_cleanup_checkouts(&CleanupCheckoutsResult {
                removed_count: 1,
                cleaned: vec!["acme/platform#42".into()],
            }),
            "cleaned up 1 checkout(s)\n  acme/platform#42"
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
    fn formats_local_state_reset_result() {
        assert_eq!(
            format_local_state_reset(&nitpick_agent_core::LocalStateResetResult {
                removed_activity_count: 2,
                removed_artifact_count: 3,
                removed_processed_review_count: 4,
                removed_checkout_count: 1,
                truncated_log: true,
            }),
            "\u{1b}[32mlocal state reset\u{1b}[0m\n  \u{1b}[2mactivities\u{1b}[0m         2\n  \u{1b}[2martifacts\u{1b}[0m          3\n  \u{1b}[2mprocessed reviews\u{1b}[0m  4\n  \u{1b}[2mcheckouts\u{1b}[0m          1\n  \u{1b}[2mdaemon log\u{1b}[0m         truncated"
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
            attention: None,
        };

        assert_eq!(
            format_host_status(&status),
            "Host\n  \u{1b}[2mstatus\u{1b}[0m  \u{1b}[32mconnected\u{1b}[0m\n\nReviews\n  \u{1b}[2mopen\u{1b}[0m     4\n  \u{1b}[2mrunning\u{1b}[0m  \u{1b}[33m2\u{1b}[0m\n  \u{1b}[2mqueued\u{1b}[0m   \u{1b}[33m1\u{1b}[0m\n\nHistory\n  \u{1b}[2mcompleted\u{1b}[0m  3\n  \u{1b}[2merrored\u{1b}[0m    0\n\nArtifacts\n  \u{1b}[2mtotal\u{1b}[0m    5\n  \u{1b}[2mpending\u{1b}[0m  \u{1b}[33m1\u{1b}[0m\n\nAgent\n  \u{1b}[2mprovider\u{1b}[0m  claude\n  \u{1b}[2mmodel\u{1b}[0m     sonnet\n\nReview source\n  \u{1b}[2mname\u{1b}[0m    github\n  \u{1b}[2mstatus\u{1b}[0m  \u{1b}[2mdisabled\u{1b}[0m"
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
            attention: None,
        };

        let output = super::format_host_status_at(&status, 1_000);
        assert!(
            output.contains("Review source")
                && output.contains("\u{1b}[2mname\u{1b}[0m")
                && output.contains("github")
                && output.contains("\u{1b}[2mlast poll\u{1b}[0m")
                && output.contains("never"),
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
                attention: None,
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
