use clap::{Args, Subcommand, ValueEnum};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::{
    Activity, ActivityKind, ActivityStatus, ReviewInput, ReviewMode, ReviewRequest, ReviewSubject,
};

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewCommand {
    Start {
        subject: String,
        force: bool,
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
        status: ReviewListStatus,
        limit: usize,
    },
}

#[derive(Args)]
pub struct ReviewArgs {
    #[command(subcommand)]
    pub command: ReviewSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ReviewSubcommand {
    Start {
        #[arg(long = "force")]
        force: bool,
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
        #[arg(long = "limit", default_value_t = DEFAULT_REVIEW_LIST_LIMIT)]
        limit: usize,
    },
}

const DEFAULT_REVIEW_LIST_LIMIT: usize = 20;

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
            ReviewSubcommand::Start { subject, force } => Self::Start { subject, force },
            ReviewSubcommand::Chat { target } => Self::Chat { target },
            ReviewSubcommand::OpenEditor { target } => Self::OpenEditor { target },
            ReviewSubcommand::Show { target } => Self::Show { target },
            ReviewSubcommand::List { status, limit } => Self::List { status, limit },
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
        ReviewCommand::Start { subject, force } => {
            let mut input = start_review_input(&subject, &context)?;
            input.disable_sandbox = options.disable_sandbox;
            input.force = force;
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
            crate::activity::ensure_review_chat_available(activity).map_err(CliError::from)?;
            let mut config = nitpick_agent_host::AgentConfig::load_or_default(&context.config_path)
                .map_err(CliError::from)?;
            crate::support::apply_sandbox_option(&mut config, &options);
            if let Some(provider) = activity.session.provider.clone() {
                config.provider = provider;
            }
            let checkout =
                crate::support::ensure_cached_checkout(&target, &config, &context.data_dir)
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
        ReviewCommand::List { status, limit } => {
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
                client.filtered_activities(
                    Some("review"),
                    Some(review_activity_status_filter(status)),
                    Some(review_activity_query_limit(status, limit)),
                )?
            };
            Ok(format_review_list(&requests, &activities, status, limit))
        }
    }
}

fn start_review_input(subject: &str, context: &CliRunContext) -> Result<ReviewInput, CliError> {
    if subject.parse::<nitpick_agent_github::PullRequestRef>().is_ok() {
        return crate::support::github_review_input(subject, &context.config_path, &context.data_dir)
            .map_err(CliError::from);
    }
    Ok(review_input(
        subject.to_owned(),
        context.repo_dir.clone(),
        context.diff.clone(),
    ))
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
        review_mode: ReviewMode::SelfReview,
        subject: review_subject,
        diff,
        ..ReviewInput::default()
    }
}

pub fn format_review_list(
    requests: &[ReviewRequest],
    activities: &[Activity],
    status: ReviewListStatus,
    limit: usize,
) -> String {
    let mut rows = Vec::new();
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
            rows.push(vec![
                crate::style::label("requested"),
                request.display_reference(),
                String::new(),
                String::new(),
                String::new(),
            ]);
        }
    }
    let mut review_activities = activities
        .iter()
        .filter(|activity| {
            activity.kind == ActivityKind::Review && review_status_matches(&activity.status, status)
        })
        .collect::<Vec<_>>();
    review_activities.sort_by(|lhs, rhs| {
        rhs.updated_at_unix
            .cmp(&lhs.updated_at_unix)
            .then_with(|| rhs.id.cmp(&lhs.id))
    });
    for activity in review_activities {
        if activity.kind != ActivityKind::Review || !review_status_matches(&activity.status, status)
        {
            continue;
        }
        rows.push(review_activity_row(activity));
    }
    let limit = limit.max(1);
    rows.truncate(limit);
    if rows.is_empty() {
        return "no reviews".into();
    }
    rows.insert(0, review_list_header());
    crate::style::table(rows)
}

fn review_status_matches(status: &ActivityStatus, filter: ReviewListStatus) -> bool {
    match filter {
        ReviewListStatus::Inbox | ReviewListStatus::Active => is_active_review_status(status),
        ReviewListStatus::Requested => false,
        ReviewListStatus::History => is_history_review_status(status),
        ReviewListStatus::Any => true,
    }
}

fn review_activity_status_filter(status: ReviewListStatus) -> &'static str {
    match status {
        ReviewListStatus::Inbox | ReviewListStatus::Active => "active",
        ReviewListStatus::Requested => "any",
        ReviewListStatus::History => "history",
        ReviewListStatus::Any => "any",
    }
}

fn review_activity_query_limit(status: ReviewListStatus, limit: usize) -> usize {
    match status {
        ReviewListStatus::Inbox => 1_000,
        ReviewListStatus::Requested => 1,
        ReviewListStatus::Active | ReviewListStatus::History | ReviewListStatus::Any => {
            limit.max(1)
        }
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

fn review_list_header() -> Vec<String> {
    vec![
        crate::style::label("status"),
        crate::style::label("review"),
        crate::style::label("activity"),
        crate::style::label("updated_at"),
        crate::style::label("error"),
    ]
}

fn review_activity_row(activity: &Activity) -> Vec<String> {
    let label = activity
        .label
        .as_deref()
        .and_then(|label| label.strip_prefix("review on "))
        .unwrap_or("review");
    vec![
        crate::style::status_lower(&activity.status),
        label.into(),
        crate::style::label(activity.id.to_string()),
        if activity.updated_at_unix > 0 {
            format_unix_iso_utc(activity.updated_at_unix)
        } else {
            String::new()
        },
        activity
            .error
            .as_ref()
            .map(|error| crate::style::error(error))
            .unwrap_or_default(),
    ]
}

fn format_unix_iso_utc(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn format_review_started(activity: &Activity, subject: &str) -> String {
    format!(
        "review {}  {}\n  {}  nitpick review show {subject}\n  {}   nitpick debug logs {}\n  {}  nitpick review list --status active",
        crate::style::status_lower(&activity.status),
        crate::style::label(activity.id.to_string()),
        crate::style::label("status"),
        crate::style::label("debug"),
        activity.id,
        crate::style::label("active")
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ReviewCommand, ReviewListStatus, format_review_list, format_review_requests,
        format_review_started, review_input,
    };
    use crate::{CliCommand, parse_command};
    use nitpick_agent_core::{Activity, ActivityStatus, ReviewMode, ReviewRequest};

    #[test]
    fn parses_review_start_command() {
        let command = parse_command([
            "review".to_owned(),
            "start".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Start {
                subject: "acme/platform#42".into(),
                force: false,
            })
        );
    }

    #[test]
    fn parses_review_start_force_flag() {
        let command = parse_command([
            "review".to_owned(),
            "start".to_owned(),
            "--force".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::Start {
                subject: "acme/platform#42".into(),
                force: true,
            })
        );
    }

    #[test]
    fn formats_review_start_status_instructions() {
        let mut activity = Activity::new(
            nitpick_agent_core::ActivityId::new("activity-7"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = ActivityStatus::Running;

        assert_eq!(
            format_review_started(&activity, "acme/platform#42"),
            "review \u{1b}[34mrunning\u{1b}[0m  \u{1b}[2mactivity-7\u{1b}[0m\n  \u{1b}[2mstatus\u{1b}[0m  nitpick review show acme/platform#42\n  \u{1b}[2mdebug\u{1b}[0m   nitpick debug logs activity-7\n  \u{1b}[2mactive\u{1b}[0m  nitpick review list --status active"
        );
    }

    #[test]
    fn rejects_review_start_without_subject() {
        let error =
            parse_command(["review".to_owned(), "start".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick review start <SUBJECT>"));
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
                status: super::ReviewListStatus::Inbox,
                limit: 20,
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
                status: super::ReviewListStatus::History,
                limit: 20,
            })
        );
    }

    #[test]
    fn parses_reviews_limit() {
        let command = parse_command([
            "review".to_owned(),
            "list".to_owned(),
            "--limit".to_owned(),
            "5".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review(ReviewCommand::List {
                status: super::ReviewListStatus::Inbox,
                limit: 5,
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
    fn formats_review_list_as_table_like_rows() {
        let requests = vec![ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            number: Some(42),
            id: "42".into(),
            head_sha: "abc123".into(),
        }];
        let mut activity = Activity::new(
            nitpick_agent_core::ActivityId::new("activity-7"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = ActivityStatus::Running;
        activity.label = Some("review on acme/platform#43".into());
        activity.updated_at_unix = 1_200;

        assert_eq!(
            format_review_list(&requests, &[activity], ReviewListStatus::Any, 20),
            "\u{1b}[2mstatus\u{1b}[0m     \u{1b}[2mreview\u{1b}[0m            \u{1b}[2mactivity\u{1b}[0m    \u{1b}[2mupdated_at\u{1b}[0m            \u{1b}[2merror\u{1b}[0m\n\u{1b}[2mrequested\u{1b}[0m  acme/platform#42\n\u{1b}[34mrunning\u{1b}[0m    acme/platform#43  \u{1b}[2mactivity-7\u{1b}[0m  1970-01-01T00:20:00Z"
        );
    }

    #[test]
    fn review_list_sorts_activities_latest_first_and_applies_limit() {
        let mut newer = Activity::new(
            nitpick_agent_core::ActivityId::new("activity-10"),
            nitpick_agent_core::ActivityKind::Review,
        );
        newer.status = ActivityStatus::Completed;
        newer.label = Some("review on acme/platform#10".into());
        newer.updated_at_unix = 2_000;
        let mut older = Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        older.status = ActivityStatus::Completed;
        older.label = Some("review on acme/platform#1".into());
        older.updated_at_unix = 1_000;

        assert_eq!(
            format_review_list(&[], &[older, newer], ReviewListStatus::History, 1),
            "\u{1b}[2mstatus\u{1b}[0m     \u{1b}[2mreview\u{1b}[0m            \u{1b}[2mactivity\u{1b}[0m     \u{1b}[2mupdated_at\u{1b}[0m            \u{1b}[2merror\u{1b}[0m\n\u{1b}[32mcompleted\u{1b}[0m  acme/platform#10  \u{1b}[2mactivity-10\u{1b}[0m  1970-01-01T00:33:20Z"
        );
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
        assert_eq!(input.review_mode, ReviewMode::SelfReview);
    }
}
