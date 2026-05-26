use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{Activity, ActivityOutput, ActivityStatus, ProviderDiagnosticInput};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

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
            let config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            let display = ProviderDiagnosticDisplay::new(
                &config,
                provider.as_ref(),
                model.as_deref(),
                options.disable_sandbox,
            );
            print_provider_diagnostic_start(&display)?;
            let activity = client.provider_diagnostic(&ProviderDiagnosticInput {
                repo_dir: context.repo_dir,
                provider,
                model,
                disable_sandbox: options.disable_sandbox,
            })?;
            print_provider_diagnostic_activity(&activity)?;
            let activity = wait_for_provider_diagnostic(&client, activity)?;
            Ok(format_provider_diagnostic(&activity, &display))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderDiagnosticDisplay {
    provider: String,
    command: String,
    model: String,
    sandbox: String,
}

impl ProviderDiagnosticDisplay {
    fn new(
        config: &nitpick_agent_host::AgentConfig,
        provider: Option<&nitpick_agent_core::AgentProviderKind>,
        model: Option<&str>,
        sandbox_disabled: bool,
    ) -> Self {
        let provider = provider.unwrap_or(&config.provider);
        let command = config
            .command
            .as_deref()
            .unwrap_or_else(|| provider.as_str());
        let model = model.or(config.model.as_deref()).unwrap_or("(default)");
        let sandbox = if sandbox_disabled {
            "none (--no-sandbox)"
        } else {
            config.sandbox.mode.as_str()
        };
        Self {
            provider: provider.as_str().into(),
            command: command.into(),
            model: model.into(),
            sandbox: sandbox.into(),
        }
    }
}

fn print_provider_diagnostic_start(display: &ProviderDiagnosticDisplay) -> Result<(), CliError> {
    let rows = vec![
        vec![crate::style::label("provider"), display.provider.clone()],
        vec![crate::style::label("command"), display.command.clone()],
        vec![crate::style::label("model"), display.model.clone()],
        vec![crate::style::label("sandbox"), display.sandbox.clone()],
        vec![
            crate::style::label("prompt"),
            "Hi. Reply with exactly: OK".into(),
        ],
        vec![
            crate::style::label("checks"),
            "provider command, prompt delivery, sandbox, stdout/stderr capture".into(),
        ],
    ];
    println!(
        "{}",
        crate::activity::format_section("Starting provider diagnostic", crate::style::table(rows))
    );
    io::stdout()
        .flush()
        .map_err(|error| CliError::from(format!("flush diagnostic progress: {error}")))
}

fn print_provider_diagnostic_activity(activity: &Activity) -> Result<(), CliError> {
    println!();
    println!(
        "{}",
        crate::activity::format_section(
            "Diagnostic activity",
            crate::style::table(vec![
                vec![crate::style::label("activity"), activity.id.to_string()],
                vec![
                    crate::style::label("status"),
                    crate::style::status_title(&activity.status)
                ],
                vec![
                    crate::style::label("logs"),
                    format!("nitpick debug logs {}", activity.id),
                ],
            ]),
        )
    );
    println!();
    println!("Waiting for provider result...");
    io::stdout()
        .flush()
        .map_err(|error| CliError::from(format!("flush diagnostic progress: {error}")))
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

fn format_provider_diagnostic(activity: &Activity, display: &ProviderDiagnosticDisplay) -> String {
    let provider = activity
        .session
        .provider
        .as_ref()
        .map(|provider| provider.as_str())
        .unwrap_or(display.provider.as_str());
    let title = if matches!(
        activity.status,
        ActivityStatus::Queued | ActivityStatus::Running
    ) {
        "Diagnostic still running"
    } else {
        "Diagnostic"
    };
    let rows = vec![
        vec![crate::style::label("activity"), activity.id.to_string()],
        vec![crate::style::label("provider"), provider.into()],
        vec![
            crate::style::label("command"),
            crate::activity::provider_run_field(activity, "command")
                .unwrap_or_else(|| display.command.clone()),
        ],
        vec![
            crate::style::label("model"),
            crate::activity::provider_run_field(activity, "model")
                .unwrap_or_else(|| display.model.clone()),
        ],
        vec![
            crate::style::label("sandbox"),
            crate::activity::provider_run_field(activity, "sandbox")
                .unwrap_or_else(|| display.sandbox.clone()),
        ],
        vec![
            crate::style::label("status"),
            crate::style::status_title(&activity.status),
        ],
    ];
    let mut sections = vec![crate::activity::format_section(
        title,
        crate::style::table(rows),
    )];
    if matches!(
        activity.status,
        ActivityStatus::Queued | ActivityStatus::Running
    ) {
        sections.push(crate::activity::format_section(
            "Status",
            format!(
                "provider diagnostic is still running after {}s\ncheck logs: nitpick debug logs {}",
                PROVIDER_DIAGNOSTIC_WAIT.as_secs(),
                activity.id
            ),
        ));
    }
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
    if activity.status != ActivityStatus::Completed
        && let Some(debug_file) = crate::activity::provider_debug_file(activity)
    {
        sections.push(crate::activity::format_section(
            "Provider debug file",
            crate::activity::format_provider_debug_file(&debug_file),
        ));
    }
    sections.push(crate::activity::format_section(
        "Logs",
        format!("nitpick debug logs {}", activity.id),
    ));
    sections.join("\n\n")
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

    #[test]
    fn formats_running_provider_diagnostic_with_expected_values() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-24"),
            nitpick_agent_core::ActivityKind::Chat,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Running;
        let display = super::ProviderDiagnosticDisplay {
            provider: "claude".into(),
            command: "/opt/homebrew/bin/claude".into(),
            model: "claude-opus-4-6".into(),
            sandbox: "macos-seatbelt".into(),
        };

        assert_eq!(
            super::format_provider_diagnostic(&activity, &display),
            "Diagnostic still running\n  \u{1b}[2mactivity\u{1b}[0m  activity-24\n  \u{1b}[2mprovider\u{1b}[0m  claude\n  \u{1b}[2mcommand\u{1b}[0m   /opt/homebrew/bin/claude\n  \u{1b}[2mmodel\u{1b}[0m     claude-opus-4-6\n  \u{1b}[2msandbox\u{1b}[0m   macos-seatbelt\n  \u{1b}[2mstatus\u{1b}[0m    \u{1b}[34mRunning\u{1b}[0m\n\nStatus\n  provider diagnostic is still running after 60s\n  check logs: nitpick debug logs activity-24\n\nLogs\n  nitpick debug logs activity-24"
        );
    }

    #[test]
    fn completed_provider_diagnostic_omits_debug_file_details() {
        let dir = tempfile::tempdir().expect("temp dir");
        let debug_file = dir.path().join("provider-debug.log");
        std::fs::write(&debug_file, "debug noise").expect("debug file");
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-28"),
            nitpick_agent_core::ActivityKind::Chat,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Completed;
        activity.output = Some(nitpick_agent_core::ActivityOutput::Chat("OK".into()));
        activity.session.provider = Some(nitpick_agent_core::AgentProviderKind::Claude);
        activity.session.messages = vec![nitpick_agent_core::AgentMessage {
            role: "provider.run".into(),
            content: format!(
                "provider claude command completed\ncommand: claude\nmodel: claude-opus-4-6\nsandbox: enabled\ndebug_file: {}",
                debug_file.display()
            ),
        }];
        let display = super::ProviderDiagnosticDisplay {
            provider: "claude".into(),
            command: "claude".into(),
            model: "claude-opus-4-6".into(),
            sandbox: "macos-seatbelt".into(),
        };

        let output = super::format_provider_diagnostic(&activity, &display);

        assert!(output.contains("Output\n  OK"));
        assert!(!output.contains("Provider debug file"));
        assert!(output.contains("nitpick debug logs activity-28"));
    }
}
