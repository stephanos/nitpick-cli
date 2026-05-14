use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("{message}")]
    Message { message: String },
    #[error("{context}: {error}")]
    Io { context: String, error: String },
    #[error("{context} at {path}: {error}")]
    Json {
        context: String,
        path: String,
        error: String,
    },
    #[error("{resource} not found: {id}")]
    NotFound { resource: String, id: String },
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("{message}")]
    Provider { message: String },
    #[error("{message}")]
    Sandbox { message: String },
    #[error("{message}")]
    Config { message: String },
    #[error("{message}")]
    GitHubCli { message: String },
}

impl AgentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
        }
    }

    pub fn io(context: impl Into<String>, source: impl std::fmt::Display) -> Self {
        Self::Io {
            context: context.into(),
            error: source.to_string(),
        }
    }

    pub fn io_path(
        context: impl AsRef<str>,
        path: impl Into<PathBuf>,
        source: impl std::fmt::Display,
    ) -> Self {
        Self::Io {
            context: format!("{} {}", context.as_ref(), path.into().display()),
            error: source.to_string(),
        }
    }

    pub fn json(
        context: impl Into<String>,
        path: impl std::fmt::Display,
        source: impl std::fmt::Display,
    ) -> Self {
        Self::Json {
            context: context.into(),
            path: path.to_string(),
            error: source.to_string(),
        }
    }

    pub fn not_found(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
            id: id.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider {
            message: message.into(),
        }
    }

    pub fn sandbox(message: impl Into<String>) -> Self {
        Self::Sandbox {
            message: message.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    pub fn github_cli(message: impl Into<String>) -> Self {
        Self::GitHubCli {
            message: message.into(),
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

pub type AgentResult<T> = Result<T, AgentError>;
