use serde::Deserialize;

use nitpick_agent_client::HostClient;
use nitpick_agent_core::{Activity, Artifact, ChatInput, ReviewInput, ReviewSubject};
use nitpick_agent_github::DiscoveredPullRequest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    Version,
    Review {
        subject: String,
    },
    Chat {
        prompt: String,
    },
    Status,
    ReviewRequests {
        only_new: bool,
    },
    Activities,
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
    SyncPending {
        destination: Option<String>,
    },
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
            Some(_) => Err("usage: nitpick-agent review-requests [--new]".into()),
        },
        Some("activities") => Ok(CliCommand::Activities),
        Some("artifacts") => {
            let activity_id = args
                .next()
                .ok_or_else(|| "usage: nitpick-agent artifacts <activity-id>".to_owned())?;
            Ok(CliCommand::Artifacts { activity_id })
        }
        Some("artifact") => {
            let artifact_id = args
                .next()
                .ok_or_else(|| "usage: nitpick-agent artifact <artifact-id>".to_owned())?;
            Ok(CliCommand::Artifact { artifact_id })
        }
        Some("artifact-sync") => {
            let artifact_id = args.next().ok_or_else(|| {
                "usage: nitpick-agent artifact-sync <artifact-id> <destination>".to_owned()
            })?;
            let destination = args.next().ok_or_else(|| {
                "usage: nitpick-agent artifact-sync <artifact-id> <destination>".to_owned()
            })?;
            Ok(CliCommand::ArtifactSync {
                artifact_id,
                destination,
                target: args.next(),
            })
        }
        Some("sync-pending") => Ok(CliCommand::SyncPending {
            destination: args.next(),
        }),
        Some("review") => {
            let subject = args
                .next()
                .ok_or_else(|| "usage: nitpick-agent review <subject>".to_owned())?;
            Ok(CliCommand::Review { subject })
        }
        Some("chat") => {
            let prompt = args
                .next()
                .ok_or_else(|| "usage: nitpick-agent chat <prompt>".to_owned())?;
            Ok(CliCommand::Chat { prompt })
        }
        Some(command) => Err(format!("unknown command `{command}`")),
    }
}

pub fn help_text(version: &str) -> String {
    format!(
        "nitpick-agent {version}\n\nUsage: nitpick-agent <command>\n\nCommands:\n  review <subject>                                   Start a review activity\n  review-requests [--new]                            List GitHub PRs requesting your review\n  chat <prompt>                                      Start a chat activity\n  status                                             Show local activity status\n  activities                                         List local activities\n  artifacts <activity-id>                            List local artifacts for an activity\n  artifact <artifact-id>                             Show one local artifact\n  artifact-sync <artifact-id> <destination> [target]  Sync an artifact to a destination\n  sync-pending [destination]                         List artifacts pending sync\n  version                                            Print version\n\nOptions:\n  -h, --help                                         Print help\n  -V, --version                                      Print version"
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

pub fn format_review_requests(requests: &[DiscoveredPullRequest]) -> String {
    if requests.is_empty() {
        return "no GitHub review requests".into();
    }

    requests
        .iter()
        .map(|request| format!("{}/{}#{}", request.owner, request.repo, request.number))
        .collect::<Vec<_>>()
        .join("\n")
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

pub fn run_cli_command(
    command: CliCommand,
    host_addr: &str,
    repo_dir: std::path::PathBuf,
    diff: String,
    context: String,
) -> Result<String, String> {
    let client = HostClient::new(host_addr);
    match command {
        CliCommand::Help => Ok(help_text(env!("CARGO_PKG_VERSION"))),
        CliCommand::Version => Ok(format!("nitpick-agent {}", env!("CARGO_PKG_VERSION"))),
        CliCommand::Status => match client.status() {
            Ok(status) => Ok(format_host_status(&host_status(status))),
            Err(error) if error.starts_with("nitpick-agent-host unavailable") => Ok(format!(
                "nitpick-agent-host: not connected\naddress: {host_addr}"
            )),
            Err(error) => Err(error),
        },
        CliCommand::ReviewRequests { only_new } => Ok(format_review_requests(
            &client.github_review_requests(only_new)?,
        )),
        CliCommand::Activities => Ok(format_activities(&client.activities()?)),
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
        CliCommand::SyncPending { destination } => Ok(format_artifacts(
            &client.pending_sync_artifacts(destination.as_deref())?,
        )),
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
    }
}

#[cfg(test)]
mod tests {
    use super::{CliCommand, HostStatus, format_host_status, parse_command};
    use nitpick_agent_github::DiscoveredPullRequest;

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

        assert_eq!(error, "usage: nitpick-agent review <subject>");
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

        assert_eq!(error, "usage: nitpick-agent artifacts <activity-id>");
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
        };

        assert_eq!(
            format_host_status(&status),
            "nitpick-agent-host: connected\nactivities: 2 (1 running, 1 completed, 0 error)\nartifacts: 5\nlocal-only artifacts: 3\npending-sync artifacts: 1\nagent: claude\nmodel: sonnet"
        );
    }

    #[test]
    fn parses_host_status_json() {
        let status = super::parse_host_status_json(
            r#"{"activity_count":2,"running_activity_count":1,"completed_activity_count":1,"error_activity_count":0,"artifact_count":5,"local_only_artifact_count":3,"pending_sync_artifact_count":1,"provider":"claude","model":null}"#,
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
    fn formats_review_requests() {
        let requests = vec![DiscoveredPullRequest {
            owner: "acme".into(),
            repo: "platform".into(),
            number: 42,
            head_sha: "abc123".into(),
        }];

        assert_eq!(super::format_review_requests(&requests), "acme/platform#42");
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
}
