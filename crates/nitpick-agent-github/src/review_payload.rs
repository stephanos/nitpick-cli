use nitpick_agent_core::{AgentError, AgentResult};
use nitpick_agent_model::{Artifact, ArtifactContent, ArtifactId, ReviewComment};

pub struct GitHubReviewPayload;

impl GitHubReviewPayload {
    pub fn comment(comment: ReviewComment) -> serde_json::Value {
        Self::review_comment(&comment, None)
    }

    fn artifact_comment(artifact: &Artifact) -> AgentResult<serde_json::Value> {
        let ArtifactContent::ReviewComment(comment) = &artifact.content else {
            return Err(AgentError::invalid_input(
                "github-review comment payload requires a review comment artifact",
            ));
        };
        Ok(Self::review_comment(comment, Some(&artifact.id)))
    }

    fn review_comment(
        comment: &ReviewComment,
        artifact_id: Option<&ArtifactId>,
    ) -> serde_json::Value {
        let body = match artifact_id {
            Some(artifact_id) => marked_robot_body(&comment.body, artifact_id),
            None => robot_prefixed_body(&comment.body),
        };
        if comment.line == 0 {
            serde_json::json!({
                "path": comment.path,
                "subject_type": "file",
                "body": body,
            })
        } else {
            serde_json::json!({
                "path": comment.path,
                "line": comment.line,
                "side": "RIGHT",
                "body": body,
            })
        }
    }

    pub(crate) fn append_comment_to_pending_review(
        review_node_id: &str,
        artifact: &Artifact,
    ) -> AgentResult<serde_json::Value> {
        let ArtifactContent::ReviewComment(comment) = &artifact.content else {
            return Err(AgentError::invalid_input(
                "github-review comment payload requires a review comment artifact",
            ));
        };
        let mut input = serde_json::json!({
            "pullRequestReviewId": review_node_id,
            "path": comment.path,
            "body": marked_robot_body(&comment.body, &artifact.id),
        });
        if comment.line == 0 {
            input["subjectType"] = serde_json::Value::String("FILE".into());
        } else {
            input["line"] = serde_json::Value::from(comment.line);
            input["side"] = serde_json::Value::String("RIGHT".into());
        }
        Ok(serde_json::json!({
            "query": "mutation AddPullRequestReviewThread($input: AddPullRequestReviewThreadInput!) { addPullRequestReviewThread(input: $input) { thread { id } } }",
            "variables": { "input": input },
        }))
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
                    body = Some(marked_body(summary, &artifact.id));
                }
                ArtifactContent::ReviewComment(_) => {
                    comments.push(Self::artifact_comment(artifact)?);
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

        let mut payload = serde_json::json!({
            "commit_id": head_sha,
            "comments": comments,
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

pub(crate) fn marked_body(body: &str, artifact_id: &ArtifactId) -> String {
    format!("{}\n\n{}", body.trim_end(), artifact_marker(artifact_id))
}

pub(crate) fn marked_robot_body(body: &str, artifact_id: &ArtifactId) -> String {
    marked_body(&robot_prefixed_body(body), artifact_id)
}

pub(crate) fn artifact_marker(artifact_id: &ArtifactId) -> String {
    format!("<!-- nitpick-agent:{artifact_id} -->")
}
