use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{
    AgentError, AgentProvider, AgentProviderKind, AgentResult, AgentSession, ChatInput,
    ReviewInput, ReviewJourney, ReviewOutput,
};

pub struct CommandAgentProvider {
    kind: AgentProviderKind,
    model: Option<String>,
    command: PathBuf,
}

impl CommandAgentProvider {
    pub fn new(
        kind: AgentProviderKind,
        model: Option<String>,
        command: impl Into<PathBuf>,
    ) -> Self {
        Self {
            kind,
            model,
            command: command.into(),
        }
    }

    pub fn for_kind(kind: AgentProviderKind, model: Option<String>) -> Self {
        let command = kind.as_str();
        Self::new(kind, model, command)
    }

    pub fn kind(&self) -> &AgentProviderKind {
        &self.kind
    }

    pub fn command(&self) -> &std::path::Path {
        &self.command
    }

    fn run_prompt(&self, prompt: &str, args: &[String]) -> AgentResult<String> {
        let mut child = Command::new(&self.command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?;

        child
            .stdin
            .as_mut()
            .ok_or_else(|| AgentError::new("provider command stdin unavailable"))?
            .write_all(prompt.as_bytes())
            .map_err(|error| AgentError::new(format!("write provider prompt: {error}")))?;

        let output = child
            .wait_with_output()
            .map_err(|error| AgentError::new(format!("wait for provider command: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(AgentError::new(format!(
                "{} provider command failed with status {}{}",
                self.kind,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn run_interactive(&self, args: &[String]) -> AgentResult<()> {
        let status = Command::new(&self.command)
            .args(args)
            .status()
            .map_err(|error| {
                AgentError::new(format!(
                    "failed to start {} provider command `{}`: {error}",
                    self.kind,
                    self.command.display()
                ))
            })?;
        if !status.success() {
            return Err(AgentError::new(format!(
                "{} provider command failed with status {}",
                self.kind, status
            )));
        }
        Ok(())
    }
}

impl AgentProvider for CommandAgentProvider {
    fn review(&self, session: &mut AgentSession, input: &ReviewInput) -> AgentResult<ReviewOutput> {
        session.provider = Some(self.kind.clone());
        let prompt = review_prompt(self.model.as_deref(), input);
        let args = self.review_args(session);
        let summary = self.run_prompt(&prompt, &args)?;
        Ok(ReviewOutput {
            summary,
            comments: Vec::new(),
            journey: ReviewJourney {
                summary: "external provider command completed".into(),
                steps: Vec::new(),
            },
        })
    }

    fn chat(&self, session: &mut AgentSession, input: &ChatInput) -> AgentResult<String> {
        session.provider = Some(self.kind.clone());
        self.run_prompt(&chat_prompt(self.model.as_deref(), input), &[])
    }

    fn attach_session(&self, session: &AgentSession) -> AgentResult<()> {
        let session_id = session
            .provider_session_id
            .as_deref()
            .ok_or_else(|| AgentError::new("activity has no provider session id"))?;
        match self.kind {
            AgentProviderKind::Claude => {
                self.run_interactive(&["--resume".into(), session_id.into()])
            }
            AgentProviderKind::Codex => Err(AgentError::new(
                "codex provider does not support session resume yet",
            )),
        }
    }
}

impl CommandAgentProvider {
    fn review_args(&self, session: &AgentSession) -> Vec<String> {
        match (&self.kind, session.provider_session_id.as_deref()) {
            (AgentProviderKind::Claude, Some(session_id)) => {
                vec!["--session-id".into(), session_id.into()]
            }
            _ => Vec::new(),
        }
    }
}

fn review_prompt(model: Option<&str>, input: &ReviewInput) -> String {
    format!(
        "You are reviewing code. Return a concise review summary.\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ninstructions:\n{}\n\ndiff:\n{}\n",
        model.unwrap_or("(default)"),
        input.subject.repository,
        input
            .subject
            .number
            .map(|number| number.to_string())
            .unwrap_or_else(|| "(none)".into()),
        input.subject.title,
        input.subject.author,
        input.repo_dir.display(),
        input.instructions,
        input.diff,
    )
}

fn chat_prompt(model: Option<&str>, input: &ChatInput) -> String {
    format!(
        "You are answering a development question.\n\nmodel: {}\nrepo_dir: {}\ncontext:\n{}\n\nprompt:\n{}\n",
        model.unwrap_or("(default)"),
        input.repo_dir.display(),
        input.context,
        input.prompt,
    )
}
