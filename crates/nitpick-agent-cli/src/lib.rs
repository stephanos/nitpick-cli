mod activity;
mod artifact;
mod chat;
mod cli;
mod context;
mod review;
mod system;

use serde::Deserialize;
use std::{path::Path, process::Command};

use nitpick_agent_client::HostClientError;
use nitpick_agent_core::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore, AgentError, Artifact,
    ArtifactContent, ChatInput, FsActivityStore, ReviewInput, ReviewRequest, ReviewSubject,
    config_path_from_env_value, data_dir_from_env_value,
};
use nitpick_agent_github::{GitHubCliDiscovery, PullRequestRef};

pub use activity::{ActivityArgs, ActivityCommand};
pub use artifact::{ArtifactArgs, ArtifactCommand};
pub use chat::{ChatArgs, ChatCommand};
pub use cli::{
    CliInvocation, CliOptions, CommandGroup as CliCommand, help_text, parse_command,
    parse_invocation,
};
pub use context::CliRunContext;
pub use review::{ReviewArgs, ReviewCommand};
pub use system::{SystemArgs, SystemCommand};

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

pub(crate) fn inspect_checkout_with_discovery(
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
        ..ChatInput::default()
    }
}

pub fn config_path_from_env(
    nitpick_agent_config: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    config_path_from_env_value(nitpick_agent_config)
}

pub fn data_dir_from_env(nitpick_agent_data_dir: Option<std::ffi::OsString>) -> std::path::PathBuf {
    data_dir_from_env_value(nitpick_agent_data_dir)
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

pub(crate) fn handle_resume_error(activity: &Activity, data_dir: &Path, error: String) -> String {
    if !provider_session_missing(&error) {
        return error;
    }
    let Some(session_id) = activity.session.provider_session_id.as_deref() else {
        return error;
    };
    let message = format!(
        "activity {} can no longer be resumed because provider session {} was not found; cleared the stored session id",
        activity.id, session_id
    );
    match clear_provider_session_id(data_dir, activity) {
        Ok(()) => message,
        Err(clear_error) => format!("{message} (failed to persist recovery: {clear_error})"),
    }
}

fn provider_session_missing(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("session not found")
        || error.contains("session does not exist")
        || error.contains("conversation not found")
}

fn clear_provider_session_id(data_dir: &Path, activity: &Activity) -> Result<(), String> {
    let store = FsActivityStore::new(data_dir).map_err(|error| error.to_string())?;
    let mut stored = store.get(&activity.id).map_err(|error| error.to_string())?;
    stored.session.provider_session_id = None;
    stored.touch();
    store.save(&stored).map_err(|error| error.to_string())
}

fn configured_github_discovery(config: &nitpick_agent_host::AgentConfig) -> GitHubCliDiscovery {
    match &config.checkout_dir {
        Some(checkout_dir) => GitHubCliDiscovery::with_checkout_commands(
            config.github_command.as_deref().unwrap_or("gh"),
            "git",
            checkout_dir,
        ),
        None => GitHubCliDiscovery::new(config.github_command.as_deref().unwrap_or("gh")),
    }
}

pub(crate) fn apply_sandbox_option(
    config: &mut nitpick_agent_host::AgentConfig,
    options: &CliOptions,
) {
    if options.disable_sandbox {
        config.sandbox = nitpick_agent_host::AgentSandboxConfig {
            mode: "none".into(),
        };
    }
}

pub(crate) fn require_cached_checkout(
    target: &str,
    config: &nitpick_agent_host::AgentConfig,
) -> Result<std::path::PathBuf, String> {
    let pull_request = target
        .parse::<PullRequestRef>()
        .map_err(|error| format!("invalid GitHub pull request reference: {error}"))?;
    let checkout = configured_github_discovery(config).checkout_path_for(&pull_request);
    if !checkout.join(".git").is_dir() {
        return Err(format!("checkout not found for {target}"));
    }
    Ok(checkout)
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
        CliCommand::Version => Ok(format!("nitpick {}", env!("CARGO_PKG_VERSION"))),
        CliCommand::Review(command) => review::run(command, context, options),
        CliCommand::Activity(command) => activity::run(command, context, options),
        CliCommand::Artifact(command) => artifact::run(command, context, options),
        CliCommand::System(command) => system::run(command, context, options),
        CliCommand::Chat(command) => chat::run(command, context, options),
    }
}

pub(crate) fn host_status(status: nitpick_agent_client::HostStatus) -> HostStatus {
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
    use super::{
        ActivityCommand, ArtifactCommand, ChatCommand, CliCommand, HostStatus, ReviewCommand,
        SystemCommand, format_host_status, parse_command, parse_invocation,
    };
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
    fn parses_status_command() {
        let command =
            parse_command(["system".to_owned(), "status".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::System(SystemCommand::Status));
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
    fn parses_activities_command() {
        let command =
            parse_command(["activity".to_owned(), "list".to_owned()]).expect("command parses");

        assert_eq!(command, CliCommand::Activity(ActivityCommand::List));
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
    fn parses_logs_command() {
        let command = parse_command([
            "activity".to_owned(),
            "logs".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Activity(ActivityCommand::Logs {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn parses_resume_command() {
        let command = parse_command([
            "activity".to_owned(),
            "resume".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Activity(ActivityCommand::Resume {
                target: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn rejects_logs_without_target() {
        let error =
            parse_command(["activity".to_owned(), "logs".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick activity logs <TARGET>"));
    }

    #[test]
    fn rejects_resume_without_target() {
        let error =
            parse_command(["activity".to_owned(), "resume".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick activity resume <TARGET>"));
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
    fn parses_inspect_command() {
        let command = parse_command([
            "activity".to_owned(),
            "inspect".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Activity(ActivityCommand::Inspect {
                pull_request: "acme/platform#42".into(),
            })
        );
    }

    #[test]
    fn rejects_inspect_without_pr_ref() {
        let error = parse_command(["activity".to_owned(), "inspect".to_owned()])
            .expect_err("command fails");

        assert!(error.contains("Usage: nitpick activity inspect <PULL_REQUEST>"));
    }

    #[test]
    fn parses_artifacts_command() {
        let command = parse_command([
            "artifact".to_owned(),
            "list".to_owned(),
            "activity-1".to_owned(),
        ])
        .expect("command parses");

        assert_eq!(
            command,
            CliCommand::Artifact(ArtifactCommand::List {
                activity_id: "activity-1".into(),
            })
        );
    }

    #[test]
    fn rejects_artifacts_without_activity_id() {
        let error =
            parse_command(["artifact".to_owned(), "list".to_owned()]).expect_err("command fails");

        assert!(error.contains("Usage: nitpick artifact list <ACTIVITY_ID>"));
    }

    #[test]
    fn parses_artifact_command() {
        let command = parse_command([
            "artifact".to_owned(),
            "show".to_owned(),
            "artifact-1".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Artifact(ArtifactCommand::Show {
                artifact_id: "artifact-1".into(),
            })
        );
    }

    #[test]
    fn parses_artifact_sync_command() {
        let command = parse_command([
            "artifact".to_owned(),
            "sync".to_owned(),
            "artifact-1".to_owned(),
            "github".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Artifact(ArtifactCommand::Sync {
                artifact_id: "artifact-1".into(),
                destination: "github".into(),
                target: None,
            })
        );
    }

    #[test]
    fn parses_artifact_sync_command_with_target() {
        let command = parse_command([
            "artifact".to_owned(),
            "sync".to_owned(),
            "artifact-1".to_owned(),
            "github".to_owned(),
            "acme/platform#42".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::Artifact(ArtifactCommand::Sync {
                artifact_id: "artifact-1".into(),
                destination: "github".into(),
                target: Some("acme/platform#42".into()),
            })
        );
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
    fn parses_sync_pending_command_with_destination() {
        let command = parse_command([
            "system".to_owned(),
            "sync-pending".to_owned(),
            "github".to_owned(),
        ])
        .expect("command");

        assert_eq!(
            command,
            CliCommand::System(SystemCommand::SyncPending {
                destination: Some("github".into()),
            })
        );
    }

    #[test]
    fn parses_sync_pending_command_without_destination() {
        let command =
            parse_command(["system".to_owned(), "sync-pending".to_owned()]).expect("command");

        assert_eq!(
            command,
            CliCommand::System(SystemCommand::SyncPending { destination: None })
        );
    }

    #[test]
    fn parses_cleanup_checkouts_command() {
        let command =
            parse_command(["system".to_owned(), "cleanup-checkouts".to_owned()]).expect("command");

        assert_eq!(command, CliCommand::System(SystemCommand::CleanupCheckouts));
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
            super::config_path_from_env(Some("/tmp/config.toml".into())),
            std::path::PathBuf::from("/tmp/config.toml")
        );
        assert_eq!(
            super::config_path_from_env(None),
            nitpick_agent_core::default_config_path()
        );
    }

    #[test]
    fn resolves_data_dir_like_host() {
        assert_eq!(
            super::data_dir_from_env(Some("/tmp/data".into())),
            std::path::PathBuf::from("/tmp/data")
        );
        assert_eq!(
            super::data_dir_from_env(None),
            nitpick_agent_core::default_data_dir()
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
            CliCommand::Activity(ActivityCommand::Logs {
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
