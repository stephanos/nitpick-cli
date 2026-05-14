use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::{AgentError, AgentResult};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoPath(Utf8PathBuf);

impl RepoPath {
    pub fn parse(value: &str) -> AgentResult<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AgentError::new(format!(
                "review comment path escapes repository: {value}"
            )));
        }

        let path = Utf8Path::new(trimmed);
        if path.is_absolute() {
            return Err(AgentError::new(format!(
                "review comment path escapes repository: {value}"
            )));
        }

        for component in path.components() {
            if matches!(
                component,
                Utf8Component::ParentDir | Utf8Component::RootDir | Utf8Component::Prefix(_)
            ) {
                return Err(AgentError::new(format!(
                    "review comment path escapes repository: {value}"
                )));
            }
        }

        Ok(Self(path.to_path_buf()))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
