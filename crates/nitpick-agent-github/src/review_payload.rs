use nitpick_agent_core::{AgentError, AgentResult, Artifact, ArtifactContent, ReviewComment};

pub struct GitHubReviewPayload;

impl GitHubReviewPayload {
    pub fn comment(comment: ReviewComment) -> serde_json::Value {
        if comment.line == 0 {
            serde_json::json!({
                "path": comment.path,
                "subject_type": "file",
                "body": robot_prefixed_body(&comment.body),
            })
        } else {
            serde_json::json!({
                "path": comment.path,
                "line": comment.line,
                "side": "RIGHT",
                "body": robot_prefixed_body(&comment.body),
            })
        }
    }

    pub(crate) fn batch(
        head_sha: String,
        artifacts: &[Artifact],
    ) -> AgentResult<serde_json::Value> {
        let mut body = None;
        let mut comments = Vec::new();
        for artifact in artifacts {
            match &artifact.content {
                ArtifactContent::ReviewSummary(summary) => {
                    body = Some(summary.clone());
                }
                ArtifactContent::ReviewComment(comment) => {
                    comments.push(comment.clone());
                }
                ArtifactContent::ChatResponse(_) => {
                    return Err(AgentError::invalid_input(
                        "github-review sync only supports review artifacts",
                    ));
                }
            }
        }
        if body.is_none() && comments.is_empty() {
            return Err(AgentError::invalid_input(
                "github-review sync requires at least one review summary or comment",
            ));
        }

        let payload_comments = comments.into_iter().map(Self::comment).collect::<Vec<_>>();
        let mut payload = serde_json::json!({
            "commit_id": head_sha,
            "comments": payload_comments,
        });
        if let Some(body) = body {
            payload["body"] = serde_json::Value::String(body);
        }
        Ok(payload)
    }
}

pub(crate) fn robot_prefixed_body(body: &str) -> String {
    if body.starts_with("🤖") {
        body.to_owned()
    } else {
        format!("🤖 {body}")
    }
}
