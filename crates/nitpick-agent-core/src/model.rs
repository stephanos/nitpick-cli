use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::review_identity::ReviewIdentity;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewInput {
    pub repo_dir: PathBuf,
    #[serde(default)]
    pub review_prompt: String,
    #[serde(default)]
    pub review_mode: ReviewMode,
    pub instructions: String,
    pub subject: ReviewSubject,
    #[serde(default)]
    pub head_sha: String,
    pub diff: String,
    #[serde(default)]
    pub disable_sandbox: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewMode {
    #[default]
    Requested,
    #[serde(rename = "self")]
    SelfReview,
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
        ReviewIdentity::from_request(self).display_reference()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub comments: Vec<ReviewComment>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u32,
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatInput {
    pub repo_dir: PathBuf,
    pub prompt: String,
    pub context: String,
    #[serde(default)]
    pub disable_sandbox: bool,
}
