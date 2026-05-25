use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use unidiff::PatchSet;

use crate::{AgentError, AgentResult, RepoPath, ReviewComment};

#[derive(Debug)]
pub struct ReviewCommentValidator {
    repo_dir: PathBuf,
    changeset: Option<DiffChangeset>,
}

impl ReviewCommentValidator {
    pub fn new(repo_dir: impl AsRef<Path>) -> AgentResult<Self> {
        Ok(Self {
            repo_dir: canonical_repo_dir(repo_dir.as_ref())?,
            changeset: None,
        })
    }

    pub fn for_diff(repo_dir: impl AsRef<Path>, diff: &str) -> AgentResult<Self> {
        Ok(Self {
            repo_dir: canonical_repo_dir(repo_dir.as_ref())?,
            changeset: Some(DiffChangeset::parse(diff)?),
        })
    }

    pub fn validate_comment(
        &self,
        path: &str,
        line: u32,
        body: impl Into<String>,
    ) -> AgentResult<ReviewComment> {
        let comment_path = RepoPath::parse(path)?;
        let body = body.into();
        if body.trim().is_empty() {
            return Err(AgentError::invalid_input(format!(
                "review comment body is empty: {path}"
            )));
        }
        let comment_file = self.repo_dir.join(comment_path.as_str());
        if !comment_file.exists() {
            return Err(AgentError::invalid_input(format!(
                "review comment path does not exist in repository: {path}"
            )));
        }
        if !comment_file.is_file() {
            return Err(AgentError::invalid_input(format!(
                "review comment path is not a file: {path}"
            )));
        }
        if let Some(changeset) = &self.changeset {
            changeset.validate_comment_location(comment_path.as_str(), line)?;
        }

        Ok(ReviewComment {
            path: comment_path.as_str().to_owned(),
            line,
            body,
        })
    }
}

fn canonical_repo_dir(repo_dir: &Path) -> AgentResult<PathBuf> {
    repo_dir
        .canonicalize()
        .map_err(|error| AgentError::io("canonicalize repository directory", error))
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

pub fn first_changed_file_for_diff(diff: &str) -> AgentResult<Option<String>> {
    let mut patch = PatchSet::new();
    patch
        .parse(diff)
        .map_err(|error| AgentError::invalid_input(format!("invalid review diff: {error}")))?;

    Ok(patch.into_iter().find_map(|file| {
        normalized_target_path(&file.target_file)
            .or_else(|| normalized_source_path(&file.source_file))
    }))
}

fn normalized_target_path(path: &str) -> Option<String> {
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix("b/").unwrap_or(path).to_owned())
}

fn normalized_source_path(path: &str) -> Option<String> {
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix("a/").unwrap_or(path).to_owned())
}
