use std::{env, process::ExitCode};

use nitpick_agent_cli::{
    config_path_from_env, data_dir_from_env, host_addr_from_env, parse_command, run_cli_command,
};

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
    let command = parse_command(env::args().skip(1))?;
    let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
    let repo_dir = current_dir()?;
    let diff = git_output(&repo_dir, &["diff"]).unwrap_or_default();
    let context = git_output(&repo_dir, &["status", "--short"]).unwrap_or_default();
    let config_path = config_path_from_env(
        env::var_os("NITPICK_AGENT_CONFIG"),
        env::var_os("XDG_CONFIG_HOME"),
        env::var_os("HOME"),
    );
    let data_dir = data_dir_from_env(
        env::var_os("NITPICK_AGENT_DATA_DIR"),
        env::var_os("XDG_DATA_HOME"),
        env::var_os("HOME"),
    );
    let output = run_cli_command(
        command,
        &addr,
        repo_dir,
        diff,
        context,
        config_path,
        data_dir,
    )?;
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
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
