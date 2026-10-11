use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use rhai::AST;
use thiserror::Error;

use crate::command_prompt::StatusKeys;
use crate::grid::TerminalColor;
use crate::host_terminal::TerminalFeatures;
use crate::input::{BindingSpec, KeySequence, ModeSpec};
use crate::presentation::TabBarPosition;

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
    /// Colors of the dividers between panes.
    pub pane_borders: PaneBorderColors,
}

/// Colors of the dividers between panes, like tmux's `pane-border-style` and
/// `pane-active-border-style`. A divider cell next to the focused pane takes
/// `active`, every other one `inactive`; an unset color leaves the cells dim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneBorderColors {
    pub active: Option<TerminalColor>,
    pub inactive: Option<TerminalColor>,
}

/// The sixteen ANSI color names, as tmux spells them, with their palette
/// indexes. They follow the host terminal's color scheme.
const ANSI_COLOR_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightblack",
    "brightred",
    "brightgreen",
    "brightyellow",
    "brightblue",
    "brightmagenta",
    "brightcyan",
    "brightwhite",
];

/// Parse a color given as `#RRGGBB` or as one of the sixteen ANSI color
/// names (`magenta`, `brightblack`, ...).
pub fn parse_terminal_color(value: &str) -> Result<TerminalColor, PaletteError> {
    if let Some(index) = ANSI_COLOR_NAMES.iter().position(|name| *name == value) {
        return Ok(TerminalColor::Indexed(index as u8));
    }
    RgbColor::parse(value).map(|color| TerminalColor::Rgb(color.into()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleSpec {
    pub fg: Option<RgbColor>,
    pub bg: Option<RgbColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub blink: bool,
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

/// Where the client goes when the session it shows is destroyed, because its
/// last pane closed or it was killed (`set_detach_on_destroy`), like tmux's
/// `detach-on-destroy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetachOnDestroy {
    /// Detach: the interactive client exits.
    #[default]
    On,
    /// Switch to the session the client showed before, else the first one.
    Off,
    /// Switch to the session listed before the destroyed one, wrapping around.
    Previous,
    /// Switch to the session listed after the destroyed one, wrapping around.
    Next,
}

impl DetachOnDestroy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "previous" => Some(Self::Previous),
            "next" => Some(Self::Next),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseSettings {
    pub click_focus: bool,
    pub click_forward: bool,
    pub wheel_scroll: bool,
    pub wheel_forward: bool,
    pub drag_resize: bool,
    /// Drag to select and copy, double-click for a word, triple-click for a
    /// line, in panes that have not requested mouse reporting.
    pub select: bool,
}

impl MouseSettings {
    pub const fn capture_enabled(self) -> bool {
        self.click_focus
            || self.click_forward
            || self.wheel_scroll
            || self.wheel_forward
            || self.drag_resize
            || self.select
    }
}

/// Hint-mode configuration. When `patterns` is empty the built-in default set
/// (URLs, paths, SHAs, UUIDs, IPs, numbers) is used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintsSettings {
    pub patterns: Vec<String>,
}

/// Which panes may write to the host terminal through tmux passthrough
/// (`set_allow_passthrough`), like tmux's `allow-passthrough`. Off by default,
/// as in tmux: passthrough lets any program on screen write raw sequences to
/// the host terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassthroughMode {
    /// Drop passthrough sequences.
    #[default]
    Off,
    /// Forward them from panes that are on screen.
    On,
    /// Forward them from every pane in the client's session, on screen or not.
    All,
}

impl PassthroughMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            "all" => Some(Self::All),
            _ => None,
        }
    }
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
    /// Where the root tab bar is drawn (`tabbar.set_position`).
    pub tab_bar_position: TabBarPosition,
    pub mouse: MouseSettings,
    pub hints: HintsSettings,
    pub theme: ThemeSpec,
    /// Forward OSC 52 clipboard writes from programs in panes to the host
    /// terminal (`set_clipboard`). On by default.
    pub clipboard: bool,
    /// Number of the first window (`set_base_index`), if the config set one.
    pub base_index: Option<u32>,
    /// Which panes may write to the host terminal through tmux passthrough
    /// (`set_allow_passthrough`). `Off` by default.
    pub passthrough: PassthroughMode,
    /// Host terminal capabilities forced on or off (`set_terminal_features`).
    pub terminal_features: TerminalFeatures,
    /// Where the client goes when its session is destroyed
    /// (`set_detach_on_destroy`).
    pub detach_on_destroy: DetachOnDestroy,
    /// The keys prompts are edited with (`set_status_keys`).
    pub status_keys: StatusKeys,
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
            .field("tab_bar_position", &self.tab_bar_position)
            .field("mouse", &self.mouse)
            .field("hints", &self.hints)
            .field("theme", &self.theme)
            .field("clipboard", &self.clipboard)
            .field("base_index", &self.base_index)
            .field("passthrough", &self.passthrough)
            .field("terminal_features", &self.terminal_features)
            .field("status_keys", &self.status_keys)
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
