use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use nitpick_agent_core::{
    Activity, ActivityKind, ActivityStatus, ActivityStore, AgentResult, AgentRuntime, ReviewInput,
};

use crate::review_slots::ReviewSlotManager;

#[derive(Clone)]
pub(crate) struct ReviewExecutionQueue {
    store: Arc<dyn ActivityStore>,
    slots: ReviewSlotManager,
    running: Arc<Mutex<BTreeSet<nitpick_agent_core::ActivityId>>>,
}

impl ReviewExecutionQueue {
    pub(crate) fn new(store: Arc<dyn ActivityStore>, max_concurrent: usize) -> Self {
        Self {
            store,
            slots: ReviewSlotManager::new(max_concurrent),
            running: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub(crate) fn enqueue(
        &self,
        input: ReviewInput,
        runtime: AgentRuntime,
        run_review: impl FnOnce(Activity, ReviewInput) -> AgentResult<Activity> + Send + 'static,
        after_slot_release: impl FnOnce(&AgentResult<Activity>, &ReviewInput) + Send + 'static,
    ) -> AgentResult<Activity> {
        if input.force {
            self.cancel_active_reviews_for_same_pr(&input)?;
        }
        if let Some(activity) = self.active_review_for_input(&input)? {
            if activity.status != ActivityStatus::Running
                || self.activity_is_running_in_this_host(&activity)?
            {
                return Ok(activity);
            }
            self.mark_stale_running_activity(activity)?;
        }
        let same_pr_active = self.has_active_review_for_same_pr(&input)?;
        let mut activity = runtime.create_queued_review_activity(&input)?;
        let slot_acquired = !same_pr_active && self.slots.try_acquire()?;
        if slot_acquired {
            activity = runtime.mark_activity_running(activity)?;
            self.register_running(&activity)?;
        }
        let queued = activity.clone();
        let queue = self.clone();
        thread::spawn(move || {
            let _ = queue.run(
                activity,
                input,
                slot_acquired,
                run_review,
                after_slot_release,
            );
        });
        Ok(queued)
    }

    fn run(
        &self,
        activity: Activity,
        input: ReviewInput,
        slot_acquired: bool,
        run_review: impl FnOnce(Activity, ReviewInput) -> AgentResult<Activity>,
        after_slot_release: impl FnOnce(&AgentResult<Activity>, &ReviewInput),
    ) -> AgentResult<Activity> {
        let activity_id = activity.id.clone();
        let post_review_input = input.clone();
        if !slot_acquired {
            self.wait_for_prior_reviews_on_same_pr(&activity)?;
            self.slots.wait_and_acquire()?;
            self.register_running(&activity)?;
        }
        let result = run_review(activity, input);
        self.unregister_running(&activity_id)?;
        self.slots.release()?;
        after_slot_release(&result, &post_review_input);
        result
    }

    fn active_review_for_input(&self, input: &ReviewInput) -> AgentResult<Option<Activity>> {
        if input.head_sha.is_empty() {
            return Ok(None);
        }
        let Some(number) = input.subject.number else {
            return Ok(None);
        };
        let label = format!("review on {}#{number}", input.subject.repository);
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|activity| activity.kind == ActivityKind::Review)
            .filter(|activity| active_review_status(&activity.status))
            .filter(|activity| activity.label.as_deref() == Some(label.as_str()))
            .find(|activity| {
                review_activity_head_sha(activity).as_deref() == Some(&input.head_sha)
            }))
    }

    fn has_active_review_for_same_pr(&self, input: &ReviewInput) -> AgentResult<bool> {
        let Some(label) = review_label(input) else {
            return Ok(false);
        };
        Ok(self.store.list()?.into_iter().any(|activity| {
            activity.kind == ActivityKind::Review
                && active_review_status(&activity.status)
                && activity.label.as_deref() == Some(label.as_str())
        }))
    }

    fn wait_for_prior_reviews_on_same_pr(&self, activity: &Activity) -> AgentResult<()> {
        let Some(label) = activity.label.as_deref() else {
            return Ok(());
        };
        loop {
            let has_prior = self.store.list()?.into_iter().any(|candidate| {
                candidate.kind == ActivityKind::Review
                    && active_review_status(&candidate.status)
                    && candidate.id != activity.id
                    && candidate.label.as_deref() == Some(label)
                    && activity_started_before(&candidate, activity)
            });
            if !has_prior {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn cancel_active_reviews_for_same_pr(&self, input: &ReviewInput) -> AgentResult<()> {
        let Some(label) = review_label(input) else {
            return Ok(());
        };
        for mut activity in self.store.list()?.into_iter().filter(|activity| {
            activity.kind == ActivityKind::Review
                && active_review_status(&activity.status)
                && activity.label.as_deref() == Some(label.as_str())
        }) {
            activity.status = ActivityStatus::Error;
            activity.session.status =
                nitpick_agent_core::SessionStatus::Error("superseded by forced review".into());
            activity.error = Some("superseded by forced review".into());
            activity.touch();
            self.store.save(&activity)?;
        }
        Ok(())
    }

    fn register_running(&self, activity: &Activity) -> AgentResult<()> {
        self.running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review queue lock", "poisoned"))?
            .insert(activity.id.clone());
        Ok(())
    }

    fn unregister_running(&self, activity_id: &nitpick_agent_core::ActivityId) -> AgentResult<()> {
        self.running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review queue lock", "poisoned"))?
            .remove(activity_id);
        Ok(())
    }

    fn activity_is_running_in_this_host(&self, activity: &Activity) -> AgentResult<bool> {
        Ok(self
            .running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review queue lock", "poisoned"))?
            .contains(&activity.id))
    }

    fn mark_stale_running_activity(&self, mut activity: Activity) -> AgentResult<()> {
        if activity.status != ActivityStatus::Running {
            return Ok(());
        }
        activity.status = ActivityStatus::Error;
        activity.session.status =
            nitpick_agent_core::SessionStatus::Error("stale running review recovered".into());
        activity.error = Some("stale running review recovered".into());
        activity.touch();
        self.store.save(&activity)
    }
}

fn review_label(input: &ReviewInput) -> Option<String> {
    input
        .subject
        .number
        .map(|number| format!("review on {}#{number}", input.subject.repository))
}

fn active_review_status(status: &ActivityStatus) -> bool {
    matches!(status, ActivityStatus::Queued | ActivityStatus::Running)
}

fn activity_started_before(candidate: &Activity, activity: &Activity) -> bool {
    candidate
        .created_at_unix
        .cmp(&activity.created_at_unix)
        .then_with(|| candidate.id.cmp(&activity.id))
        .is_lt()
}

fn review_activity_head_sha(activity: &Activity) -> Option<String> {
    activity
        .session
        .messages
        .iter()
        .find(|message| message.role == "nitpick.review.head_sha")
        .map(|message| message.content.clone())
}
