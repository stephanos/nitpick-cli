use crate::{
    activity::{Activity, ActivityKind, ActivityStatus},
    model::{ReviewInput, ReviewRequest},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewIdentity {
    source: String,
    repository: String,
    number: Option<u64>,
    id: String,
    head_sha: String,
}

impl ReviewIdentity {
    pub(crate) fn from_request(request: &ReviewRequest) -> Self {
        Self {
            source: request.source.clone(),
            repository: request.repository.clone(),
            number: request.number,
            id: request.id.clone(),
            head_sha: request.head_sha.clone(),
        }
    }

    pub(crate) fn from_input(input: &ReviewInput) -> Self {
        Self {
            source: match input.subject.number {
                Some(_) => "github".into(),
                None => "review".into(),
            },
            repository: input.subject.repository.clone(),
            number: input.subject.number,
            id: String::new(),
            head_sha: input.head_sha.clone(),
        }
    }

    pub(crate) fn display_reference(&self) -> String {
        match self.number {
            Some(number) => format!("{}#{}", self.repository, number),
            None if self.id.is_empty() => self.repository.clone(),
            None => format!("{}#{}", self.repository, self.id),
        }
    }

    pub(crate) fn activity_label(&self) -> String {
        format!("review on {}", self.display_reference())
    }

    #[cfg(test)]
    pub(crate) fn version_key(&self) -> String {
        let mut key = format!("{}:{}", self.source, self.display_reference());
        if !self.head_sha.is_empty() {
            key.push('@');
            key.push_str(&self.head_sha);
        }
        key
    }
}

pub struct ReviewActivityIdentity<'a> {
    activity: &'a Activity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewActivityTarget {
    repository: String,
    number: Option<u64>,
}

impl<'a> ReviewActivityIdentity<'a> {
    pub fn new(activity: &'a Activity) -> Self {
        Self { activity }
    }

    pub fn is_active_review(&self) -> bool {
        self.activity.kind == ActivityKind::Review
            && matches!(
                self.activity.status,
                ActivityStatus::Queued | ActivityStatus::Running
            )
    }

    pub fn matches_input(&self, input: &ReviewInput) -> bool {
        !input.head_sha.is_empty()
            && self.matches_target(&input.subject.repository, input.subject.number)
            && self.head_sha() == Some(input.head_sha.as_str())
    }

    pub fn matches_target(&self, repository: &str, number: Option<u64>) -> bool {
        self.target()
            .is_some_and(|target| target.repository == repository && target.number == number)
    }

    pub fn matches_activity_target(&self, other: &ReviewActivityIdentity<'_>) -> bool {
        self.target()
            .zip(other.target())
            .is_some_and(|(lhs, rhs)| lhs == rhs)
    }

    pub fn head_sha(&self) -> Option<&str> {
        self.activity
            .session
            .messages
            .iter()
            .find(|message| message.role == "nitpick.review.head_sha")
            .map(|message| message.content.as_str())
    }

    fn target(&self) -> Option<ReviewActivityTarget> {
        self.activity
            .retry
            .as_ref()
            .and_then(|retry| retry.review.as_ref())
            .map(|review| ReviewActivityTarget {
                repository: review.repository.clone(),
                number: review.number,
            })
            .or_else(|| {
                self.activity
                    .label
                    .as_deref()
                    .and_then(review_activity_target_from_label)
            })
    }
}

fn review_activity_target_from_label(label: &str) -> Option<ReviewActivityTarget> {
    let reference = label.strip_prefix("review on ")?;
    let (repository, number) = match reference.rsplit_once('#') {
        Some((repository, number)) => {
            let number = number.parse::<u64>().ok()?;
            (repository.to_owned(), Some(number))
        }
        None => (reference.to_owned(), None),
    };
    Some(ReviewActivityTarget { repository, number })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_version_key_includes_head_sha_without_requiring_pr_number() {
        let identity = ReviewIdentity::from_request(&ReviewRequest {
            source: "github".into(),
            repository: "acme/platform".into(),
            id: "PR_kwDOExample".into(),
            head_sha: "def456".into(),
            ..ReviewRequest::default()
        });

        assert_eq!(
            identity.version_key(),
            "github:acme/platform#PR_kwDOExample@def456"
        );
    }
}
