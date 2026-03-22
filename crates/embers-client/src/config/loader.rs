use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use super::discover::{ConfigDiscoveryOptions, ConfigOrigin, DiscoveredConfig, discover_config};
use super::error::{ConfigError, ConfigManagerError, ConfigResult};
use crate::scripting::ScriptEngine;

pub const BUILTIN_CONFIG_SOURCE: &str = r#"mouse.set_click_focus(true);
mouse.set_click_forward(true);
mouse.set_wheel_scroll(true);
mouse.set_wheel_forward(true);

bind("normal", "<PageUp>", action.scroll_page_up());
bind("normal", "<PageDown>", action.scroll_page_down());
bind("normal", "/", action.enter_search_mode());
bind("normal", "n", action.search_next());
bind("normal", "N", action.search_prev());
bind("normal", "v", action.enter_select_char());
bind("normal", "V", action.enter_select_line());
bind("normal", "<C-v>", action.enter_select_block());

bind("select", "<Left>", action.select_move_left());
bind("select", "<Right>", action.select_move_right());
bind("select", "<Up>", action.select_move_up());
bind("select", "<Down>", action.select_move_down());
bind("select", "h", action.select_move_left());
bind("select", "j", action.select_move_down());
bind("select", "k", action.select_move_up());
bind("select", "l", action.select_move_right());
bind("select", "y", action.yank_selection());
bind("select", "<Esc>", action.cancel_selection());
"#;

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

pub struct ConfigManager {
    discovery: ConfigDiscoveryOptions,
    active_source: LoadedConfigSource,
    active_script: ScriptEngine,
}

impl ConfigManager {
    pub fn load(discovery: ConfigDiscoveryOptions) -> Result<Self, ConfigManagerError> {
        let active_source = load_config_source(&discovery)?;
        let active_script = match active_source.origin {
            ConfigOrigin::BuiltIn => ScriptEngine::load(&active_source)?,
            _ => ScriptEngine::load_with_overlay(BUILTIN_CONFIG_SOURCE, &active_source)?,
        };
        Ok(Self {
            discovery,
            active_source,
            active_script,
        })
    }

    pub fn from_process(explicit_path: Option<PathBuf>) -> Result<Self, ConfigManagerError> {
        Self::load(ConfigDiscoveryOptions::from_process(explicit_path))
    }

    pub fn discovery(&self) -> &ConfigDiscoveryOptions {
        &self.discovery
    }

    pub fn active_source(&self) -> &LoadedConfigSource {
        &self.active_source
    }

    pub fn active_script(&self) -> &ScriptEngine {
        &self.active_script
    }

    pub fn reload(&mut self) -> Result<(), ConfigManagerError> {
        let candidate_source = load_config_source(&self.discovery)?;
        let candidate_script = match candidate_source.origin {
            ConfigOrigin::BuiltIn => ScriptEngine::load(&candidate_source)?,
            _ => ScriptEngine::load_with_overlay(BUILTIN_CONFIG_SOURCE, &candidate_source)?,
        };
        self.active_source = candidate_source;
        self.active_script = candidate_script;
        Ok(())
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
            let Some(path) = discovered.path.clone() else {
                return Err(ConfigError::MissingPath { origin });
            };
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

    #[test]
    fn missing_paths_fail_without_panicking() {
        let error = super::load_discovered_source(&crate::config::DiscoveredConfig {
            origin: ConfigOrigin::Explicit,
            path: None,
        })
        .expect_err("missing path should error");

        assert!(matches!(
            error,
            crate::config::ConfigError::MissingPath { .. }
        ));
    }
}
