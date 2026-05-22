use clap::{Args, Subcommand, ValueEnum};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{
    Activity, ActivityKind, ActivityStatus, ReviewInput, ReviewRequest, ReviewSubject,
};

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewCommand {
    Run { subject: String },
    Chat { target: String },
    OpenEditor { target: String },
    Show { target: String },
    List { status: ReviewListStatus },
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
    OpenEditor {
        target: String,
    },
    Show {
        target: String,
    },
    List {
        #[arg(long = "status", value_enum, default_value_t = ReviewListStatus::Inbox)]
        status: ReviewListStatus,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReviewListStatus {
    Inbox,
    Requested,
    Active,
    History,
    Any,
}

impl From<ReviewSubcommand> for ReviewCommand {
    fn from(command: ReviewSubcommand) -> Self {
        match command {
            ReviewSubcommand::Run { subject } => Self::Run { subject },
            ReviewSubcommand::Chat { target } => Self::Chat { target },
            ReviewSubcommand::OpenEditor { target } => Self::OpenEditor { target },
            ReviewSubcommand::Show { target } => Self::Show { target },
            ReviewSubcommand::List { status } => Self::List { status },
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
            let mut input = review_input(subject.clone(), context.repo_dir, context.diff);
            input.disable_sandbox = options.disable_sandbox;
            let activity = client.review(&input)?;
            let output = format_review_started(&activity, &subject);
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
            if let Some(provider) = activity.session.provider.clone() {
                config.provider = provider;
            }
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
        ReviewCommand::OpenEditor { target } => {
            let mut config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            crate::support::apply_sandbox_option(&mut config, &options);
            crate::support::open_cached_checkout(&target, &config, &context.data_dir, None)
                .map_err(CliError::from)
        }
        ReviewCommand::Show { target } => {
            let activities = client.activities()?;
            let activity = crate::activity::resolve_log_activity(&activities, &target)
                .map_err(CliError::from)?;
            let artifacts = client.activity_artifacts(activity.id.as_str())?;
            Ok(crate::activity::format_activity_logs(activity, &artifacts))
        }
        ReviewCommand::List { status } => {
            let requests = if matches!(
                status,
                ReviewListStatus::Inbox | ReviewListStatus::Requested | ReviewListStatus::Any
            ) {
                client.review_requests(false)?
            } else {
                Vec::new()
            };
            let activities = if matches!(status, ReviewListStatus::Requested) {
                Vec::new()
            } else {
                client.activities()?
            };
            Ok(format_review_list(&requests, &activities, status))
        }
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
    let review_subject = match subject.parse::<nitpick_agent_github::PullRequestRef>() {
        Ok(reference) => ReviewSubject {
            repository: format!("{}/{}", reference.owner, reference.repo),
            number: Some(reference.number),
            ..ReviewSubject::default()
        },
        Err(_) => ReviewSubject {
            repository: subject,
            ..ReviewSubject::default()
        },
    };
    ReviewInput {
        repo_dir,
        subject: review_subject,
        diff,
        ..ReviewInput::default()
    }
}

pub fn format_review_list(
    requests: &[ReviewRequest],
    activities: &[Activity],
    status: ReviewListStatus,
) -> String {
    let mut lines = Vec::new();
    if matches!(
        status,
        ReviewListStatus::Inbox | ReviewListStatus::Requested | ReviewListStatus::Any
    ) {
        for request in requests {
            if status == ReviewListStatus::Inbox
                && activities.iter().any(|activity| {
                    activity.kind == ActivityKind::Review
                        && activity.label.as_deref()
                            == Some(format!("review on {}", request.display_reference()).as_str())
                        && is_active_review_status(&activity.status)
                })
            {
                continue;
            }
            lines.push(format!("{} requested", request.display_reference()));
        }
    }
    for activity in activities {
        if activity.kind != ActivityKind::Review || !review_status_matches(&activity.status, status)
        {
            continue;
        }
        lines.push(format_review_activity(activity));
    }
    if lines.is_empty() {
        return "no reviews".into();
    }
    lines.join("\n")
}

fn review_status_matches(status: &ActivityStatus, filter: ReviewListStatus) -> bool {
    match filter {
        ReviewListStatus::Inbox | ReviewListStatus::Active => is_active_review_status(status),
        ReviewListStatus::Requested => false,
        ReviewListStatus::History => is_history_review_status(status),
        ReviewListStatus::Any => true,
    }
}

fn is_active_review_status(status: &ActivityStatus) -> bool {
    matches!(status, ActivityStatus::Queued | ActivityStatus::Running)
}

fn is_history_review_status(status: &ActivityStatus) -> bool {
    matches!(
        status,
        ActivityStatus::Completed | ActivityStatus::Error | ActivityStatus::Cancelled
    )
}

fn format_review_activity(activity: &Activity) -> String {
    let label = activity
        .label
        .as_deref()
        .and_then(|label| label.strip_prefix("review on "))
        .unwrap_or("review");
    format!("{label} {:?} {}", activity.status, activity.id)
}

fn format_review_started(activity: &Activity, subject: &str) -> String {
    format!(
        "{}\ncheck status: nitpick review show {subject}\nlist active reviews: nitpick review list --status active",
        crate::activity::format_activity(activity)
    )
}

#[cfg(test)]
mod tests {
    use super::{ReviewCommand, format_review_requests, format_review_started, review_input};
    use crate::{CliCommand, parse_command};
    use nitpick_agent_core::{Activity, ActivityStatus, ReviewRequest};

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
    fn formats_review_run_status_instructions() {
        let activity = Activity {
            id: "activity-7".into(),
            status: ActivityStatus::Running,
            ..Activity::default()
        };

        assert_eq!(
            format_review_started(&activity, "acme/platform#42"),
            "activity-7: Running\ncheck status: nitpick review show acme/platform#42\nlist active reviews: nitpick review list --status active"
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
    fn parses_review_open_editor_command() {
        let command = parse_command([
            "review".to_owned(),
            "open-editor".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::OpenEditor {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn parses_review_open_editor_command_with_github_url() {
        let command = parse_command([
            "review".to_owned(),
            "open-editor".to_owned(),
            "https://github.com/acme/platform/pull/42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::OpenEditor {
                target: "https://github.com/acme/platform/pull/42".into(),
            })
        );
    }

    #[test]
    fn rejects_review_open_editor_without_target() {
        let error = parse_command(["review".to_owned(), "open-editor".to_owned()])
            .expect_err("command fails");

        assert!(error.contains("Usage: nitpick review open-editor <TARGET>"));
    }

    #[test]
    fn rejects_review_editor_command() {
        let error = parse_command([
            "review".to_owned(),
            "editor".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'editor'"));
    }

    #[test]
    fn rejects_review_chat_without_target() {
        let error =
            parse_command(["review".to_owned(), "chat".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick review chat <TARGET>"));
    }

    #[test]
    fn rejects_review_requests_command() {
        let error =
            parse_command(["review".to_owned(), "requests".to_owned()]).expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'requests'"));
    }

    #[test]
    fn parses_reviews_command() {
        let command =
            parse_command(["review".to_owned(), "list".to_owned()]).expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::List {
                status: super::ReviewListStatus::Inbox
            })
        );
    }

    #[test]
    fn parses_reviews_status_command() {
        let command = parse_command([
            "review".to_owned(),
            "list".to_owned(),
            "--status".to_owned(),
            "history".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::List {
                status: super::ReviewListStatus::History
            })
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
    fn rejects_review_sync_command() {
        let error = parse_command([
            "review".to_owned(),
            "sync".to_owned(),
            "activity-1".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'sync'"));
    }

    #[test]
    fn parses_review_show_command() {
        let command = parse_command([
            "review".to_owned(),
            "show".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Show {
                target: "acme/platform#42".into(),
            })
        );
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

        assert_eq!(input.subject.repository, "acme/platform");
        assert_eq!(input.subject.number, Some(42));
        assert_eq!(input.repo_dir, std::path::PathBuf::from("/tmp/repo"));
        assert_eq!(input.diff, "diff --git");
    }
}
