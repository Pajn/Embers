use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use super::error::{ConfigError, ConfigResult};

const APPLICATION_NAME: &str = "embers";
const CONFIG_FILE_NAME: &str = "config.rhai";

pub const CONFIG_ENV_VAR: &str = "EMBERS_CONFIG";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigOrigin {
    Explicit,
    Environment,
    Standard,
    BuiltIn,
}

impl fmt::Display for ConfigOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Explicit => "explicit",
            Self::Environment => "environment",
            Self::Standard => "standard",
            Self::BuiltIn => "built-in",
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigDiscoveryOptions {
    pub explicit_path: Option<PathBuf>,
    pub env_path: Option<PathBuf>,
    pub standard_config_path: Option<PathBuf>,
}

impl ConfigDiscoveryOptions {
    pub fn from_process(explicit_path: Option<PathBuf>) -> Self {
        Self {
            explicit_path,
            env_path: env::var_os(CONFIG_ENV_VAR).map(PathBuf::from),
            standard_config_path: default_config_path(),
        }
    }

    pub fn with_project_config_dir(mut self, project_config_dir: impl Into<PathBuf>) -> Self {
        self.standard_config_path = Some(config_file_in_dir(project_config_dir.into()));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredConfig {
    pub origin: ConfigOrigin,
    pub path: Option<PathBuf>,
}

pub fn config_file_in_dir(project_config_dir: impl AsRef<Path>) -> PathBuf {
    project_config_dir.as_ref().join(CONFIG_FILE_NAME)
}

pub fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", APPLICATION_NAME)
        .map(|project_dirs| config_file_in_dir(project_dirs.config_dir()))
}

pub fn discover_config(options: &ConfigDiscoveryOptions) -> ConfigResult<DiscoveredConfig> {
    if let Some(path) = options.explicit_path.as_deref() {
        return resolve_path(path, ConfigOrigin::Explicit, true);
    }

    if let Some(path) = options.env_path.as_deref() {
        return resolve_path(path, ConfigOrigin::Environment, true);
    }

    if let Some(path) = options.standard_config_path.as_deref() {
        return resolve_path(path, ConfigOrigin::Standard, false);
    }

    Ok(DiscoveredConfig {
        origin: ConfigOrigin::BuiltIn,
        path: None,
    })
}

fn resolve_path(
    path: &Path,
    origin: ConfigOrigin,
    required: bool,
) -> ConfigResult<DiscoveredConfig> {
    match path.try_exists().map_err(|source| ConfigError::PathCheck {
        origin,
        path: path.to_path_buf(),
        source,
    })? {
        true => Ok(DiscoveredConfig {
            origin,
            path: Some(
                path.canonicalize()
                    .map_err(|source| ConfigError::Canonicalize {
                        origin,
                        path: path.to_path_buf(),
                        source,
                    })?,
            ),
        }),
        false if required => Err(ConfigError::MissingConfig {
            origin,
            path: path.to_path_buf(),
        }),
        false => Ok(DiscoveredConfig {
            origin: ConfigOrigin::BuiltIn,
            path: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        CONFIG_ENV_VAR, ConfigDiscoveryOptions, ConfigOrigin, config_file_in_dir, discover_config,
    };
    use crate::config::ConfigError;

    #[test]
    fn config_file_in_dir_appends_config_name() {
        let path = config_file_in_dir(PathBuf::from("/tmp/embers"));
        assert_eq!(path, PathBuf::from("/tmp/embers/config.rhai"));
    }

    #[test]
    fn explicit_override_wins_over_env_and_standard() {
        let tempdir = tempdir().unwrap();
        let explicit_path = write_config(tempdir.path().join("explicit.rhai"), "explicit");
        let env_path = write_config(tempdir.path().join("env.rhai"), "env");
        let standard_path = write_config(tempdir.path().join("config.rhai"), "standard");
        let options = ConfigDiscoveryOptions {
            explicit_path: Some(explicit_path.clone()),
            env_path: Some(env_path),
            standard_config_path: Some(standard_path),
        };

        let discovered = discover_config(&options).unwrap();

        assert_eq!(discovered.origin, ConfigOrigin::Explicit);
        assert_eq!(discovered.path, Some(explicit_path.canonicalize().unwrap()));
    }

    #[test]
    fn env_override_wins_over_standard() {
        let tempdir = tempdir().unwrap();
        let env_path = write_config(tempdir.path().join("env.rhai"), "env");
        let standard_path = write_config(tempdir.path().join("config.rhai"), "standard");
        let options = ConfigDiscoveryOptions {
            explicit_path: None,
            env_path: Some(env_path.clone()),
            standard_config_path: Some(standard_path),
        };

        let discovered = discover_config(&options).unwrap();

        assert_eq!(discovered.origin, ConfigOrigin::Environment);
        assert_eq!(discovered.path, Some(env_path.canonicalize().unwrap()));
    }

    #[test]
    fn missing_implicit_path_uses_builtin_config() {
        let tempdir = tempdir().unwrap();
        let options = ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path());

        let discovered = discover_config(&options).unwrap();

        assert_eq!(discovered.origin, ConfigOrigin::BuiltIn);
        assert_eq!(discovered.path, None);
    }

    #[test]
    fn missing_explicit_path_is_an_error() {
        let tempdir = tempdir().unwrap();
        let missing = tempdir.path().join("missing.rhai");
        let options = ConfigDiscoveryOptions {
            explicit_path: Some(missing.clone()),
            env_path: None,
            standard_config_path: None,
        };

        let error = discover_config(&options).unwrap_err();

        assert!(matches!(
            error,
            ConfigError::MissingConfig {
                origin: ConfigOrigin::Explicit,
                path,
            } if path == missing
        ));
    }

    #[test]
    fn missing_env_path_is_an_error() {
        let tempdir = tempdir().unwrap();
        let missing = tempdir.path().join("missing-env.rhai");
        let options = ConfigDiscoveryOptions {
            explicit_path: None,
            env_path: Some(missing.clone()),
            standard_config_path: None,
        };

        let error = discover_config(&options).unwrap_err();

        assert!(matches!(
            error,
            ConfigError::MissingConfig {
                origin: ConfigOrigin::Environment,
                path,
            } if path == missing
        ));
    }

    #[test]
    fn process_env_var_name_is_embers_config() {
        assert_eq!(CONFIG_ENV_VAR, "EMBERS_CONFIG");
    }

    fn write_config(path: PathBuf, contents: &str) -> PathBuf {
        fs::write(&path, contents).unwrap();
        path
    }
}
