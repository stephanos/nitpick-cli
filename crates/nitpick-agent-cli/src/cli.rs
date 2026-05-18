use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

use crate::{
    ActivityArgs, ActivityCommand, ArtifactArgs, ArtifactCommand, ChatArgs, ChatCommand,
    ReviewArgs, ReviewCommand, SystemArgs, SystemCommand,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliOptions {
    pub disable_sandbox: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliInvocation {
    pub command: CommandGroup,
    pub options: CliOptions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandGroup {
    Help,
    Version,
    Review(ReviewCommand),
    Activity(ActivityCommand),
    Artifact(ArtifactCommand),
    System(SystemCommand),
    Chat(ChatCommand),
}

#[derive(Parser)]
#[command(name = "nitpick", version)]
struct Cli {
    #[arg(long = "no-sandbox", help = "Run provider command without sandboxing")]
    no_sandbox: bool,
    #[command(subcommand)]
    command: Option<RootCommand>,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
enum RootCommand {
    Review(ReviewArgs),
    Activity(ActivityArgs),
    Artifact(ArtifactArgs),
    System(SystemArgs),
    Chat(ChatArgs),
}

pub fn parse_invocation(args: impl IntoIterator<Item = String>) -> Result<CliInvocation, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if matches!(args.first().map(String::as_str), Some("version")) {
        return Ok(CliInvocation {
            command: CommandGroup::Version,
            options: CliOptions::default(),
        });
    }

    let cli = match Cli::try_parse_from(std::iter::once("nitpick".to_owned()).chain(args)) {
        Ok(cli) => cli,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            return Ok(CliInvocation {
                command: CommandGroup::Help,
                options: CliOptions::default(),
            });
        }
        Err(error) if error.kind() == ErrorKind::DisplayVersion => {
            return Ok(CliInvocation {
                command: CommandGroup::Version,
                options: CliOptions::default(),
            });
        }
        Err(error) => return Err(error.to_string()),
    };

    let command = match cli.command {
        Some(RootCommand::Review(args)) => CommandGroup::Review(args.command.into()),
        Some(RootCommand::Activity(args)) => CommandGroup::Activity(args.command.into()),
        Some(RootCommand::Artifact(args)) => CommandGroup::Artifact(args.command.into()),
        Some(RootCommand::System(args)) => CommandGroup::System(args.command.into()),
        Some(RootCommand::Chat(args)) => CommandGroup::Chat(args.command.into()),
        None => CommandGroup::Help,
    };

    Ok(CliInvocation {
        command,
        options: CliOptions {
            disable_sandbox: cli.no_sandbox,
        },
    })
}

pub fn parse_command(args: impl IntoIterator<Item = String>) -> Result<CommandGroup, String> {
    parse_invocation(args).map(|invocation| invocation.command)
}

pub fn help_text(_version: &str) -> String {
    let mut command = Cli::command();
    let mut help = Vec::new();
    command.write_long_help(&mut help).expect("write help");
    String::from_utf8(help).expect("help is utf8")
}

#[cfg(test)]
mod tests {
    use crate::{ChatCommand, CliCommand, parse_invocation};

    #[test]
    fn help_text_mentions_nested_commands() {
        let help = super::help_text("0.1.0");
        assert!(help.contains("review"));
        assert!(help.contains("activity"));
        assert!(help.contains("artifact"));
        assert!(help.contains("system"));
        assert!(help.contains("--no-sandbox"));
    }

    #[test]
    fn parses_no_sandbox_global_flag() {
        let invocation = parse_invocation([
            "--no-sandbox".to_owned(),
            "chat".to_owned(),
            "start".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("invocation");

        assert!(invocation.options.disable_sandbox);
        assert_eq!(
            invocation.command,
            CliCommand::Chat(ChatCommand::Start {
                target: "acme/platform#42".into(),
            })
        );
    }
}
