use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path},
};

use crate::{AgentError, AgentResult, ReviewOutput};

pub const REVIEW_OUTPUT_RELATIVE_PATH: &str = ".nitpick/review-output.json";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewOutput {
    summary: String,
    comments: Vec<StrictReviewComment>,
    journey: StrictReviewJourney,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewComment {
    path: String,
    line: u32,
    body: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewJourney {
    summary: String,
    steps: Vec<StrictReviewJourneyStep>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictReviewJourneyStep {
    file: String,
    reason: String,
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
    validate_review_output_file_with_changes(
        repo_dir,
        output_path,
        Some(&DiffChangeset::parse(diff)),
    )
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
        return Err(AgentError::new(format!(
            "review output file missing: {display}"
        )));
    }

    let output_path = output_path
        .canonicalize()
        .map_err(|error| AgentError::new(format!("canonicalize review output file: {error}")))?;
    if !output_path.starts_with(&repo_dir) {
        return Err(AgentError::new("review output file escapes repository"));
    }

    let input = fs::read_to_string(&output_path)
        .map_err(|error| AgentError::new(format!("read review output file: {error}")))?;
    let output: StrictReviewOutput = serde_json::from_str(&input)
        .map_err(|error| AgentError::new(format!("invalid review output JSON: {error}")))?;
    validate_review_output(repo_dir.as_path(), output, changeset)
}

fn canonical_repo_dir(repo_dir: &Path) -> AgentResult<std::path::PathBuf> {
    repo_dir
        .canonicalize()
        .map_err(|error| AgentError::new(format!("canonicalize repository directory: {error}")))
}

fn validate_review_output(
    repo_dir: &Path,
    output: StrictReviewOutput,
    changeset: Option<&DiffChangeset>,
) -> AgentResult<ReviewOutput> {
    if output.summary.trim().is_empty() {
        return Err(AgentError::new("review summary is empty"));
    }

    let mut comments = Vec::with_capacity(output.comments.len());
    for comment in output.comments {
        validate_relative_repo_path(&comment.path)?;
        if comment.body.trim().is_empty() {
            return Err(AgentError::new(format!(
                "review comment body is empty: {}",
                comment.path
            )));
        }
        let comment_file = repo_dir.join(&comment.path);
        if !comment_file.exists() {
            return Err(AgentError::new(format!(
                "review comment path does not exist in repository: {}",
                comment.path
            )));
        }
        if let Some(changeset) = changeset {
            changeset.validate_comment_location(&comment.path, comment.line)?;
        }
        comments.push(crate::ReviewComment {
            path: comment.path,
            line: comment.line,
            body: comment.body,
        });
    }

    Ok(ReviewOutput {
        summary: output.summary,
        comments,
        journey: crate::ReviewJourney {
            summary: output.journey.summary,
            steps: output
                .journey
                .steps
                .into_iter()
                .map(|step| crate::ReviewJourneyStep {
                    file: step.file,
                    reason: step.reason,
                })
                .collect(),
        },
    })
}

fn validate_relative_repo_path(path: &str) -> AgentResult<()> {
    let path_value = Path::new(path);
    if path.trim().is_empty() || path_value.is_absolute() {
        return Err(AgentError::new(format!(
            "review comment path escapes repository: {path}"
        )));
    }

    for component in path_value.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(AgentError::new(format!(
                "review comment path escapes repository: {path}"
            )));
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
struct DiffChangeset {
    changed_lines: HashMap<String, HashSet<u32>>,
}

impl DiffChangeset {
    fn parse(diff: &str) -> Self {
        let mut changeset = Self::default();
        let mut current_path = None::<String>;
        let mut new_line = None::<u32>;

        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ ") {
                current_path = normalized_diff_path(path);
                continue;
            }

            if let Some(header) = line.strip_prefix("@@ ") {
                new_line = parse_hunk_new_start(header);
                continue;
            }

            let Some(path) = current_path.as_ref() else {
                continue;
            };
            let Some(line_number) = new_line.as_mut() else {
                continue;
            };

            if line.starts_with('+') && !line.starts_with("+++") {
                changeset
                    .changed_lines
                    .entry(path.clone())
                    .or_default()
                    .insert(*line_number);
                *line_number += 1;
            } else if (line.starts_with('-') && !line.starts_with("---")) || line.starts_with('\\')
            {
                continue;
            } else {
                *line_number += 1;
            }
        }

        changeset
    }

    fn validate_comment_location(&self, path: &str, line: u32) -> AgentResult<()> {
        let Some(changed_lines) = self.changed_lines.get(path) else {
            return Err(AgentError::new(format!(
                "review comment path is outside the diff changeset: {path}"
            )));
        };

        if line != 0 && !changed_lines.contains(&line) {
            return Err(AgentError::new(format!(
                "review comment line is outside the diff changeset: {path}:{line}"
            )));
        }

        Ok(())
    }
}

fn normalized_diff_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.strip_prefix("b/")
            .or_else(|| path.strip_prefix("a/"))
            .unwrap_or(path)
            .to_owned(),
    )
}

fn parse_hunk_new_start(header: &str) -> Option<u32> {
    let plus = header
        .split_whitespace()
        .find(|part| part.starts_with('+'))?;
    let number = plus
        .trim_start_matches('+')
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or_else(|| plus.trim_start_matches('+'));
    number.parse().ok()
}
