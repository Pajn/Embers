use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, LineLength};
use alacritty_terminal::term::{Config, LineDamageBounds, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{
    self, Color as AnsiColor, CursorShape as AlacrittyCursorShape, NamedColor,
};
use embers_core::{
    ActivityState, CellAttrs, CursorPosition, CursorShape, CursorState, PtySize, SnapshotLine,
    StyledRun, TermColor, TerminalModes, TerminalSnapshot,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendMetadata {
    pub title: Option<String>,
    pub viewport_top_line: u64,
    pub total_lines: u64,
    pub alternate_screen: bool,
    pub mouse_reporting: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub cursor: Option<CursorState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendScrollbackSlice {
    pub start_line: u64,
    pub total_lines: u64,
    pub lines: Vec<SnapshotLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendDamage {
    None,
    Full,
    Partial(Vec<LineDamageBounds>),
}

/// Terminal emulation boundary used by the runtime keeper.
///
/// Raw PTY bytes are routed through `RawByteRouter` and then ingested here. The backend owns
/// terminal parsing, alternate-screen state, scrollback, snapshots, cursor metadata, and render
/// damage tracking.
pub trait TerminalBackend: Send {
    fn ingest_bytes(&mut self, bytes: &[u8]);
    fn resize(&mut self, size: PtySize);
    fn visible_snapshot(
        &self,
        sequence: u64,
        size: PtySize,
        cwd: Option<PathBuf>,
    ) -> TerminalSnapshot;
    fn capture_scrollback(&self) -> Vec<String>;
    fn capture_scrollback_slice(&self, start_line: u64, line_count: u32) -> BackendScrollbackSlice;
    fn metadata(&self) -> BackendMetadata;
    /// Drain, under a single lock acquisition, the pending activity state and any
    /// terminal replies the emulator generated while ingesting bytes
    /// (device-attribute, cursor-position, and mode queries). The replies are
    /// written back to the PTY master; the returned `Vec` is empty when there is
    /// nothing pending. Combining the two drains keeps the read hot path to one
    /// lock acquisition against the client-thread readers of the same state.
    fn take_events(&mut self) -> (ActivityState, Vec<u8>);
    fn take_damage(&mut self) -> BackendDamage;
}

#[derive(Clone, Debug, Default)]
pub struct RawByteRouter {
    osc7: Osc7Scanner,
}

impl RawByteRouter {
    /// Route client-originated bytes before they reach the PTY.
    ///
    /// The current implementation is intentionally passthrough, but the method is the explicit
    /// seam for future prefix/passthrough-aware interception.
    pub fn route_input(&self, bytes: Vec<u8>) -> Vec<u8> {
        bytes
    }

    /// Route PTY output bytes before terminal emulation.
    ///
    /// Output is forwarded directly into the backend (alacritty drops OSC 7), while a
    /// side scanner sniffs OSC 7 `file://host/path` working-directory reports so the
    /// keeper can report the shell's live cwd the way tmux does.
    pub fn route_output(&mut self, backend: &mut dyn TerminalBackend, bytes: &[u8]) {
        self.osc7.feed(bytes);
        backend.ingest_bytes(bytes);
    }

    /// The most recent working directory reported by the inner shell via OSC 7, if any.
    pub fn reported_cwd(&self) -> Option<PathBuf> {
        self.osc7.cwd.clone()
    }
}

/// Streaming scanner for OSC 7 (`ESC ] 7 ; file://<host><path> (BEL | ESC \)`) working
/// directory reports. Sequences may split across read chunks, so state is retained
/// between `feed` calls. Only OSC sequences whose numeric prefix is `7` are buffered;
/// any other OSC payload (titles, large OSC 52 clipboard blobs, …) is skipped without
/// accumulation.
#[derive(Clone, Debug, Default)]
struct Osc7Scanner {
    state: Osc7State,
    buf: Vec<u8>,
    cwd: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Osc7State {
    /// Scanning ordinary output for the ESC that could begin a sequence.
    #[default]
    Ground,
    /// Saw ESC; expecting `]` to enter an OSC.
    Escape,
    /// Inside an OSC whose prefix is (still possibly) `7`; buffering the payload.
    Collect,
    /// Inside `7;` OSC and saw ESC; a following `\` terminates (ST).
    CollectEscape,
    /// Inside a non-`7` OSC; discarding until the terminator.
    Skip,
    /// Inside a skipped OSC and saw ESC; a following `\` terminates (ST).
    SkipEscape,
}

/// Upper bound on a buffered OSC 7 payload; a real cwd URI is far shorter, and this
/// caps memory if a malformed sequence never terminates.
const OSC7_MAX_PAYLOAD: usize = 4096;

impl Osc7Scanner {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    fn step(&mut self, byte: u8) {
        const ESC: u8 = 0x1b;
        const BEL: u8 = 0x07;
        match self.state {
            Osc7State::Ground => {
                if byte == ESC {
                    self.state = Osc7State::Escape;
                }
            }
            Osc7State::Escape => {
                if byte == b']' {
                    self.buf.clear();
                    self.state = Osc7State::Collect;
                } else if byte == ESC {
                    // Stay armed on a run of ESCs.
                } else {
                    self.state = Osc7State::Ground;
                }
            }
            Osc7State::Collect => match byte {
                BEL => {
                    self.finish();
                    self.state = Osc7State::Ground;
                }
                ESC => self.state = Osc7State::CollectEscape,
                _ => {
                    // OSC 7 begins with the single digit `7`; anything else is a
                    // different OSC we don't care about. Cap the payload so a
                    // malformed, never-terminated sequence can't grow unbounded.
                    if (self.buf.is_empty() && byte != b'7') || self.buf.len() >= OSC7_MAX_PAYLOAD {
                        self.state = Osc7State::Skip;
                    } else {
                        self.buf.push(byte);
                    }
                }
            },
            Osc7State::CollectEscape => {
                if byte == b'\\' {
                    self.finish();
                }
                self.state = Osc7State::Ground;
            }
            Osc7State::Skip => match byte {
                BEL => self.state = Osc7State::Ground,
                ESC => self.state = Osc7State::SkipEscape,
                _ => {}
            },
            Osc7State::SkipEscape => {
                self.state = Osc7State::Ground;
            }
        }
    }

    /// Parse a completed `7;file://host/path` payload and store the decoded path.
    fn finish(&mut self) {
        let payload = match self.buf.strip_prefix(b"7;") {
            Some(rest) => rest,
            None => return,
        };
        let Some(rest) = payload.strip_prefix(b"file://") else {
            return;
        };
        // Skip the authority (hostname) up to the first path separator.
        let path_start = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
        let path_bytes = &rest[path_start..];
        if path_bytes.is_empty() {
            return;
        }
        let decoded = percent_decode(path_bytes);
        if let Ok(text) = String::from_utf8(decoded) {
            self.cwd = Some(PathBuf::from(text));
        }
    }
}

/// Percent-decode a byte slice (`%XX` escapes) as used by `file://` URIs.
fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = (bytes[index + 1] as char).to_digit(16);
            let lo = (bytes[index + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

pub struct AlacrittyTerminalBackend {
    term: Term<BackendEventProxy>,
    parser: ansi::Processor,
    events: Arc<Mutex<BackendEventState>>,
}

impl std::fmt::Debug for AlacrittyTerminalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AlacrittyTerminalBackend")
            .field("metadata", &self.metadata())
            .finish()
    }
}

#[derive(Clone, Debug)]
struct BackendEventProxy {
    state: Arc<Mutex<BackendEventState>>,
}

#[derive(Clone, Debug, Default)]
struct BackendEventState {
    title: Option<String>,
    bell_pending: bool,
    pty_write: Vec<u8>,
}

impl BackendEventProxy {
    fn new(state: Arc<Mutex<BackendEventState>>) -> Self {
        Self { state }
    }
}

impl EventListener for BackendEventProxy {
    fn send_event(&self, event: Event) {
        // Recover from a poisoned lock rather than dropping the update: the event
        // state is plain data, and silently skipping title/bell writes would
        // desync from the metadata/take_activity readers, which both recover.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match event {
            Event::Title(title) => state.title = Some(title),
            Event::ResetTitle => state.title = None,
            Event::Bell => state.bell_pending = true,
            Event::PtyWrite(text) => state.pty_write.extend_from_slice(text.as_bytes()),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BackendSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for BackendSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl AlacrittyTerminalBackend {
    /// `max_scrollback_lines` is the per-buffer scrollback ceiling, resolved by
    /// the caller (the runtime keeper) so this emulation layer stays independent
    /// of server configuration and env.
    pub fn new(size: PtySize, max_scrollback_lines: usize) -> Self {
        let events = Arc::new(Mutex::new(BackendEventState::default()));
        let dimensions = BackendSize {
            columns: size.cols as usize,
            screen_lines: size.rows as usize,
        };
        let config = Config {
            scrolling_history: max_scrollback_lines,
            ..Config::default()
        };

        Self {
            term: Term::new(config, &dimensions, BackendEventProxy::new(events.clone())),
            parser: ansi::Processor::new(),
            events,
        }
    }

    fn visible_lines(&self) -> Vec<SnapshotLine> {
        let grid = self.term.grid();
        let display_offset = grid.display_offset() as i32;
        let top = Line(-display_offset);
        let bottom = Line(grid.screen_lines() as i32 - display_offset - 1);
        self.collect_styled_lines(top, bottom)
    }

    /// Full history + screen as plain text.
    ///
    /// Uses the same cell-walk text projection as [`Self::styled_line`] (tabs to
    /// spaces, spacer cells skipped) so search columns computed over captures
    /// agree with the styled lines the client displays. Run building is skipped:
    /// full capture stays plain text.
    fn all_lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        if grid.columns() == 0 {
            return Vec::new();
        }
        let top = Line(-(grid.history_size() as i32));
        let bottom = Line(grid.screen_lines() as i32 - 1);
        let mut lines = Vec::new();
        let mut line = top;
        while line <= bottom {
            lines.push(self.line_text(line));
            line += 1;
        }
        lines
    }

    fn collect_styled_lines(&self, start: Line, end: Line) -> Vec<SnapshotLine> {
        let grid = self.term.grid();
        if grid.columns() == 0 || end < start {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut line = start;
        while line <= end {
            lines.push(self.styled_line(line));
            line += 1;
        }
        lines
    }

    /// Plain-text projection of a single row (no run building).
    fn line_text(&self, line: Line) -> String {
        let row = &self.term.grid()[line];
        let content_len = content_length(row);
        let mut text = String::new();
        for column in 0..content_len {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            emit_cell_text(cell, &mut text);
        }
        text
    }

    /// Walk one grid row into a styled line.
    ///
    /// Column count comes from `row.len()` (alacritty reflows history on resize,
    /// so a cached `columns()` can disagree with a history row). Trailing painted
    /// cells (non-default background or inverse) survive as styled spaces; default
    /// trailing blanks are trimmed so the plain-text projection matches the legacy
    /// extraction.
    fn styled_line(&self, line: Line) -> SnapshotLine {
        let row = &self.term.grid()[line];
        let content_len = content_length(row);

        let mut text = String::new();
        let mut runs: Vec<StyledRun> = Vec::new();
        let mut styled = false;

        for column in 0..content_len {
            let cell = &row[Column(column)];
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            let fg = map_color(cell.fg);
            let bg = map_color(cell.bg);
            let attrs = map_attrs(flags);

            // `emit_cell_text` always writes at least one byte (a char or a
            // space), so every cell contributes to exactly one run.
            let start = text.len();
            emit_cell_text(cell, &mut text);
            let byte_len = u32::try_from(text.len() - start).unwrap_or(u32::MAX);

            if fg != TermColor::Default || bg != TermColor::Default || attrs != CellAttrs::empty() {
                styled = true;
            }

            match runs.last_mut() {
                Some(last) if last.fg == fg && last.bg == bg && last.attrs == attrs => {
                    last.len += byte_len;
                }
                _ => runs.push(StyledRun {
                    len: byte_len,
                    fg,
                    bg,
                    attrs,
                }),
            }
        }

        // A line that is entirely default-styled coalesces to a single default
        // run; drop it so the on-wire invariant is "empty runs = plain line".
        if !styled {
            runs.clear();
        }

        SnapshotLine { text, runs }
    }

    fn cursor_state(&self) -> Option<CursorState> {
        let cursor = self.term.renderable_content().cursor;
        let shape = match cursor.shape {
            AlacrittyCursorShape::Hidden => return None,
            AlacrittyCursorShape::Block | AlacrittyCursorShape::HollowBlock => CursorShape::Block,
            AlacrittyCursorShape::Underline => CursorShape::Underline,
            AlacrittyCursorShape::Beam => CursorShape::Beam,
        };
        let row = u16::try_from(cursor.point.line.0).ok()?;
        let col = u16::try_from(cursor.point.column.0).ok()?;
        Some(CursorState {
            position: CursorPosition { row, col },
            shape,
        })
    }

    fn terminal_modes(&self) -> TerminalModes {
        let mode = *self.term.mode();
        TerminalModes {
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            mouse_reporting: mode.intersects(
                TermMode::MOUSE_REPORT_CLICK
                    | TermMode::MOUSE_DRAG
                    | TermMode::MOUSE_MOTION
                    | TermMode::SGR_MOUSE
                    | TermMode::UTF8_MOUSE,
            ),
            focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        }
    }

    fn viewport_top_line(&self) -> u64 {
        let grid = self.term.grid();
        grid.history_size().saturating_sub(grid.display_offset()) as u64
    }

    fn total_lines(&self) -> u64 {
        let grid = self.term.grid();
        (grid.history_size() + grid.screen_lines()) as u64
    }
}

impl TerminalBackend for AlacrittyTerminalBackend {
    fn ingest_bytes(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn resize(&mut self, size: PtySize) {
        self.term.resize(BackendSize {
            columns: size.cols as usize,
            screen_lines: size.rows as usize,
        });
    }

    fn visible_snapshot(
        &self,
        sequence: u64,
        size: PtySize,
        cwd: Option<PathBuf>,
    ) -> TerminalSnapshot {
        let metadata = self.metadata();
        TerminalSnapshot {
            sequence,
            size,
            cursor: metadata.cursor,
            lines: self.visible_lines(),
            title: metadata.title,
            cwd,
            viewport_top_line: metadata.viewport_top_line,
            total_lines: metadata.total_lines,
            modes: TerminalModes {
                alternate_screen: metadata.alternate_screen,
                mouse_reporting: metadata.mouse_reporting,
                focus_reporting: metadata.focus_reporting,
                bracketed_paste: metadata.bracketed_paste,
            },
        }
    }

    fn capture_scrollback(&self) -> Vec<String> {
        let mut lines = self.all_lines();
        while matches!(lines.last(), Some(last) if last.is_empty()) {
            lines.pop();
        }
        lines
    }

    fn capture_scrollback_slice(&self, start_line: u64, line_count: u32) -> BackendScrollbackSlice {
        let grid = self.term.grid();
        if grid.columns() == 0 {
            return BackendScrollbackSlice::default();
        }

        // Global line 0 is the top of history; global `history_size` is the first
        // screen row. This mirrors the `all_lines` mapping but only walks the
        // requested window instead of the whole history.
        let history = grid.history_size() as i64;
        let total_lines = (grid.history_size() + grid.screen_lines()) as u64;
        let start_line = start_line.min(total_lines);
        let end_line = start_line
            .saturating_add(u64::from(line_count))
            .min(total_lines);

        let mut lines = Vec::with_capacity((end_line - start_line) as usize);
        for global in start_line..end_line {
            let index = i32::try_from(global as i64 - history).unwrap_or(0);
            lines.push(self.styled_line(Line(index)));
        }

        BackendScrollbackSlice {
            start_line,
            total_lines,
            lines,
        }
    }

    fn metadata(&self) -> BackendMetadata {
        // Recover from a poisoned lock rather than crashing the backend: the
        // event state is plain data and a panic here would take down the buffer.
        let state = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let modes = self.terminal_modes();
        BackendMetadata {
            title: state.title.clone(),
            viewport_top_line: self.viewport_top_line(),
            total_lines: self.total_lines(),
            alternate_screen: modes.alternate_screen,
            mouse_reporting: modes.mouse_reporting,
            focus_reporting: modes.focus_reporting,
            bracketed_paste: modes.bracketed_paste,
            cursor: self.cursor_state(),
        }
    }

    fn take_events(&mut self) -> (ActivityState, Vec<u8>) {
        // Recover from a poisoned lock rather than crashing: the event state is
        // plain data, and dropping replies here would leave inner apps waiting on
        // query timeouts.
        let mut state = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let activity = if std::mem::take(&mut state.bell_pending) {
            ActivityState::Bell
        } else {
            ActivityState::Activity
        };
        (activity, std::mem::take(&mut state.pty_write))
    }

    fn take_damage(&mut self) -> BackendDamage {
        let damage = match self.term.damage() {
            TermDamage::Full => BackendDamage::Full,
            TermDamage::Partial(iter) => {
                let lines: Vec<_> = iter.collect();
                if lines.is_empty() {
                    BackendDamage::None
                } else {
                    BackendDamage::Partial(lines)
                }
            }
        };
        self.term.reset_damage();
        damage
    }
}

/// Occupied width of a row, extended to keep trailing painted cells.
///
/// `line_length` trims trailing spaces (default blanks), which is right for
/// plain text. But a trailing space with a non-default background or the inverse
/// flag is a painted cell the client must draw (vim/htop status bars), so extend
/// the length to the furthest painted column.
fn content_length(row: &Row<Cell>) -> usize {
    let columns = row.len();
    if columns == 0 {
        return 0;
    }
    let mut content_len = row.line_length().0;
    for column in (content_len..columns).rev() {
        if cell_paints_background(&row[Column(column)]) {
            content_len = column + 1;
            break;
        }
    }
    content_len
}

/// Whether a cell paints a visible background even when it holds no glyph.
fn cell_paints_background(cell: &Cell) -> bool {
    !matches!(cell.bg, AnsiColor::Named(NamedColor::Background))
        || cell.flags.contains(Flags::INVERSE)
}

/// Emit a cell's text into `text`.
///
/// Tabs become a single space (tab stops are private to the emulator and a raw
/// `\t` would break the client's column math). Wide-char spacer cells are the
/// caller's responsibility to skip; here we push the primary char plus any
/// zero-width combining marks. Hidden cells keep their char (the client blanks
/// them from the attribute).
fn emit_cell_text(cell: &Cell, text: &mut String) {
    if cell.c == '\t' {
        text.push(' ');
    } else {
        text.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth.iter().copied());
        }
    }
}

/// Map an emulator color to a semantic [`TermColor`].
///
/// Named/indexed colors stay indexed so the outer terminal's palette resolves
/// them; only true-color specs become RGB. Default fg/bg and cursor colors map
/// to [`TermColor::Default`].
fn map_color(color: AnsiColor) -> TermColor {
    match color {
        AnsiColor::Spec(rgb) => TermColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
        AnsiColor::Indexed(index) => TermColor::Indexed(index),
        AnsiColor::Named(named) => map_named_color(named),
    }
}

fn map_named_color(named: NamedColor) -> TermColor {
    match named {
        NamedColor::Black | NamedColor::DimBlack => TermColor::Indexed(0),
        NamedColor::Red | NamedColor::DimRed => TermColor::Indexed(1),
        NamedColor::Green | NamedColor::DimGreen => TermColor::Indexed(2),
        NamedColor::Yellow | NamedColor::DimYellow => TermColor::Indexed(3),
        NamedColor::Blue | NamedColor::DimBlue => TermColor::Indexed(4),
        NamedColor::Magenta | NamedColor::DimMagenta => TermColor::Indexed(5),
        NamedColor::Cyan | NamedColor::DimCyan => TermColor::Indexed(6),
        NamedColor::White | NamedColor::DimWhite => TermColor::Indexed(7),
        NamedColor::BrightBlack => TermColor::Indexed(8),
        NamedColor::BrightRed => TermColor::Indexed(9),
        NamedColor::BrightGreen => TermColor::Indexed(10),
        NamedColor::BrightYellow => TermColor::Indexed(11),
        NamedColor::BrightBlue => TermColor::Indexed(12),
        NamedColor::BrightMagenta => TermColor::Indexed(13),
        NamedColor::BrightCyan => TermColor::Indexed(14),
        NamedColor::BrightWhite => TermColor::Indexed(15),
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => TermColor::Default,
    }
}

fn map_attrs(flags: Flags) -> CellAttrs {
    let mut attrs = CellAttrs::empty();
    if flags.contains(Flags::BOLD) {
        attrs.insert(CellAttrs::BOLD);
    }
    if flags.contains(Flags::DIM) {
        attrs.insert(CellAttrs::DIM);
    }
    if flags.contains(Flags::ITALIC) {
        attrs.insert(CellAttrs::ITALIC);
    }
    if flags.intersects(
        Flags::UNDERLINE | Flags::UNDERCURL | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE,
    ) {
        attrs.insert(CellAttrs::UNDERLINE);
    }
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        attrs.insert(CellAttrs::DOUBLE_UNDERLINE);
    }
    if flags.contains(Flags::INVERSE) {
        attrs.insert(CellAttrs::INVERSE);
    }
    if flags.contains(Flags::HIDDEN) {
        attrs.insert(CellAttrs::HIDDEN);
    }
    if flags.contains(Flags::STRIKEOUT) {
        attrs.insert(CellAttrs::STRIKEOUT);
    }
    attrs
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        AlacrittyTerminalBackend, BackendDamage, BackendMetadata, BackendScrollbackSlice,
        RawByteRouter, TerminalBackend,
    };
    use crate::config::DEFAULT_MAX_SCROLLBACK_LINES;
    use embers_core::{
        ActivityState, CellAttrs, CursorShape, PtySize, SnapshotLine, TermColor, TerminalSnapshot,
    };

    fn backend(size: PtySize) -> AlacrittyTerminalBackend {
        AlacrittyTerminalBackend::new(size, DEFAULT_MAX_SCROLLBACK_LINES)
    }

    #[derive(Default)]
    struct StubBackend {
        ingested: Vec<u8>,
    }

    impl TerminalBackend for StubBackend {
        fn ingest_bytes(&mut self, bytes: &[u8]) {
            self.ingested.extend_from_slice(bytes);
        }

        fn resize(&mut self, _size: PtySize) {}

        fn visible_snapshot(
            &self,
            sequence: u64,
            size: PtySize,
            cwd: Option<PathBuf>,
        ) -> embers_core::TerminalSnapshot {
            let mut snapshot = embers_core::TerminalSnapshot::from_lines(
                sequence,
                size,
                [String::from_utf8_lossy(&self.ingested).into_owned()],
            );
            snapshot.cwd = cwd;
            snapshot
        }

        fn capture_scrollback(&self) -> Vec<String> {
            vec![String::from_utf8_lossy(&self.ingested).into_owned()]
        }

        fn capture_scrollback_slice(
            &self,
            start_line: u64,
            _line_count: u32,
        ) -> BackendScrollbackSlice {
            BackendScrollbackSlice {
                start_line,
                total_lines: 1,
                lines: vec![SnapshotLine::plain(
                    String::from_utf8_lossy(&self.ingested).into_owned(),
                )],
            }
        }

        fn metadata(&self) -> BackendMetadata {
            BackendMetadata::default()
        }

        fn take_events(&mut self) -> (ActivityState, Vec<u8>) {
            (ActivityState::Activity, Vec::new())
        }

        fn take_damage(&mut self) -> BackendDamage {
            BackendDamage::None
        }
    }

    fn snapshot_lines(snapshot: TerminalSnapshot) -> Vec<String> {
        snapshot.lines.into_iter().map(|line| line.text).collect()
    }

    fn first_line(backend: &AlacrittyTerminalBackend, size: PtySize) -> SnapshotLine {
        backend
            .visible_snapshot(1, size, None)
            .lines
            .into_iter()
            .next()
            .expect("at least one line")
    }

    #[test]
    fn sgr_bold_red_produces_indexed_run() {
        let size = PtySize::new(8, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[31;1mAB\x1b[0mC");

        let line = first_line(&backend, size);
        assert_eq!(line.text, "ABC");
        assert_eq!(line.runs.len(), 2);
        assert_eq!(line.runs[0].len, 2);
        assert_eq!(line.runs[0].fg, TermColor::Indexed(1));
        assert!(line.runs[0].attrs.contains(CellAttrs::BOLD));
        assert_eq!(line.runs[1].len, 1);
        assert_eq!(line.runs[1].fg, TermColor::Default);
        assert_eq!(line.runs[1].attrs, CellAttrs::empty());
    }

    #[test]
    fn indexed_256_color_survives_as_indexed() {
        let size = PtySize::new(4, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[38;5;208mX");

        let line = first_line(&backend, size);
        assert_eq!(line.text, "X");
        assert_eq!(line.runs[0].fg, TermColor::Indexed(208));
    }

    #[test]
    fn truecolor_becomes_rgb() {
        let size = PtySize::new(4, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[38;2;10;20;30mX");

        let line = first_line(&backend, size);
        assert_eq!(
            line.runs[0].fg,
            TermColor::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn bright_named_color_maps_to_high_index() {
        let size = PtySize::new(4, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[91mX");

        let line = first_line(&backend, size);
        assert_eq!(line.runs[0].fg, TermColor::Indexed(9));
    }

    #[test]
    fn wide_char_is_a_single_run_and_spacer_adds_nothing() {
        let size = PtySize::new(6, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        // Color the wide char so it forms its own run; the spacer column must not
        // add bytes or a run of its own.
        backend.ingest_bytes("\x1b[31m界\x1b[0ma".as_bytes());

        let line = first_line(&backend, size);
        assert_eq!(line.text, "界a");
        assert_eq!(line.runs.len(), 2);
        assert_eq!(line.runs[0].len, "界".len() as u32);
        assert_eq!(line.runs[0].fg, TermColor::Indexed(1));
        assert_eq!(line.runs[1].len, 1);
    }

    #[test]
    fn combining_char_folds_into_the_base_cell_run() {
        let size = PtySize::new(4, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        // 'e' + combining acute accent lands in one cell as a zero-width mark.
        backend.ingest_bytes("\x1b[31me\u{301}".as_bytes());

        let line = first_line(&backend, size);
        assert_eq!(line.text, "e\u{301}");
        assert_eq!(line.runs.len(), 1);
        assert_eq!(line.runs[0].len, "e\u{301}".len() as u32);
        assert_eq!(line.runs[0].fg, TermColor::Indexed(1));
    }

    #[test]
    fn trailing_background_survives_while_default_blanks_trim() {
        let size = PtySize::new(6, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        // Set a blue background then erase to end of line: cells become painted
        // spaces that must survive as styled trailing content.
        backend.ingest_bytes(b"\x1b[44m\x1b[K");

        let line = first_line(&backend, size);
        assert_eq!(line.text, "      ");
        assert_eq!(line.runs.len(), 1);
        assert_eq!(line.runs[0].len, 6);
        assert_eq!(line.runs[0].bg, TermColor::Indexed(4));

        // A default line trims to empty (plain projection unchanged).
        let mut plain =
            AlacrittyTerminalBackend::new(PtySize::new(6, 1), DEFAULT_MAX_SCROLLBACK_LINES);
        let _ = plain.take_damage();
        plain.ingest_bytes(b"hi");
        let plain_line = first_line(&plain, PtySize::new(6, 1));
        assert_eq!(plain_line.text, "hi");
        assert!(plain_line.runs.is_empty());
    }

    #[test]
    fn tab_cells_project_to_spaces() {
        let size = PtySize::new(12, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"a\tb");

        let line = first_line(&backend, size);
        assert!(!line.text.contains('\t'), "text: {:?}", line.text);
        assert!(line.text.starts_with('a'));
        assert!(line.text.trim_end().ends_with('b'));
        assert!(line.runs.is_empty());
    }

    #[test]
    fn ranged_scrollback_slice_carries_styles() {
        let size = PtySize::new(6, 2);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[31mone\x1b[0m\r\ntwo\r\nthree\r\nfour");

        // The oldest history line "one" is red; request the window containing it.
        let slice = backend.capture_scrollback_slice(0, 1);
        assert_eq!(slice.start_line, 0);
        assert_eq!(slice.lines[0].text, "one");
        assert_eq!(slice.lines[0].runs[0].fg, TermColor::Indexed(1));
    }

    #[test]
    fn alternate_screen_snapshot_carries_styles() {
        let size = PtySize::new(20, 4);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[?1049h\x1b[H\x1b[32malt\x1b[0m");

        let snapshot = backend.visible_snapshot(2, size, None);
        assert!(snapshot.modes.alternate_screen);
        let styled = snapshot
            .lines
            .iter()
            .find(|line| line.text.contains("alt"))
            .expect("alt line present");
        assert_eq!(styled.runs[0].fg, TermColor::Indexed(2));
    }

    #[test]
    fn visible_snapshot_extracts_plain_text_lines() {
        let mut backend = backend(PtySize::new(8, 3));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"hello\r\nworld");
        let snapshot = backend.visible_snapshot(3, PtySize::new(8, 3), None);

        let lines = snapshot_lines(snapshot.clone());
        assert_eq!(lines, vec!["hello", "world", ""]);
        assert_eq!(snapshot.total_lines, 3);
        assert_eq!(snapshot.viewport_top_line, 0);
        assert!(matches!(
            snapshot.cursor.as_ref().map(|cursor| cursor.shape),
            Some(CursorShape::Block) | Some(CursorShape::Underline) | Some(CursorShape::Beam)
        ));
    }

    #[test]
    fn carriage_return_overwrites_cells_without_advancing_the_row() {
        let mut backend = backend(PtySize::new(8, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"hello\rHEY");

        let lines = snapshot_lines(backend.visible_snapshot(1, PtySize::new(8, 2), None));
        assert_eq!(lines, vec!["HEYlo", ""]);
    }

    #[test]
    fn automatic_wrap_moves_following_bytes_to_the_next_row() {
        let mut backend = backend(PtySize::new(4, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"abcdX");

        let lines = snapshot_lines(backend.visible_snapshot(1, PtySize::new(4, 2), None));
        assert_eq!(lines, vec!["abcd", "X"]);
    }

    #[test]
    fn erase_in_line_clears_trailing_cells_from_the_cursor() {
        let mut backend = backend(PtySize::new(6, 1));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"abcdef\rabc\x1b[K");

        let lines = snapshot_lines(backend.visible_snapshot(1, PtySize::new(6, 1), None));
        assert_eq!(lines, vec!["abc"]);
    }

    #[test]
    fn clear_screen_resets_visible_cells_before_new_output() {
        let mut backend = backend(PtySize::new(6, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"one\r\ntwo\x1b[2J\x1b[Hdone");

        let lines = snapshot_lines(backend.visible_snapshot(1, PtySize::new(6, 2), None));
        assert_eq!(lines, vec!["done", ""]);
    }

    #[test]
    fn scrollback_capture_preserves_history_beyond_viewport() {
        let mut backend = backend(PtySize::new(6, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"one\r\ntwo\r\nthree\r\nfour");

        let visible = backend.visible_snapshot(4, PtySize::new(6, 2), None);
        let visible_lines: Vec<_> = visible.lines.into_iter().map(|line| line.text).collect();
        assert_eq!(visible_lines, vec!["three", "four"]);
        assert_eq!(visible.viewport_top_line, 2);
        assert_eq!(visible.total_lines, 4);

        let history = backend.capture_scrollback();
        assert!(history.iter().any(|line| line == "one"));
        assert!(history.iter().any(|line| line == "four"));
    }

    #[test]
    fn scrollback_slice_returns_requested_window() {
        let mut backend = backend(PtySize::new(6, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"one\r\ntwo\r\nthree\r\nfour");

        let slice = backend.capture_scrollback_slice(1, 2);
        assert_eq!(slice.start_line, 1);
        assert_eq!(slice.total_lines, 4);
        let slice_text: Vec<_> = slice.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(slice_text, vec!["two", "three"]);
    }

    #[test]
    fn damage_can_be_read_and_reset() {
        let mut backend = backend(PtySize::new(6, 2));

        assert!(matches!(backend.take_damage(), BackendDamage::Full));
        assert!(!matches!(backend.take_damage(), BackendDamage::Full));

        backend.ingest_bytes(b"hello");
        assert!(!matches!(backend.take_damage(), BackendDamage::None));
        assert!(!matches!(backend.take_damage(), BackendDamage::Full));
    }

    #[test]
    fn metadata_surfaces_terminal_modes_and_title() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x1b[?1049h\x1b[?1000h\x1b[?1004h\x1b[?2004h");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert!(metadata.alternate_screen);
        assert!(metadata.mouse_reporting);
        assert!(metadata.focus_reporting);
        assert!(metadata.bracketed_paste);
    }

    #[test]
    fn metadata_mode_flags_clear_when_disable_sequences_arrive() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[?1049h\x1b[?1000h\x1b[?1004h\x1b[?2004h");
        let enabled = backend.metadata();
        assert!(enabled.alternate_screen);
        assert!(enabled.mouse_reporting);
        assert!(enabled.focus_reporting);
        assert!(enabled.bracketed_paste);

        backend.ingest_bytes(b"\x1b[?1049l\x1b[?1000l\x1b[?1004l\x1b[?2004l");
        let disabled = backend.metadata();
        assert!(!disabled.alternate_screen);
        assert!(!disabled.mouse_reporting);
        assert!(!disabled.focus_reporting);
        assert!(!disabled.bracketed_paste);
    }

    #[test]
    fn bell_activity_is_consumed_separately_from_metadata() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x07");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_events().0, ActivityState::Bell);

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_events().0, ActivityState::Activity);
    }

    #[test]
    fn da1_query_produces_device_attributes_reply() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[c");

        let reply = backend.take_events().1;
        let text = String::from_utf8(reply).expect("reply is utf8");
        assert!(text.starts_with("\x1b[?"), "reply: {text:?}");
        assert!(text.ends_with('c'), "reply: {text:?}");

        // Drained: a second call returns nothing.
        assert!(backend.take_events().1.is_empty());
    }

    #[test]
    fn dsr_cursor_position_report_reports_row_and_column() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"ab\x1b[6n");

        let reply = backend.take_events().1;
        let text = String::from_utf8(reply).expect("reply is utf8");
        // Cursor sits after "ab" on row 1: CPR is ESC [ <row> ; <col> R.
        assert_eq!(text, "\x1b[1;3R", "reply: {text:?}");
    }

    #[test]
    fn decrqm_reports_bracketed_paste_mode() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[?2004h\x1b[?2004$p");

        let reply = backend.take_events().1;
        let text = String::from_utf8(reply).expect("reply is utf8");
        assert!(text.starts_with("\x1b[?2004;"), "reply: {text:?}");
        assert!(text.ends_with("$y"), "reply: {text:?}");
    }

    #[test]
    fn pty_writes_accumulate_alongside_title_and_bell() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x1b[c\x07");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));

        // A single drain returns both the bell activity and the accumulated reply.
        let (activity, reply) = backend.take_events();
        assert_eq!(activity, ActivityState::Bell);
        assert!(!reply.is_empty(), "device-attributes reply should survive");
    }

    #[test]
    fn raw_byte_router_is_explicit_passthrough_for_input_and_output() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let input = b"\x1b[200~paste\x1b[201~".to_vec();

        assert_eq!(router.route_input(input.clone()), input);

        router.route_output(&mut backend, b"hello");
        router.route_output(&mut backend, b" world");

        assert_eq!(backend.ingested, b"hello world");
    }

    #[test]
    fn router_sniffs_osc7_cwd_reports() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        assert_eq!(router.reported_cwd(), None);

        // BEL-terminated, whole in one chunk.
        router.route_output(&mut backend, b"\x1b]7;file://host/tmp/work\x07");
        assert_eq!(router.reported_cwd(), Some(PathBuf::from("/tmp/work")));
        // Output still passes through unchanged.
        assert!(backend.ingested.ends_with(b"\x07"));

        // ST-terminated, percent-encoded space.
        router.route_output(&mut backend, b"\x1b]7;file://host/tmp/a%20b\x1b\\");
        assert_eq!(router.reported_cwd(), Some(PathBuf::from("/tmp/a b")));
    }

    #[test]
    fn router_reassembles_osc7_split_across_chunks() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        router.route_output(&mut backend, b"\x1b]7;file://ho");
        router.route_output(&mut backend, b"st/home/u");
        assert_eq!(router.reported_cwd(), None, "not terminated yet");
        router.route_output(&mut backend, b"ser\x07");
        assert_eq!(router.reported_cwd(), Some(PathBuf::from("/home/user")));
    }

    #[test]
    fn router_ignores_non_osc7_sequences() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        // OSC 0 title and OSC 52 clipboard must not be mistaken for a cwd report.
        router.route_output(&mut backend, b"\x1b]0;a title\x07");
        router.route_output(&mut backend, b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(router.reported_cwd(), None);
        // A malformed OSC 7 (no file:// scheme) is rejected.
        router.route_output(&mut backend, b"\x1b]7;notauri\x07");
        assert_eq!(router.reported_cwd(), None);
    }

    #[test]
    fn alternate_screen_visible_snapshot_tracks_active_screen_and_restores_primary_screen() {
        let mut backend = backend(PtySize::new(20, 4));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"main-one\r\nmain-two");
        backend.ingest_bytes(b"\x1b[?1049h\x1b[Halt-screen");

        let alternate = backend.visible_snapshot(2, PtySize::new(20, 4), None);
        let alternate_lines: Vec<_> = alternate
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert!(alternate.modes.alternate_screen);
        assert!(
            alternate_lines
                .iter()
                .any(|line| line.contains("alt-screen")),
            "alternate visible lines: {alternate_lines:?}"
        );

        backend.ingest_bytes(b"\x1b[?1049l");

        let restored = backend.visible_snapshot(3, PtySize::new(20, 4), None);
        let restored_lines: Vec<_> = restored
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert!(!restored.modes.alternate_screen);
        assert!(
            restored_lines.iter().any(|line| line.contains("main-one")),
            "restored visible lines: {restored_lines:?}"
        );
        assert!(
            restored_lines.iter().any(|line| line.contains("main-two")),
            "restored visible lines: {restored_lines:?}"
        );
    }
}
