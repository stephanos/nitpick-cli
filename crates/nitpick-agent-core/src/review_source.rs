use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::{AgentError, AgentResult, ReviewInput, ReviewRequest};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessedReview {
    pub request: ReviewRequest,
    pub activity_id: Option<String>,
    pub reviewed_at_unix: u64,
}

impl ProcessedReview {
    pub fn from_request_at(
        request: &ReviewRequest,
        activity_id: Option<String>,
        reviewed_at_unix: u64,
    ) -> Self {
        Self {
            request: request.clone(),
            activity_id,
            reviewed_at_unix,
        }
    }
}

#[derive(Default)]
pub struct MemoryProcessedReviewStore {
    reviews: Mutex<BTreeMap<String, ProcessedReview>>,
}

impl ProcessedReviewStore for MemoryProcessedReviewStore {
    fn get_processed(&self, request: &ReviewRequest) -> AgentResult<Option<ProcessedReview>> {
        let reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        Ok(reviews.get(&processed_key(request)).cloned())
    }

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()> {
        let mut reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        reviews.insert(processed_review_key(review), review.clone());
        Ok(())
    }

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>> {
        let reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentError::new("processed review store lock poisoned"))?;
        Ok(reviews.values().cloned().collect())
    }
}

pub struct FsProcessedReviewStore {
    base: PathBuf,
}

impl FsProcessedReviewStore {
    pub fn new(base: impl AsRef<Path>) -> AgentResult<Self> {
        let base = base.as_ref().to_path_buf();
        fs::create_dir_all(&base)
            .map_err(|error| AgentError::new(format!("create processed review dir: {error}")))?;
        Ok(Self { base })
    }
}

impl ProcessedReviewStore for FsProcessedReviewStore {
    fn get_processed(&self, request: &ReviewRequest) -> AgentResult<Option<ProcessedReview>> {
        let path = self.base.join(format!("{}.json", processed_key(request)));
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_processed_review(&path)?))
    }

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()> {
        write_processed_review(
            &self
                .base
                .join(format!("{}.json", processed_review_key(review))),
            review,
        )
    }

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>> {
        let mut paths = fs::read_dir(&self.base)
            .map_err(|error| AgentError::new(format!("read processed review dir: {error}")))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AgentError::new(format!("read processed review dir entry: {error}"))
            })?;
        paths.sort();

        let mut reviews = Vec::new();
        for path in paths {
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                reviews.push(read_processed_review(&path)?);
            }
        }
        Ok(reviews)
    }
}

fn processed_key(request: &ReviewRequest) -> String {
    sanitize_key(&format!(
        "{}__{}__{}",
        request.source, request.repository, request.id
    ))
}

fn processed_review_key(review: &ProcessedReview) -> String {
    processed_key(&review.request)
}

fn sanitize_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn write_processed_review(path: &Path, review: &ProcessedReview) -> AgentResult<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(review)
        .map_err(|error| AgentError::new(format!("serialize processed review: {error}")))?;
    fs::write(&tmp, bytes)
        .map_err(|error| AgentError::new(format!("write processed review temp file: {error}")))?;
    fs::rename(&tmp, path)
        .map_err(|error| AgentError::new(format!("replace processed review: {error}")))
}

fn read_processed_review(path: &Path) -> AgentResult<ProcessedReview> {
    let bytes = fs::read(path)
        .map_err(|error| AgentError::new(format!("read processed review: {error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AgentError::new(format!("parse {}: {error}", path.display())))
}

pub trait ReviewSource: Send + Sync {
    fn name(&self) -> &'static str;

    fn requested_reviews(&self) -> AgentResult<Vec<ReviewRequest>>;

    fn review_input(&self, request: &ReviewRequest) -> AgentResult<ReviewInput>;
}

pub trait ProcessedReviewStore: Send + Sync {
    fn get_processed(&self, request: &ReviewRequest) -> AgentResult<Option<ProcessedReview>>;

    fn save_processed(&self, review: &ProcessedReview) -> AgentResult<()>;

    fn list_processed(&self) -> AgentResult<Vec<ProcessedReview>>;

    fn needs_review(&self, request: &ReviewRequest) -> AgentResult<bool> {
        Ok(self
            .get_processed(request)?
            .is_none_or(|processed| processed.request.head_sha != request.head_sha))
    }

    fn mark_processed_at(
        &self,
        request: &ReviewRequest,
        activity_id: Option<String>,
        reviewed_at_unix: u64,
    ) -> AgentResult<()> {
        self.save_processed(&ProcessedReview::from_request_at(
            request,
            activity_id,
            reviewed_at_unix,
        ))
    }
}
