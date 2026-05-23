use nitpick_agent_core::Artifact;

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
        .map(|artifact| {
            format!(
                "{}  {:?}  {:?}",
                crate::style::label(artifact.id.to_string()),
                artifact.kind,
                artifact.sync_state
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_artifact(artifact: &Artifact) -> String {
    format!(
        "{} {}\n{} {}\n{} {:?}\n{} {:?}",
        crate::style::label("artifact"),
        artifact.id,
        crate::style::label("activity"),
        artifact.activity_id,
        crate::style::label("kind"),
        artifact.kind,
        crate::style::label("sync"),
        artifact.sync_state
    )
}

#[cfg(test)]
mod tests {
    use crate::parse_command;

    #[test]
    fn rejects_artifact_command() {
        let error = parse_command([
            "artifact".to_owned(),
            "list".to_owned(),
            "activity-1".to_owned(),
        ])
        .expect_err("command fails");

        assert!(error.contains("unrecognized subcommand 'artifact'"));
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
