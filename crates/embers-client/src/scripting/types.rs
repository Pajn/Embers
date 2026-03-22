use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use rhai::AST;
use thiserror::Error;

use crate::input::{BindingSpec, KeySequence, ModeSpec};

use super::model::Action;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScriptFunctionRef {
    pub name: String,
}

impl ScriptFunctionRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub fn parse(value: &str) -> Result<Self, PaletteError> {
        let Some(hex) = value.strip_prefix('#') else {
            return Err(PaletteError::InvalidColor {
                value: value.to_owned(),
            });
        };
        if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(PaletteError::InvalidColor {
                value: value.to_owned(),
            });
        }

        let red = u8::from_str_radix(&hex[0..2], 16).map_err(|_| PaletteError::InvalidColor {
            value: value.to_owned(),
        })?;
        let green = u8::from_str_radix(&hex[2..4], 16).map_err(|_| PaletteError::InvalidColor {
            value: value.to_owned(),
        })?;
        let blue = u8::from_str_radix(&hex[4..6], 16).map_err(|_| PaletteError::InvalidColor {
            value: value.to_owned(),
        })?;

        Ok(Self { red, green, blue })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeSpec {
    pub palette: BTreeMap<String, RgbColor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleSpec {
    pub fg: Option<RgbColor>,
    pub bg: Option<RgbColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarTarget {
    Tab {
        tabs_node_id: embers_core::NodeId,
        index: usize,
    },
    Floating {
        floating_id: embers_core::FloatingId,
    },
    Buffer {
        buffer_id: embers_core::BufferId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarSegment {
    pub text: String,
    pub style: StyleSpec,
    pub target: Option<BarTarget>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarSpec {
    pub left: Vec<BarSegment>,
    pub center: Vec<BarSegment>,
    pub right: Vec<BarSegment>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModeHooks {
    pub on_enter: Option<ScriptFunctionRef>,
    pub on_leave: Option<ScriptFunctionRef>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseSettings {
    pub click_focus: bool,
    pub click_forward: bool,
    pub wheel_scroll: bool,
    pub wheel_forward: bool,
}

#[derive(Clone)]
pub struct LoadedConfig {
    pub source_path: Option<PathBuf>,
    pub source_hash: u64,
    pub ast: AST,
    pub leader: KeySequence,
    pub modes: BTreeMap<String, ModeSpec>,
    pub mode_hooks: BTreeMap<String, ModeHooks>,
    pub bindings: BTreeMap<String, Vec<BindingSpec<Vec<Action>>>>,
    pub named_actions: BTreeMap<String, ScriptFunctionRef>,
    pub event_handlers: BTreeMap<String, Vec<ScriptFunctionRef>>,
    pub tab_bar_formatter: Option<ScriptFunctionRef>,
    pub mouse: MouseSettings,
    pub theme: ThemeSpec,
}

impl LoadedConfig {
    pub fn has_action(&self, name: &str) -> bool {
        self.named_actions.contains_key(name)
    }

    pub fn has_event_handlers(&self, event: &str) -> bool {
        self.event_handlers
            .get(event)
            .is_some_and(|handlers| !handlers.is_empty())
    }

    pub fn has_tab_bar_formatter(&self) -> bool {
        self.tab_bar_formatter.is_some()
    }
}

impl fmt::Debug for LoadedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedConfig")
            .field("source_path", &self.source_path)
            .field("source_hash", &self.source_hash)
            .field("ast", &"<ast>")
            .field("leader", &self.leader)
            .field("modes", &self.modes)
            .field("mode_hooks", &self.mode_hooks)
            .field("bindings", &self.bindings)
            .field("named_actions", &self.named_actions)
            .field("event_handlers", &self.event_handlers)
            .field("tab_bar_formatter", &self.tab_bar_formatter)
            .field("mouse", &self.mouse)
            .field("theme", &self.theme)
            .finish()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PaletteError {
    #[error("palette color '{value}' must be in '#RRGGBB' form")]
    InvalidColor { value: String },
}

#[cfg(test)]
mod tests {
    use super::{PaletteError, RgbColor};

    #[test]
    fn parses_hex_colors() {
        assert_eq!(
            RgbColor::parse("#12abef").unwrap(),
            RgbColor {
                red: 0x12,
                green: 0xab,
                blue: 0xef,
            }
        );
    }

    #[test]
    fn rejects_invalid_hex_colors() {
        assert_eq!(
            RgbColor::parse("red").unwrap_err(),
            PaletteError::InvalidColor {
                value: "red".to_owned(),
            }
        );
    }
}
