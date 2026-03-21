use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, LineDamageBounds, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{self, CursorShape as AlacrittyCursorShape};
use embers_core::{
    ActivityState, CursorPosition, CursorShape, CursorState, PtySize, SnapshotLine,
    TerminalModes, TerminalSnapshot,
};
use tracing::error;

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
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendDamage {
    None,
    Full,
    Partial(Vec<LineDamageBounds>),
}

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
    fn take_activity(&mut self) -> ActivityState;
    fn take_damage(&mut self) -> BackendDamage;
}

#[derive(Clone, Debug, Default)]
pub struct RawByteRouter;

impl RawByteRouter {
    pub fn route_input(&self, bytes: Vec<u8>) -> Vec<u8> {
        bytes
    }

    pub fn route_output(&mut self, backend: &mut dyn TerminalBackend, bytes: &[u8]) {
        backend.ingest_bytes(bytes);
    }
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
}

impl BackendEventProxy {
    fn new(state: Arc<Mutex<BackendEventState>>) -> Self {
        Self { state }
    }
}

impl EventListener for BackendEventProxy {
    fn send_event(&self, event: Event) {
        let Ok(mut state) = self.state.lock() else {
            error!(?event, "backend event lock poisoned");
            return;
        };

        match event {
            Event::Title(title) => state.title = Some(title),
            Event::ResetTitle => state.title = None,
            Event::Bell => state.bell_pending = true,
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
    pub fn new(size: PtySize) -> Self {
        let events = Arc::new(Mutex::new(BackendEventState::default()));
        let dimensions = BackendSize {
            columns: size.cols as usize,
            screen_lines: size.rows as usize,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };

        Self {
            term: Term::new(config, &dimensions, BackendEventProxy::new(events.clone())),
            parser: ansi::Processor::new(),
            events,
        }
    }

    fn visible_lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        let display_offset = grid.display_offset() as i32;
        let top = Line(-display_offset);
        let bottom = Line(grid.screen_lines() as i32 - display_offset - 1);
        self.collect_lines(top, bottom, false)
    }

    fn all_lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        let top = Line(-(grid.history_size() as i32));
        let bottom = Line(grid.screen_lines() as i32 - 1);
        self.collect_lines(top, bottom, false)
    }

    fn collect_lines(&self, start: Line, end: Line, trim_trailing_empty: bool) -> Vec<String> {
        let grid = self.term.grid();
        if grid.columns() == 0 || end < start {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut line = start;
        while line <= end {
            let text = self.term.bounds_to_string(
                Point::new(line, Column(0)),
                Point::new(line, Column(grid.columns() - 1)),
            );
            lines.push(text.trim_end_matches('\n').to_owned());
            line += 1;
        }

        if trim_trailing_empty {
            while matches!(lines.last(), Some(last) if last.is_empty()) {
                lines.pop();
            }
        }

        lines
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
            lines: self
                .visible_lines()
                .into_iter()
                .map(|text| SnapshotLine { text })
                .collect(),
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
        let lines = self.all_lines();
        let total_lines = lines.len() as u64;
        let start_line = start_line.min(total_lines);
        let end_line = start_line.saturating_add(u64::from(line_count)).min(total_lines);
        let lines = lines[start_line as usize..end_line as usize].to_vec();

        BackendScrollbackSlice {
            start_line,
            total_lines,
            lines,
        }
    }

    fn metadata(&self) -> BackendMetadata {
        let state = self.events.lock().expect("backend event lock");
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

    fn take_activity(&mut self) -> ActivityState {
        let mut state = self.events.lock().expect("backend event lock");
        if std::mem::take(&mut state.bell_pending) {
            ActivityState::Bell
        } else {
            ActivityState::Activity
        }
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

#[cfg(test)]
mod tests {
    use super::{AlacrittyTerminalBackend, BackendDamage, TerminalBackend};
    use embers_core::{ActivityState, CursorShape, PtySize};

    #[test]
    fn visible_snapshot_extracts_plain_text_lines() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(8, 3));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"hello\r\nworld");
        let snapshot = backend.visible_snapshot(3, PtySize::new(8, 3), None);

        let lines: Vec<_> = snapshot.lines.into_iter().map(|line| line.text).collect();
        assert_eq!(lines, vec!["hello", "world", ""]);
        assert_eq!(snapshot.total_lines, 3);
        assert_eq!(snapshot.viewport_top_line, 0);
        assert!(matches!(
            snapshot.cursor.as_ref().map(|cursor| cursor.shape),
            Some(CursorShape::Block)
                | Some(CursorShape::Underline)
                | Some(CursorShape::Beam)
        ));
    }

    #[test]
    fn scrollback_capture_preserves_history_beyond_viewport() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(6, 2));
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
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(6, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"one\r\ntwo\r\nthree\r\nfour");

        let slice = backend.capture_scrollback_slice(1, 2);
        assert_eq!(slice.start_line, 1);
        assert_eq!(slice.total_lines, 4);
        assert_eq!(slice.lines, vec!["two", "three"]);
    }

    #[test]
    fn damage_can_be_read_and_reset() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(6, 2));

        assert!(matches!(backend.take_damage(), BackendDamage::Full));
        assert!(!matches!(backend.take_damage(), BackendDamage::Full));

        backend.ingest_bytes(b"hello");
        assert!(!matches!(backend.take_damage(), BackendDamage::None));
        assert!(!matches!(backend.take_damage(), BackendDamage::Full));
    }

    #[test]
    fn metadata_surfaces_terminal_modes_and_title() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(10, 2));
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
    fn bell_activity_is_consumed_separately_from_metadata() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x07");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_activity(), ActivityState::Bell);

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_activity(), ActivityState::Activity);
    }
}
