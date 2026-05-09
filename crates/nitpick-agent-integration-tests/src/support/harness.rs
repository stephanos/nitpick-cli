use std::sync::Arc;

use nitpick_agent_core::{ActivityStore, FsActivityStore};
use nitpick_agent_github::FsProcessedReviewStore;
use nitpick_agent_host::{AgentConfig, GitHubDiscoveryConfig, HostDaemon};
use tempfile::TempDir;

use crate::support::{ManualClock, RecordingProvider, StubDiscovery};

pub struct TestHarness {
    pub _temp: TempDir,
    pub store: Arc<FsActivityStore>,
    pub processed: Arc<FsProcessedReviewStore>,
    pub discovery: Arc<StubDiscovery>,
    pub provider: Arc<RecordingProvider>,
    pub clock: Arc<ManualClock>,
    pub daemon: HostDaemon,
}

impl TestHarness {
    pub fn new(config: AgentConfig, discovery: Arc<StubDiscovery>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(FsActivityStore::new(temp.path().join("store")).expect("store"));
        let processed = Arc::new(
            FsProcessedReviewStore::new(temp.path().join("processed-reviews")).expect("processed"),
        );
        Self::from_parts(temp, store, processed, discovery, config)
    }

    fn from_parts(
        temp: TempDir,
        store: Arc<FsActivityStore>,
        processed: Arc<FsProcessedReviewStore>,
        discovery: Arc<StubDiscovery>,
        config: AgentConfig,
    ) -> Self {
        let provider = Arc::new(RecordingProvider::default());
        let clock = Arc::new(ManualClock::new(1_000));
        let daemon = HostDaemon::with_dependencies(
            store.clone(),
            config,
            processed.clone(),
            provider.clone(),
            discovery.clone(),
            clock.clone(),
        );
        Self {
            _temp: temp,
            store,
            processed,
            discovery,
            provider,
            clock,
            daemon,
        }
    }

    pub fn activity_count(&self) -> usize {
        self.store.list().expect("activities").len()
    }
}

pub fn github_auto_review_config() -> AgentConfig {
    github_discovery_config(true, true, 300)
}

pub fn github_discovery_only_config() -> AgentConfig {
    github_discovery_config(true, false, 300)
}

pub fn github_disabled_config() -> AgentConfig {
    github_discovery_config(false, true, 300)
}

fn github_discovery_config(enabled: bool, auto_review: bool, interval_seconds: u64) -> AgentConfig {
    AgentConfig {
        github_discovery: GitHubDiscoveryConfig {
            enabled,
            auto_review,
            interval_seconds,
            ..GitHubDiscoveryConfig::default()
        },
        ..AgentConfig::default()
    }
}
