use std::sync::Arc;

use nitpick_agent_cli::{
    ActivityCommand, CliCommand, ReviewCommand, SystemCommand, run_cli_command,
};
use nitpick_agent_core::FsProcessedReviewStore;
use nitpick_agent_core::{ActivityStore, FsActivityStore};
use nitpick_agent_host::{HostDaemon, api_router};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, github_auto_review_config, pull_request,
};

#[tokio::test(flavor = "multi_thread")]
async fn cli_commands_talk_to_the_host_api() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_claude = temp.path().join("claude");
    let resume_log = temp.path().join("resume.log");
    std::fs::write(
        &fake_claude,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            resume_log.display()
        ),
    )
    .expect("fake claude");
    make_executable(&fake_claude);
    let fake_gh = temp.path().join("gh");
    let review_sync_log = temp.path().join("review-sync.log");
    std::fs::write(
        &fake_gh,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {log}
if [ "$1" = "pr" ]; then
  printf '{{"headRefOid":"abc123"}}\n'
  exit 0
fi
cat >/dev/null
printf '{{"id":99,"html_url":"https://github.com/stephanos/nitpick-agent/pull/42#pullrequestreview-99","state":"PENDING","commit_id":"abc123"}}\n'
"#,
            log = review_sync_log.display(),
        ),
    )
    .expect("fake gh");
    make_executable(&fake_gh);
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[agent]\nprovider = \"claude\"\ncommand = \"{}\"\nsandbox = \"none\"\n\n[github]\ncommand = \"{}\"\n",
            fake_claude.display(),
            fake_gh.display()
        ),
    )
    .expect("config");
    let data_dir = temp.path().join("data");
    let daemon_log = data_dir.join("logs/daemon.log");
    std::fs::create_dir_all(daemon_log.parent().expect("daemon log parent"))
        .expect("daemon log dir");
    std::fs::write(&daemon_log, "daemon started\n").expect("daemon log");
    let mut config = github_auto_review_config();
    config.github_command = Some(fake_gh.display().to_string());
    let daemon = HostDaemon::with_dependencies(
        Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store")),
        config,
        Arc::new(
            FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
        ),
        Arc::new(RecordingProvider::default()),
        Arc::new(StubDiscovery::new(vec![pull_request("sha-one")])),
        Arc::new(ManualClock::new(1_000)),
    );
    let host_addr = serve_host(daemon.clone()).await;
    let repo_dir = temp.path().to_path_buf();

    let status = run_cli_command(
        CliCommand::System(SystemCommand::Status),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("status command");
    assert!(status.contains("nitpick-agent-host: connected"));

    let requests = run_cli_command(
        CliCommand::Review(ReviewCommand::Requests { only_new: true }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review requests command");
    assert_eq!(requests, "github stephanos/nitpick-agent#42");

    daemon.poll_review_requests().expect("poll");

    let activities = run_cli_command(
        CliCommand::Activity(ActivityCommand::List),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("activities command");
    assert!(activities.contains("activity-1: Completed"));

    let reviews = run_cli_command(
        CliCommand::Review(ReviewCommand::List { include_all: true }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("reviews command");
    assert!(reviews.contains("Completed review on stephanos/nitpick-agent#42 activity-"));

    let logs = run_cli_command(
        CliCommand::Activity(ActivityCommand::Logs {
            target: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("logs command");
    assert!(logs.contains("activity: activity-2"));
    assert!(logs.contains("session: github:stephanos/nitpick-agent#42"));
    assert!(logs.contains("review complete"));

    let review_sync = run_cli_command(
        CliCommand::Review(ReviewCommand::Sync {
            activity_id: "activity-2".into(),
            target: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review sync command");
    assert!(review_sync.contains(
        "artifact-1: ReviewSummary Pending { destination: \"github-review\", remote_id: Some(\"99\"), remote_url: Some(\"https://github.com/stephanos/nitpick-agent/pull/42#pullrequestreview-99\") }"
    ));
    assert_eq!(
        std::fs::read_to_string(review_sync_log).expect("review sync args"),
        "pr view 42 --repo stephanos/nitpick-agent --json headRefOid\napi repos/stephanos/nitpick-agent/pulls/42/reviews --method POST --input -\n"
    );

    let daemon_logs = run_cli_command(
        CliCommand::Activity(ActivityCommand::Logs {
            target: "daemon".into(),
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("daemon logs command");
    assert_eq!(daemon_logs, "daemon started\n");

    let resume = run_cli_command(
        CliCommand::Activity(ActivityCommand::Resume {
            target: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("resume command");
    assert_eq!(resume, "");
    assert_eq!(
        std::fs::read_to_string(resume_log).expect("resume args"),
        "--resume github:stephanos/nitpick-agent#42\n"
    );

    let cleanup = run_cli_command(
        CliCommand::System(SystemCommand::CleanupCheckouts),
        &host_addr,
        repo_dir,
        String::new(),
        String::new(),
        config_path,
        data_dir,
    )
    .expect("cleanup command");
    assert_eq!(cleanup, "no checkouts cleaned up");
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_clears_missing_provider_session_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_claude = temp.path().join("claude");
    std::fs::write(
        &fake_claude,
        "#!/bin/sh\nprintf 'session not found\\n' >&2\nexit 1\n",
    )
    .expect("fake claude");
    make_executable(&fake_claude);
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[agent]\nprovider = \"claude\"\ncommand = \"{}\"\nsandbox = \"none\"\n",
            fake_claude.display(),
        ),
    )
    .expect("config");
    let data_dir = temp.path().join("data");
    let store = Arc::new(FsActivityStore::new(&data_dir).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let mut activity = store
        .create(nitpick_agent_core::ActivityKind::Review)
        .expect("activity");
    activity.label = Some("review on stephanos/nitpick-agent#42".into());
    activity.status = nitpick_agent_core::ActivityStatus::Completed;
    activity.session.provider = Some(nitpick_agent_core::AgentProviderKind::Claude);
    activity.session.provider_session_id = Some("github:stephanos/nitpick-agent#42".into());
    activity.session.status = nitpick_agent_core::SessionStatus::Completed;
    store.save(&activity).expect("save activity");
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_auto_review_config(),
        processed,
        Arc::new(RecordingProvider::default()),
        Arc::new(StubDiscovery::new(vec![])),
        Arc::new(ManualClock::new(1_000)),
    );
    let host_addr = serve_host(daemon).await;

    let error = run_cli_command(
        CliCommand::Activity(ActivityCommand::Resume {
            target: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        temp.path().to_path_buf(),
        String::new(),
        String::new(),
        config_path,
        data_dir,
    )
    .expect_err("resume fails");

    assert_eq!(
        error,
        "activity activity-1 can no longer be resumed because provider session github:stephanos/nitpick-agent#42 was not found; cleared the stored session id"
    );
    assert_eq!(
        store
            .get(&activity.id)
            .expect("stored activity")
            .session
            .provider_session_id,
        None
    );
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

async fn serve_host(daemon: HostDaemon) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, api_router(daemon))
            .await
            .expect("serve host");
    });
    addr.to_string()
}
