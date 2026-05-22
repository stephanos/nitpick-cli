use std::{
    env, net::SocketAddr, path::PathBuf, process::ExitCode, sync::Arc, thread, time::Duration,
};

use nitpick_agent_core::{
    FsActivityStore, FsProcessedReviewStore, config_path_from_env_value, data_dir_from_env_value,
};
use nitpick_agent_host::{AgentConfig, HostDaemon, api_router, review_mcp};

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    if env::args().nth(1).as_deref() == Some("daemon") {
        return run_daemon().await;
    }
    if env::args().nth(1).as_deref() == Some("review-mcp") {
        return run_review_mcp().await;
    }

    let (daemon, config_path, data_dir) = match build_daemon() {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    match daemon.status() {
        Ok(status) => {
            println!("nitpick-agent-host {}", env!("CARGO_PKG_VERSION"));
            println!("activities: {}", status.activity_count);
            println!("running activities: {}", status.running_activity_count);
            println!("completed activities: {}", status.completed_activity_count);
            println!("error activities: {}", status.error_activity_count);
            println!("artifacts: {}", status.artifact_count);
            println!("local-only artifacts: {}", status.local_only_artifact_count);
            println!(
                "pending-sync artifacts: {}",
                status.pending_sync_artifact_count
            );
            println!("config: {}", config_path.display());
            println!("data: {}", data_dir.display());
            println!("agent: {}", status.provider);
            println!("model: {}", status.model.as_deref().unwrap_or("(default)"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

async fn run_review_mcp() -> ExitCode {
    let Some(state_path) = env::args_os().nth(2).map(PathBuf::from) else {
        eprintln!("review-mcp requires a session state path");
        return ExitCode::from(2);
    };
    match review_mcp::serve_review_mcp_stdio(state_path).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

async fn run_daemon() -> ExitCode {
    let (daemon, config_path, data_dir) = match build_daemon() {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let addr = match host_addr() {
        Ok(addr) => addr,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind {addr}: {error}");
            return ExitCode::from(2);
        }
    };

    println!(
        "nitpick-agent-host {} listening on {addr}",
        env!("CARGO_PKG_VERSION")
    );
    println!("config: {}", config_path.display());
    println!("data: {}", data_dir.display());
    spawn_review_source_poller(daemon.clone());
    spawn_activity_pruner(daemon.clone());
    tracing::info!(%addr, "host daemon listening");

    match axum::serve(listener, api_router(daemon)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("host server failed: {error}");
            ExitCode::from(2)
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nitpick_agent=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn build_daemon() -> Result<(HostDaemon, PathBuf, PathBuf), String> {
    let config_path = config_path();
    AgentConfig::init_template_file(&config_path).map_err(|error| error.to_string())?;
    AgentConfig::write_config_example_file(&config_path).map_err(|error| error.to_string())?;
    AgentConfig::init_review_prompt_file(&config_path).map_err(|error| error.to_string())?;
    let config = AgentConfig::load_or_default(&config_path).map_err(|error| error.to_string())?;
    let data_dir = data_dir();
    let store = FsActivityStore::new(&data_dir).map_err(|error| error.to_string())?;
    let processed_reviews =
        FsProcessedReviewStore::new(data_dir.join("review-sources/processed-reviews"))
            .map_err(|error| error.to_string())?;
    let daemon = HostDaemon::with_config_and_processed_reviews(
        Arc::new(store),
        config,
        Arc::new(processed_reviews),
    );
    daemon
        .recover_interrupted_activities()
        .map_err(|error| error.to_string())?;
    Ok((daemon, config_path, data_dir))
}

fn spawn_review_source_poller(daemon: HostDaemon) {
    let interval_seconds = daemon.config().github_discovery.interval_seconds;
    if !daemon.config().github_discovery.enabled {
        return;
    }

    thread::spawn(move || {
        loop {
            if let Err(error) = daemon.poll_review_requests() {
                eprintln!("review source discovery failed: {error}");
            }
            thread::sleep(Duration::from_secs(interval_seconds));
        }
    });
}

fn spawn_activity_pruner(daemon: HostDaemon) {
    thread::spawn(move || {
        loop {
            if let Err(error) = daemon.prune_old_activities() {
                eprintln!("activity pruning failed: {error}");
            }
            thread::sleep(Duration::from_secs(60 * 60));
        }
    });
}

fn host_addr() -> Result<SocketAddr, String> {
    env::var("NITPICK_AGENT_HOST_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:19783".into())
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid NITPICK_AGENT_HOST_ADDR: {error}"))
}

fn data_dir() -> PathBuf {
    data_dir_from_env_value(env::var_os("NITPICK_AGENT_DATA_DIR"))
}

fn config_path() -> PathBuf {
    config_path_from_env_value(env::var_os("NITPICK_AGENT_CONFIG"))
}
