mod activity;
mod artifact;
mod cli;
mod context;
mod debug;
mod review;
mod style;
mod support;
mod system;

use nitpick_agent_client::HostClientError;
use nitpick_agent_core::AgentError;

pub use activity::{
    daemon_log_path, ensure_resumable_activity, format_activities, format_activity,
    format_activity_debug_logs, format_activity_logs, format_daemon_log, format_reviews,
    parse_activities_json, parse_activity_json, resolve_log_activity,
};
pub use artifact::{format_artifact, format_artifacts, parse_artifact_json, parse_artifacts_json};
pub use cli::{
    CliInvocation, CliOptions, CommandGroup as CliCommand, Confirmation, help_text, parse_command,
    parse_invocation,
};
pub use context::{CliRunContext, config_path_from_env, data_dir_from_env, host_addr_from_env};
pub use debug::{DebugArgs, DebugCommand};
pub use nitpick_agent_core::HostStatus;
pub use review::{
    ReviewArgs, ReviewCommand, ReviewListStatus, format_review_requests, review_input,
};
pub use style::format_error_message;
pub use system::{
    SystemArgs, SystemCommand, format_cleanup_checkouts, format_host_status,
    format_local_state_reset, host_status_url, parse_host_status_json,
};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Host(#[from] HostClientError),
    #[error("{0}")]
    Agent(#[from] AgentError),
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for CliError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

pub fn run_cli_command(
    command: CliCommand,
    host_addr: &str,
    repo_dir: std::path::PathBuf,
    diff: String,
    context: String,
    config_path: std::path::PathBuf,
    data_dir: std::path::PathBuf,
) -> Result<String, String> {
    run_cli_command_with_options(
        command,
        CliRunContext {
            host_addr: host_addr.into(),
            repo_dir,
            diff,
            context,
            config_path,
            data_dir,
        },
        CliOptions::default(),
    )
}

pub fn run_cli_command_with_options(
    command: CliCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, String> {
    run_cli_command_typed(command, context, options).map_err(|error| error.to_string())
}

pub fn run_cli_command_typed(
    command: CliCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    match command {
        CliCommand::Help => Ok(help_text(env!("CARGO_PKG_VERSION"))),
        CliCommand::HelpText(help) => Ok(help),
        CliCommand::Version => Ok(format!("nitpick {}", env!("CARGO_PKG_VERSION"))),
        CliCommand::Status => system::status(context),
        CliCommand::Review(command) => review::run(command, context, options),
        CliCommand::System(command) => system::run(command, context, options),
        CliCommand::Debug(command) => debug::run(command, context, options),
    }
}
