use clap::{Args, Subcommand};

use crate::{CliError, CliOptions, CliRunContext, apply_sandbox_option, require_cached_checkout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCommand {
    Start { target: String },
}

#[derive(Args)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: ChatSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ChatSubcommand {
    Start { target: String },
}

impl From<ChatSubcommand> for ChatCommand {
    fn from(command: ChatSubcommand) -> Self {
        match command {
            ChatSubcommand::Start { target } => Self::Start { target },
        }
    }
}

pub fn run(
    command: ChatCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    match command {
        ChatCommand::Start { target } => {
            let mut config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            apply_sandbox_option(&mut config, &options);
            let checkout = require_cached_checkout(&target, &config).map_err(CliError::from)?;
            config
                .command_provider()
                .start_interactive_session_in_repo(&checkout)
                .map_err(CliError::from)?;
            Ok(String::new())
        }
    }
}
