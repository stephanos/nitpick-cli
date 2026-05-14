use std::{ffi::OsString, path::PathBuf};

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "nitpick";
const APPLICATION: &str = "nitpick-agent";

pub fn config_path_from_env_value(value: Option<OsString>) -> PathBuf {
    value.map(PathBuf::from).unwrap_or_else(default_config_path)
}

pub fn data_dir_from_env_value(value: Option<OsString>) -> PathBuf {
    value.map(PathBuf::from).unwrap_or_else(default_data_dir)
}

pub fn checkout_root_from_env_values(
    checkout_dir: Option<OsString>,
    data_dir: Option<OsString>,
) -> PathBuf {
    if let Some(path) = checkout_dir {
        return PathBuf::from(path);
    }
    if let Some(path) = data_dir {
        return PathBuf::from(path).join("checkouts");
    }
    default_checkout_root()
}

pub fn default_config_path() -> PathBuf {
    project_dirs()
        .map(|dirs| dirs.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

pub fn default_data_dir() -> PathBuf {
    project_dirs()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_checkout_root() -> PathBuf {
    default_data_dir().join("checkouts")
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}
