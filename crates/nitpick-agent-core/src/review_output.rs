use std::path::Path;

use fs_err as fs;

use crate::{AgentError, AgentResult, ReviewCommentValidator, ReviewOutput, parse_json_str};

pub const REVIEW_OUTPUT_RELATIVE_PATH: &str = ".nitpick/review-output.json";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewOutput {
    comments: Vec<StrictReviewComment>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewComment {
    path: String,
    line: u32,
    body: String,
}

pub fn validate_review_output_file(
    repo_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> AgentResult<ReviewOutput> {
    let repo_dir = canonical_repo_dir(repo_dir.as_ref())?;
    let validator = ReviewCommentValidator::new(repo_dir.as_path())?;
    validate_review_output_file_with_validator(repo_dir.as_path(), output_path, &validator)
}

pub fn validate_review_output_file_for_diff(
    repo_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    diff: &str,
) -> AgentResult<ReviewOutput> {
    let repo_dir = canonical_repo_dir(repo_dir.as_ref())?;
    let validator = ReviewCommentValidator::for_diff(repo_dir.as_path(), diff)?;
    validate_review_output_file_with_validator(repo_dir.as_path(), output_path, &validator)
}

fn validate_review_output_file_with_validator(
    repo_dir: &Path,
    output_path: impl AsRef<Path>,
    validator: &ReviewCommentValidator,
) -> AgentResult<ReviewOutput> {
    let output_path = output_path.as_ref();
    if !output_path.exists() {
        let display = output_path
            .strip_prefix(repo_dir)
            .unwrap_or(output_path)
            .display();
        return Err(AgentError::invalid_input(format!(
            "review output file missing: {display}"
        )));
    }

    let output_path = output_path
        .canonicalize()
        .map_err(|error| AgentError::io("canonicalize review output file", error))?;
    if !output_path.starts_with(repo_dir) {
        return Err(AgentError::invalid_input(
            "review output file escapes repository",
        ));
    }

    let input = fs::read_to_string(&output_path)
        .map_err(|error| AgentError::io_path("read review output file", &output_path, error))?;
    let output: StrictReviewOutput = parse_json_str(&input, "invalid review output JSON")?;
    validate_review_output(output, validator)
}

fn canonical_repo_dir(repo_dir: &Path) -> AgentResult<std::path::PathBuf> {
    repo_dir
        .canonicalize()
        .map_err(|error| AgentError::io("canonicalize repository directory", error))
}

fn validate_review_output(
    output: StrictReviewOutput,
    validator: &ReviewCommentValidator,
) -> AgentResult<ReviewOutput> {
    let mut comments = Vec::with_capacity(output.comments.len());
    for comment in output.comments {
        comments.push(validator.validate_comment(&comment.path, comment.line, comment.body)?);
    }

    Ok(ReviewOutput { comments })
}
