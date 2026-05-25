use std::{
    fs,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use nitpick_agent_cli::{
    CliCommand, DebugCommand, ReviewCommand, ReviewListStatus, SystemCommand, run_cli_command,
};
use nitpick_agent_core::{
    ActivityStatus, ActivityStore, AgentProvider, AgentResult, AgentSession, ArtifactContent,
    ArtifactKind, ArtifactSyncState, ChatInput, FsActivityStore, FsProcessedReviewStore,
    ProviderReviewContext, ProviderRunContext, ReviewInput, ReviewOutput,
};
use nitpick_agent_host::{
    HostDaemon, api_router,
    review_mcp::{AddReviewCommentInput, ReviewMcpTools, load_review_mcp_session_state},
};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, github_auto_review_config,
    github_disabled_config, pull_request,
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
    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
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
    assert!(status.contains("Host"));
    assert!(status.contains("Reviews"));
    assert!(status.contains("connected"));

    let requests = run_cli_command(
        CliCommand::Review(ReviewCommand::List {
            status: ReviewListStatus::Requested,
            limit: 20,
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("review list requested command");
    assert!(requests.contains("requested"));
    assert!(requests.contains("stephanos/nitpick-agent#42"));

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
    assert!(activities.contains("activity-1"));
    assert!(activities.contains("completed"));

    let reviews = run_cli_command(
        CliCommand::Review(ReviewCommand::List {
            status: ReviewListStatus::History,
            limit: 20,
        }),
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
        config_path.clone(),
        data_dir.clone(),
    )
    .expect("reviews command");
    assert!(reviews.contains("completed"));
    assert!(reviews.contains("stephanos/nitpick-agent#42"));
    assert!(reviews.contains("activity-"));

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
    assert!(logs.contains("activity"));
    assert!(logs.contains("activity-2"));
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
    assert!(review_run.contains("status"));
    assert!(review_run.contains("nitpick review show stephanos/nitpick-agent#42"));
    assert!(review_run.contains("active"));
    assert!(review_run.contains("nitpick review list --status active"));
    let activity_id = activity_id_from_review_run(&review_run);
    let activity = wait_for_completed_activity(store.as_ref(), &activity_id);
    let artifacts = wait_for_synced_artifacts(store.as_ref(), &activity.id);
    assert!(matches!(
        artifacts[0].sync_state,
        ArtifactSyncState::Pending {
            ref destination,
            ref remote_id,
            ref remote_url
        } if destination == "github-review"
            && remote_id.as_deref() == Some("99")
            && remote_url.as_deref() == Some("https://github.com/stephanos/nitpick-agent/pull/42#pullrequestreview-99")
    ));

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

#[tokio::test(flavor = "multi_thread")]
async fn review_chat_resumes_with_activity_provider() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_provider = temp.path().join("provider");
    let resume_log = temp.path().join("resume.log");
    fs::write(
        &fake_provider,
        format!(
            "#!/bin/sh\nprintf 'pwd=%s args=%s\\n' \"$PWD\" \"$*\" > '{}'\n",
            resume_log.display()
        ),
    )
    .expect("fake provider");
    make_executable(&fake_provider);
    let config_path = temp.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "[agent]\nprovider = \"claude\"\ncommand = \"{}\"\nsandbox = \"none\"\n",
            fake_provider.display(),
        ),
    )
    .expect("config");
    let data_dir = temp.path().join("data");
    let checkout = data_dir.join("checkouts/stephanos/subvoc/pr-1");
    fs::create_dir_all(checkout.join(".git")).expect("checkout");
    let store = Arc::new(FsActivityStore::new(&data_dir).expect("store"));
    let processed = Arc::new(
        FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
    );
    let mut activity = store
        .create(nitpick_agent_core::ActivityKind::Review)
        .expect("activity");
    activity.label = Some("review on stephanos/subvoc#1".into());
    activity.status = nitpick_agent_core::ActivityStatus::Completed;
    activity.session.provider = Some(nitpick_agent_core::AgentProviderKind::Codex);
    activity.session.provider_session_id = Some("codex-session-1".into());
    activity.session.status = nitpick_agent_core::SessionStatus::Completed;
    store.save(&activity).expect("save activity");
    let daemon = HostDaemon::with_dependencies(
        store,
        github_disabled_config(),
        processed,
        Arc::new(RecordingProvider::default()),
        Arc::new(StubDiscovery::new(vec![])),
        Arc::new(ManualClock::new(1_000)),
    );
    let host_addr = serve_host(daemon).await;

    let output = run_cli_command(
        CliCommand::Review(ReviewCommand::Chat {
            target: "https://github.com/stephanos/subvoc/pull/1".into(),
        }),
        &host_addr,
        temp.path().to_path_buf(),
        String::new(),
        String::new(),
        config_path,
        data_dir,
    )
    .expect("review chat command");

    assert_eq!(output, "");
    assert_eq!(
        fs::read_to_string(resume_log).expect("review chat args"),
        format!(
            "pwd={} args=resume codex-session-1\n",
            checkout
                .canonicalize()
                .expect("canonical checkout")
                .display()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_run_uses_mcp_tools_for_local_smoke_comments() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = temp.path().join("repo");
    fs::create_dir(&repo_dir).expect("repo dir");
    run_git(&repo_dir, &["init"]);
    fs::create_dir(repo_dir.join("src")).expect("src dir");
    fs::write(
        repo_dir.join("src/lib.rs"),
        "pub fn value() -> i32 {\n    1\n}\n",
    )
    .expect("repo file");
    run_git(&repo_dir, &["add", "."]);
    run_git(
        &repo_dir,
        &[
            "-c",
            "user.email=nitpick@example.com",
            "-c",
            "user.name=Nitpick",
            "commit",
            "-m",
            "initial",
        ],
    );
    fs::write(
        repo_dir.join("src/lib.rs"),
        "pub fn value() -> i32 {\n    1\n}\n\npub fn extra() -> i32 {\n    2\n}\n",
    )
    .expect("changed repo file");
    let diff = run_git(&repo_dir, &["diff", "--", "src/lib.rs"]);

    let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
    let daemon = HostDaemon::with_dependencies(
        store.clone(),
        github_disabled_config(),
        Arc::new(
            FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
        ),
        Arc::new(McpSmokeProvider),
        Arc::new(StubDiscovery::new(vec![])),
        Arc::new(ManualClock::new(1_000)),
    );
    let host_addr = serve_host(daemon).await;

    let review_run = run_cli_command(
        CliCommand::Review(ReviewCommand::Run {
            subject: "local-smoke".into(),
        }),
        &host_addr,
        repo_dir,
        diff,
        String::new(),
        temp.path().join("config.toml"),
        temp.path().join("data"),
    )
    .expect("review run command");

    assert!(review_run.contains("activity-"));
    assert!(review_run.contains("status"));
    assert!(review_run.contains("nitpick review show local-smoke"));
    assert!(review_run.contains("active"));
    assert!(review_run.contains("nitpick review list --status active"));
    let activity = wait_for_completed_review(store.as_ref());
    let activities = store.list().expect("activities");
    assert_eq!(activities.len(), 1);
    let artifacts = store
        .list_artifacts_for(&activity.id)
        .expect("activity artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::ReviewComment);
    assert!(
        artifacts
            .iter()
            .all(|artifact| !matches!(artifact.content, ArtifactContent::ReviewSummary(_)))
    );
    assert_eq!(
        artifacts[0].content,
        ArtifactContent::ReviewComment(nitpick_agent_core::ReviewComment {
            path: "src/lib.rs".into(),
            line: 4,
            body: "smoke comment from MCP tools".into(),
        })
    );
}

struct McpSmokeProvider;

impl AgentProvider for McpSmokeProvider {
    fn supports_review_tools(&self) -> bool {
        true
    }

    fn review(
        &self,
        _session: &mut AgentSession,
        input: &ReviewInput,
        context: ProviderReviewContext<'_>,
    ) -> AgentResult<ReviewOutput> {
        let tools = context.tools.expect("MCP smoke provider should use tools");
        let state_path = state_path_from_config(&tools.mcp_config_path);
        let state = load_review_mcp_session_state(&state_path)?;
        assert_eq!(state.repo_dir, input.repo_dir);
        assert!(!state.finished);

        let tools = ReviewMcpTools::from_state_path(state_path);
        tools.add_review_comment(AddReviewCommentInput {
            path: "src/lib.rs".into(),
            line: 4,
            body: "smoke comment from MCP tools".into(),
        })?;
        tools.finish_review()?;
        Ok(ReviewOutput::default())
    }

    fn chat(
        &self,
        _session: &mut AgentSession,
        _input: &ChatInput,
        _context: ProviderRunContext<'_>,
    ) -> AgentResult<String> {
        Ok(String::new())
    }
}

fn state_path_from_config(config_path: &std::path::Path) -> String {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(config_path).expect("mcp config bytes"))
            .expect("mcp config json");
    let args = config["mcpServers"]["nitpick-review"]["args"]
        .as_array()
        .expect("server args");
    assert_eq!(args[0], "review-mcp");
    args[1].as_str().expect("state path").to_owned()
}

fn run_git(repo_dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("git output utf-8")
}

fn wait_for_completed_review(store: &FsActivityStore) -> nitpick_agent_core::Activity {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let activities = store.list().expect("activities");
        if let Some(activity) = activities
            .iter()
            .find(|activity| activity.kind == nitpick_agent_core::ActivityKind::Review)
            && activity.status == ActivityStatus::Completed
        {
            return activity.clone();
        }
        assert!(
            Instant::now() < deadline,
            "review did not complete: {activities:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_completed_activity(
    store: &FsActivityStore,
    activity_id: &nitpick_agent_core::ActivityId,
) -> nitpick_agent_core::Activity {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let activities = store.list().expect("activities");
        if let Some(activity) = activities
            .iter()
            .find(|activity| &activity.id == activity_id)
            && activity.status == ActivityStatus::Completed
        {
            return activity.clone();
        }
        assert!(
            Instant::now() < deadline,
            "activity {activity_id} did not complete: {activities:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn activity_id_from_review_run(output: &str) -> nitpick_agent_core::ActivityId {
    let start = output.find("activity-").expect("review run activity id");
    let id = output[start..]
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    nitpick_agent_core::ActivityId::new(id)
}

fn wait_for_synced_artifacts(
    store: &FsActivityStore,
    activity_id: &nitpick_agent_core::ActivityId,
) -> Vec<nitpick_agent_core::Artifact> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let artifacts = store
            .list_artifacts_for(activity_id)
            .expect("activity artifacts");
        if artifacts
            .iter()
            .any(|artifact| !matches!(artifact.sync_state, ArtifactSyncState::LocalOnly))
        {
            return artifacts;
        }
        assert!(
            Instant::now() < deadline,
            "review artifacts did not sync: {artifacts:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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
