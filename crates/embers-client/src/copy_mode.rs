//! Copy-mode cursor motions, modelled on tmux's `copy-mode-vi` table.
//!
//! Motions are pure functions over the buffer's captured text so a count can
//! repeat them without extra round-trips. Positions use the same coordinates as
//! [`SelectionPoint`]: an absolute line in the buffer's history (0 is the
//! oldest line) and a display column.
//!
//! Word motions split text into three classes, as vim does: whitespace,
//! separators, and word characters. The separators are ASCII punctuation other
//! than `_`, which is tmux's default `word-separators` and, for ASCII text, the
//! complement of vim's default `iskeyword`. Everything else, including `_`,
//! digits and non-ASCII text, is a word character. The space-separated
//! variants (`W`, `B`, `E`) treat every non-whitespace character as part of the
//! word.

use std::time::{Duration, Instant};

use embers_core::{BufferId, NodeId};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::state::{SearchMatch, SelectionPoint};

/// Largest count prefix accepted; further digits are ignored.
pub const MAX_COUNT: u32 = 99_999;

/// Presses on the same cell within this interval count as a double or triple
/// click.
pub const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(300);

/// A copy-mode cursor motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyMotion {
    Left,
    Right,
    Up,
    Down,
    /// `w`: start of the next word.
    NextWord,
    /// `b`: start of the previous word.
    PreviousWord,
    /// `e`: end of the next word.
    NextWordEnd,
    /// `W`: start of the next space-separated word.
    NextSpace,
    /// `B`: start of the previous space-separated word.
    PreviousSpace,
    /// `E`: end of the next space-separated word.
    NextSpaceEnd,
    /// `0`: first column.
    StartOfLine,
    /// `^`: first non-blank character.
    BackToIndentation,
    /// `$`: last character.
    EndOfLine,
    /// `gg`: first line of history.
    HistoryTop,
    /// `G`: last line of the buffer.
    HistoryBottom,
    /// `H`: top row of the view.
    TopLine,
    /// `M`: middle row of the view.
    MiddleLine,
    /// `L`: bottom row of the view.
    BottomLine,
    /// `{`: the blank line before the previous paragraph.
    PreviousParagraph,
    /// `}`: the blank line after the next paragraph.
    NextParagraph,
    /// `C-u`: scroll and move up half a page.
    HalfPageUp,
    /// `C-d`: scroll and move down half a page.
    HalfPageDown,
    /// `C-b`: scroll and move up a page.
    PageUp,
    /// `C-f`: scroll and move down a page.
    PageDown,
}

impl CopyMotion {
    /// Whether the motion reads line text, so the buffer must be captured.
    pub fn needs_text(self) -> bool {
        matches!(
            self,
            Self::NextWord
                | Self::PreviousWord
                | Self::NextWordEnd
                | Self::NextSpace
                | Self::PreviousSpace
                | Self::NextSpaceEnd
                | Self::BackToIndentation
                | Self::EndOfLine
                | Self::PreviousParagraph
                | Self::NextParagraph
        )
    }

    /// Whether a count prefix repeats the motion. Motions to an absolute
    /// position ignore the count.
    pub fn repeats(self) -> bool {
        !matches!(
            self,
            Self::StartOfLine
                | Self::BackToIndentation
                | Self::EndOfLine
                | Self::HistoryTop
                | Self::HistoryBottom
                | Self::TopLine
                | Self::MiddleLine
                | Self::BottomLine
        )
    }
}

/// `f`/`F`/`t`/`T`: jump to (or just before) a character on the cursor line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JumpKind {
    /// `f`: onto the next occurrence.
    Forward,
    /// `F`: onto the previous occurrence.
    Backward,
    /// `t`: just before the next occurrence.
    ToForward,
    /// `T`: just after the previous occurrence.
    ToBackward,
}

impl JumpKind {
    /// The same jump in the opposite direction, for `,`.
    pub fn reversed(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
            Self::ToForward => Self::ToBackward,
            Self::ToBackward => Self::ToForward,
        }
    }
}

/// The part of a pane's history a motion can see and scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyViewport {
    /// First history line shown.
    pub top: u64,
    /// Number of rows shown.
    pub rows: u64,
    /// Total lines in the buffer (history plus screen).
    pub total: u64,
    /// Width of the terminal, bounding rightward movement.
    pub cols: u16,
}

impl CopyViewport {
    fn last_line(self) -> u64 {
        self.total.saturating_sub(1)
    }

    fn bottom_top(self) -> u64 {
        self.total.saturating_sub(self.rows)
    }

    /// The scroll position closest to `top` that shows `line`.
    pub fn reveal(self, top: u64, line: u64) -> u64 {
        let rows = self.rows.max(1);
        let top = if line < top {
            line
        } else if line >= top.saturating_add(rows) {
            line.saturating_sub(rows - 1)
        } else {
            top
        };
        top.min(self.bottom_top())
    }
}

/// Where a motion leaves the cursor and the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionOutcome {
    pub cursor: SelectionPoint,
    pub top: u64,
}

/// Apply `motion` once. `lines` is the buffer text indexed by history line;
/// lines beyond it read as empty.
pub fn apply_motion(
    motion: CopyMotion,
    cursor: SelectionPoint,
    viewport: CopyViewport,
    lines: &[String],
) -> MotionOutcome {
    let last_line = viewport.last_line();
    let mut top = viewport.top;
    let cursor = match motion {
        CopyMotion::Left => SelectionPoint {
            column: cursor.column.saturating_sub(1),
            ..cursor
        },
        CopyMotion::Right => SelectionPoint {
            column: cursor
                .column
                .saturating_add(1)
                .min(viewport.cols.saturating_sub(1)),
            ..cursor
        },
        CopyMotion::Up => SelectionPoint {
            line: cursor.line.saturating_sub(1),
            ..cursor
        },
        CopyMotion::Down => SelectionPoint {
            line: cursor.line.saturating_add(1).min(last_line),
            ..cursor
        },
        CopyMotion::NextWord => next_word_start(lines, last_line, cursor, false),
        CopyMotion::NextSpace => next_word_start(lines, last_line, cursor, true),
        CopyMotion::PreviousWord => previous_word_start(lines, last_line, cursor, false),
        CopyMotion::PreviousSpace => previous_word_start(lines, last_line, cursor, true),
        CopyMotion::NextWordEnd => next_word_end(lines, last_line, cursor, false),
        CopyMotion::NextSpaceEnd => next_word_end(lines, last_line, cursor, true),
        CopyMotion::StartOfLine => SelectionPoint {
            column: 0,
            ..cursor
        },
        CopyMotion::BackToIndentation => {
            let cells = cells(line_text(lines, cursor.line));
            SelectionPoint {
                column: cells
                    .iter()
                    .find(|cell| class_of(cell.text, true) != CharClass::Blank)
                    .map_or(0, |cell| cell.column),
                ..cursor
            }
        }
        CopyMotion::EndOfLine => SelectionPoint {
            column: cells(line_text(lines, cursor.line))
                .last()
                .map_or(0, |cell| cell.column),
            ..cursor
        },
        CopyMotion::HistoryTop => SelectionPoint { line: 0, column: 0 },
        CopyMotion::HistoryBottom => SelectionPoint {
            line: last_line,
            column: 0,
        },
        CopyMotion::TopLine => SelectionPoint {
            line: top,
            column: 0,
        },
        CopyMotion::MiddleLine => SelectionPoint {
            line: top
                .saturating_add(viewport.rows.max(1).saturating_sub(1) / 2)
                .min(last_line),
            column: 0,
        },
        CopyMotion::BottomLine => SelectionPoint {
            line: top
                .saturating_add(viewport.rows.max(1).saturating_sub(1))
                .min(last_line),
            column: 0,
        },
        CopyMotion::PreviousParagraph => previous_paragraph(lines, cursor),
        CopyMotion::NextParagraph => next_paragraph(lines, last_line, cursor),
        CopyMotion::HalfPageUp | CopyMotion::PageUp => {
            let delta = page_delta(motion, viewport.rows);
            top = top.saturating_sub(delta);
            SelectionPoint {
                line: cursor.line.saturating_sub(delta),
                ..cursor
            }
        }
        CopyMotion::HalfPageDown | CopyMotion::PageDown => {
            let delta = page_delta(motion, viewport.rows);
            top = top.saturating_add(delta).min(viewport.bottom_top());
            SelectionPoint {
                line: cursor.line.saturating_add(delta).min(last_line),
                ..cursor
            }
        }
    };
    MotionOutcome {
        cursor,
        top: viewport.reveal(top, cursor.line),
    }
}

fn page_delta(motion: CopyMotion, rows: u64) -> u64 {
    let rows = rows.max(1);
    match motion {
        CopyMotion::HalfPageUp | CopyMotion::HalfPageDown => (rows / 2).max(1),
        _ => rows,
    }
}

/// Apply a character jump on the cursor line, returning the new column or
/// `None` when the character does not occur. `repeat` is set for `;`/`,` so
/// that a repeated `t`/`T` moves past the character it already stops beside.
pub fn jump(
    kind: JumpKind,
    target: char,
    cursor: SelectionPoint,
    line: &str,
    repeat: bool,
) -> Option<u16> {
    let cells = cells(line);
    let mut buffer = [0; 4];
    let target = &*target.encode_utf8(&mut buffer);
    let index = cell_index(&cells, cursor.column).min(cells.len());
    let skip = usize::from(repeat);
    let is_target = |i: &usize| cells[*i].text == target;
    let found = match kind {
        JumpKind::Forward => (index + 1..cells.len()).find(is_target),
        JumpKind::ToForward => (index + 1 + skip..cells.len())
            .find(is_target)
            .map(|i| i - 1),
        JumpKind::Backward => (0..index).rev().find(is_target),
        JumpKind::ToBackward => (0..index.saturating_sub(skip))
            .rev()
            .find(is_target)
            .map(|i| i + 1),
    }?;
    Some(cells[found].column)
}

/// The first and last columns of the word under `column`, as selected by a
/// double click. Runs of whitespace and of separators count as words too, as
/// in tmux. `None` when the column is past the end of the line.
pub fn word_bounds(line: &str, column: u16) -> Option<(u16, u16)> {
    let cells = cells(line);
    let index = cell_index(&cells, column);
    let class = class_of(cells.get(index)?.text, false);
    let mut start = index;
    while start > 0 && class_of(cells[start - 1].text, false) == class {
        start -= 1;
    }
    let mut end = index;
    while end + 1 < cells.len() && class_of(cells[end + 1].text, false) == class {
        end += 1;
    }
    Some((cells[start].column, cells[end].column))
}

/// Index of the search match `n`/`N` should land on from `cursor`: the first
/// match after it (forward) or the last before it (backward), wrapping around
/// the buffer.
pub fn next_match_index(
    matches: &[SearchMatch],
    cursor: SelectionPoint,
    forward: bool,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let position = |found: &SearchMatch| (found.line, found.start_column);
    let cursor = (cursor.line, cursor.column);
    if forward {
        Some(
            matches
                .iter()
                .position(|found| position(found) > cursor)
                .unwrap_or(0),
        )
    } else {
        Some(
            matches
                .iter()
                .rposition(|found| position(found) < cursor)
                .unwrap_or(matches.len() - 1),
        )
    }
}

fn line_text(lines: &[String], line: u64) -> &str {
    usize::try_from(line)
        .ok()
        .and_then(|line| lines.get(line))
        .map_or("", String::as_str)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell<'a> {
    column: u16,
    width: u16,
    text: &'a str,
}

fn cells(line: &str) -> Vec<Cell<'_>> {
    let mut column = 0_u16;
    line.graphemes(true)
        .map(|text| {
            let width = u16::try_from(UnicodeWidthStr::width(text))
                .unwrap_or(u16::MAX)
                .max(1);
            let cell = Cell {
                column,
                width,
                text,
            };
            column = column.saturating_add(width);
            cell
        })
        .collect()
}

/// The cell covering `column`, or `cells.len()` when it lies past the end.
fn cell_index(cells: &[Cell<'_>], column: u16) -> usize {
    cells
        .iter()
        .position(|cell| column < cell.column.saturating_add(cell.width))
        .unwrap_or(cells.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Blank,
    Separator,
    Word,
}

fn class_of(text: &str, space_separated: bool) -> CharClass {
    let Some(ch) = text.chars().next() else {
        return CharClass::Blank;
    };
    if ch.is_whitespace() {
        CharClass::Blank
    } else if !space_separated && ch.is_ascii_punctuation() && ch != '_' {
        CharClass::Separator
    } else {
        CharClass::Word
    }
}

/// Walks text cell by cell across lines. Each line ends in a virtual position
/// past its last cell that reads as whitespace, so words never continue across
/// a line break.
struct Walker<'a> {
    lines: &'a [String],
    last_line: u64,
    space_separated: bool,
    line: u64,
    cells: Vec<Cell<'a>>,
    index: usize,
}

impl<'a> Walker<'a> {
    fn new(lines: &'a [String], last_line: u64, at: SelectionPoint, space_separated: bool) -> Self {
        let line = at.line.min(last_line);
        let cells = cells(line_text(lines, line));
        let index = cell_index(&cells, at.column);
        Self {
            lines,
            last_line,
            space_separated,
            line,
            cells,
            index,
        }
    }

    fn class(&self) -> CharClass {
        self.class_at(self.index)
    }

    fn class_at(&self, index: usize) -> CharClass {
        self.cells.get(index).map_or(CharClass::Blank, |cell| {
            class_of(cell.text, self.space_separated)
        })
    }

    fn on_empty_line(&self) -> bool {
        self.cells.is_empty()
    }

    fn load(&mut self, line: u64) {
        self.line = line;
        self.cells = cells(line_text(self.lines, line));
    }

    fn forward(&mut self) -> bool {
        if self.index < self.cells.len() {
            self.index += 1;
            true
        } else if self.line < self.last_line {
            self.load(self.line + 1);
            self.index = 0;
            true
        } else {
            false
        }
    }

    fn backward(&mut self) -> bool {
        if self.index > 0 {
            self.index -= 1;
            true
        } else if self.line > 0 {
            self.load(self.line - 1);
            self.index = self.cells.len();
            true
        } else {
            false
        }
    }

    /// The current position, moved onto the last character when it is the
    /// virtual end of a non-empty line.
    fn point(&self) -> SelectionPoint {
        let index = self.index.min(self.cells.len().saturating_sub(1));
        SelectionPoint {
            line: self.line,
            column: self.cells.get(index).map_or(0, |cell| cell.column),
        }
    }
}

fn next_word_start(
    lines: &[String],
    last_line: u64,
    at: SelectionPoint,
    space_separated: bool,
) -> SelectionPoint {
    let mut walker = Walker::new(lines, last_line, at, space_separated);
    let start = walker.class();
    if !walker.forward() {
        return walker.point();
    }
    if start != CharClass::Blank {
        while walker.class() == start {
            if !walker.forward() {
                return walker.point();
            }
        }
    }
    // An empty line is a word of its own, as in vim.
    while walker.class() == CharClass::Blank && !walker.on_empty_line() {
        if !walker.forward() {
            return walker.point();
        }
    }
    walker.point()
}

fn next_word_end(
    lines: &[String],
    last_line: u64,
    at: SelectionPoint,
    space_separated: bool,
) -> SelectionPoint {
    let mut walker = Walker::new(lines, last_line, at, space_separated);
    if !walker.forward() {
        return walker.point();
    }
    while walker.class() == CharClass::Blank {
        if !walker.forward() {
            return walker.point();
        }
    }
    let class = walker.class();
    while walker.class_at(walker.index + 1) == class {
        walker.index += 1;
    }
    walker.point()
}

fn previous_word_start(
    lines: &[String],
    last_line: u64,
    at: SelectionPoint,
    space_separated: bool,
) -> SelectionPoint {
    let mut walker = Walker::new(lines, last_line, at, space_separated);
    if !walker.backward() {
        return walker.point();
    }
    while walker.class() == CharClass::Blank && !walker.on_empty_line() {
        if !walker.backward() {
            return walker.point();
        }
    }
    if walker.on_empty_line() {
        return walker.point();
    }
    let class = walker.class();
    while walker.index > 0 && walker.class_at(walker.index - 1) == class {
        walker.index -= 1;
    }
    walker.point()
}

/// Whether a line separates paragraphs. Terminal rows pad with spaces, so a
/// line of only whitespace counts as blank.
fn is_blank_line(lines: &[String], line: u64) -> bool {
    line_text(lines, line).trim().is_empty()
}

/// `{`: the nearest blank line above the cursor that follows text, as in vim.
/// Without one, the start of the first line.
fn previous_paragraph(lines: &[String], cursor: SelectionPoint) -> SelectionPoint {
    let mut line = cursor.line;
    while line > 0 && is_blank_line(lines, line) {
        line -= 1;
    }
    while line > 0 {
        line -= 1;
        if is_blank_line(lines, line) {
            return SelectionPoint { line, column: 0 };
        }
    }
    SelectionPoint { line: 0, column: 0 }
}

/// `}`: the nearest blank line below the cursor that follows text, as in vim.
/// Without one, the last character of the last line.
fn next_paragraph(lines: &[String], last_line: u64, cursor: SelectionPoint) -> SelectionPoint {
    let mut line = cursor.line;
    while line < last_line && is_blank_line(lines, line) {
        line += 1;
    }
    while line < last_line {
        line += 1;
        if is_blank_line(lines, line) {
            return SelectionPoint { line, column: 0 };
        }
    }
    SelectionPoint {
        line: last_line,
        column: cells(line_text(lines, last_line))
            .last()
            .map_or(0, |cell| cell.column),
    }
}

/// Detects double and triple clicks: repeated presses on the same cell of the
/// same pane, each within [`MULTI_CLICK_INTERVAL`] of the last.
#[derive(Clone, Debug, Default)]
pub struct ClickTracker {
    last: Option<(NodeId, SelectionPoint, Instant, u8)>,
}

impl ClickTracker {
    /// Record a press and return its click count: 1, 2 or 3. A fourth quick
    /// press starts over at 1.
    pub fn press(&mut self, node_id: NodeId, point: SelectionPoint, now: Instant) -> u8 {
        let count = match self.last {
            Some((last_node, last_point, at, count))
                if last_node == node_id
                    && last_point == point
                    && count < 3
                    && now.saturating_duration_since(at) <= MULTI_CLICK_INTERVAL =>
            {
                count + 1
            }
            _ => 1,
        };
        self.last = Some((node_id, point, now, count));
        count
    }
}

/// Buffer text captured for motions, reused until the buffer changes.
#[derive(Clone, Debug)]
pub(crate) struct CapturedText {
    pub buffer_id: BufferId,
    pub sequence: u64,
    pub lines: Vec<String>,
}

/// A left-button press that may become a drag selection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MouseSelection {
    pub node_id: NodeId,
    pub anchor: SelectionPoint,
    pub dragging: bool,
}

/// Client-wide copy-mode bookkeeping that is not tied to one view.
#[derive(Clone, Debug, Default)]
pub(crate) struct CopyModeState {
    /// Count typed so far, waiting for the binding it applies to.
    pub pending_count: Option<u32>,
    /// Count of the binding currently running; the first motion takes it.
    pub active_count: Option<u32>,
    /// `f`/`F`/`t`/`T` waiting for its target character, with its count.
    pub pending_jump: Option<(JumpKind, u32)>,
    /// Last character jump, for `;` and `,`.
    pub last_jump: Option<(JumpKind, char)>,
    /// Whether the last search was backward (`?`), which reverses `n`/`N`.
    pub search_backward: bool,
    pub text: Option<CapturedText>,
    pub clicks: ClickTracker,
    pub mouse: Option<MouseSelection>,
    /// Copy mode began by scrolling back, so scrolling to the bottom again
    /// leaves it, as tmux's `copy-mode -e` does.
    pub exit_at_bottom: bool,
}

impl CopyModeState {
    pub fn push_count_digit(&mut self, digit: u32) {
        let count = self
            .pending_count
            .unwrap_or(0)
            .saturating_mul(10)
            .saturating_add(digit);
        self.pending_count = Some(count.min(MAX_COUNT));
    }

    pub fn take_count(&mut self) -> u32 {
        self.active_count.take().unwrap_or(1).max(1)
    }

    /// Forget counts and pending jumps, e.g. when leaving copy mode.
    pub fn clear_pending(&mut self) {
        self.pending_count = None;
        self.active_count = None;
        self.pending_jump = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use embers_core::NodeId;

    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    fn point(line: u64, column: u16) -> SelectionPoint {
        SelectionPoint { line, column }
    }

    fn viewport(lines: &[String]) -> CopyViewport {
        CopyViewport {
            top: 0,
            rows: lines.len() as u64,
            total: lines.len() as u64,
            cols: 80,
        }
    }

    fn run(text: &[String], motion: CopyMotion, from: SelectionPoint) -> (u64, u16) {
        let outcome = apply_motion(motion, from, viewport(text), text);
        (outcome.cursor.line, outcome.cursor.column)
    }

    #[test]
    fn word_motions_treat_separators_as_their_own_words() {
        let text = lines(&["let x=foo(bar_baz);"]);
        let mut at = point(0, 0);
        let mut stops = Vec::new();
        for _ in 0..7 {
            at = apply_motion(CopyMotion::NextWord, at, viewport(&text), &text).cursor;
            stops.push(at.column);
        }
        // let | x | = | foo | ( | bar_baz | ); then the final character.
        assert_eq!(stops, vec![4, 5, 6, 9, 10, 17, 18]);
    }

    #[test]
    fn paragraph_motions_stop_on_blank_lines_between_text() {
        let text = lines(&["one", "two", "", "   ", "three", "", "four"]);
        assert_eq!(run(&text, CopyMotion::NextParagraph, point(0, 1)), (2, 0));
        // From a blank line, skip the blank run and the paragraph after it.
        assert_eq!(run(&text, CopyMotion::NextParagraph, point(2, 0)), (5, 0));
        assert_eq!(
            run(&text, CopyMotion::PreviousParagraph, point(6, 2)),
            (5, 0)
        );
        assert_eq!(
            run(&text, CopyMotion::PreviousParagraph, point(5, 0)),
            (3, 0)
        );
    }

    #[test]
    fn paragraph_motions_stop_at_the_ends_without_a_blank_line() {
        let text = lines(&["one", "two", "three"]);
        assert_eq!(run(&text, CopyMotion::NextParagraph, point(0, 0)), (2, 4));
        assert_eq!(
            run(&text, CopyMotion::PreviousParagraph, point(2, 3)),
            (0, 0)
        );
    }

    #[test]
    fn word_end_and_previous_word_cross_lines() {
        let text = lines(&["alpha beta", "  gamma"]);
        assert_eq!(run(&text, CopyMotion::NextWordEnd, point(0, 6)), (0, 9));
        assert_eq!(run(&text, CopyMotion::NextWordEnd, point(0, 9)), (1, 6));
        assert_eq!(run(&text, CopyMotion::PreviousWord, point(1, 2)), (0, 6));
        assert_eq!(run(&text, CopyMotion::PreviousWord, point(0, 0)), (0, 0));
    }

    #[test]
    fn previous_word_stops_at_empty_lines() {
        let text = lines(&["one", "", "two"]);
        assert_eq!(run(&text, CopyMotion::PreviousWord, point(2, 0)), (1, 0));
        assert_eq!(run(&text, CopyMotion::PreviousWord, point(1, 0)), (0, 0));
    }

    #[test]
    fn word_end_skips_empty_lines() {
        let text = lines(&["one", "", "two"]);
        assert_eq!(run(&text, CopyMotion::NextWordEnd, point(0, 2)), (2, 2));
    }

    #[test]
    fn space_separated_words_include_punctuation() {
        let text = lines(&["a.b c-d"]);
        assert_eq!(run(&text, CopyMotion::NextSpace, point(0, 0)), (0, 4));
        assert_eq!(run(&text, CopyMotion::NextSpaceEnd, point(0, 0)), (0, 2));
        assert_eq!(run(&text, CopyMotion::PreviousSpace, point(0, 6)), (0, 4));
    }

    #[test]
    fn word_motions_use_display_columns_for_wide_characters() {
        let text = lines(&["日本 語x"]);
        assert_eq!(run(&text, CopyMotion::NextWord, point(0, 0)), (0, 5));
        assert_eq!(run(&text, CopyMotion::NextWordEnd, point(0, 0)), (0, 2));
        assert_eq!(run(&text, CopyMotion::EndOfLine, point(0, 0)), (0, 7));
    }

    #[test]
    fn cursor_past_the_end_of_a_line_moves_to_the_next_word() {
        let text = lines(&["ab", "cd"]);
        assert_eq!(run(&text, CopyMotion::NextWord, point(0, 40)), (1, 0));
        assert_eq!(run(&text, CopyMotion::PreviousWord, point(0, 40)), (0, 0));
    }

    #[test]
    fn line_motions_handle_blank_lines() {
        let text = lines(&["   ", ""]);
        assert_eq!(
            run(&text, CopyMotion::BackToIndentation, point(0, 2)),
            (0, 0)
        );
        assert_eq!(run(&text, CopyMotion::EndOfLine, point(1, 3)), (1, 0));
    }

    #[test]
    fn right_motion_stops_at_the_terminal_width() {
        let text = lines(&["x"]);
        let viewport = CopyViewport {
            cols: 3,
            ..viewport(&text)
        };
        let cursor = apply_motion(CopyMotion::Right, point(0, 2), viewport, &text).cursor;
        assert_eq!(cursor, point(0, 2));
    }

    #[test]
    fn page_motions_scroll_with_the_cursor_and_clamp() {
        let text = vec![String::new(); 20];
        let viewport = CopyViewport {
            top: 10,
            rows: 5,
            total: 20,
            cols: 80,
        };
        let up = apply_motion(CopyMotion::HalfPageUp, point(12, 3), viewport, &text);
        assert_eq!((up.cursor, up.top), (point(10, 3), 8));
        let down = apply_motion(CopyMotion::PageDown, point(12, 3), viewport, &text);
        assert_eq!((down.cursor, down.top), (point(17, 3), 15));
        let top = apply_motion(
            CopyMotion::PageUp,
            point(1, 0),
            CopyViewport { top: 0, ..viewport },
            &text,
        );
        assert_eq!((top.cursor, top.top), (point(0, 0), 0));
    }

    #[test]
    fn motions_scroll_just_enough_to_reveal_the_cursor() {
        let text = vec![String::new(); 20];
        let viewport = CopyViewport {
            top: 10,
            rows: 5,
            total: 20,
            cols: 80,
        };
        assert_eq!(
            apply_motion(CopyMotion::Up, point(10, 0), viewport, &text).top,
            9
        );
        assert_eq!(
            apply_motion(CopyMotion::Down, point(14, 0), viewport, &text).top,
            11
        );
        assert_eq!(
            apply_motion(CopyMotion::HistoryBottom, point(10, 0), viewport, &text).top,
            15
        );
        assert_eq!(
            apply_motion(CopyMotion::MiddleLine, point(10, 0), viewport, &text).cursor,
            point(12, 0)
        );
    }

    #[test]
    fn jumps_find_characters_on_the_cursor_line() {
        let line = "a,b,c";
        let at = point(0, 0);
        assert_eq!(jump(JumpKind::Forward, ',', at, line, false), Some(1));
        assert_eq!(jump(JumpKind::ToForward, 'c', at, line, false), Some(3));
        assert_eq!(jump(JumpKind::Forward, 'z', at, line, false), None);
        let at = point(0, 4);
        assert_eq!(jump(JumpKind::Backward, ',', at, line, false), Some(3));
        assert_eq!(jump(JumpKind::ToBackward, 'a', at, line, false), Some(1));
        // Repeating `T` from just after a match moves on to the next one.
        assert_eq!(
            jump(JumpKind::ToBackward, ',', point(0, 4), line, true),
            Some(2)
        );
        assert_eq!(
            jump(JumpKind::ToBackward, ',', point(0, 4), line, false),
            Some(4)
        );
    }

    #[test]
    fn word_bounds_cover_the_run_under_the_column() {
        assert_eq!(word_bounds("say hello_there!", 6), Some((4, 14)));
        assert_eq!(word_bounds("say hello_there!", 15), Some((15, 15)));
        assert_eq!(word_bounds("a   b", 2), Some((1, 3)));
        assert_eq!(word_bounds("short", 9), None);
    }

    #[test]
    fn next_match_index_wraps_in_both_directions() {
        let matches = [
            SearchMatch {
                line: 1,
                start_column: 0,
                end_column: 1,
            },
            SearchMatch {
                line: 3,
                start_column: 2,
                end_column: 3,
            },
        ];
        assert_eq!(next_match_index(&matches, point(1, 0), true), Some(1));
        assert_eq!(next_match_index(&matches, point(3, 2), true), Some(0));
        assert_eq!(next_match_index(&matches, point(3, 2), false), Some(0));
        assert_eq!(next_match_index(&matches, point(1, 0), false), Some(1));
        assert_eq!(next_match_index(&[], point(0, 0), true), None);
    }

    #[test]
    fn click_tracker_counts_quick_presses_on_one_cell() {
        let mut tracker = ClickTracker::default();
        let node = NodeId(1);
        let start = Instant::now();
        let quick = Duration::from_millis(100);
        assert_eq!(tracker.press(node, point(0, 0), start), 1);
        assert_eq!(tracker.press(node, point(0, 0), start + quick), 2);
        assert_eq!(tracker.press(node, point(0, 0), start + quick * 2), 3);
        assert_eq!(tracker.press(node, point(0, 0), start + quick * 3), 1);
        // Too slow, or a different cell, starts a new click.
        assert_eq!(
            tracker.press(node, point(0, 0), start + Duration::from_secs(1)),
            1
        );
        let later = start + Duration::from_secs(1);
        assert_eq!(tracker.press(node, point(0, 1), later), 1);
        assert_eq!(tracker.press(NodeId(2), point(0, 1), later), 1);
    }

    #[test]
    fn count_digits_accumulate_and_saturate() {
        let mut state = CopyModeState::default();
        state.push_count_digit(1);
        state.push_count_digit(0);
        assert_eq!(state.pending_count, Some(10));
        for _ in 0..8 {
            state.push_count_digit(9);
        }
        assert_eq!(state.pending_count, Some(MAX_COUNT));
        state.active_count = state.pending_count.take();
        assert_eq!(state.take_count(), MAX_COUNT);
        assert_eq!(state.take_count(), 1);
    }
}
