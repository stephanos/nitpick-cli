use nitpick_agent_host::HostStatusProjection;
use nitpick_agent_model::{
    Activity, ActivityId, ActivityKind, ActivityStatus, AgentProviderKind, Artifact,
    ArtifactContent, ArtifactId, ArtifactKind,
};

#[test]
fn host_status_projection_counts_activity_and_artifact_states() {
    let mut review = Activity::new(ActivityId::new("activity-1"), ActivityKind::Review);
    review.status = ActivityStatus::Queued;
    let artifact = Artifact::local(
        ArtifactId::new("artifact-1"),
        ActivityId::new("activity-1"),
        ArtifactKind::ReviewSummary,
        ArtifactContent::ReviewSummary("summary".into()),
    );

    let status = HostStatusProjection {
        activities: &[review],
        artifacts: &[artifact],
        open_review_count: 3,
        provider: AgentProviderKind::Claude,
        model: None,
        review_source_name: "github".into(),
        review_source_enabled: true,
        review_source_last_poll_unix: Some(10),
        review_source_last_poll_summary: Some("reviewed 0 of 3 PRs".into()),
    }
    .project();

    assert_eq!(status.activity_count, 1);
    assert_eq!(status.queued_review_count, 1);
    assert_eq!(status.local_only_artifact_count, 1);
    assert_eq!(status.open_review_count, 3);
}
