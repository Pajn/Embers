use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::geometry::PtySize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    pub position: CursorPosition,
    pub shape: CursorShape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModes {
    pub alternate_screen: bool,
    pub mouse_reporting: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    /// Kitty keyboard protocol flags active on the focused screen (bit 0 =
    /// disambiguate-esc-codes, bit 4 = report-all-keys-as-esc). Zero when the
    /// program has not enabled the protocol.
    pub keyboard_mode: u8,
}

/// Semantic terminal color.
///
/// Colors stay semantic through the pipeline: named/indexed ANSI colors ship as
/// [`TermColor::Indexed`] so the outer terminal's palette resolves them, rather
/// than baking the emulator's palette RGB. Only true-color (`38;2;…`) sequences
/// become [`TermColor::Rgb`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TermColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

/// Per-cell attribute bitflags.
///
/// A hand-rolled newtype over `u16` (no `bitflags` dependency). Serializes
/// transparently as its inner integer. `WIDE_CHAR`/`WRAPLINE`/spacer flags are
/// consumed by the emulator walk and never represented here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAttrs(pub u16);

impl CellAttrs {
    pub const BOLD: u16 = 1;
    pub const DIM: u16 = 2;
    pub const ITALIC: u16 = 4;
    pub const UNDERLINE: u16 = 8;
    pub const DOUBLE_UNDERLINE: u16 = 16;
    pub const INVERSE: u16 = 32;
    pub const HIDDEN: u16 = 64;
    pub const STRIKEOUT: u16 = 128;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    pub const fn insert(&mut self, flag: u16) {
        self.0 |= flag;
    }
}

/// Run-length style annotation over a [`SnapshotLine`]'s text.
///
/// `len` is measured in bytes of `text`. Invariant on a line: either `runs` is
/// empty (the whole line is default-styled) or `sum(run.len) == text.len()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledRun {
    /// Number of bytes of the line's text this run covers.
    pub len: u32,
    pub fg: TermColor,
    pub bg: TermColor,
    pub attrs: CellAttrs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLine {
    pub text: String,
    /// Run-length style annotations. Empty means the whole line is default-styled.
    /// `#[serde(default)]` keeps this compatible with older keeper processes that
    /// serialize `SnapshotLine` without a `runs` field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<StyledRun>,
}

impl SnapshotLine {
    /// A line with no per-cell styling.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            runs: Vec::new(),
        }
    }
}

impl From<&str> for SnapshotLine {
    fn from(value: &str) -> Self {
        Self::plain(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub sequence: u64,
    pub size: PtySize,
    pub cursor: Option<CursorState>,
    pub lines: Vec<SnapshotLine>,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub viewport_top_line: u64,
    pub total_lines: u64,
    pub modes: TerminalModes,
}

impl TerminalSnapshot {
    pub fn from_lines<I, S>(sequence: u64, size: PtySize, lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            sequence,
            size,
            cursor: None,
            lines: lines.into_iter().map(SnapshotLine::plain).collect(),
            title: None,
            cwd: None,
            viewport_top_line: 0,
            total_lines: u64::from(size.rows),
            modes: TerminalModes::default(),
        }
    }

    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use crate::geometry::PtySize;

    use super::{CellAttrs, SnapshotLine, StyledRun, TermColor, TerminalSnapshot};

    #[test]
    fn plain_text_joins_lines() {
        let snapshot = TerminalSnapshot::from_lines(7, PtySize::new(80, 24), ["hello", "world"]);

        assert_eq!(snapshot.plain_text(), "hello\nworld");
    }

    #[test]
    fn styled_run_round_trips_through_json() {
        let mut attrs = CellAttrs::empty();
        attrs.insert(CellAttrs::BOLD);
        attrs.insert(CellAttrs::UNDERLINE);
        let line = SnapshotLine {
            text: "hi".to_owned(),
            runs: vec![
                StyledRun {
                    len: 1,
                    fg: TermColor::Indexed(1),
                    bg: TermColor::Default,
                    attrs,
                },
                StyledRun {
                    len: 1,
                    fg: TermColor::Rgb {
                        r: 10,
                        g: 20,
                        b: 30,
                    },
                    bg: TermColor::Indexed(4),
                    attrs: CellAttrs::empty(),
                },
            ],
        };

        let json = serde_json::to_string(&line).unwrap();
        let decoded: SnapshotLine = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, line);
    }

    #[test]
    fn plain_line_serializes_without_runs_key() {
        let line = SnapshotLine::plain("hello");
        let json = serde_json::to_string(&line).unwrap();
        assert_eq!(json, r#"{"text":"hello"}"#);
    }

    #[test]
    fn line_without_runs_key_deserializes_as_plain() {
        // Old keeper processes serialize `SnapshotLine` without a `runs` field;
        // that JSON must still decode against the new type.
        let decoded: SnapshotLine = serde_json::from_str(r#"{"text":"x"}"#).unwrap();
        assert_eq!(decoded, SnapshotLine::plain("x"));
        assert!(decoded.runs.is_empty());
    }

    #[test]
    fn cell_attrs_serializes_as_integer() {
        let mut attrs = CellAttrs::empty();
        attrs.insert(CellAttrs::BOLD);
        assert_eq!(serde_json::to_string(&attrs).unwrap(), "1");
        let decoded: CellAttrs = serde_json::from_str("1").unwrap();
        assert_eq!(decoded, attrs);
    }
}
