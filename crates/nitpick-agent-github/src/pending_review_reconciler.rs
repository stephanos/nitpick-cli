use nitpick_agent_model::{Artifact, ArtifactContent};

use crate::{GitHubReviewComment, review_payload::artifact_marker};

pub(crate) fn remote_comment_matches(
    remote_comments: &[GitHubReviewComment],
    artifact: &Artifact,
) -> bool {
    let ArtifactContent::ReviewComment(local) = &artifact.content else {
        return false;
    };
    let marker = artifact_marker(&artifact.id);
    remote_comments.iter().any(|remote| {
        remote.body.contains(&marker)
            || (remote.path == local.path
                && remote.line.unwrap_or(0) == local.line
                && normalized_comment_body(&remote.body) == normalized_comment_body(&local.body))
    })
}

fn normalized_comment_body(body: &str) -> String {
    let body = body.trim();
    let body = body.strip_prefix('🤖').unwrap_or(body).trim_start();
    let body = match body.rfind("<!-- nitpick-agent:") {
        Some(marker_start)
            if body[marker_start..].find("-->").is_some_and(|marker_end| {
                body[marker_start + marker_end + 3..].trim().is_empty()
            }) =>
        {
            &body[..marker_start]
        }
        _ => body,
    };
    body.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use nitpick_agent_model::{ActivityId, ArtifactId, ArtifactKind, ReviewComment};

    use super::*;

    #[test]
    fn exact_artifact_marker_matches_before_location_and_body() {
        let artifact = review_comment_artifact("artifact-1", "new body");
        let remote = remote_comment(
            "different/path.rs",
            Some(3),
            "🤖 old body\n\n<!-- nitpick-agent:artifact-1 -->",
        );

        assert!(remote_comment_matches(&[remote], &artifact));
    }

    #[test]
    fn semantic_identity_matches_finding_from_another_activity() {
        let artifact = review_comment_artifact("artifact-2", "Prefer this.");
        let remote = remote_comment(
            "src/lib.rs",
            Some(12),
            "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->",
        );

        assert!(remote_comment_matches(&[remote], &artifact));
    }

    #[test]
    fn semantic_identity_normalizes_file_level_location() {
        let mut artifact = review_comment_artifact("artifact-2", "No findings.");
        let ArtifactContent::ReviewComment(comment) = &mut artifact.content else {
            panic!("review comment artifact");
        };
        comment.line = 0;
        let remote = remote_comment(
            "src/lib.rs",
            None,
            "🤖 No findings.\n\n<!-- nitpick-agent:older-artifact -->",
        );

        assert!(remote_comment_matches(&[remote], &artifact));
    }

    #[test]
    fn semantic_identity_keeps_different_findings_distinct() {
        let artifact = review_comment_artifact("artifact-2", "Prefer this.");
        let remote = remote_comment(
            "src/lib.rs",
            Some(13),
            "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->",
        );

        assert!(!remote_comment_matches(&[remote], &artifact));
    }

    #[test]
    fn semantic_identity_does_not_strip_marker_followed_by_prose() {
        let artifact = review_comment_artifact("artifact-2", "Prefer this.");
        let remote = remote_comment(
            "src/lib.rs",
            Some(12),
            "🤖 Prefer this.\n\n<!-- nitpick-agent:artifact-1 -->\nExtra <!-- note -->",
        );

        assert!(!remote_comment_matches(&[remote], &artifact));
    }

    fn review_comment_artifact(id: &str, body: &str) -> Artifact {
        Artifact::local(
            ArtifactId::new(id),
            ActivityId::new("activity"),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(ReviewComment {
                path: "src/lib.rs".into(),
                line: 12,
                body: body.into(),
            }),
        )
    }

    fn remote_comment(path: &str, line: Option<u32>, body: &str) -> GitHubReviewComment {
        GitHubReviewComment {
            id: "101".into(),
            review_id: Some("99".into()),
            path: path.into(),
            line,
            body: body.into(),
            author: Some("nitpick".into()),
            draft: true,
        }
    }
}
