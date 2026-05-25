use crate::model::{ReviewInput, ReviewRequest};

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

    pub(crate) fn session_key(&self) -> String {
        let mut key = format!("{}:{}", self.source, self.display_reference());
        if self.number.is_some() && !self.head_sha.is_empty() {
            key.push('@');
            key.push_str(&self.head_sha);
        }
        key
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
