use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use serde::{Deserialize, Serialize};

use crate::{AgentError, AgentResult};

pub const NONO_SANDBOX_HELPER_ARG: &str = "__nitpick-nono-sandbox";
pub const NONO_SANDBOX_SPEC_ENV: &str = "NITPICK_NONO_SANDBOX_SPEC";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NonoSandboxSpec {
    pub read_paths: Vec<PathBuf>,
    pub read_write_paths: Vec<PathBuf>,
    pub platform_rules: Vec<String>,
}

impl NonoSandboxSpec {
    pub(crate) fn new(
        read_paths: Vec<PathBuf>,
        read_write_paths: Vec<PathBuf>,
        platform_rules: Vec<String>,
    ) -> Self {
        Self {
            read_paths,
            read_write_paths,
            platform_rules,
        }
    }

    pub(crate) fn to_env_value(&self) -> AgentResult<String> {
        serde_json::to_string(self).map_err(|error| {
            AgentError::sandbox(format!("serialize nono sandbox capabilities: {error}"))
        })
    }
}

pub fn run_nono_sandbox_helper(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    match run_nono_sandbox_helper_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(126)
        }
    }
}

fn run_nono_sandbox_helper_inner(args: impl IntoIterator<Item = OsString>) -> AgentResult<()> {
    let spec = nono_sandbox_spec_from_env()?;
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some(separator) if separator == "--" => {}
        _ => {
            return Err(AgentError::sandbox(format!(
                "{NONO_SANDBOX_HELPER_ARG} requires -- before provider command"
            )));
        }
    }
    let provider_command = args.next().ok_or_else(|| {
        AgentError::sandbox(format!(
            "{NONO_SANDBOX_HELPER_ARG} requires a provider command"
        ))
    })?;
    apply_nono_sandbox(&spec)?;
    exec_provider(provider_command, args.collect())
}

fn nono_sandbox_spec_from_env() -> AgentResult<NonoSandboxSpec> {
    let value = std::env::var(NONO_SANDBOX_SPEC_ENV)
        .map_err(|error| AgentError::sandbox(format!("read {NONO_SANDBOX_SPEC_ENV}: {error}")))?;
    serde_json::from_str(&value)
        .map_err(|error| AgentError::sandbox(format!("parse {NONO_SANDBOX_SPEC_ENV}: {error}")))
}

fn apply_nono_sandbox(spec: &NonoSandboxSpec) -> AgentResult<()> {
    let mut caps = nono::CapabilitySet::new();
    for path in &spec.read_paths {
        add_nono_path(&mut caps, path, nono::AccessMode::Read)?;
    }
    for path in &spec.read_write_paths {
        add_nono_path(&mut caps, path, nono::AccessMode::ReadWrite)?;
    }
    for rule in &spec.platform_rules {
        caps.add_platform_rule(rule).map_err(|error| {
            AgentError::sandbox(format!("add nono platform sandbox rule: {error}"))
        })?;
    }
    nono::Sandbox::apply(&caps)
        .map_err(|error| AgentError::sandbox(format!("apply nono sandbox: {error}")))?;
    Ok(())
}

fn add_nono_path(
    caps: &mut nono::CapabilitySet,
    path: &std::path::Path,
    mode: nono::AccessMode,
) -> AgentResult<()> {
    let metadata = match fs_err::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AgentError::sandbox(format!(
                "inspect nono sandbox path {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.is_dir() {
        caps.add_fs(nono::FsCapability::new_dir(path, mode).map_err(|error| {
            AgentError::sandbox(format!(
                "allow nono sandbox read path {}: {error}",
                path.display()
            ))
        })?);
    } else {
        caps.add_fs(nono::FsCapability::new_file(path, mode).map_err(|error| {
            AgentError::sandbox(format!(
                "allow nono sandbox file {}: {error}",
                path.display()
            ))
        })?);
    }
    Ok(())
}

#[cfg(unix)]
fn exec_provider(provider_command: OsString, args: Vec<OsString>) -> AgentResult<()> {
    use std::os::unix::process::CommandExt;

    let error = std::process::Command::new(&provider_command)
        .args(args)
        .exec();
    Err(AgentError::provider(format!(
        "exec sandboxed provider command `{}`: {error}",
        std::path::Path::new(&provider_command).display()
    )))
}

#[cfg(not(unix))]
fn exec_provider(_provider_command: OsString, _args: Vec<OsString>) -> AgentResult<()> {
    Err(AgentError::sandbox(
        "nono sandbox helper is only implemented on Unix platforms",
    ))
}
