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

pub fn host_addr_from_env(value: Option<String>) -> String {
    value.unwrap_or_else(|| "127.0.0.1:19783".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn resolves_config_path_like_host() {
        assert_eq!(
            super::config_path_from_env(Some("/tmp/config.toml".into())),
            std::path::PathBuf::from("/tmp/config.toml")
        );
        assert_eq!(
            super::config_path_from_env(None),
            nitpick_agent_core::default_config_path()
        );
    }

    #[test]
    fn resolves_data_dir_like_host() {
        assert_eq!(
            super::data_dir_from_env(Some("/tmp/data".into())),
            std::path::PathBuf::from("/tmp/data")
        );
        assert_eq!(
            super::data_dir_from_env(None),
            nitpick_agent_core::default_data_dir()
        );
    }

    #[test]
    fn defaults_host_address_when_env_is_unset() {
        assert_eq!(super::host_addr_from_env(None), "127.0.0.1:19783");
    }
}
