use serde::Deserialize;
use std::{path::Path, process::Command};

use nitpick_agent_client::HostClient;
use nitpick_agent_core::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, AgentProvider, Artifact,
    ArtifactContent, ChatInput, ReviewInput, ReviewRequest, ReviewSubject,
};
use nitpick_agent_github::{GitHubCliDiscovery, PullRequestRef};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    Version,
    Review {
        subject: String,
    },
    Inspect {
        pull_request: String,
    },
    Chat {
        prompt: String,
    },
    Status,
    ReviewRequests {
        only_new: bool,
    },
    Activities,
    Reviews {
        include_all: bool,
    },
    Logs {
        target: String,
    },
    Resume {
        target: String,
    },
    Artifacts {
        activity_id: String,
    },
    Artifact {
        artifact_id: String,
    },
    ArtifactSync {
        artifact_id: String,
        destination: String,
        target: Option<String>,
    },
    ReviewSync {
        activity_id: String,
        target: String,
    },
    SyncPending {
        destination: Option<String>,
    },
    CleanupCheckouts,
}

pub fn parse_command(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None | Some("-h") | Some("--help") => Ok(CliCommand::Help),
        Some("-V") | Some("--version") | Some("version") => Ok(CliCommand::Version),
        Some("status") => Ok(CliCommand::Status),
        Some("review-requests") => match args.next().as_deref() {
            None => Ok(CliCommand::ReviewRequests { only_new: false }),
            Some("--new") => Ok(CliCommand::ReviewRequests { only_new: true }),
            Some(_) => Err("usage: nitpick review-requests [--new]".into()),
        },
        Some("activities") => Ok(CliCommand::Activities),
        Some("reviews") => match args.next().as_deref() {
            None => Ok(CliCommand::Reviews { include_all: false }),
            Some("--all") => Ok(CliCommand::Reviews { include_all: true }),
            Some(_) => Err("usage: nitpick reviews [--all]".into()),
        },
        Some("logs") => {
            let target = args
                .next()
                .ok_or_else(|| "usage: nitpick logs <activity-id|pr-ref>".to_owned())?;
            Ok(CliCommand::Logs { target })
        }
        Some("resume") => {
            let target = args
                .next()
                .ok_or_else(|| "usage: nitpick resume <activity-id|pr-ref>".to_owned())?;
            Ok(CliCommand::Resume { target })
        }
        Some("artifacts") => {
            let activity_id = args
                .next()
                .ok_or_else(|| "usage: nitpick artifacts <activity-id>".to_owned())?;
            Ok(CliCommand::Artifacts { activity_id })
        }
        Some("artifact") => {
            let artifact_id = args
                .next()
                .ok_or_else(|| "usage: nitpick artifact <artifact-id>".to_owned())?;
            Ok(CliCommand::Artifact { artifact_id })
        }
        Some("artifact-sync") => {
            let artifact_id = args.next().ok_or_else(|| {
                "usage: nitpick artifact-sync <artifact-id> <destination>".to_owned()
            })?;
            let destination = args.next().ok_or_else(|| {
                "usage: nitpick artifact-sync <artifact-id> <destination>".to_owned()
            })?;
            Ok(CliCommand::ArtifactSync {
                artifact_id,
                destination,
                target: args.next(),
            })
        }
        Some("review-sync") => {
            let activity_id = args
                .next()
                .ok_or_else(|| "usage: nitpick review-sync <activity-id> <pr-ref>".to_owned())?;
            let target = args
                .next()
                .ok_or_else(|| "usage: nitpick review-sync <activity-id> <pr-ref>".to_owned())?;
            Ok(CliCommand::ReviewSync {
                activity_id,
                target,
            })
        }
        Some("sync-pending") => Ok(CliCommand::SyncPending {
            destination: args.next(),
        }),
        Some("cleanup-checkouts") => Ok(CliCommand::CleanupCheckouts),
        Some("inspect") => {
            let pull_request = args
                .next()
                .ok_or_else(|| "usage: nitpick inspect <pr-ref>".to_owned())?;
            Ok(CliCommand::Inspect { pull_request })
        }
        Some("review") => {
            let subject = args
                .next()
                .ok_or_else(|| "usage: nitpick review <subject>".to_owned())?;
            Ok(CliCommand::Review { subject })
        }
        Some("chat") => {
            let prompt = args
                .next()
                .ok_or_else(|| "usage: nitpick chat <prompt>".to_owned())?;
            Ok(CliCommand::Chat { prompt })
        }
        Some(command) => Err(format!("unknown command `{command}`")),
    }
}

pub fn help_text(version: &str) -> String {
    format!(
        "nitpick {version}\n\nUsage: nitpick <command>\n\nCommands:\n  review <subject>                                   Start a review activity\n  inspect <pr-ref>                                   Open a reviewed PR checkout in an editor\n  reviews [--all]                                    List review activities\n  logs <activity-id|pr-ref>                          Show review logs for an activity or PR\n  resume <activity-id|pr-ref>                        Reopen a supported provider session\n  review-sync <activity-id> <pr-ref>                 Sync an activity as one GitHub review\n  review-requests [--new]                            List review requests from enabled sources\n  chat <prompt>                                      Start a chat activity\n  status                                             Show local activity status\n  activities                                         List local activities\n  artifacts <activity-id>                            List local artifacts for an activity\n  artifact <artifact-id>                             Show one local artifact\n  artifact-sync <artifact-id> <destination> [target]  Sync an artifact to a destination\n  sync-pending [destination]                         List artifacts pending sync\n  cleanup-checkouts                                  Remove closed or merged PR checkouts\n  version                                            Print version\n\nOptions:\n  -h, --help                                         Print help\n  -V, --version                                      Print version"
    )
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostStatus {
    pub activity_count: usize,
    pub running_activity_count: usize,
    pub completed_activity_count: usize,
    pub error_activity_count: usize,
    pub artifact_count: usize,
    pub local_only_artifact_count: usize,
    pub pending_sync_artifact_count: usize,
    pub provider: String,
    pub model: Option<String>,
    pub review_source_name: String,
    pub review_source_enabled: bool,
    pub review_source_last_poll_unix: Option<u64>,
    pub review_source_last_poll_summary: Option<String>,
}

pub fn format_host_status(status: &HostStatus) -> String {
    format!(
        "nitpick-agent-host: connected\nactivities: {} ({} running, {} completed, {} error)\nartifacts: {}\nlocal-only artifacts: {}\npending-sync artifacts: {}\nagent: {}\nmodel: {}",
        status.activity_count,
        status.running_activity_count,
        status.completed_activity_count,
        status.error_activity_count,
        status.artifact_count,
        status.local_only_artifact_count,
        status.pending_sync_artifact_count,
        status.provider,
        status.model.as_deref().unwrap_or("(default)")
    )
}

pub fn parse_host_status_json(body: &str) -> Result<HostStatus, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host status response: {error}"))
}

pub fn parse_activity_json(body: &str) -> Result<Activity, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host activity response: {error}"))
}

pub fn parse_activities_json(body: &str) -> Result<Vec<Activity>, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host activities response: {error}"))
}

pub fn parse_artifacts_json(body: &str) -> Result<Vec<Artifact>, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host artifacts response: {error}"))
}

pub fn parse_artifact_json(body: &str) -> Result<Artifact, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host artifact response: {error}"))
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

fn is_active_review_status(status: &ActivityStatus) -> bool {
    matches!(status, ActivityStatus::Queued | ActivityStatus::Running)
}

fn format_review_activity(activity: &Activity) -> String {
    let mut output = format!(
        "{:?} {} {} updated={} session={}",
        activity.status,
        activity.label.as_deref().unwrap_or("review"),
        activity.id,
        activity.updated_at_unix,
        activity
            .session
            .provider_session_id
            .as_deref()
            .unwrap_or("(none)")
    );
    if let Some(error) = &activity.error {
        output.push_str(&format!(" error={error:?}"));
    }
    output
}

pub fn format_artifacts(artifacts: &[Artifact]) -> String {
    if artifacts.is_empty() {
        return "no local artifacts".into();
    }

    artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{}: {:?} {:?}",
                artifact.id, artifact.kind, artifact.sync_state
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_artifact(artifact: &Artifact) -> String {
    format!(
        "{}\nactivity: {}\nkind: {:?}\nsync: {:?}",
        artifact.id, artifact.activity_id, artifact.kind, artifact.sync_state
    )
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

fn review_label_for_target(target: &str) -> Result<String, String> {
    let reference = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid log target: {error}"))?;
    Ok(format!(
        "review on {}/{}#{}",
        reference.owner, reference.repo, reference.number
    ))
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
    lines.push(format!(
        "session: {}",
        activity
            .session
            .provider_session_id
            .as_deref()
            .unwrap_or("(none)")
    ));
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

pub fn format_cleanup_checkouts(result: &nitpick_agent_client::CleanupCheckoutsResult) -> String {
    if result.removed_count == 0 {
        return "no checkouts cleaned up".into();
    }
    format!(
        "cleaned up {} checkout(s)\n{}",
        result.removed_count,
        result.cleaned.join("\n")
    )
}

pub fn inspect_checkout(
    pull_request: &str,
    checkout_root: &Path,
    editor: Option<&Path>,
) -> Result<String, String> {
    inspect_checkout_with_discovery(
        pull_request,
        &GitHubCliDiscovery::with_checkout_commands("gh", "git", checkout_root),
        editor,
    )
}

fn inspect_checkout_with_discovery(
    pull_request: &str,
    discovery: &GitHubCliDiscovery,
    editor: Option<&Path>,
) -> Result<String, String> {
    let reference = pull_request
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid PR reference: {error}"))?;
    let checkout = discovery.checkout_path_for(&reference);
    if !checkout.join(".git").is_dir() {
        return Err(format!("checkout not found for {pull_request}"));
    }

    let editor = editor
        .map(std::path::PathBuf::from)
        .or_else(editor_from_env)
        .ok_or_else(|| "set VISUAL or EDITOR to inspect checkouts".to_owned())?;
    let status = Command::new(&editor)
        .arg(&checkout)
        .status()
        .map_err(|error| format!("failed to start editor `{}`: {error}", editor.display()))?;
    if !status.success() {
        return Err(format!("editor `{}` failed: {status}", editor.display()));
    }
    Ok(format!("opened {}", checkout.display()))
}

fn editor_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("VISUAL")
        .or_else(|| std::env::var_os("EDITOR"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            if cfg!(target_os = "macos") {
                Some(std::path::PathBuf::from("open"))
            } else {
                None
            }
        })
}

pub fn host_status_url(addr: &str) -> String {
    format!("http://{addr}/status")
}

pub fn host_addr_from_env(value: Option<String>) -> String {
    value.unwrap_or_else(|| "127.0.0.1:19783".into())
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

pub fn chat_input(prompt: String, repo_dir: std::path::PathBuf, context: String) -> ChatInput {
    ChatInput {
        repo_dir,
        prompt,
        context,
    }
}

pub fn config_path_from_env(
    nitpick_agent_config: Option<std::ffi::OsString>,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    if let Some(path) = nitpick_agent_config {
        return std::path::PathBuf::from(path);
    }
    if let Some(config_home) = xdg_config_home {
        return std::path::PathBuf::from(config_home)
            .join("nitpick-agent")
            .join("config.toml");
    }
    std::path::PathBuf::from(home.unwrap_or_else(|| ".".into()))
        .join(".config")
        .join("nitpick-agent")
        .join("config.toml")
}

pub fn data_dir_from_env(
    nitpick_agent_data_dir: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    if let Some(path) = nitpick_agent_data_dir {
        return std::path::PathBuf::from(path);
    }
    if let Some(data_home) = xdg_data_home {
        return std::path::PathBuf::from(data_home).join("nitpick-agent");
    }
    std::path::PathBuf::from(home.unwrap_or_else(|| ".".into()))
        .join(".local")
        .join("share")
        .join("nitpick-agent")
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

pub fn run_cli_command(
    command: CliCommand,
    host_addr: &str,
    repo_dir: std::path::PathBuf,
    diff: String,
    context: String,
    config_path: std::path::PathBuf,
    data_dir: std::path::PathBuf,
) -> Result<String, String> {
    let client = HostClient::new(host_addr);
    match command {
        CliCommand::Help => Ok(help_text(env!("CARGO_PKG_VERSION"))),
        CliCommand::Version => Ok(format!("nitpick {}", env!("CARGO_PKG_VERSION"))),
        CliCommand::Status => match client.status() {
            Ok(status) => Ok(format_host_status(&host_status(status))),
            Err(error) if error.starts_with("nitpick-agent-host unavailable") => Ok(format!(
                "nitpick-agent-host: not connected\naddress: {host_addr}"
            )),
            Err(error) => Err(error),
        },
        CliCommand::ReviewRequests { only_new } => {
            Ok(format_review_requests(&client.review_requests(only_new)?))
        }
        CliCommand::Activities => Ok(format_activities(&client.activities()?)),
        CliCommand::Reviews { include_all } => {
            Ok(format_reviews(&client.activities()?, include_all))
        }
        CliCommand::Logs { target } if target == "daemon" => {
            format_daemon_log(&daemon_log_path(&data_dir))
        }
        CliCommand::Logs { target } => {
            let activities = client.activities()?;
            let activity = resolve_log_activity(&activities, &target)?;
            let artifacts = client.activity_artifacts(activity.id.as_str())?;
            Ok(format_activity_logs(activity, &artifacts))
        }
        CliCommand::Resume { target } => {
            let activities = client.activities()?;
            let activity = resolve_log_activity(&activities, &target)?;
            ensure_resumable_activity(activity)?;
            let config = nitpick_agent_host::AgentConfig::load_or_default(&config_path)
                .map_err(|error| error.to_string())?;
            config
                .command_provider()
                .attach_session(&activity.session)
                .map_err(|error| error.to_string())?;
            Ok(String::new())
        }
        CliCommand::Artifacts { activity_id } => {
            Ok(format_artifacts(&client.activity_artifacts(&activity_id)?))
        }
        CliCommand::Artifact { artifact_id } => {
            Ok(format_artifact(&client.artifact(&artifact_id)?))
        }
        CliCommand::ArtifactSync {
            artifact_id,
            destination,
            target,
        } => Ok(format_artifact(&client.sync_artifact(
            &artifact_id,
            &destination,
            target.as_deref(),
        )?)),
        CliCommand::ReviewSync {
            activity_id,
            target,
        } => Ok(format_artifacts(&client.sync_activity_artifacts(
            &activity_id,
            "github-review",
            Some(&target),
        )?)),
        CliCommand::SyncPending { destination } => Ok(format_artifacts(
            &client.pending_sync_artifacts(destination.as_deref())?,
        )),
        CliCommand::CleanupCheckouts => Ok(format_cleanup_checkouts(&client.cleanup_checkouts()?)),
        CliCommand::Inspect { pull_request } => {
            inspect_checkout_with_discovery(&pull_request, &GitHubCliDiscovery::new("gh"), None)
        }
        CliCommand::Review { subject } => {
            let activity = client.review(&review_input(subject, repo_dir, diff))?;
            let output = format_activity(&activity);
            if let Some(error) = activity.error {
                return Err(error);
            }
            Ok(output)
        }
        CliCommand::Chat { prompt } => {
            let activity = client.chat(&chat_input(prompt, repo_dir, context))?;
            let output = format_activity(&activity);
            if let Some(error) = activity.error {
                return Err(error);
            }
            Ok(output)
        }
    }
}

fn host_status(status: nitpick_agent_client::HostStatus) -> HostStatus {
    HostStatus {
        activity_count: status.activity_count,
        running_activity_count: status.running_activity_count,
        completed_activity_count: status.completed_activity_count,
        error_activity_count: status.error_activity_count,
        artifact_count: status.artifact_count,
        local_only_artifact_count: status.local_only_artifact_count,
        pending_sync_artifact_count: status.pending_sync_artifact_count,
        provider: status.provider,
        model: status.model,
        review_source_name: status.review_source_name,
        review_source_enabled: status.review_source_enabled,
        review_source_last_poll_unix: status.review_source_last_poll_unix,
        review_source_last_poll_summary: status.review_source_last_poll_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::{CliCommand, HostStatus, format_host_status, parse_command};
    use nitpick_agent_core::ReviewRequest;

    #[test]
    fn parses_review_command_subject() {
        let command = parse_command(["review".to_owned(), "acme/platform#42".to_owned()])
            .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Review {
                subject: "acme/platform#42".into()
            }
        );
    }

    #[test]
    fn rejects_review_without_subject() {
        let error = parse_command(["review".to_owned()]).expect_err("command fails");

        assert_eq!(error, "usage: nitpick review <subject>");
    }

    #[test]
    fn parses_status_command() {
        let command = parse_command(["status".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Status);
    }

    #[test]
    fn parses_review_requests_command() {
        let command = parse_command(["review-requests".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::ReviewRequests { only_new: false });
    }

    #[test]
    fn parses_new_review_requests_command() {
        let command = parse_command(["review-requests".to_owned(), "--new".to_owned()])
            .expect("command parses");

        assert_eq!(command, CliCommand::ReviewRequests { only_new: true });
    }

    #[test]
    fn parses_activities_command() {
        let command = parse_command(["activities".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Activities);
    }

    #[test]
    fn parses_reviews_command() {
        let command = parse_command(["reviews".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Reviews { include_all: false });
    }

    #[test]
    fn parses_reviews_all_command() {
        let command =
            parse_command(["reviews".to_owned(), "--all".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Reviews { include_all: true });
    }

    #[test]
    fn parses_logs_command() {
        let command =
            parse_command(["logs".to_owned(), "acme/platform#42".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::Logs {
                target: "acme/platform#42".into()
            }
        );
    }

    #[test]
    fn parses_resume_command() {
        let command =
            parse_command(["resume".to_owned(), "acme/platform#42".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::Resume {
                target: "acme/platform#42".into()
            }
        );
    }

    #[test]
    fn rejects_logs_without_target() {
        let error = parse_command(["logs".to_owned()]).expect_err("command fails");

        assert_eq!(error, "usage: nitpick logs <activity-id|pr-ref>");
    }

    #[test]
    fn rejects_resume_without_target() {
        let error = parse_command(["resume".to_owned()]).expect_err("command fails");

        assert_eq!(error, "usage: nitpick resume <activity-id|pr-ref>");
    }

    #[test]
    fn rejects_unknown_reviews_flag() {
        let error =
            parse_command(["reviews".to_owned(), "--running".to_owned()]).expect_err("command");

        assert_eq!(error, "usage: nitpick reviews [--all]");
    }

    #[test]
    fn parses_inspect_command() {
        let command = parse_command(["inspect".to_owned(), "acme/platform#42".to_owned()])
            .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Inspect {
                pull_request: "acme/platform#42".into()
            }
        );
    }

    #[test]
    fn rejects_inspect_without_pr_ref() {
        let error = parse_command(["inspect".to_owned()]).expect_err("command fails");

        assert_eq!(error, "usage: nitpick inspect <pr-ref>");
    }

    #[test]
    fn parses_artifacts_command() {
        let command = parse_command(["artifacts".to_owned(), "activity-1".to_owned()])
            .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Artifacts {
                activity_id: "activity-1".into()
            }
        );
    }

    #[test]
    fn rejects_artifacts_without_activity_id() {
        let error = parse_command(["artifacts".to_owned()]).expect_err("command fails");

        assert_eq!(error, "usage: nitpick artifacts <activity-id>");
    }

    #[test]
    fn parses_artifact_command() {
        let command =
            parse_command(["artifact".to_owned(), "artifact-1".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::Artifact {
                artifact_id: "artifact-1".into()
            }
        );
    }

    #[test]
    fn parses_artifact_sync_command() {
        let command = parse_command([
            "artifact-sync".to_owned(),
            "artifact-1".to_owned(),
            "github".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::ArtifactSync {
                artifact_id: "artifact-1".into(),
                destination: "github".into(),
                target: None,
            }
        );
    }

    #[test]
    fn parses_artifact_sync_command_with_target() {
        let command = parse_command([
            "artifact-sync".to_owned(),
            "artifact-1".to_owned(),
            "github".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::ArtifactSync {
                artifact_id: "artifact-1".into(),
                destination: "github".into(),
                target: Some("acme/platform#42".into()),
            }
        );
    }

    #[test]
    fn parses_review_sync_command() {
        let command = parse_command([
            "review-sync".to_owned(),
            "activity-1".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::ReviewSync {
                activity_id: "activity-1".into(),
                target: "acme/platform#42".into(),
            }
        );
    }

    #[test]
    fn rejects_review_sync_without_target() {
        let error = parse_command(["review-sync".to_owned(), "activity-1".to_owned()])
            .expect_err("command");

        assert_eq!(error, "usage: nitpick review-sync <activity-id> <pr-ref>");
    }

    #[test]
    fn parses_sync_pending_command_with_destination() {
        let command =
            parse_command(["sync-pending".to_owned(), "github".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::SyncPending {
                destination: Some("github".into())
            }
        );
    }

    #[test]
    fn parses_sync_pending_command_without_destination() {
        let command = parse_command(["sync-pending".to_owned()]).expect("command");

        assert_eq!(command, CliCommand::SyncPending { destination: None });
    }

    #[test]
    fn parses_cleanup_checkouts_command() {
        let command = parse_command(["cleanup-checkouts".to_owned()]).expect("command");

        assert_eq!(command, CliCommand::CleanupCheckouts);
    }

    #[test]
    fn formats_cleanup_checkouts_result() {
        assert_eq!(
            super::format_cleanup_checkouts(&nitpick_agent_client::CleanupCheckoutsResult {
                removed_count: 1,
                cleaned: vec!["acme/platform#42".into()],
            }),
            "cleaned up 1 checkout(s)\nacme/platform#42"
        );
        assert_eq!(
            super::format_cleanup_checkouts(&nitpick_agent_client::CleanupCheckoutsResult {
                removed_count: 0,
                cleaned: Vec::new(),
            }),
            "no checkouts cleaned up"
        );
    }

    #[test]
    fn formats_host_status() {
        let status = HostStatus {
            activity_count: 2,
            running_activity_count: 1,
            completed_activity_count: 1,
            error_activity_count: 0,
            artifact_count: 5,
            local_only_artifact_count: 3,
            pending_sync_artifact_count: 1,
            provider: "claude".into(),
            model: Some("sonnet".into()),
            review_source_name: "github".into(),
            review_source_enabled: true,
            review_source_last_poll_unix: Some(1_000),
            review_source_last_poll_summary: Some("reviewed 1 of 1 PRs".into()),
        };

        assert_eq!(
            format_host_status(&status),
            "nitpick-agent-host: connected\nactivities: 2 (1 running, 1 completed, 0 error)\nartifacts: 5\nlocal-only artifacts: 3\npending-sync artifacts: 1\nagent: claude\nmodel: sonnet"
        );
    }

    #[test]
    fn parses_host_status_json() {
        let status = super::parse_host_status_json(
            r#"{"activity_count":2,"running_activity_count":1,"completed_activity_count":1,"error_activity_count":0,"artifact_count":5,"local_only_artifact_count":3,"pending_sync_artifact_count":1,"provider":"claude","model":null,"review_source_name":"github","review_source_enabled":true,"review_source_last_poll_unix":1000,"review_source_last_poll_summary":"reviewed 1 of 1 PRs"}"#,
        )
        .expect("status parses");

        assert_eq!(
            status,
            HostStatus {
                activity_count: 2,
                running_activity_count: 1,
                completed_activity_count: 1,
                error_activity_count: 0,
                artifact_count: 5,
                local_only_artifact_count: 3,
                pending_sync_artifact_count: 1,
                provider: "claude".into(),
                model: None,
                review_source_name: "github".into(),
                review_source_enabled: true,
                review_source_last_poll_unix: Some(1_000),
                review_source_last_poll_summary: Some("reviewed 1 of 1 PRs".into()),
            }
        );
    }

    #[test]
    fn builds_host_status_url() {
        assert_eq!(
            super::host_status_url("127.0.0.1:19783"),
            "http://127.0.0.1:19783/status"
        );
    }

    #[test]
    fn defaults_host_address_when_env_is_unset() {
        assert_eq!(super::host_addr_from_env(None), "127.0.0.1:19783");
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
        assert_eq!(super::format_activities(&[]), "no local activities");
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
            super::format_reviews(
                &[completed_review.clone(), running_chat, running_review],
                false
            ),
            "Running review on acme/platform#42 activity-1 updated=1200 session=github:acme/platform#42"
        );
        assert_eq!(
            super::format_reviews(&[completed_review], false),
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
            super::format_reviews(&[completed_review, running_review], true),
            "Running review on acme/platform#42 activity-1 updated=1200 session=github:acme/platform#42\nCompleted review on acme/platform#41 activity-2 updated=1000 session=github:acme/platform#41"
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
            super::format_reviews(&[failed_review], true),
            "Error review on acme/platform#42 activity-1 updated=1200 session=github:acme/platform#42 error=\"provider failed\""
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
            "activity: activity-1\nkind: Review\nstatus: Error\nlabel: review on acme/platform#42\nupdated: 1200\nsession: github:acme/platform#42\nerror: provider failed\noutput:\nsummary body\nsrc/lib.rs:12 comment body\nartifacts:\n== artifact-1 ReviewSummary ==\nartifact summary"
        );
    }

    #[test]
    fn resume_requires_provider_session_id() {
        let activity = nitpick_agent_core::Activity::new(
            nitpick_agent_core::ActivityId::new("activity-1"),
            nitpick_agent_core::ActivityKind::Review,
        );

        let error = super::ensure_resumable_activity(&activity).expect_err("missing session id");

        assert_eq!(error, "activity activity-1 has no provider session id");
    }

    #[test]
    fn resolves_config_path_like_host() {
        assert_eq!(
            super::config_path_from_env(Some("/tmp/config.toml".into()), None, None),
            std::path::PathBuf::from("/tmp/config.toml")
        );
        assert_eq!(
            super::config_path_from_env(None, Some("/tmp/xdg".into()), None),
            std::path::PathBuf::from("/tmp/xdg/nitpick-agent/config.toml")
        );
        assert_eq!(
            super::config_path_from_env(None, None, Some("/Users/stephan".into())),
            std::path::PathBuf::from("/Users/stephan/.config/nitpick-agent/config.toml")
        );
    }

    #[test]
    fn resolves_data_dir_like_host() {
        assert_eq!(
            super::data_dir_from_env(Some("/tmp/data".into()), None, None),
            std::path::PathBuf::from("/tmp/data")
        );
        assert_eq!(
            super::data_dir_from_env(None, Some("/tmp/xdg-data".into()), None),
            std::path::PathBuf::from("/tmp/xdg-data/nitpick-agent")
        );
        assert_eq!(
            super::data_dir_from_env(None, None, Some("/Users/stephan".into())),
            std::path::PathBuf::from("/Users/stephan/.local/share/nitpick-agent")
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

        let output = super::run_cli_command(
            CliCommand::Logs {
                target: "daemon".into(),
            },
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

    #[test]
    fn inspect_checkout_opens_existing_checkout_with_editor() {
        let dir = tempfile::tempdir().expect("temp dir");
        let checkout_root = dir.path().join("checkouts");
        let checkout = checkout_root.join("acme/platform/pr-42");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        let editor = dir.path().join("editor");
        let log = dir.path().join("editor.log");
        std::fs::write(
            &editor,
            format!("#!/bin/sh\nprintf '%s\\n' \"$1\" > '{}'\n", log.display()),
        )
        .expect("editor");
        make_executable(&editor);

        let output =
            super::inspect_checkout("acme/platform#42", &checkout_root, Some(editor.as_path()))
                .expect("inspect");

        assert_eq!(output, format!("opened {}", checkout.display()));
        assert_eq!(
            std::fs::read_to_string(log).expect("log"),
            format!("{}\n", checkout.display())
        );
    }

    #[test]
    fn inspect_checkout_reports_missing_checkout() {
        let dir = tempfile::tempdir().expect("temp dir");
        let checkout_root = dir.path().join("checkouts");
        let editor = dir.path().join("editor");

        let error = super::inspect_checkout("acme/platform#42", &checkout_root, Some(&editor))
            .expect_err("missing checkout");

        assert_eq!(error, "checkout not found for acme/platform#42");
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

        assert_eq!(
            super::format_review_requests(&requests),
            "github acme/platform#42"
        );
    }

    #[test]
    fn parses_artifacts_json() {
        let artifacts = super::parse_artifacts_json(
            r#"[{"id":"artifact-1","activity_id":"activity-1","kind":"ChatResponse","content":{"ChatResponse":"done"},"sync_state":"LocalOnly"}]"#,
        )
        .expect("artifacts parse");

        assert_eq!(artifacts.len(), 1);
    }

    #[test]
    fn parses_artifact_json() {
        let artifact = super::parse_artifact_json(
            r#"{"id":"artifact-1","activity_id":"activity-1","kind":"ChatResponse","content":{"ChatResponse":"done"},"sync_state":"LocalOnly"}"#,
        )
        .expect("artifact parses");

        assert_eq!(artifact.id.to_string(), "artifact-1");
    }

    #[test]
    fn builds_review_input_with_repo_dir_and_diff() {
        let input = super::review_input(
            "acme/platform#42".into(),
            "/tmp/repo".into(),
            "diff --git".into(),
        );

        assert_eq!(input.subject.repository, "acme/platform#42");
        assert_eq!(input.repo_dir, std::path::PathBuf::from("/tmp/repo"));
        assert_eq!(input.diff, "diff --git");
    }

    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
}
