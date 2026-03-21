use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use super::discover::{ConfigDiscoveryOptions, ConfigOrigin, DiscoveredConfig, discover_config};
use super::error::{ConfigError, ConfigResult};

pub const BUILTIN_CONFIG_SOURCE: &str = "";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfigSource {
    pub origin: ConfigOrigin,
    pub path: Option<PathBuf>,
    pub source: String,
    pub source_hash: u64,
}

impl LoadedConfigSource {
    pub fn display_path(&self) -> String {
        self.path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<built-in>".to_owned())
    }
}

#[derive(Clone, Debug)]
pub struct ConfigManager {
    discovery: ConfigDiscoveryOptions,
    active_source: LoadedConfigSource,
}

impl ConfigManager {
    pub fn load(discovery: ConfigDiscoveryOptions) -> ConfigResult<Self> {
        let active_source = load_config_source(&discovery)?;
        Ok(Self {
            discovery,
            active_source,
        })
    }

    pub fn from_process(explicit_path: Option<PathBuf>) -> ConfigResult<Self> {
        Self::load(ConfigDiscoveryOptions::from_process(explicit_path))
    }

    pub fn discovery(&self) -> &ConfigDiscoveryOptions {
        &self.discovery
    }

    pub fn active_source(&self) -> &LoadedConfigSource {
        &self.active_source
    }
}

pub fn load_config_source(discovery: &ConfigDiscoveryOptions) -> ConfigResult<LoadedConfigSource> {
    let discovered = discover_config(discovery)?;
    load_discovered_source(&discovered)
}

fn load_discovered_source(discovered: &DiscoveredConfig) -> ConfigResult<LoadedConfigSource> {
    match discovered.origin {
        ConfigOrigin::BuiltIn => Ok(LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: None,
            source: BUILTIN_CONFIG_SOURCE.to_owned(),
            source_hash: source_hash(BUILTIN_CONFIG_SOURCE),
        }),
        origin => {
            let path = discovered
                .path
                .clone()
                .expect("non-built-in config is missing a path");
            let source = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
                origin,
                path: path.clone(),
                source,
            })?;
            Ok(LoadedConfigSource {
                origin,
                path: Some(path),
                source_hash: source_hash(&source),
                source,
            })
        }
    }
}

fn source_hash(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        BUILTIN_CONFIG_SOURCE, ConfigDiscoveryOptions, LoadedConfigSource, load_config_source,
    };
    use crate::config::ConfigOrigin;

    #[test]
    fn builtin_source_loads_when_no_file_exists() {
        let loaded = load_config_source(&ConfigDiscoveryOptions::default()).unwrap();

        assert_eq!(
            loaded,
            LoadedConfigSource {
                origin: ConfigOrigin::BuiltIn,
                path: None,
                source: BUILTIN_CONFIG_SOURCE.to_owned(),
                source_hash: super::source_hash(BUILTIN_CONFIG_SOURCE),
            }
        );
        assert_eq!(loaded.display_path(), "<built-in>");
    }

    #[test]
    fn explicit_file_loads_source_and_hash() {
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("config.rhai");
        fs::write(&path, "bind(\"normal\", \"q\", ())").unwrap();
        let options = ConfigDiscoveryOptions {
            explicit_path: Some(path.clone()),
            env_path: None,
            standard_config_path: None,
        };

        let loaded = load_config_source(&options).unwrap();

        assert_eq!(loaded.origin, ConfigOrigin::Explicit);
        assert_eq!(loaded.path, Some(path.canonicalize().unwrap()));
        assert_eq!(loaded.source, "bind(\"normal\", \"q\", ())");
        assert_eq!(loaded.source_hash, super::source_hash(&loaded.source));
    }
}
