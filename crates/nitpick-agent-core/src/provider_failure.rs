use crate::{Activity, ActivityStatus, AgentProviderKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFailureClassification {
    pub kind: ProviderFailureKind,
    pub title: String,
    pub detail: String,
    pub suggested_action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    AuthInvalidCredentials,
    SandboxPermissionDenied,
    ProviderUnavailable,
    UnknownProviderFailure,
}

pub fn classify_provider_failure(activity: &Activity) -> Option<ProviderFailureClassification> {
    if activity.status != ActivityStatus::Error {
        return None;
    }

    let text = provider_failure_text(activity);
    let lower = text.to_ascii_lowercase();

    if lower.contains("invalid authentication credentials")
        || lower.contains("failed to authenticate")
        || lower.contains("api error: 401")
        || lower.contains("\"authentication_error\"")
    {
        let (title, fallback_detail, suggested_action) = if activity.session.provider
            == Some(AgentProviderKind::Claude)
        {
            (
                "Claude authentication failed",
                "Claude returned 401 Invalid authentication credentials.",
                "Run `claude auth logout && claude auth login`, then rerun the provider diagnostic.",
            )
        } else {
            (
                "Provider authentication failed",
                "The provider reported an authentication failure.",
                "Refresh the configured provider credentials, then rerun the provider diagnostic.",
            )
        };
        return Some(ProviderFailureClassification {
            kind: ProviderFailureKind::AuthInvalidCredentials,
            title: title.into(),
            detail: first_matching_line(&text, "invalid authentication credentials")
                .or_else(|| first_matching_line(&text, "failed to authenticate"))
                .or_else(|| first_matching_line(&text, "authentication_error"))
                .unwrap_or(fallback_detail)
                .into(),
            suggested_action: Some(suggested_action.into()),
        });
    }

    if lower.contains("sandbox") && lower.contains("deny(") {
        return Some(ProviderFailureClassification {
            kind: ProviderFailureKind::SandboxPermissionDenied,
            title: "Provider sandbox blocked access".into(),
            detail: first_matching_line(&text, "deny(")
                .unwrap_or("The macOS sandbox blocked provider file access.")
                .into(),
            suggested_action: Some(
                "Run `nitpick --no-sandbox debug provider` to confirm whether the sandbox is involved."
                    .into(),
            ),
        });
    }

    if lower.contains("not found on path")
        || lower.contains("failed to start")
        || lower.contains("timed out")
    {
        return Some(ProviderFailureClassification {
            kind: ProviderFailureKind::ProviderUnavailable,
            title: "Provider command unavailable".into(),
            detail: activity
                .error
                .clone()
                .unwrap_or_else(|| "The provider command could not run.".into()),
            suggested_action: Some("Check the configured provider command and PATH.".into()),
        });
    }

    if lower.contains("provider command failed") {
        return Some(ProviderFailureClassification {
            kind: ProviderFailureKind::UnknownProviderFailure,
            title: "Provider command failed".into(),
            detail: activity
                .error
                .clone()
                .unwrap_or_else(|| "The provider command failed.".into()),
            suggested_action: Some("Run the provider diagnostic for more details.".into()),
        });
    }

    None
}

fn provider_failure_text(activity: &Activity) -> String {
    let mut parts = Vec::new();
    if let Some(error) = &activity.error {
        parts.push(error.as_str());
    }
    parts.extend(
        activity
            .session
            .messages
            .iter()
            .filter(|message| message.role.starts_with("provider."))
            .map(|message| message.content.as_str()),
    );
    parts.join("\n")
}

fn first_matching_line<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let marker = marker.to_ascii_lowercase();
    text.lines()
        .find(|line| line.to_ascii_lowercase().contains(&marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Activity, ActivityId, ActivityKind, ActivityStatus, AgentMessage, AgentProviderKind,
    };

    fn failed_review_with_logs(error: &str, messages: Vec<(&str, &str)>) -> Activity {
        let mut activity = Activity::new(ActivityId::new("activity-1"), ActivityKind::Review);
        activity.status = ActivityStatus::Error;
        activity.error = Some(error.into());
        activity.session.messages = messages
            .into_iter()
            .map(|(role, content)| AgentMessage {
                role: role.into(),
                content: content.into(),
            })
            .collect();
        activity
    }

    #[test]
    fn classifies_claude_401_as_invalid_credentials() {
        let mut activity = failed_review_with_logs(
            "claude provider command failed with status exit status: 1",
            vec![(
                "provider.stdout",
                "Failed to authenticate. API Error: 401 Invalid authentication credentials",
            )],
        );
        activity.session.provider = Some(AgentProviderKind::Claude);

        let classification = classify_provider_failure(&activity).expect("classification");

        assert_eq!(
            classification.kind,
            ProviderFailureKind::AuthInvalidCredentials
        );
        assert_eq!(classification.title, "Claude authentication failed");
        assert!(
            classification
                .detail
                .contains("Invalid authentication credentials")
        );
        assert_eq!(
            classification.suggested_action.as_deref(),
            Some(
                "Run `claude auth logout && claude auth login`, then rerun the provider diagnostic."
            )
        );
    }

    #[test]
    fn classifies_non_claude_auth_failure_with_provider_neutral_remediation() {
        let mut activity = failed_review_with_logs(
            "codex provider command failed with status exit status: 1",
            vec![(
                "provider.stderr",
                r#"{"type":"authentication_error","message":"Failed to authenticate"}"#,
            )],
        );
        activity.session.provider = Some(AgentProviderKind::Codex);

        let classification = classify_provider_failure(&activity).expect("classification");

        assert_eq!(
            classification.kind,
            ProviderFailureKind::AuthInvalidCredentials
        );
        assert_eq!(classification.title, "Provider authentication failed");
        assert_eq!(
            classification.suggested_action.as_deref(),
            Some(
                "Refresh the configured provider credentials, then rerun the provider diagnostic."
            )
        );
        assert!(!classification.title.contains("Claude"));
        assert!(!classification.detail.contains("Claude"));
        assert!(
            !classification
                .suggested_action
                .as_deref()
                .unwrap_or_default()
                .contains("claude")
        );
    }

    #[test]
    fn classifies_sandbox_denied_path_as_permission_denied() {
        let activity = failed_review_with_logs(
            "claude provider command failed with status exit status: 1; sandbox was enabled",
            vec![(
                "provider.sandbox",
                "matching macOS sandbox violations:\nSandbox: claude deny(1) file-read-data /Users/stephan/.claude.lock",
            )],
        );

        let classification = classify_provider_failure(&activity).expect("classification");

        assert_eq!(
            classification.kind,
            ProviderFailureKind::SandboxPermissionDenied
        );
        assert_eq!(classification.title, "Provider sandbox blocked access");
        assert!(classification.detail.contains(".claude.lock"));
    }

    #[test]
    fn classifies_missing_provider_command_as_unavailable() {
        let activity =
            failed_review_with_logs("provider command `claude` not found on PATH", vec![]);

        let classification = classify_provider_failure(&activity).expect("classification");

        assert_eq!(
            classification.kind,
            ProviderFailureKind::ProviderUnavailable
        );
        assert_eq!(classification.title, "Provider command unavailable");
    }

    #[test]
    fn ignores_completed_activity() {
        let mut activity =
            failed_review_with_logs("401 Invalid authentication credentials", vec![]);
        activity.status = ActivityStatus::Completed;

        assert_eq!(classify_provider_failure(&activity), None);
    }
}
