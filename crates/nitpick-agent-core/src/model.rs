use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewInput {
    pub repo_dir: PathBuf,
    #[serde(default)]
    pub review_prompt: String,
    pub instructions: String,
    pub subject: ReviewSubject,
    pub diff: String,
    #[serde(default)]
    pub disable_sandbox: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSubject {
    pub repository: String,
    pub number: Option<u64>,
    pub title: String,
    pub author: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub source: String,
    pub repository: String,
    pub number: Option<u64>,
    pub id: String,
    pub head_sha: String,
}

impl ReviewRequest {
    pub fn display_reference(&self) -> String {
        match self.number {
            Some(number) => format!("{}#{}", self.repository, number),
            None if self.id.is_empty() => self.repository.clone(),
            None => format!("{}#{}", self.repository, self.id),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub summary: String,
    pub comments: Vec<ReviewComment>,
    pub journey: ReviewJourney,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u32,
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewJourney {
    pub summary: String,
    pub steps: Vec<ReviewJourneyStep>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewJourneyStep {
    pub file: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatInput {
    pub repo_dir: PathBuf,
    pub prompt: String,
    pub context: String,
    #[serde(default)]
    pub disable_sandbox: bool,
}
