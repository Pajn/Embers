use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, LineDamageBounds, Term, TermDamage};
use alacritty_terminal::vte::ansi;
use embers_core::{ActivityState, CursorPosition, PtySize, SnapshotLine, TerminalSnapshot};
use tracing::error;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendMetadata {
    pub title: Option<String>,
    pub activity: ActivityState,
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
    fn metadata(&self) -> BackendMetadata;
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
        self.collect_lines(top, bottom)
    }

    fn collect_lines(&self, start: Line, end: Line) -> Vec<String> {
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

        while matches!(lines.last(), Some(last) if last.is_empty()) {
            lines.pop();
        }

        lines
    }

    fn cursor_position(&self) -> Option<CursorPosition> {
        let cursor = self.term.grid().cursor.point;
        let row = u16::try_from(cursor.line.0).ok()?;
        let col = u16::try_from(cursor.column.0).ok()?;
        Some(CursorPosition { row, col })
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
        TerminalSnapshot {
            sequence,
            size,
            cursor: self.cursor_position(),
            lines: self
                .visible_lines()
                .into_iter()
                .map(|text| SnapshotLine { text })
                .collect(),
            title: self.metadata().title,
            cwd,
        }
    }

    fn capture_scrollback(&self) -> Vec<String> {
        let grid = self.term.grid();
        let top = Line(-(grid.history_size() as i32));
        let bottom = Line(grid.screen_lines() as i32 - 1);
        self.collect_lines(top, bottom)
    }

    fn metadata(&self) -> BackendMetadata {
        let mut state = self.events.lock().expect("backend event lock");
        let bell_pending = state.bell_pending;
        state.bell_pending = false;
        BackendMetadata {
            title: state.title.clone(),
            activity: if bell_pending {
                ActivityState::Bell
            } else {
                ActivityState::Activity
            },
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
    use embers_core::{ActivityState, PtySize};

    #[test]
    fn visible_snapshot_extracts_plain_text_lines() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(8, 3));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"hello\r\nworld");
        let snapshot = backend.visible_snapshot(3, PtySize::new(8, 3), None);

        let lines: Vec<_> = snapshot.lines.into_iter().map(|line| line.text).collect();
        assert_eq!(lines, vec!["hello", "world"]);
        assert!(snapshot.cursor.is_some());
    }

    #[test]
    fn scrollback_capture_preserves_history_beyond_viewport() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(6, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"one\r\ntwo\r\nthree\r\nfour");

        let visible = backend.visible_snapshot(4, PtySize::new(6, 2), None);
        let visible_lines: Vec<_> = visible.lines.into_iter().map(|line| line.text).collect();
        assert_eq!(visible_lines, vec!["three", "four"]);

        let history = backend.capture_scrollback();
        assert!(history.iter().any(|line| line == "one"));
        assert!(history.iter().any(|line| line == "four"));
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
    fn title_and_bell_events_update_metadata() {
        let mut backend = AlacrittyTerminalBackend::new(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x07");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(metadata.activity, ActivityState::Bell);

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(metadata.activity, ActivityState::Activity);
    }
}
