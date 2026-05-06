use std::{env, process::ExitCode};

use nitpick_agent_cli::{
    CliCommand, format_activities, format_activity, format_artifact, format_artifacts,
    format_host_status, format_review_requests, help_text, host_addr_from_env, parse_command,
    review_input,
};
use nitpick_agent_client::HostClient;
use nitpick_agent_core::ChatInput;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    match parse_command(env::args().skip(1))? {
        CliCommand::Help => {
            println!("{}", help_text(env!("CARGO_PKG_VERSION")));
        }
        CliCommand::Version => {
            println!("nitpick-agent {}", env!("CARGO_PKG_VERSION"));
        }
        CliCommand::Status => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            match HostClient::new(&addr).status() {
                Ok(status) => println!("{}", format_host_status(&host_status(status))),
                Err(error) if error.starts_with("nitpick-agent-host unavailable") => {
                    println!("nitpick-agent-host: not connected");
                    println!("address: {addr}");
                }
                Err(error) => return Err(error),
            }
        }
        CliCommand::ReviewRequests { only_new } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let requests = HostClient::new(&addr).github_review_requests(only_new)?;
            println!("{}", format_review_requests(&requests));
        }
        CliCommand::Activities => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let activities = HostClient::new(&addr).activities()?;
            println!("{}", format_activities(&activities));
        }
        CliCommand::Artifacts { activity_id } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let artifacts = HostClient::new(&addr).activity_artifacts(&activity_id)?;
            println!("{}", format_artifacts(&artifacts));
        }
        CliCommand::Artifact { artifact_id } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let artifact = HostClient::new(&addr).artifact(&artifact_id)?;
            println!("{}", format_artifact(&artifact));
        }
        CliCommand::ArtifactSync {
            artifact_id,
            destination,
            target,
        } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let artifact = HostClient::new(&addr).sync_artifact(
                &artifact_id,
                &destination,
                target.as_deref(),
            )?;
            println!("{}", format_artifact(&artifact));
        }
        CliCommand::SyncPending { destination } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let artifacts =
                HostClient::new(&addr).pending_sync_artifacts(destination.as_deref())?;
            println!("{}", format_artifacts(&artifacts));
        }
        CliCommand::Review { subject } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let repo_dir = current_dir()?;
            let diff = git_output(&repo_dir, &["diff"]).unwrap_or_default();
            let activity = HostClient::new(&addr).review(&review_input(subject, repo_dir, diff))?;
            println!("{}", format_activity(&activity));
            if let Some(error) = activity.error {
                return Err(error);
            }
        }
        CliCommand::Chat { prompt } => {
            let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
            let repo_dir = current_dir()?;
            let context = git_output(&repo_dir, &["status", "--short"]).unwrap_or_default();
            let activity = HostClient::new(&addr).chat(&ChatInput {
                repo_dir,
                prompt,
                context,
            })?;
            println!("{}", format_activity(&activity));
            if let Some(error) = activity.error {
                return Err(error);
            }
        }
    }

    Ok(())
}

fn host_status(status: nitpick_agent_client::HostStatus) -> nitpick_agent_cli::HostStatus {
    nitpick_agent_cli::HostStatus {
        activity_count: status.activity_count,
        running_activity_count: status.running_activity_count,
        completed_activity_count: status.completed_activity_count,
        error_activity_count: status.error_activity_count,
        artifact_count: status.artifact_count,
        local_only_artifact_count: status.local_only_artifact_count,
        pending_sync_artifact_count: status.pending_sync_artifact_count,
        provider: status.provider,
        model: status.model,
    }
}

fn current_dir() -> Result<std::path::PathBuf, String> {
    env::current_dir().map_err(|error| format!("read current directory: {error}"))
}

fn git_output(repo_dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .map_err(|error| format!("run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
