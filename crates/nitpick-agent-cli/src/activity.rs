use nitpick_agent_core::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, Artifact, ArtifactContent,
};
use nitpick_agent_github::PullRequestRef;

pub fn parse_activity_json(body: &str) -> Result<Activity, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host activity response: {error}"))
}

pub fn parse_activities_json(body: &str) -> Result<Vec<Activity>, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host activities response: {error}"))
}

pub fn format_activity(activity: &Activity) -> String {
    format!(
        "{}  {}",
        crate::style::status_lower(&activity.status),
        crate::style::label(activity.id.to_string())
    )
}

pub fn format_activities(activities: &[Activity]) -> String {
    if activities.is_empty() {
        return "no local activities".into();
    }

    activities
        .iter()
        .map(format_activity)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_reviews(activities: &[Activity], include_all: bool) -> String {
    let mut reviews = activities
        .iter()
        .filter(|activity| activity.kind == ActivityKind::Review)
        .filter(|activity| include_all || is_active_review_status(&activity.status))
        .collect::<Vec<_>>();
    if reviews.is_empty() {
        return if include_all {
            "no reviews".into()
        } else {
            "no active reviews".into()
        };
    }

    reviews.sort_by(|lhs, rhs| {
        rhs.updated_at_unix
            .cmp(&lhs.updated_at_unix)
            .then_with(|| rhs.id.cmp(&lhs.id))
    });
    reviews
        .into_iter()
        .map(format_review_activity)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn resolve_log_activity<'a>(
    activities: &'a [Activity],
    target: &str,
) -> Result<&'a Activity, String> {
    if let Some(activity) = activities
        .iter()
        .find(|activity| activity.id.as_str() == target)
    {
        return Ok(activity);
    }

    let label = review_label_for_target(target)?;
    activities
        .iter()
        .filter(|activity| activity.kind == ActivityKind::Review)
        .filter(|activity| activity.label.as_deref() == Some(label.as_str()))
        .max_by(|lhs, rhs| {
            lhs.updated_at_unix
                .cmp(&rhs.updated_at_unix)
                .then_with(|| lhs.id.cmp(&rhs.id))
        })
        .ok_or_else(|| format!("no review activity found for {target}"))
}

pub fn format_activity_logs(activity: &Activity, artifacts: &[Artifact]) -> String {
    format_activity_logs_with_options(activity, artifacts, false)
}

pub fn format_activity_debug_logs(activity: &Activity, artifacts: &[Artifact]) -> String {
    format_activity_logs_with_options(activity, artifacts, true)
}

fn format_activity_logs_with_options(
    activity: &Activity,
    artifacts: &[Artifact],
    include_provider_logs: bool,
) -> String {
    let mut rows = vec![
        vec![crate::style::label("activity"), activity.id.to_string()],
        vec![crate::style::label("kind"), format!("{:?}", activity.kind)],
    ];
    if let Some(review) = review_shorthand(activity) {
        rows.push(vec![crate::style::label("review"), review.into()]);
        if let Some(url) = review_url(review) {
            rows.push(vec![
                crate::style::label("url"),
                crate::style::hyperlink(&url, &url),
            ]);
        }
    }
    rows.push(vec![
        crate::style::label("status"),
        crate::style::status_title(&activity.status),
    ]);
    if let Some(label) = &activity.label {
        rows.push(vec![crate::style::label("label"), label.clone()]);
    }
    rows.push(vec![
        crate::style::label("created"),
        format_unix_iso_utc(activity.created_at_unix),
    ]);
    if let Some(started_at_unix) = activity.started_at_unix {
        rows.push(vec![
            crate::style::label("started"),
            format_unix_iso_utc(started_at_unix),
        ]);
    }
    rows.push(vec![
        crate::style::label("updated"),
        format_unix_iso_utc(activity.updated_at_unix),
    ]);
    if let Some(error) = &activity.error {
        rows.push(vec![
            crate::style::label("error"),
            crate::style::error(error),
        ]);
    }

    let title = if activity.kind == ActivityKind::Review {
        "Review"
    } else {
        "Activity"
    };
    let mut sections = vec![format_section(title, crate::style::table(rows))];
    if let Some(output) = &activity.output {
        sections.push(format_section("Output", format_activity_output(output)));
    }
    if include_provider_logs {
        sections.push(format_section(
            "Provider logs",
            format_provider_logs(activity),
        ));
    }
    sections.push(format_section(
        "Artifacts",
        format_artifacts_table(artifacts),
    ));
    sections.join("\n\n")
}

fn format_provider_logs(activity: &Activity) -> String {
    let logs = activity
        .session
        .messages
        .iter()
        .filter(|message| {
            message.role == "provider.stdout"
                || message.role == "provider.stderr"
                || message.role == "provider.sandbox"
                || message.role == "provider.run"
        })
        .map(|message| {
            format!(
                "{}\n{}",
                crate::style::label(message.role.trim_start_matches("provider.")),
                indent_block_by(&message.content, "  ")
            )
        })
        .collect::<Vec<_>>();
    if logs.is_empty() {
        "no provider logs captured".into()
    } else {
        logs.join("\n")
    }
}

pub fn daemon_log_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("logs").join("daemon.log")
}

pub fn format_daemon_log(path: &std::path::Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(format!(
            "daemon log not found: {}\nrestart the macOS app or host after updating to log persistence",
            path.display()
        )),
        Err(error) => Err(format!("read daemon log {}: {error}", path.display())),
    }
}

pub fn ensure_resumable_activity(activity: &Activity) -> Result<(), String> {
    if activity.session.provider_session_id.is_none() {
        return Err(format!(
            "activity {} has no provider session id",
            activity.id
        ));
    }
    Ok(())
}

pub fn ensure_review_chat_available(activity: &Activity) -> Result<(), String> {
    if is_active_review_status(&activity.status) {
        return Err(format!(
            "cannot open review chat for {} while the review is {}; the provider session is locked by the active review",
            activity.id,
            crate::style::status_plain_title(&activity.status)
        ));
    }
    ensure_resumable_activity(activity)
}

fn is_active_review_status(status: &ActivityStatus) -> bool {
    matches!(status, ActivityStatus::Queued | ActivityStatus::Running)
}

fn review_shorthand(activity: &Activity) -> Option<&str> {
    activity
        .label
        .as_deref()
        .and_then(|label| label.strip_prefix("review on "))
}

fn review_url(review: &str) -> Option<String> {
    let reference = review.parse::<PullRequestRef>().ok()?;
    Some(format!(
        "https://github.com/{}/{}/pull/{}",
        reference.owner, reference.repo, reference.number
    ))
}

fn format_review_activity(activity: &Activity) -> String {
    let mut output = format!(
        "{}  {}  {}  {} {}",
        crate::style::status_lower(&activity.status),
        activity.label.as_deref().unwrap_or("review"),
        crate::style::label(activity.id.to_string()),
        crate::style::label("updated"),
        activity.updated_at_unix
    );
    if let Some(error) = &activity.error {
        output.push_str(&format!(
            "  {} {}",
            crate::style::label("error"),
            crate::style::error(error)
        ));
    }
    output
}

fn review_label_for_target(target: &str) -> Result<String, String> {
    let reference = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid log target: {error}"))?;
    Ok(format!(
        "review on {}/{}#{}",
        reference.owner, reference.repo, reference.number
    ))
}

fn format_activity_output(output: &ActivityOutput) -> String {
    match output {
        ActivityOutput::Review(output) => {
            if output.comments.is_empty() {
                return "no review comments".into();
            }
            let mut rows = vec![vec![
                crate::style::label("path"),
                crate::style::label("line"),
                crate::style::label("comment"),
            ]];
            rows.extend(output.comments.iter().map(|comment| {
                vec![
                    comment.path.clone(),
                    comment.line.to_string(),
                    comment.body.clone(),
                ]
            }));
            crate::style::table(rows)
        }
        ActivityOutput::Chat(output) => output.clone(),
    }
}

fn format_artifacts_table(artifacts: &[Artifact]) -> String {
    if artifacts.is_empty() {
        return "none".into();
    }
    let mut rows = vec![vec![
        crate::style::label("id"),
        crate::style::label("kind"),
        crate::style::label("content"),
    ]];
    rows.extend(artifacts.iter().map(|artifact| {
        vec![
            crate::style::label(artifact.id.to_string()),
            format!("{:?}", artifact.kind),
            format_artifact_content(&artifact.content),
        ]
    }));
    crate::style::table(rows)
}

fn format_artifact_content(content: &ArtifactContent) -> String {
    match content {
        ArtifactContent::ReviewSummary(summary) => summary.clone(),
        ArtifactContent::ReviewComment(comment) => {
            format!("{}:{} {}", comment.path, comment.line, comment.body)
        }
        ArtifactContent::ChatResponse(response) => response.clone(),
    }
}

fn indent_block(value: &str) -> String {
    indent_block_by(value, "  ")
}

fn format_section(title: &str, body: String) -> String {
    format!("{title}\n{}", indent_block(&body))
}

fn format_unix_iso_utc(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn indent_block_by(value: &str, prefix: &str) -> String {
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{format_activities, format_reviews};
    use crate::{CliCommand, parse_command, run_cli_command};

    #[test]
    fn rejects_activity_command() {
        let error =
            parse_command(["activity".to_owned(), "list".to_owned()]).expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'activity'"));
    }

    #[test]
    fn rejects_inspect_command() {
        let error = parse_command([
            "activity".to_owned(),
            "inspect".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'activity'"));
    }

    #[test]
    fn parses_activity_json() {
        let activity = super::parse_activity_json(
            r#"{"id":"activity-1","kind":"Chat","status":"Completed","session":{"provider":null,"provider_session_id":null,"status":"Completed","messages":[]},"output":{"Chat":"done"},"error":null}"#,
        )
        .expect("activity parses");

        assert_eq!(activity.id.to_string(), "activity-1");
    }

    #[test]
    fn formats_empty_activity_list() {
        assert_eq!(format_activities(&[]), "no local activities");
    }

    #[test]
    fn formats_active_reviews() {
        let mut running_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        running_review.status = nitpick_agent_core::ActivityStatus::Running;
        running_review.label = Some("review on acme/platform#42".into());
        running_review.session.provider_session_id = Some("github:acme/platform#42".into());
        running_review.updated_at_unix = 1_200;
        let mut completed_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-2"),
            nitpick_agent_core::ActivityKind::Review,
        );
        completed_review.status = nitpick_agent_core::ActivityStatus::Completed;
        completed_review.label = Some("review on acme/platform#41".into());
        let mut running_chat = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-3"),
            nitpick_agent_core::ActivityKind::Chat,
        );
        running_chat.status = nitpick_agent_core::ActivityStatus::Running;

        assert_eq!(
            format_reviews(
                &[completed_review.clone(), running_chat, running_review],
                false
            ),
            "\u{1b}[34mrunning\u{1b}[0m  review on acme/platform#42  \u{1b}[2mactivity-1\u{1b}[0m  \u{1b}[2mupdated\u{1b}[0m 1200"
        );
        assert_eq!(
            format_reviews(&[completed_review], false),
            "no active reviews"
        );
    }

    #[test]
    fn formats_all_reviews() {
        let mut running_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        running_review.status = nitpick_agent_core::ActivityStatus::Running;
        running_review.label = Some("review on acme/platform#42".into());
        running_review.session.provider_session_id = Some("github:acme/platform#42".into());
        running_review.updated_at_unix = 1_200;
        let mut completed_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-2"),
            nitpick_agent_core::ActivityKind::Review,
        );
        completed_review.status = nitpick_agent_core::ActivityStatus::Completed;
        completed_review.label = Some("review on acme/platform#41".into());
        completed_review.session.provider_session_id = Some("github:acme/platform#41".into());
        completed_review.updated_at_unix = 1_000;

        assert_eq!(
            format_reviews(&[completed_review, running_review], true),
            "\u{1b}[34mrunning\u{1b}[0m  review on acme/platform#42  \u{1b}[2mactivity-1\u{1b}[0m  \u{1b}[2mupdated\u{1b}[0m 1200\n\u{1b}[32mcompleted\u{1b}[0m  review on acme/platform#41  \u{1b}[2mactivity-2\u{1b}[0m  \u{1b}[2mupdated\u{1b}[0m 1000"
        );
    }

    #[test]
    fn formats_failed_reviews_with_error() {
        let mut failed_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        failed_review.status = nitpick_agent_core::ActivityStatus::Error;
        failed_review.label = Some("review on acme/platform#42".into());
        failed_review.session.provider_session_id = Some("github:acme/platform#42".into());
        failed_review.updated_at_unix = 1_200;
        failed_review.error = Some("provider failed".into());

        assert_eq!(
            format_reviews(&[failed_review], true),
            "\u{1b}[31merror\u{1b}[0m  review on acme/platform#42  \u{1b}[2mactivity-1\u{1b}[0m  \u{1b}[2mupdated\u{1b}[0m 1200  \u{1b}[2merror\u{1b}[0m \u{1b}[31mprovider failed\u{1b}[0m"
        );
    }

    #[test]
    fn resolves_log_activity_by_id_or_latest_pr_review() {
        let mut older_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        older_review.label = Some("review on acme/platform#42".into());
        older_review.updated_at_unix = 1_000;
        let mut latest_review = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-2"),
            nitpick_agent_core::ActivityKind::Review,
        );
        latest_review.label = Some("review on acme/platform#42".into());
        latest_review.updated_at_unix = 1_200;

        assert_eq!(
            super::resolve_log_activity(
                &[older_review.clone(), latest_review.clone()],
                "activity-1"
            )
            .expect("activity")
            .id,
            older_review.id
        );
        assert_eq!(
            super::resolve_log_activity(&[older_review, latest_review.clone()], "acme/platform#42")
                .expect("activity")
                .id,
            latest_review.id
        );
    }

    #[test]
    fn formats_activity_logs_with_output_artifacts_and_error() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Error;
        activity.label = Some("review on acme/platform#42".into());
        activity.session.provider_session_id = Some("github:acme/platform#42".into());
        activity.created_at_unix = 1_000;
        activity.started_at_unix = Some(1_100);
        activity.updated_at_unix = 1_200;
        activity.error = Some("provider failed".into());
        activity.output = Some(nitpick_agent_core::ActivityOutput::Review(
            nitpick_agent_core::ReviewOutput {
                comments: vec![nitpick_agent_core::ReviewComment {
                    path: "src/lib.rs".into(),
                    line: 12,
                    body: "comment body".into(),
                }],
            },
        ));
        let artifact = nitpick_agent_core::Artifact::local(
            nitpick_agent_core::ArtifactId::new("artifact-1"),
            activity.id.clone(),
            nitpick_agent_core::ArtifactKind::ReviewSummary,
            nitpick_agent_core::ArtifactContent::ReviewSummary("artifact summary".into()),
        );

        assert_eq!(
            super::format_activity_logs(&activity, &[artifact]),
            "Review\n  \u{1b}[2mactivity\u{1b}[0m  activity-1\n  \u{1b}[2mkind\u{1b}[0m      Review\n  \u{1b}[2mreview\u{1b}[0m    acme/platform#42\n  \u{1b}[2murl\u{1b}[0m       \u{1b}]8;;https://github.com/acme/platform/pull/42\u{1b}\\https://github.com/acme/platform/pull/42\u{1b}]8;;\u{1b}\\\n  \u{1b}[2mstatus\u{1b}[0m    \u{1b}[31mError\u{1b}[0m\n  \u{1b}[2mlabel\u{1b}[0m     review on acme/platform#42\n  \u{1b}[2mcreated\u{1b}[0m   1970-01-01T00:16:40Z\n  \u{1b}[2mstarted\u{1b}[0m   1970-01-01T00:18:20Z\n  \u{1b}[2mupdated\u{1b}[0m   1970-01-01T00:20:00Z\n  \u{1b}[2merror\u{1b}[0m     \u{1b}[31mprovider failed\u{1b}[0m\n\nOutput\n  \u{1b}[2mpath\u{1b}[0m        \u{1b}[2mline\u{1b}[0m  \u{1b}[2mcomment\u{1b}[0m\n  src/lib.rs  12    comment body\n\nArtifacts\n  \u{1b}[2mid\u{1b}[0m          \u{1b}[2mkind\u{1b}[0m           \u{1b}[2mcontent\u{1b}[0m\n  \u{1b}[2martifact-1\u{1b}[0m  ReviewSummary  artifact summary"
        );
    }

    #[test]
    fn formats_activity_logs_with_review_shorthand() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Completed;
        activity.label = Some("review on stephanos/subvoc#1".into());
        activity.created_at_unix = 1_000;
        activity.started_at_unix = Some(1_100);
        activity.updated_at_unix = 1_200;

        assert_eq!(
            super::format_activity_logs(&activity, &[]),
            "Review\n  \u{1b}[2mactivity\u{1b}[0m  activity-1\n  \u{1b}[2mkind\u{1b}[0m      Review\n  \u{1b}[2mreview\u{1b}[0m    stephanos/subvoc#1\n  \u{1b}[2murl\u{1b}[0m       \u{1b}]8;;https://github.com/stephanos/subvoc/pull/1\u{1b}\\https://github.com/stephanos/subvoc/pull/1\u{1b}]8;;\u{1b}\\\n  \u{1b}[2mstatus\u{1b}[0m    \u{1b}[32mCompleted\u{1b}[0m\n  \u{1b}[2mlabel\u{1b}[0m     review on stephanos/subvoc#1\n  \u{1b}[2mcreated\u{1b}[0m   1970-01-01T00:16:40Z\n  \u{1b}[2mstarted\u{1b}[0m   1970-01-01T00:18:20Z\n  \u{1b}[2mupdated\u{1b}[0m   1970-01-01T00:20:00Z\n\nArtifacts\n  none"
        );
    }

    #[test]
    fn debug_logs_include_provider_output() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Completed;
        activity.created_at_unix = 1_000;
        activity.updated_at_unix = 1_200;
        activity.session.messages = vec![
            nitpick_agent_core::AgentMessage {
                role: "provider.stdout".into(),
                content: "review progress\ncompleted".into(),
            },
            nitpick_agent_core::AgentMessage {
                role: "provider.stderr".into(),
                content: "warning".into(),
            },
            nitpick_agent_core::AgentMessage {
                role: "provider.sandbox".into(),
                content: "retry with --no-sandbox".into(),
            },
            nitpick_agent_core::AgentMessage {
                role: "provider.run".into(),
                content: "provider claude command completed".into(),
            },
        ];

        assert_eq!(
            super::format_activity_debug_logs(&activity, &[]),
            "Review\n  \u{1b}[2mactivity\u{1b}[0m  activity-1\n  \u{1b}[2mkind\u{1b}[0m      Review\n  \u{1b}[2mstatus\u{1b}[0m    \u{1b}[32mCompleted\u{1b}[0m\n  \u{1b}[2mcreated\u{1b}[0m   1970-01-01T00:16:40Z\n  \u{1b}[2mupdated\u{1b}[0m   1970-01-01T00:20:00Z\n\nProvider logs\n  \u{1b}[2mstdout\u{1b}[0m\n    review progress\n    completed\n  \u{1b}[2mstderr\u{1b}[0m\n    warning\n  \u{1b}[2msandbox\u{1b}[0m\n    retry with --no-sandbox\n  \u{1b}[2mrun\u{1b}[0m\n    provider claude command completed\n\nArtifacts\n  none"
        );
    }

    #[test]
    fn debug_logs_explain_missing_provider_output() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Completed;
        activity.created_at_unix = 1_000;
        activity.updated_at_unix = 1_200;

        assert_eq!(
            super::format_activity_debug_logs(&activity, &[]),
            "Review\n  \u{1b}[2mactivity\u{1b}[0m  activity-1\n  \u{1b}[2mkind\u{1b}[0m      Review\n  \u{1b}[2mstatus\u{1b}[0m    \u{1b}[32mCompleted\u{1b}[0m\n  \u{1b}[2mcreated\u{1b}[0m   1970-01-01T00:16:40Z\n  \u{1b}[2mupdated\u{1b}[0m   1970-01-01T00:20:00Z\n\nProvider logs\n  no provider logs captured\n\nArtifacts\n  none"
        );
    }

    #[test]
    fn requires_provider_session_id() {
        let activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );

        let error = super::ensure_resumable_activity(&activity).expect_err("missing session id");

        assert_eq!(error, "activity activity-1 has no provider session id");
    }

    #[test]
    fn rejects_review_chat_for_active_review() {
        let mut activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );
        activity.status = nitpick_agent_core::ActivityStatus::Running;
        activity.session.provider_session_id = Some("123e4567-e89b-12d3-a456-426614174000".into());

        let error = super::ensure_review_chat_available(&activity)
            .expect_err("running review rejects chat");

        assert_eq!(
            error,
            "cannot open review chat for activity-1 while the review is Running; the provider session is locked by the active review"
        );
    }

    #[test]
    fn daemon_log_path_lives_under_data_dir() {
        assert_eq!(
            super::daemon_log_path(&std::path::PathBuf::from("/tmp/nitpick-data")),
            std::path::PathBuf::from("/tmp/nitpick-data/logs/daemon.log")
        );
    }

    #[test]
    fn formats_missing_daemon_log_with_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("logs/daemon.log");

        let output = super::format_daemon_log(&path).expect("daemon log");

        assert_eq!(
            output,
            format!(
                "daemon log not found: {}\nrestart the macOS app or host after updating to log persistence",
                path.display()
            )
        );
    }

    #[test]
    fn formats_existing_daemon_log() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("logs/daemon.log");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("log dir");
        std::fs::write(&path, "started\npoll failed\n").expect("write log");

        let output = super::format_daemon_log(&path).expect("daemon log");

        assert_eq!(output, "started\npoll failed\n");
    }

    #[test]
    fn logs_daemon_reads_daemon_log_without_host_lookup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().join("data");
        let path = super::daemon_log_path(&data_dir);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("log dir");
        std::fs::write(&path, "daemon started\n").expect("write log");

        let output = run_cli_command(
            CliCommand::Debug(crate::DebugCommand::Logs {
                target: "daemon".into(),
            }),
            "127.0.0.1:1",
            dir.path().to_path_buf(),
            String::new(),
            String::new(),
            dir.path().join("config.toml"),
            data_dir,
        )
        .expect("logs daemon");

        assert_eq!(output, "daemon started\n");
    }
}
