use clap::{Args, Subcommand};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::Artifact;

use crate::{CliError, CliOptions, CliRunContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactCommand {
    List {
        activity_id: String,
    },
    Show {
        artifact_id: String,
    },
    Sync {
        artifact_id: String,
        destination: String,
        target: Option<String>,
    },
}

#[derive(Args)]
pub struct ArtifactArgs {
    #[command(subcommand)]
    pub command: ArtifactSubcommand,
}

#[derive(Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ArtifactSubcommand {
    List {
        activity_id: String,
    },
    Show {
        artifact_id: String,
    },
    Sync {
        artifact_id: String,
        destination: String,
        target: Option<String>,
    },
}

impl From<ArtifactSubcommand> for ArtifactCommand {
    fn from(command: ArtifactSubcommand) -> Self {
        match command {
            ArtifactSubcommand::List { activity_id } => Self::List { activity_id },
            ArtifactSubcommand::Show { artifact_id } => Self::Show { artifact_id },
            ArtifactSubcommand::Sync {
                artifact_id,
                destination,
                target,
            } => Self::Sync {
                artifact_id,
                destination,
                target,
            },
        }
    }
}

pub fn run(
    command: ArtifactCommand,
    context: CliRunContext,
    _options: CliOptions,
) -> Result<String, CliError> {
    let client = HostClient::new(&context.host_addr);
    match command {
        ArtifactCommand::List { activity_id } => {
            Ok(format_artifacts(&client.activity_artifacts(&activity_id)?))
        }
        ArtifactCommand::Show { artifact_id } => Ok(format_artifact(&client.artifact(&artifact_id)?)),
        ArtifactCommand::Sync {
            artifact_id,
            destination,
            target,
        } => Ok(format_artifact(&client.sync_artifact(
            &artifact_id,
            &destination,
            target.as_deref(),
        )?)),
    }
}

pub fn parse_artifacts_json(body: &str) -> Result<Vec<Artifact>, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host artifacts response: {error}"))
}

pub fn parse_artifact_json(body: &str) -> Result<Artifact, String> {
    serde_json::from_str(body).map_err(|error| format!("invalid host artifact response: {error}"))
}

pub fn format_artifacts(artifacts: &[Artifact]) -> String {
    if artifacts.is_empty() {
        return "no local artifacts".into();
    }

    artifacts
        .iter()
        .map(|artifact| format!("{}: {:?} {:?}", artifact.id, artifact.kind, artifact.sync_state))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_artifact(artifact: &Artifact) -> String {
    format!(
        "{}\nactivity: {}\nkind: {:?}\nsync: {:?}",
        artifact.id, artifact.activity_id, artifact.kind, artifact.sync_state
    )
}

#[cfg(test)]
mod tests {
    use super::ArtifactCommand;
    use crate::{CliCommand, parse_command};

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
}
