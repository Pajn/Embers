use std::path::PathBuf;

use crate::geometry::PtySize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorState {
    pub position: CursorPosition,
    pub shape: CursorShape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalModes {
    pub alternate_screen: bool,
    pub mouse_reporting: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotLine {
    pub text: String,
}

impl From<&str> for SnapshotLine {
    fn from(value: &str) -> Self {
        Self {
            text: value.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
            lines: lines
                .into_iter()
                .map(|line| SnapshotLine { text: line.into() })
                .collect(),
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

    use super::TerminalSnapshot;

    #[test]
    fn plain_text_joins_lines() {
        let snapshot = TerminalSnapshot::from_lines(7, PtySize::new(80, 24), ["hello", "world"]);

        assert_eq!(snapshot.plain_text(), "hello\nworld");
    }
}
