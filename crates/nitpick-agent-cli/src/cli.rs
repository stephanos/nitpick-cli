use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

use crate::{DebugArgs, DebugCommand, ReviewArgs, ReviewCommand, SystemArgs, SystemCommand};

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
    HelpText(String),
    Version,
    Status,
    Review(ReviewCommand),
    System(SystemCommand),
    Debug(DebugCommand),
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
    Status,
    Review(ReviewArgs),
    System(SystemArgs),
    Debug(DebugArgs),
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
                command: CommandGroup::HelpText(error.to_string()),
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
        Some(RootCommand::Status) => CommandGroup::Status,
        Some(RootCommand::Review(args)) => CommandGroup::Review(args.command.into()),
        Some(RootCommand::System(args)) => CommandGroup::System(args.command.into()),
        Some(RootCommand::Debug(args)) => CommandGroup::Debug(args.command.into()),
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
    use crate::{CliCommand, parse_invocation};

    #[test]
    fn help_text_mentions_nested_commands() {
        let help = super::help_text("0.1.0");
        assert!(help.contains("review"));
        assert!(help.contains("system"));
        assert!(help.contains("debug"));
        assert!(help.contains("status"));
        assert!(help.contains("--no-sandbox"));
    }

    #[test]
    fn parses_no_sandbox_global_flag() {
        let invocation = parse_invocation([
            "--no-sandbox".to_owned(),
            "review".to_owned(),
            "chat".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("invocation");

        assert!(invocation.options.disable_sandbox);
        assert_eq!(
            invocation.command,
            CliCommand::Review(crate::ReviewCommand::Chat {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn parses_root_status_command() {
        let invocation = parse_invocation(["status".to_owned()]).expect("invocation");

        assert_eq!(invocation.command, CliCommand::Status);
    }

    #[test]
    fn nested_help_preserves_nested_command_help() {
        let invocation =
            parse_invocation(["review".to_owned(), "--help".to_owned()]).expect("help invocation");

        assert!(matches!(
            invocation.command,
            CliCommand::HelpText(ref help) if help.contains("Usage: nitpick review")
        ));
    }
}
