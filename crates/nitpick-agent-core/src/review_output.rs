use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use fs_err as fs;

use crate::{AgentError, AgentResult, RepoPath, ReviewOutput, parse_json_str};
use unidiff::PatchSet;

pub const REVIEW_OUTPUT_RELATIVE_PATH: &str = ".nitpick/review-output.json";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewOutput {
    summary: String,
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
    validate_review_output_file_with_changes(repo_dir, output_path, None)
}

pub fn validate_review_output_file_for_diff(
    repo_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    diff: &str,
) -> AgentResult<ReviewOutput> {
    let changeset = DiffChangeset::parse(diff)?;
    validate_review_output_file_with_changes(repo_dir, output_path, Some(&changeset))
}

fn validate_review_output_file_with_changes(
    repo_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    changeset: Option<&DiffChangeset>,
) -> AgentResult<ReviewOutput> {
    let repo_dir = canonical_repo_dir(repo_dir.as_ref())?;
    let output_path = output_path.as_ref();
    if !output_path.exists() {
        let display = output_path
            .strip_prefix(&repo_dir)
            .unwrap_or(output_path)
            .display();
        return Err(AgentError::invalid_input(format!(
            "review output file missing: {display}"
        )));
    }

    let output_path = output_path
        .canonicalize()
        .map_err(|error| AgentError::io("canonicalize review output file", error))?;
    if !output_path.starts_with(&repo_dir) {
        return Err(AgentError::invalid_input(
            "review output file escapes repository",
        ));
    }

    let input = fs::read_to_string(&output_path)
        .map_err(|error| AgentError::io_path("read review output file", &output_path, error))?;
    let output: StrictReviewOutput = parse_json_str(&input, "invalid review output JSON")?;
    validate_review_output(repo_dir.as_path(), output, changeset)
}

fn canonical_repo_dir(repo_dir: &Path) -> AgentResult<std::path::PathBuf> {
    repo_dir
        .canonicalize()
        .map_err(|error| AgentError::io("canonicalize repository directory", error))
}

fn validate_review_output(
    repo_dir: &Path,
    output: StrictReviewOutput,
    changeset: Option<&DiffChangeset>,
) -> AgentResult<ReviewOutput> {
    if output.summary.trim().is_empty() {
        return Err(AgentError::invalid_input("review summary is empty"));
    }

    let mut comments = Vec::with_capacity(output.comments.len());
    for comment in output.comments {
        let comment_path = RepoPath::parse(&comment.path)?;
        if comment.body.trim().is_empty() {
            return Err(AgentError::invalid_input(format!(
                "review comment body is empty: {}",
                comment.path
            )));
        }
        let comment_file = repo_dir.join(comment_path.as_str());
        if !comment_file.exists() {
            return Err(AgentError::invalid_input(format!(
                "review comment path does not exist in repository: {}",
                comment.path
            )));
        }
        if !comment_file.is_file() {
            return Err(AgentError::invalid_input(format!(
                "review comment path is not a file: {}",
                comment.path
            )));
        }
        if let Some(changeset) = changeset {
            changeset.validate_comment_location(comment_path.as_str(), comment.line)?;
        }
        comments.push(crate::ReviewComment {
            path: comment_path.as_str().to_owned(),
            line: comment.line,
            body: comment.body,
        });
    }

    Ok(ReviewOutput {
        summary: output.summary,
        comments,
    })
}

#[derive(Debug, Default)]
struct DiffChangeset {
    changed_lines: HashMap<String, HashSet<u32>>,
}

impl DiffChangeset {
    fn parse(diff: &str) -> AgentResult<Self> {
        let mut patch = PatchSet::new();
        patch
            .parse(diff)
            .map_err(|error| AgentError::invalid_input(format!("invalid review diff: {error}")))?;

        let mut changeset = Self::default();
        for file in patch {
            let Some(path) = normalized_target_path(&file.target_file) else {
                continue;
            };
            for hunk in file.hunks() {
                for line in hunk.lines() {
                    if line.is_added() {
                        let line_number =
                            u32::try_from(line.target_line_no.unwrap_or(0)).map_err(|_| {
                                AgentError::invalid_input("review diff line number is too large")
                            })?;
                        changeset
                            .changed_lines
                            .entry(path.clone())
                            .or_default()
                            .insert(line_number);
                    }
                }
            }
        }

        Ok(changeset)
    }

    fn validate_comment_location(&self, path: &str, line: u32) -> AgentResult<()> {
        let Some(changed_lines) = self.changed_lines.get(path) else {
            return Err(AgentError::invalid_input(format!(
                "review comment path is outside the diff changeset: {path}"
            )));
        };

        if line != 0 && !changed_lines.contains(&line) {
            return Err(AgentError::invalid_input(format!(
                "review comment line is outside the diff changeset: {path}:{line}"
            )));
        }

        Ok(())
    }
}

fn normalized_target_path(path: &str) -> Option<String> {
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix("b/").unwrap_or(path).to_owned())
}
