use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DebugCommand {
    Activities,
    Logs { target: String },
    Artifacts { activity_id: String },
    Artifact { artifact_id: String },
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
    Logs { target: String },
    Artifacts { activity_id: String },
    Artifact { artifact_id: String },
}

impl From<DebugSubcommand> for DebugCommand {
    fn from(command: DebugSubcommand) -> Self {
        match command {
            DebugSubcommand::Activities => Self::Activities,
            DebugSubcommand::Logs { target } => Self::Logs { target },
            DebugSubcommand::Artifacts { activity_id } => Self::Artifacts { activity_id },
            DebugSubcommand::Artifact { artifact_id } => Self::Artifact { artifact_id },
        }
    }
}

pub fn run(
    command: DebugCommand,
    context: CliRunContext,
    _options: CliOptions,
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
            Ok(crate::activity::format_activity_logs(activity, &artifacts))
        }
        DebugCommand::Artifacts { activity_id } => Ok(crate::artifact::format_artifacts(
            &client.activity_artifacts(&activity_id)?,
        )),
        DebugCommand::Artifact { artifact_id } => Ok(crate::artifact::format_artifact(
            &client.artifact(&artifact_id)?,
        )),
    }
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
}
