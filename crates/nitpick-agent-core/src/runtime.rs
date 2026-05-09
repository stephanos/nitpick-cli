use std::sync::Arc;

use crate::{
    Activity, ActivityKind, ActivityOutput, ActivityStatus, ActivityStore, AgentProvider,
    AgentResult, ArtifactContent, ArtifactKind, ChatInput, ReviewInput, ReviewOutput,
    SessionStatus,
};

#[derive(Clone)]
pub struct AgentRuntime {
    provider: Arc<dyn AgentProvider>,
    store: Arc<dyn ActivityStore>,
}

impl AgentRuntime {
    pub fn new(provider: Arc<dyn AgentProvider>, store: Arc<dyn ActivityStore>) -> Self {
        Self { provider, store }
    }

    pub fn start_review(&self, input: ReviewInput) -> AgentResult<Activity> {
        let activity = self.create_review_activity()?;

        self.run_review(activity, input)
    }

    pub fn create_review_activity(&self) -> AgentResult<Activity> {
        self.create_running_activity(ActivityKind::Review)
    }

    pub fn run_review(&self, mut activity: Activity, input: ReviewInput) -> AgentResult<Activity> {
        activity.label_review(&input);
        if activity.session.provider_session_id.is_none() {
            activity.session.provider_session_id = Some(review_session_id(&input));
        }
        activity.touch();
        self.store.save(&activity)?;
        match self.provider.review(&mut activity.session, &input) {
            Ok(output) => {
                let artifacts = review_artifacts(self.store.as_ref(), &activity, &output)?;
                activity.status = ActivityStatus::Completed;
                activity.session.status = SessionStatus::Completed;
                activity.output = Some(ActivityOutput::Review(output));
                self.store.save_artifacts(&artifacts)?;
            }
            Err(error) => {
                activity.status = ActivityStatus::Error;
                activity.session.status = SessionStatus::Error(error.to_string());
                activity.error = Some(error.to_string());
            }
        }

        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn start_chat(&self, input: ChatInput) -> AgentResult<Activity> {
        let activity = self.create_chat_activity()?;

        self.run_chat(activity, input)
    }

    pub fn create_chat_activity(&self) -> AgentResult<Activity> {
        self.create_running_activity(ActivityKind::Chat)
    }

    pub fn run_chat(&self, mut activity: Activity, input: ChatInput) -> AgentResult<Activity> {
        match self.provider.chat(&mut activity.session, &input) {
            Ok(output) => {
                let artifact = self.store.create_artifact(
                    activity.id.clone(),
                    ArtifactKind::ChatResponse,
                    ArtifactContent::ChatResponse(output.clone()),
                )?;
                activity.status = ActivityStatus::Completed;
                activity.session.status = SessionStatus::Completed;
                activity.output = Some(ActivityOutput::Chat(output));
                self.store.save_artifacts(&[artifact])?;
            }
            Err(error) => {
                activity.status = ActivityStatus::Error;
                activity.session.status = SessionStatus::Error(error.to_string());
                activity.error = Some(error.to_string());
            }
        }

        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }

    pub fn list_activities(&self) -> AgentResult<Vec<Activity>> {
        self.store.list()
    }

    fn create_running_activity(&self, kind: ActivityKind) -> AgentResult<Activity> {
        let mut activity = self.store.create(kind)?;
        activity.status = ActivityStatus::Running;
        activity.session.status = SessionStatus::Running;
        activity.touch();
        self.store.save(&activity)?;
        Ok(activity)
    }
}

fn review_session_id(input: &ReviewInput) -> String {
    match input.subject.number {
        Some(number) => format!("github:{}#{number}", input.subject.repository),
        None => format!("review:{}", input.subject.repository),
    }
}

fn review_artifacts(
    store: &dyn ActivityStore,
    activity: &Activity,
    output: &ReviewOutput,
) -> AgentResult<Vec<crate::Artifact>> {
    let mut artifacts = vec![store.create_artifact(
        activity.id.clone(),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary(output.summary.clone()),
    )?];

    for comment in &output.comments {
        artifacts.push(store.create_artifact(
            activity.id.clone(),
            ArtifactKind::ReviewComment,
            ArtifactContent::ReviewComment(comment.clone()),
        )?);
    }

    Ok(artifacts)
}
