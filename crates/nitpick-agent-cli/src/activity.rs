use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_github::GitHubCliDiscovery;

use crate::{
    CliError, CliOptions, CliRunContext, apply_sandbox_option, daemon_log_path,
    ensure_resumable_activity, format_activities, format_activity_logs, format_daemon_log,
    handle_resume_error, inspect_checkout_with_discovery, resolve_log_activity,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityCommand {
    List,
    Logs { target: String },
    Resume { target: String },
    Inspect { pull_request: String },
}

#[derive(Args)]
pub struct ActivityArgs {
    #[command(subcommand)]
    pub command: ActivitySubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ActivitySubcommand {
    List,
    Logs { target: String },
    Resume { target: String },
    Inspect { pull_request: String },
}

impl From<ActivitySubcommand> for ActivityCommand {
    fn from(command: ActivitySubcommand) -> Self {
        match command {
            ActivitySubcommand::List => Self::List,
            ActivitySubcommand::Logs { target } => Self::Logs { target },
            ActivitySubcommand::Resume { target } => Self::Resume { target },
            ActivitySubcommand::Inspect { pull_request } => Self::Inspect { pull_request },
        }
    }
}

pub fn run(
    command: ActivityCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        ActivityCommand::List => Ok(format_activities(&client.activities()?)),
        ActivityCommand::Logs { target } if target == "daemon" => {
            format_daemon_log(&daemon_log_path(&context.data_dir)).map_err(Into::into)
        }
        ActivityCommand::Logs { target } => {
            let activities = client.activities()?;
            let activity = resolve_log_activity(&activities, &target).map_err(CliError::from)?;
            let artifacts = client.activity_artifacts(activity.id.as_str())?;
            Ok(format_activity_logs(activity, &artifacts))
        }
        ActivityCommand::Resume { target } => {
            let activities = client.activities()?;
            let activity = resolve_log_activity(&activities, &target).map_err(CliError::from)?;
            ensure_resumable_activity(activity).map_err(CliError::from)?;
            let mut config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            apply_sandbox_option(&mut config, &options);
            config
                .command_provider()
                .attach_session_in_repo(&activity.session, &context.repo_dir)
                .map_err(|error| {
                    CliError::from(handle_resume_error(
                        activity,
                        &context.data_dir,
                        error.to_string(),
                    ))
                })?;
            Ok(String::new())
        }
        ActivityCommand::Inspect { pull_request } => {
            inspect_checkout_with_discovery(&pull_request, &GitHubCliDiscovery::new("gh"), None)
                .map_err(Into::into)
        }
    }
}
