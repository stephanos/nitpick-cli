use std::sync::Arc;

use nitpick_agent_cli::{CliCommand, run_cli_command};
use nitpick_agent_core::FsActivityStore;
use nitpick_agent_core::FsProcessedReviewStore;
use nitpick_agent_host::{HostDaemon, api_router};
use nitpick_agent_integration_tests::support::{
    ManualClock, RecordingProvider, StubDiscovery, github_auto_review_config, pull_request,
};

#[tokio::test(flavor = "multi_thread")]
async fn cli_commands_talk_to_the_host_api() {
    let temp = tempfile::tempdir().expect("tempdir");
    let daemon = HostDaemon::with_dependencies(
        Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store")),
        github_auto_review_config(),
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
    )
    .expect("status command");
    assert!(status.contains("nitpick-agent-host: connected"));

    let requests = run_cli_command(
        CliCommand::ReviewRequests { only_new: true },
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
    )
    .expect("review requests command");
    assert_eq!(requests, "github stephanos/nitpick-agent#42");

    daemon.poll_review_requests().expect("poll");

    let activities = run_cli_command(
        CliCommand::Activities,
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
    )
    .expect("activities command");
    assert!(activities.contains("activity-1: Completed"));

    let reviews = run_cli_command(
        CliCommand::Reviews { include_all: true },
        &host_addr,
        repo_dir.clone(),
        String::new(),
        String::new(),
    )
    .expect("reviews command");
    assert!(reviews.contains("Completed review on stephanos/nitpick-agent#42 activity-1"));

    let cleanup = run_cli_command(
        CliCommand::CleanupCheckouts,
        &host_addr,
        repo_dir,
        String::new(),
        String::new(),
    )
    .expect("cleanup command");
    assert_eq!(cleanup, "no checkouts cleaned up");
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
