use std::sync::Arc;

use nitpick_agent_cli::{
    CliCommand, DebugCommand, ReviewCommand, ReviewListStatus, SystemCommand, run_cli_command,
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
            "#!/bin/sh\nprintf 'pwd=%s args=%s\\n' \"$PWD\" \"$*\" > '{}'\n",
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
if [ "$1 $2" = "pr view" ] && [ "$6" = "--json" ] && [ "$7" = "title,author,url,headRefOid,headRefName,state,mergedAt" ]; then
  printf '{{"title":"Stub PR","author":{{"login":"stub-author"}},"url":"https://github.com/stephanos/nitpick-agent/pull/42","headRefOid":"abc123","headRefName":"feature","state":"OPEN","mergedAt":null}}\n'
  exit 0
fi
if [ "$1 $2" = "pr view" ]; then
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
    let checkout = data_dir.join("checkouts/stephanos/nitpick-agent/pr-42");
    std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
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
        CliCommand::Status,
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
        CliCommand::Review(ReviewCommand::List {
            status: ReviewListStatus::Requested,
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review list requested command");
    assert_eq!(requests, "stephanos/nitpick-agent#42 requested");

    daemon.poll_review_requests().expect("poll");

    let activities = run_cli_command(
        CliCommand::Debug(DebugCommand::Activities),
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
        CliCommand::Review(ReviewCommand::List {
            status: ReviewListStatus::History,
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("reviews command");
    assert!(reviews.contains("stephanos/nitpick-agent#42 Completed activity-"));

    let logs = run_cli_command(
        CliCommand::Debug(DebugCommand::Logs {
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
    assert!(logs.contains("review complete"));

    let review_run = run_cli_command(
        CliCommand::Review(ReviewCommand::Run {
            subject: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review run command");
    assert!(review_run.contains("activity-"));
    assert!(review_run.contains(
        "ReviewSummary Pending { destination: \"github-review\", remote_id: Some(\"99\"), remote_url: Some(\"https://github.com/stephanos/nitpick-agent/pull/42#pullrequestreview-99\") }"
    ), "{review_run}");

    let daemon_logs = run_cli_command(
        CliCommand::Debug(DebugCommand::Logs {
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

    let review_chat = run_cli_command(
        CliCommand::Review(ReviewCommand::Chat {
            target: "https://github.com/stephanos/nitpick-agent/pull/42".into(),
        }),
        &host_addr,
        temp.path().to_path_buf(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review chat command");
    assert_eq!(review_chat, "");
    let review_chat_log = std::fs::read_to_string(resume_log).expect("review chat args");
    let prefix = format!(
        "pwd={} args=--resume ",
        checkout
            .canonicalize()
            .expect("canonical checkout")
            .display()
    );
    let session_id = review_chat_log
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('\n'))
        .expect("review chat session id");
    assert!(is_uuid_like(session_id), "{session_id}");

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
async fn review_chat_clears_missing_provider_session_id() {
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
    std::fs::create_dir_all(data_dir.join("checkouts/stephanos/nitpick-agent/pr-42/.git"))
        .expect("checkout");
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
        CliCommand::Review(ReviewCommand::Chat {
            target: "stephanos/nitpick-agent#42".into(),
        }),
        &host_addr,
        temp.path().to_path_buf(),
        String::new(),
        String::new(),
        config_path,
        data_dir,
    )
    .expect_err("review chat fails");

    assert_eq!(
        error,
        "activity activity-1 can no longer be resumed because its provider session was not found; cleared the stored session"
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

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .filter(|(index, _)| ![8, 13, 18, 23].contains(index))
            .all(|(_, byte)| byte.is_ascii_hexdigit())
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
