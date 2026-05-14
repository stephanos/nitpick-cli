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

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{
        checkout_root_from_env_values, config_path_from_env_value, data_dir_from_env_value,
        default_checkout_root,
    };

    #[test]
    fn config_path_uses_explicit_env_value() {
        assert_eq!(
            config_path_from_env_value(Some(OsString::from("/tmp/nitpick/config.toml"))),
            PathBuf::from("/tmp/nitpick/config.toml")
        );
    }

    #[test]
    fn data_dir_uses_explicit_env_value() {
        assert_eq!(
            data_dir_from_env_value(Some(OsString::from("/tmp/nitpick/data"))),
            PathBuf::from("/tmp/nitpick/data")
        );
    }

    #[test]
    fn checkout_root_prefers_checkout_dir_over_data_dir() {
        assert_eq!(
            checkout_root_from_env_values(
                Some(OsString::from("/tmp/checkouts")),
                Some(OsString::from("/tmp/data")),
            ),
            PathBuf::from("/tmp/checkouts")
        );
    }

    #[test]
    fn checkout_root_falls_back_to_data_dir_checkouts() {
        assert_eq!(
            checkout_root_from_env_values(None, Some(OsString::from("/tmp/data"))),
            PathBuf::from("/tmp/data/checkouts")
        );
    }

    #[test]
    fn checkout_root_uses_default_data_dir_checkouts() {
        assert_eq!(
            checkout_root_from_env_values(None, None),
            default_checkout_root()
        );
    }
}
