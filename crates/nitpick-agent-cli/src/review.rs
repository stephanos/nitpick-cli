use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{ReviewInput, ReviewRequest, ReviewSubject};

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewCommand {
    Run { subject: String },
    Chat { target: String },
    Requests { only_new: bool },
    Sync { activity_id: String, target: String },
    List { include_all: bool },
}

#[derive(Args)]
pub struct ReviewArgs {
    #[command(subcommand)]
    pub command: ReviewSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ReviewSubcommand {
    Run {
        subject: String,
    },
    Chat {
        target: String,
    },
    Requests {
        #[arg(long = "new")]
        only_new: bool,
    },
    Sync {
        activity_id: String,
        target: String,
    },
    List {
        #[arg(long = "all")]
        include_all: bool,
    },
}

impl From<ReviewSubcommand> for ReviewCommand {
    fn from(command: ReviewSubcommand) -> Self {
        match command {
            ReviewSubcommand::Run { subject } => Self::Run { subject },
            ReviewSubcommand::Chat { target } => Self::Chat { target },
            ReviewSubcommand::Requests { only_new } => Self::Requests { only_new },
            ReviewSubcommand::Sync {
                activity_id,
                target,
            } => Self::Sync {
                activity_id,
                target,
            },
            ReviewSubcommand::List { include_all } => Self::List { include_all },
        }
    }
}

pub fn run(
    command: ReviewCommand,
    context: CliRunContext,
    options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        ReviewCommand::Run { subject } => {
            let mut input = review_input(subject, context.repo_dir, context.diff);
            input.disable_sandbox = options.disable_sandbox;
            let activity = client.review(&input)?;
            let output = crate::activity::format_activity(&activity);
            if let Some(error) = activity.error {
                return Err(error.into());
            }
            Ok(output)
        }
        ReviewCommand::Chat { target } => {
            let activities = client.activities()?;
            let activity = crate::activity::resolve_log_activity(&activities, &target)
                .map_err(CliError::from)?;
            crate::activity::ensure_resumable_activity(activity).map_err(CliError::from)?;
            let mut config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            crate::support::apply_sandbox_option(&mut config, &options);
            let checkout =
                crate::support::require_cached_checkout(&target, &config, &context.data_dir)
                    .map_err(CliError::from)?;
            config
                .command_provider()
                .attach_session_in_repo(&activity.session, &checkout)
                .map_err(|error| {
                    CliError::from(crate::support::handle_resume_error(
                        activity,
                        &context.data_dir,
                        error.to_string(),
                    ))
                })?;
            Ok(String::new())
        }
        ReviewCommand::Requests { only_new } => {
            Ok(format_review_requests(&client.review_requests(only_new)?))
        }
        ReviewCommand::Sync {
            activity_id,
            target,
        } => Ok(crate::artifact::format_artifacts(
            &client.sync_activity_artifacts(&activity_id, "github-review", Some(&target))?,
        )),
        ReviewCommand::List { include_all } => Ok(crate::activity::format_reviews(
            &client.activities()?,
            include_all,
        )),
    }
}

pub fn format_review_requests(requests: &[ReviewRequest]) -> String {
    if requests.is_empty() {
        return "no review requests".into();
    }

    requests
        .iter()
        .map(|request| format!("{} {}", request.source, request.display_reference()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn review_input(subject: String, repo_dir: std::path::PathBuf, diff: String) -> ReviewInput {
    ReviewInput {
        repo_dir,
        subject: ReviewSubject {
            repository: subject,
            ..ReviewSubject::default()
        },
        diff,
        ..ReviewInput::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{ReviewCommand, format_review_requests, review_input};
    use crate::{CliCommand, parse_command};
    use nitpick_agent_core::ReviewRequest;

    #[test]
    fn parses_review_run_command() {
        let command = parse_command([
            "review".to_owned(),
            "run".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Run {
                subject: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn rejects_review_run_without_subject() {
        let error =
            parse_command(["review".to_owned(), "run".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick review run <SUBJECT>"));
    }

    #[test]
    fn parses_review_chat_command() {
        let command = parse_command([
            "review".to_owned(),
            "chat".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Chat {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn rejects_review_chat_without_target() {
        let error =
            parse_command(["review".to_owned(), "chat".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick review chat <TARGET>"));
    }

    #[test]
    fn parses_review_requests_command() {
        let command =
            parse_command(["review".to_owned(), "requests".to_owned()]).expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Requests { only_new: false })
        );
    }

    #[test]
    fn parses_new_review_requests_command() {
        let command = parse_command([
            "review".to_owned(),
            "requests".to_owned(),
            "--new".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Requests { only_new: true })
        );
    }

    #[test]
    fn parses_reviews_command() {
        let command =
            parse_command(["review".to_owned(), "list".to_owned()]).expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::List { include_all: false })
        );
    }

    #[test]
    fn parses_reviews_all_command() {
        let command = parse_command(["review".to_owned(), "list".to_owned(), "--all".to_owned()])
            .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::List { include_all: true })
        );
    }

    #[test]
    fn rejects_unknown_reviews_flag() {
        let error = parse_command([
            "review".to_owned(),
            "list".to_owned(),
            "--running".to_owned(),
        ])
        .expect_err("command");

        assert!(error.contains("unexpected argument '--running'"));
    }

    #[test]
    fn parses_review_sync_command() {
        let command = parse_command([
            "review".to_owned(),
            "sync".to_owned(),
            "activity-1".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Sync {
                activity_id: "activity-1".into(),
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn rejects_review_sync_without_target() {
        let error = parse_command([
            "review".to_owned(),
            "sync".to_owned(),
            "activity-1".to_owned(),
        ])
        .expect_err("command");

        assert!(error.contains("Usage: nitpick review sync <ACTIVITY_ID> <TARGET>"));
    }

    #[test]
    fn formats_review_requests() {
        let requests = vec![ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: "abc123".into(),
        }];

        assert_eq!(format_review_requests(&requests), "github acme/platform#42");
    }

    #[test]
    fn builds_review_input_with_repo_dir_and_diff() {
        let input = review_input(
            "acme/platform#42".into(),
            "/tmp/repo".into(),
            "diff --git".into(),
        );

        assert_eq!(input.subject.repository, "acme/platform#42");
        assert_eq!(input.repo_dir, std::path::PathBuf::from("/tmp/repo"));
        assert_eq!(input.diff, "diff --git");
    }
}
