#[derive(Clone, Debug)]
pub struct CliRunContext {
    pub host_addr: String,
    pub repo_dir: std::path::PathBuf,
    pub diff: String,
    pub context: String,
    pub config_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
}

pub fn config_path_from_env(
    nitpick_agent_config: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    nitpick_agent_core::config_path_from_env_value(nitpick_agent_config)
}

pub fn data_dir_from_env(nitpick_agent_data_dir: Option<std::ffi::OsString>) -> std::path::PathBuf {
    nitpick_agent_core::data_dir_from_env_value(nitpick_agent_data_dir)
}
