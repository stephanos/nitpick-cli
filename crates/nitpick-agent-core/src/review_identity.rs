use crate::{
    activity::{Activity, ActivityKind, ActivityStatus},
    model::ReviewInput,
};

#[cfg(test)]
use crate::model::ReviewRequest;

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewIdentity {
    source: String,
    repository: String,
    number: Option<u64>,
    id: String,
    head_sha: String,
}

#[cfg(test)]
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

    pub(crate) fn display_reference(&self) -> String {
        match self.number {
            Some(number) => format!("{}#{}", self.repository, number),
            None if self.id.is_empty() => self.repository.clone(),
            None => format!("{}#{}", self.repository, self.id),
        }
    }

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
struct ReviewTarget {
    repository: String,
    number: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewActivityTarget {
    pub repository: String,
    pub number: u64,
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

    pub fn pull_request_target(&self) -> Option<ReviewActivityTarget> {
        if self.activity.kind != ActivityKind::Review {
            return None;
        }
        let target = self.target()?;
        Some(ReviewActivityTarget {
            repository: target.repository,
            number: target.number?,
        })
    }

    fn target(&self) -> Option<ReviewTarget> {
        self.activity
            .retry
            .as_ref()
            .and_then(|retry| retry.review.as_ref())
            .map(|review| ReviewTarget {
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

fn review_activity_target_from_label(label: &str) -> Option<ReviewTarget> {
    let reference = label.strip_prefix("review on ")?;
    let (repository, number) = match reference.rsplit_once('#') {
        Some((repository, number)) => {
            let number = number.parse::<u64>().ok()?;
            (repository.to_owned(), Some(number))
        }
        None => (reference.to_owned(), None),
    };
    Some(ReviewTarget { repository, number })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_identity_exposes_canonical_pull_request_target() {
        let mut activity =
            Activity::new(crate::ActivityId::new("activity-1"), ActivityKind::Review);
        activity.label = Some("review on acme/platform#42".into());

        assert_eq!(
            ReviewActivityIdentity::new(&activity).pull_request_target(),
            Some(ReviewActivityTarget {
                repository: "acme/platform".into(),
                number: 42,
            })
        );
    }

    #[test]
    fn activity_identity_rejects_non_review_and_non_pull_request_targets() {
        let mut chat = Activity::new(crate::ActivityId::new("activity-1"), ActivityKind::Chat);
        chat.label = Some("review on acme/platform#42".into());
        let mut local_review =
            Activity::new(crate::ActivityId::new("activity-2"), ActivityKind::Review);
        local_review.label = Some("review on local-checkout".into());

        assert_eq!(
            ReviewActivityIdentity::new(&chat).pull_request_target(),
            None
        );
        assert_eq!(
            ReviewActivityIdentity::new(&local_review).pull_request_target(),
            None
        );
    }

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
