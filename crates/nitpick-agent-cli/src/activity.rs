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
    format!("{}: {:?}", activity.id, activity.status)
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
    let mut lines = vec![
        format!("activity: {}", activity.id),
        format!("kind: {:?}", activity.kind),
        format!("status: {:?}", activity.status),
    ];
    if let Some(label) = &activity.label {
        lines.push(format!("label: {label}"));
    }
    lines.push(format!("updated: {}", activity.updated_at_unix));
    if let Some(error) = &activity.error {
        lines.push(format!("error: {error}"));
    }
    if let Some(output) = &activity.output {
        lines.push("output:".into());
        lines.push(format_activity_output(output));
    }
    if artifacts.is_empty() {
        lines.push("artifacts: none".into());
    } else {
        lines.push("artifacts:".into());
        for artifact in artifacts {
            lines.push(format!("== {} {:?} ==", artifact.id, artifact.kind));
            lines.push(format_artifact_content(&artifact.content));
        }
    }
    lines.join("\n")
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

fn is_active_review_status(status: &ActivityStatus) -> bool {
    matches!(status, ActivityStatus::Queued | ActivityStatus::Running)
}

fn format_review_activity(activity: &Activity) -> String {
    let mut output = format!(
        "{:?} {} {} updated={}",
        activity.status,
        activity.label.as_deref().unwrap_or("review"),
        activity.id,
        activity.updated_at_unix
    );
    if let Some(error) = &activity.error {
        output.push_str(&format!(" error={error:?}"));
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
            let mut lines = vec![output.summary.clone()];
            for comment in &output.comments {
                lines.push(format!(
                    "{}:{} {}",
                    comment.path, comment.line, comment.body
                ));
            }
            lines.join("\n")
        }
        ActivityOutput::Chat(output) => output.clone(),
    }
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
            "Running review on acme/platform#42 activity-1 updated=1200"
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
            "Running review on acme/platform#42 activity-1 updated=1200\nCompleted review on acme/platform#41 activity-2 updated=1000"
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
            "Error review on acme/platform#42 activity-1 updated=1200 error=\"provider failed\""
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
        activity.updated_at_unix = 1_200;
        activity.error = Some("provider failed".into());
        activity.output = Some(nitpick_agent_core::ActivityOutput::Review(
            nitpick_agent_core::ReviewOutput {
                summary: "summary body".into(),
                comments: vec![nitpick_agent_core::ReviewComment {
                    path: "src/lib.rs".into(),
                    line: 12,
                    body: "comment body".into(),
                }],
                journey: nitpick_agent_core::ReviewJourney::default(),
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
            "activity: activity-1\nkind: Review\nstatus: Error\nlabel: review on acme/platform#42\nupdated: 1200\nerror: provider failed\noutput:\nsummary body\nsrc/lib.rs:12 comment body\nartifacts:\n== artifact-1 ReviewSummary ==\nartifact summary"
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
