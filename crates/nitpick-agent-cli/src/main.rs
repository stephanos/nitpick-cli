use std::{
    env,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use nitpick_agent_cli::{
    CliCommand, CliOptions, CliRunContext, Confirmation, SystemCommand, config_path_from_env,
    data_dir_from_env, format_error_message, host_addr_from_env, parse_invocation,
    run_cli_command_with_options,
};
use nitpick_agent_core::{NONO_SANDBOX_HELPER_ARG, run_nono_sandbox_helper};

fn main() -> ExitCode {
    if env::args().nth(1).as_deref() == Some(NONO_SANDBOX_HELPER_ARG) {
        return run_nono_sandbox_helper(env::args_os().skip(2));
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{}", format_error_message(&message));
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let invocation = parse_invocation(env::args().skip(1))?;
    let addr = host_addr_from_env(env::var("NITPICK_AGENT_HOST_ADDR").ok());
    let repo_dir = current_dir()?;
    let diff = git_output(&repo_dir, &["diff"]).unwrap_or_default();
    let context = git_output(&repo_dir, &["status", "--short"]).unwrap_or_default();
    let config_path = config_path_from_env(env::var_os("NITPICK_AGENT_CONFIG"));
    let data_dir = data_dir_from_env(env::var_os("NITPICK_AGENT_DATA_DIR"));
    let command = invocation.command;
    let options = options_for_command(&command, invocation.options)?;
    let output = run_cli_command_with_options(
        command,
        CliRunContext {
            host_addr: addr,
            repo_dir,
            diff,
            context,
            config_path,
            data_dir,
        },
        options,
    )?;
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
}

fn options_for_command(
    command: &CliCommand,
    mut options: CliOptions,
) -> Result<CliOptions, String> {
    if matches!(command, CliCommand::System(SystemCommand::Reset { .. })) {
        options.reset_confirmation = prompt_reset_confirmation()?;
    }
    Ok(options)
}

fn prompt_reset_confirmation() -> Result<Option<Confirmation>, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(None);
    }

    print!("Reset local Nitpick state? [Y/n] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("write confirmation prompt: {error}"))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("read confirmation: {error}"))?;
    let answer = input.trim();
    if answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        return Ok(Some(Confirmation::Yes));
    }
    Ok(Some(Confirmation::No))
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
