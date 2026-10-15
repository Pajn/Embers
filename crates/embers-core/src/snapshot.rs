use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

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
    /// disambiguate-esc-codes, bit 3 = report-all-keys-as-esc, matching the kitty
    /// protocol's own flag numbering). Zero when the program has not enabled the
    /// protocol.
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

impl TermColor {
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}

/// Per-cell attribute bitflags.
///
/// A hand-rolled newtype over `u16` (no `bitflags` dependency). Serializes
/// transparently as its inner integer. `WIDE_CHAR`/`WRAPLINE`/spacer flags are
/// consumed by the emulator walk and never represented here.
///
/// The underline flags (`UNDERLINE`, `DOUBLE_UNDERLINE`, `CURLY_UNDERLINE`,
/// `DOTTED_UNDERLINE`, `DASHED_UNDERLINE`) are mutually exclusive: a cell has at
/// most one underline style, mirroring SGR `4:1`..`4:5`.
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
    pub const CURLY_UNDERLINE: u16 = 256;
    pub const DOTTED_UNDERLINE: u16 = 512;
    pub const DASHED_UNDERLINE: u16 = 1024;
    /// Every underline style flag.
    pub const ANY_UNDERLINE: u16 = Self::UNDERLINE
        | Self::DOUBLE_UNDERLINE
        | Self::CURLY_UNDERLINE
        | Self::DOTTED_UNDERLINE
        | Self::DASHED_UNDERLINE;

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
    /// Underline color (SGR 58). [`TermColor::Default`] means the underline
    /// follows the foreground, which is the common case; it is omitted from the
    /// keeper JSON and travels sparsely on the wire.
    #[serde(default, skip_serializing_if = "TermColor::is_default")]
    pub underline_color: TermColor,
    /// OSC 8 hyperlink covering this run: `0` for none, otherwise a 1-based
    /// index into the owning line's [`SnapshotLine::links`]. Use
    /// [`SnapshotLine::link`] to resolve it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub link: u16,
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}

/// An OSC 8 hyperlink as the program in the pane declared it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hyperlink {
    /// The `id=` parameter, which lets a terminal treat separate spans (for
    /// example a link wrapped across lines) as one link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub uri: String,
}

/// One cell of a [`SnapshotLine`] as the emulator laid it out: `len` bytes of
/// the line's text, drawn `width` columns wide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSpan {
    pub len: u32,
    pub width: u8,
}

/// A cell of a line: where its text starts in the line's text, the text, and
/// how many columns it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCell<'a> {
    pub offset: usize,
    pub text: &'a str,
    pub width: u16,
}

/// Columns a grapheme cluster covers as one cell: its Unicode width, at least
/// one.
pub fn grapheme_width(grapheme: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(grapheme).max(1)).unwrap_or(u16::MAX)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLine {
    pub text: String,
    /// Run-length style annotations. Empty means the whole line is default-styled.
    /// `#[serde(default)]` keeps this compatible with older keeper processes that
    /// serialize `SnapshotLine` without a `runs` field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<StyledRun>,
    /// Hyperlinks referenced by `runs[..].link`, each listed once per line. The
    /// table is per line (not per snapshot) so a line stays self-contained when
    /// it is sliced out of scrollback or moved between views.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Hyperlink>,
    /// The line's cells, listed only when the text alone does not give them.
    /// Empty means one cell per grapheme cluster, as wide as Unicode says
    /// ([`grapheme_width`]). Emulators do not always agree: alacritty gives
    /// each codepoint of an emoji sequence a cell of its own, and Ghostty
    /// draws a lone regional indicator two columns wide. A line from such an
    /// emulator lists every cell, so it is drawn over the columns the program
    /// in the pane sees. Spans that do not cover `text` exactly, on character
    /// boundaries, are ignored. Set them with [`SnapshotLine::set_cells`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_spans: Vec<CellSpan>,
}

impl SnapshotLine {
    /// A line with no per-cell styling.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            runs: Vec::new(),
            links: Vec::new(),
            cell_spans: Vec::new(),
        }
    }

    /// The line's cells, in order.
    pub fn cells(&self) -> Cells<'_> {
        let spans_cover_text = !self.cell_spans.is_empty() && {
            let mut end = 0_usize;
            self.cell_spans.iter().all(|span| {
                end = end.saturating_add(span.len as usize);
                span.len > 0 && end <= self.text.len() && self.text.is_char_boundary(end)
            }) && end == self.text.len()
        };
        if spans_cover_text {
            Cells::Spans {
                text: &self.text,
                spans: self.cell_spans.iter(),
                offset: 0,
            }
        } else {
            Cells::Graphemes(self.text.grapheme_indices(true))
        }
    }

    /// Columns the line covers.
    pub fn width(&self) -> usize {
        self.cells().map(|cell| usize::from(cell.width)).sum()
    }

    /// Record the cells an emulator laid the text out in, `len` bytes and
    /// `width` columns each. They are kept only when they differ from what
    /// the text alone gives, so most lines carry none.
    ///
    /// An emulator without grapheme clustering, such as alacritty, gives each
    /// codepoint that is not zero-width a cell of its own, so a flag's two
    /// regional indicators or the parts of a ZWJ emoji sequence take a cell
    /// each. Cells that together make up exactly one grapheme cluster, over
    /// the columns Unicode gives it, become one cell, which a host terminal
    /// that clusters draws whole, unless they are styled apart. Set `runs`
    /// first.
    pub fn set_cells(&mut self, cells: Vec<CellSpan>) {
        self.cell_spans = Vec::new();
        // Most lines: a narrow cell per ASCII character, as the text gives.
        if self.text.is_ascii()
            && cells.len() == self.text.len()
            && cells.iter().all(|cell| cell.width == 1)
        {
            return;
        }

        // Where style runs end: cells styled apart are never joined.
        let mut run_ends = self
            .runs
            .iter()
            .scan(0_usize, |end, run| {
                *end += run.len as usize;
                Some(*end)
            })
            .peekable();
        let mut offset = 0_usize;
        let mut spans = Vec::with_capacity(cells.len());
        let mut differs = false;
        let mut cells = cells.into_iter().peekable();
        // Bytes of the text the cells taken so far cover past the grapheme
        // being walked, when a cell runs over a grapheme's end.
        let mut ahead = 0_usize;
        for grapheme in self.text.graphemes(true) {
            let start = offset;
            offset += grapheme.len();
            while run_ends.next_if(|&end| end <= start).is_some() {}
            let one_style = run_ends.peek().is_none_or(|&end| end >= offset);
            if ahead >= grapheme.len() {
                ahead -= grapheme.len();
                continue;
            }
            let first = spans.len();
            let mut covered = ahead;
            let mut width = 0_u16;
            while covered < grapheme.len() {
                let Some(cell) = cells.next() else {
                    break;
                };
                covered += cell.len as usize;
                width += u16::from(cell.width);
                spans.push(cell);
            }
            let aligned = ahead == 0 && covered == grapheme.len();
            ahead = covered.saturating_sub(grapheme.len());
            let joined = u32::try_from(grapheme.len())
                .ok()
                .zip(u8::try_from(width).ok());
            if let Some((len, width)) = joined
                && aligned
                && (one_style || spans.len() - first == 1)
                && u16::from(width) == grapheme_width(grapheme)
            {
                if spans.len() - first > 1 {
                    spans.truncate(first);
                    spans.push(CellSpan { len, width });
                }
            } else {
                differs = true;
            }
        }
        spans.extend(cells);
        if differs {
            self.cell_spans = spans;
        }
    }

    /// The hyperlink covering `run`, if any. An index outside `links` resolves
    /// to `None`.
    pub fn link(&self, run: &StyledRun) -> Option<&Hyperlink> {
        let index = usize::from(run.link).checked_sub(1)?;
        self.links.get(index)
    }
}

/// The cells of a [`SnapshotLine`]; see [`SnapshotLine::cells`].
pub enum Cells<'a> {
    Graphemes(GraphemeIndices<'a>),
    Spans {
        text: &'a str,
        spans: std::slice::Iter<'a, CellSpan>,
        offset: usize,
    },
}

impl<'a> Iterator for Cells<'a> {
    type Item = LineCell<'a>;

    fn next(&mut self) -> Option<LineCell<'a>> {
        match self {
            Self::Graphemes(graphemes) => {
                let (offset, text) = graphemes.next()?;
                Some(LineCell {
                    offset,
                    text,
                    width: grapheme_width(text),
                })
            }
            Self::Spans {
                text,
                spans,
                offset,
            } => {
                let span = spans.next()?;
                let start = *offset;
                *offset += span.len as usize;
                Some(LineCell {
                    offset: start,
                    text: &text[start..*offset],
                    width: u16::from(span.width.max(1)),
                })
            }
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

    use super::{CellAttrs, CellSpan, SnapshotLine, StyledRun, TermColor, TerminalSnapshot};

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
                    underline_color: TermColor::Default,
                    link: 0,
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
                    underline_color: TermColor::Default,
                    link: 0,
                },
            ],
            links: Vec::new(),
            cell_spans: Vec::new(),
        };

        let json = serde_json::to_string(&line).unwrap();
        let decoded: SnapshotLine = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, line);
    }

    #[test]
    fn underline_color_is_omitted_from_json_when_default() {
        let mut run = StyledRun {
            len: 1,
            attrs: CellAttrs(CellAttrs::CURLY_UNDERLINE),
            ..StyledRun::default()
        };
        let json = serde_json::to_string(&run).unwrap();
        assert!(!json.contains("underline_color"), "{json}");
        assert_eq!(serde_json::from_str::<StyledRun>(&json).unwrap(), run);

        run.underline_color = TermColor::Rgb { r: 1, g: 2, b: 3 };
        let json = serde_json::to_string(&run).unwrap();
        assert_eq!(serde_json::from_str::<StyledRun>(&json).unwrap(), run);
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

    const FLAG: &str = "\u{1F3F3}\u{FE0F}\u{200D}\u{26A7}\u{FE0F}";

    fn span(len: usize, width: u8) -> CellSpan {
        CellSpan {
            len: len as u32,
            width,
        }
    }

    fn cells(line: &SnapshotLine) -> Vec<(&str, u16)> {
        line.cells().map(|cell| (cell.text, cell.width)).collect()
    }

    #[test]
    fn without_spans_cells_are_graphemes_as_wide_as_unicode_says() {
        let line = SnapshotLine::plain(format!("a{FLAG}漢e\u{301}"));
        assert_eq!(
            cells(&line),
            [("a", 1), (FLAG, 2), ("漢", 2), ("e\u{301}", 1)]
        );
        assert_eq!(line.width(), 6);
    }

    #[test]
    fn cells_the_text_gives_are_not_kept() {
        let mut line = SnapshotLine::plain(format!("ab{FLAG}"));
        line.set_cells(vec![span(1, 1), span(1, 1), span(FLAG.len(), 2)]);
        assert!(line.cell_spans.is_empty());
    }

    #[test]
    fn cells_that_differ_from_the_text_are_kept_and_drawn() {
        // Alacritty: a thumbs up and its skin tone, two columns each, and a
        // symbol VS16 makes wide only to Unicode.
        let (thumb, tone, symbol) = ("\u{1F44D}", "\u{1F3FD}", "\u{26A7}\u{FE0F}");
        let mut line = SnapshotLine::plain(format!("{thumb}{tone}{symbol}x"));
        line.set_cells(vec![span(4, 2), span(4, 2), span(6, 1), span(1, 1)]);
        assert_eq!(cells(&line), [(thumb, 2), (tone, 2), (symbol, 1), ("x", 1)]);
        assert_eq!(line.width(), 6);

        let json = serde_json::to_string(&line).unwrap();
        assert_eq!(serde_json::from_str::<SnapshotLine>(&json).unwrap(), line);
    }

    #[test]
    fn cells_that_make_up_one_character_are_joined() {
        // Alacritty: the white flag with its selector and joiner in one
        // narrow cell, the transgender symbol in another, then a flag's two
        // regional indicators. Each pair covers the columns Unicode gives
        // the whole, so the text gives the cells.
        let (head, tail) = FLAG.split_at(FLAG.find('\u{26A7}').unwrap());
        let mut line = SnapshotLine::plain(format!("{FLAG}\u{1F1F8}\u{1F1EA}"));
        line.set_cells(vec![
            span(head.len(), 1),
            span(tail.len(), 1),
            span(4, 1),
            span(4, 1),
        ]);
        assert!(line.cell_spans.is_empty());

        // Alone after another cell that stays, the pair is one cell.
        let mut line = SnapshotLine::plain(format!("\u{26A7}\u{FE0F}{FLAG}"));
        line.set_cells(vec![span(6, 1), span(head.len(), 1), span(tail.len(), 1)]);
        assert_eq!(cells(&line), [("\u{26A7}\u{FE0F}", 1), (FLAG, 2)]);
    }

    #[test]
    fn spans_that_do_not_fit_the_text_are_ignored() {
        let mut line = SnapshotLine::plain("漢x");
        // Short of the text, then splitting a character.
        line.cell_spans = vec![span(3, 2)];
        assert_eq!(cells(&line), [("漢", 2), ("x", 1)]);
        line.cell_spans = vec![span(1, 1), span(3, 1)];
        assert_eq!(cells(&line), [("漢", 2), ("x", 1)]);
    }
}
