use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{Activity, ActivityOutput, ActivityStatus, ProviderDiagnosticInput};
use std::time::{Duration, Instant};

use crate::{CliError, CliOptions, CliRunContext};

const PROVIDER_DIAGNOSTIC_WAIT: Duration = Duration::from_secs(60);
const PROVIDER_DIAGNOSTIC_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DebugCommand {
    Activities,
    Logs {
        target: String,
    },
    Artifacts {
        activity_id: String,
    },
    Artifact {
        artifact_id: String,
    },
    Provider {
        provider: Option<String>,
        model: Option<String>,
    },
}

#[derive(Args)]
pub struct DebugArgs {
    #[command(subcommand)]
    pub command: DebugSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum DebugSubcommand {
    Activities,
    Logs {
        target: String,
    },
    Artifacts {
        activity_id: String,
    },
    Artifact {
        artifact_id: String,
    },
    Provider {
        #[arg(long = "provider")]
        provider: Option<String>,
        #[arg(long = "model")]
        model: Option<String>,
    },
}

impl From<DebugSubcommand> for DebugCommand {
    fn from(command: DebugSubcommand) -> Self {
        match command {
            DebugSubcommand::Activities => Self::Activities,
            DebugSubcommand::Logs { target } => Self::Logs { target },
            DebugSubcommand::Artifacts { activity_id } => Self::Artifacts { activity_id },
            DebugSubcommand::Artifact { artifact_id } => Self::Artifact { artifact_id },
            DebugSubcommand::Provider { provider, model } => Self::Provider { provider, model },
        }
    }
}

pub fn run(
    command: DebugCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        DebugCommand::Activities => Ok(crate::activity::format_activities(&client.activities()?)),
        DebugCommand::Logs { target } if target == "daemon" => {
            crate::activity::format_daemon_log(&crate::activity::daemon_log_path(&context.data_dir))
                .map_err(Into::into)
        }
        DebugCommand::Logs { target } => {
            let activities = client.activities()?;
            let activity = crate::activity::resolve_log_activity(&activities, &target)
                .map_err(CliError::from)?;
            let artifacts = client.activity_artifacts(activity.id.as_str())?;
            Ok(crate::activity::format_activity_debug_logs(
                activity, &artifacts,
            ))
        }
        DebugCommand::Artifacts { activity_id } => Ok(crate::artifact::format_artifacts(
            &client.activity_artifacts(&activity_id)?,
        )),
        DebugCommand::Artifact { artifact_id } => Ok(crate::artifact::format_artifact(
            &client.artifact(&artifact_id)?,
        )),
        DebugCommand::Provider { provider, model } => {
            let provider = provider
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(CliError::from)?;
            let activity = client.provider_diagnostic(&ProviderDiagnosticInput {
                repo_dir: context.repo_dir,
                provider,
                model,
                disable_sandbox: options.disable_sandbox,
            })?;
            let activity = wait_for_provider_diagnostic(&client, activity)?;
            Ok(format_provider_diagnostic(
                &activity,
                options.disable_sandbox,
            ))
        }
    }
}

fn wait_for_provider_diagnostic(
    client: &HostClient,
    mut activity: Activity,
) -> Result<Activity, CliError> {
    let deadline = Instant::now() + PROVIDER_DIAGNOSTIC_WAIT;
    while matches!(
        activity.status,
        ActivityStatus::Queued | ActivityStatus::Running
    ) && Instant::now() < deadline
    {
        std::thread::sleep(PROVIDER_DIAGNOSTIC_POLL_INTERVAL);
        activity = client.activity(activity.id.as_str())?;
    }
    Ok(activity)
}

fn format_provider_diagnostic(activity: &Activity, sandbox_disabled: bool) -> String {
    let provider = activity
        .session
        .provider
        .as_ref()
        .map(|provider| provider.as_str())
        .unwrap_or("(unknown)");
    let rows = vec![
        vec![crate::style::label("activity"), activity.id.to_string()],
        vec![crate::style::label("provider"), provider.into()],
        vec![
            crate::style::label("model"),
            provider_run_field(activity, "model").unwrap_or_else(|| "(unknown)".into()),
        ],
        vec![
            crate::style::label("sandbox"),
            provider_run_field(activity, "sandbox").unwrap_or_else(|| {
                if sandbox_disabled {
                    "disabled".into()
                } else {
                    "configured".into()
                }
            }),
        ],
        vec![
            crate::style::label("status"),
            crate::style::status_title(&activity.status),
        ],
    ];
    let mut sections = vec![crate::activity::format_section(
        "Diagnostic",
        crate::style::table(rows),
    )];
    if let Some(error) = &activity.error {
        sections.push(crate::activity::format_section(
            "Error",
            crate::style::error(error),
        ));
    }
    if let Some(ActivityOutput::Chat(output)) = &activity.output {
        sections.push(crate::activity::format_section("Output", output.clone()));
    } else if activity.status == ActivityStatus::Completed {
        sections.push(crate::activity::format_section("Output", "empty"));
    }
    sections.push(crate::activity::format_section(
        "Logs",
        format!("nitpick debug logs {}", activity.id),
    ));
    sections.join("\n\n")
}

fn provider_run_field(activity: &Activity, field: &str) -> Option<String> {
    let prefix = format!("{field}: ");
    activity
        .session
        .messages
        .iter()
        .filter(|message| message.role == "provider.run")
        .flat_map(|message| message.content.lines())
        .find_map(|line| line.strip_prefix(&prefix).map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::DebugCommand;
    use crate::{CliCommand, parse_command};

    #[test]
    fn parses_debug_activities_command() {
        let command =
            parse_command(["debug".to_owned(), "activities".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Debug(DebugCommand::Activities));
    }

    #[test]
    fn parses_debug_logs_command() {
        let command = parse_command([
            "debug".to_owned(),
            "logs".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Debug(DebugCommand::Logs {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn parses_debug_artifacts_command() {
        let command = parse_command([
            "debug".to_owned(),
            "artifacts".to_owned(),
            "activity-1".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Debug(DebugCommand::Artifacts {
                activity_id: "activity-1".into(),
            })
        );
    }

    #[test]
    fn parses_debug_artifact_command() {
        let command = parse_command([
            "debug".to_owned(),
            "artifact".to_owned(),
            "artifact-1".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Debug(DebugCommand::Artifact {
                artifact_id: "artifact-1".into(),
            })
        );
    }

    #[test]
    fn parses_debug_provider_command() {
        let command = parse_command([
            "debug".to_owned(),
            "provider".to_owned(),
            "--provider".to_owned(),
            "codex".to_owned(),
            "--model".to_owned(),
            "gpt-5.3-codex".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Debug(DebugCommand::Provider {
                provider: Some("codex".into()),
                model: Some("gpt-5.3-codex".into()),
            })
        );
    }
}
