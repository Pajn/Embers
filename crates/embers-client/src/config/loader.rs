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
mouse.set_drag_resize(true);
mouse.set_select(true);

bind("normal", "<PageUp>", action.scroll_page_up());
bind("normal", "<PageDown>", action.scroll_page_down());
bind("normal", "/", action.enter_search_mode());
bind("normal", "?", action.enter_search_backward());
bind("normal", "n", action.search_next());
bind("normal", "N", action.search_prev());
bind("normal", "v", action.enter_select_char());
bind("normal", "V", action.enter_select_line());
bind("normal", "<C-v>", action.enter_select_block());

// tmux copy-mode-vi motions, shared by copy mode and its selection mode.
for mode in ["copy", "select"] {
    bind(mode, "h", action.copy_cursor_left());
    bind(mode, "j", action.copy_cursor_down());
    bind(mode, "k", action.copy_cursor_up());
    bind(mode, "l", action.copy_cursor_right());
    bind(mode, "<Left>", action.copy_cursor_left());
    bind(mode, "<Down>", action.copy_cursor_down());
    bind(mode, "<Up>", action.copy_cursor_up());
    bind(mode, "<Right>", action.copy_cursor_right());
    bind(mode, "w", action.copy_next_word());
    bind(mode, "b", action.copy_previous_word());
    bind(mode, "e", action.copy_next_word_end());
    bind(mode, "W", action.copy_next_space());
    bind(mode, "B", action.copy_previous_space());
    bind(mode, "E", action.copy_next_space_end());
    bind(mode, "0", action.copy_start_of_line());
    bind(mode, "^", action.copy_back_to_indentation());
    bind(mode, "$", action.copy_end_of_line());
    bind(mode, "<Home>", action.copy_start_of_line());
    bind(mode, "<End>", action.copy_end_of_line());
    bind(mode, "gg", action.copy_history_top());
    bind(mode, "G", action.copy_history_bottom());
    bind(mode, "H", action.copy_top_line());
    bind(mode, "M", action.copy_middle_line());
    bind(mode, "L", action.copy_bottom_line());
    bind(mode, "<C-u>", action.copy_halfpage_up());
    bind(mode, "<C-d>", action.copy_halfpage_down());
    bind(mode, "<C-b>", action.copy_page_up());
    bind(mode, "<C-f>", action.copy_page_down());
    bind(mode, "<PageUp>", action.copy_page_up());
    bind(mode, "<PageDown>", action.copy_page_down());
    bind(mode, "<C-y>", action.scroll_line_up());
    bind(mode, "<C-e>", action.scroll_line_down());
    bind(mode, "f", action.copy_jump_forward());
    bind(mode, "F", action.copy_jump_backward());
    bind(mode, "t", action.copy_jump_to_forward());
    bind(mode, "T", action.copy_jump_to_backward());
    bind(mode, ";", action.copy_jump_again());
    bind(mode, ",", action.copy_jump_reverse());
    bind(mode, "/", action.enter_search_mode());
    bind(mode, "?", action.enter_search_backward());
    bind(mode, "n", action.search_next());
    bind(mode, "N", action.search_prev());
    bind(mode, "v", action.enter_select_char());
    bind(mode, "V", action.enter_select_line());
    bind(mode, "<C-v>", action.enter_select_block());
    bind(mode, "<Enter>", action.yank_selection());
    bind(mode, "q", action.exit_copy_mode());
    bind(mode, "<C-c>", action.exit_copy_mode());
}

bind("copy", "<Space>", action.enter_select_char());
bind("copy", "<Esc>", action.exit_copy_mode());

bind("select", "o", action.select_other_end());
bind("select", "y", action.yank_selection());
bind("select", "<Esc>", action.cancel_selection());

bind("search", "<Enter>", action.commit_search());
bind("search", "<Esc>", action.cancel_search());
bind("search", "n", action.search_next());
bind("search", "N", action.search_prev());
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
        let active_script = load_script_engine(&active_source)?;
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
        let candidate_script = load_script_engine(&candidate_source)?;
        self.active_source = candidate_source;
        self.active_script = candidate_script;
        Ok(())
    }

    pub fn reload_if_changed(&mut self) -> Result<bool, ConfigManagerError> {
        let candidate_source = load_config_source(&self.discovery)?;
        if candidate_source == self.active_source {
            return Ok(false);
        }

        let candidate_script = load_script_engine(&candidate_source)?;
        self.active_source = candidate_source;
        self.active_script = candidate_script;
        Ok(true)
    }
}

fn load_script_engine(source: &LoadedConfigSource) -> Result<ScriptEngine, ConfigManagerError> {
    match source.origin {
        ConfigOrigin::BuiltIn => Ok(ScriptEngine::load(source)?),
        _ => Ok(ScriptEngine::load_with_overlay(
            BUILTIN_CONFIG_SOURCE,
            source,
        )?),
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
