use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink as AlacrittyHyperlink, LineLength};
use alacritty_terminal::term::{
    ClipboardType, Config, LineDamageBounds, Osc52, Term, TermDamage, TermMode,
};
use alacritty_terminal::vte::ansi::{
    self, Color as AnsiColor, CursorShape as AlacrittyCursorShape, NamedColor,
};
use embers_core::{
    ActivityState, CellAttrs, ClipboardSelection, ClipboardWrite, CursorPosition, CursorShape,
    CursorState, Hyperlink, MAX_CLIPBOARD_WRITE_BYTES, MAX_PASSTHROUGH_BYTES, PassthroughWrite,
    PtySize, SnapshotLine, StyledRun, TermColor, TerminalModes, TerminalSnapshot,
};
use tracing::debug;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendMetadata {
    pub title: Option<String>,
    pub viewport_top_line: u64,
    pub total_lines: u64,
    pub alternate_screen: bool,
    pub mouse_reporting: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub keyboard_mode: u8,
    pub cursor: Option<CursorState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendScrollbackSlice {
    pub start_line: u64,
    pub total_lines: u64,
    pub lines: Vec<SnapshotLine>,
}

/// One-shot signals drained from the backend after ingesting output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendEvents {
    pub activity: ActivityState,
    /// Replies to terminal queries, to be written back to the PTY master. Empty
    /// when there is nothing pending.
    pub replies: Vec<u8>,
    /// The most recent OSC 52 clipboard write since the last drain. Writes in
    /// one batch replace each other, as they would on a real clipboard.
    pub clipboard: Option<ClipboardWrite>,
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
    /// Drain, under a single lock acquisition, the pending activity state, any
    /// terminal replies the emulator generated while ingesting bytes
    /// (device-attribute, cursor-position, and mode queries), and the latest
    /// OSC 52 clipboard write. The replies are written back to the PTY master.
    /// Combining the drains keeps the read hot path to one lock acquisition
    /// against the client-thread readers of the same state.
    fn take_events(&mut self) -> BackendEvents;
    fn take_damage(&mut self) -> BackendDamage;
    /// Answer a terminal query the emulator itself does not handle. The reply is
    /// queued with the emulator's own replies (drained by
    /// [`TerminalBackend::take_events`]) so replies keep their order. The router
    /// calls this right after ingesting the query's final byte, so the reply
    /// reflects the state at that point in the stream.
    fn answer_query(&mut self, query: &TerminalQuery);
    /// Where the cursor is on the screen, whether or not it is shown.
    fn cursor_position(&self) -> CursorPosition;
}

/// A DCS query intercepted by [`RawByteRouter`] for the backend to answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalQuery {
    /// DECRQSS (`DCS $ q <setting> ST`): report a setting's current value.
    /// Holds the setting, for example `m` for SGR.
    RequestSetting(Vec<u8>),
    /// XTGETTCAP (`DCS + q <hex name>;... ST`): look up terminfo capabilities.
    /// Holds the raw payload of hex-encoded names separated by `;`.
    Termcap(Vec<u8>),
    /// XTWINOPS 16 (`CSI 16 t`): report the cell size in pixels. alacritty
    /// answers the text area size (`CSI 14 t`) itself but not this.
    CellSize,
}

#[derive(Clone, Debug, Default)]
pub struct RawByteRouter {
    osc7: Osc7Scanner,
    queries: DcsQueryScanner,
    cell_size: CellSizeQueryScanner,
    passthrough: PassthroughScanner,
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
    ///
    /// A second scanner picks out the DCS queries alacritty leaves unanswered
    /// (DECRQSS and XTGETTCAP). Output is split after each one so the backend
    /// answers it with the state as of that byte.
    ///
    /// Before either, tmux passthrough sequences (`ESC P tmux; ... ESC \`) and
    /// unwrapped kitty graphics commands (`ESC _ G ... ESC \`) are taken out of
    /// the stream: the backend never sees them, and each one comes back, tmux
    /// ones unwrapped, with the cursor position the backend had reached at
    /// that point, for the keeper to forward to clients.
    pub fn route_output(
        &mut self,
        backend: &mut dyn TerminalBackend,
        bytes: &[u8],
    ) -> Vec<PassthroughWrite> {
        let mut writes = Vec::new();
        let mut forward = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            if let Some(payload) = self.passthrough.step(byte, &mut forward) {
                // Everything before the sequence reaches the backend first, so
                // the cursor is where the program left it.
                self.ingest(backend, &forward);
                forward.clear();
                writes.push(PassthroughWrite {
                    bytes: payload,
                    cursor: backend.cursor_position(),
                });
            }
        }
        self.ingest(backend, &forward);
        writes
    }

    fn ingest(&mut self, backend: &mut dyn TerminalBackend, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.osc7.feed(bytes);
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            let dcs_query = self.queries.step(byte);
            let cell_size_query = self.cell_size.step(byte);
            if let Some(query) = dcs_query.or(cell_size_query) {
                backend.ingest_bytes(&bytes[start..=index]);
                backend.answer_query(&query);
                start = index + 1;
            }
        }
        if start < bytes.len() {
            backend.ingest_bytes(&bytes[start..]);
        }
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

/// Streaming scanner for the DCS queries the router answers on the emulator's
/// behalf: DECRQSS (`ESC P $ q <setting> ESC \`) and XTGETTCAP
/// (`ESC P + q <hex names> ESC \`). Like [`Osc7Scanner`] it keeps state across
/// read chunks; any other DCS (sixel, ...) is skipped without buffering. It
/// never sees tmux passthrough, which [`PassthroughScanner`] removes first.
#[derive(Clone, Debug, Default)]
struct DcsQueryScanner {
    state: DcsState,
    kind: u8,
    buf: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DcsState {
    #[default]
    Ground,
    /// Saw ESC; expecting `P` to enter a DCS.
    Escape,
    /// Saw `ESC P`; expecting the `$` or `+` intermediate.
    Entry,
    /// Saw the intermediate; expecting the `q` final byte.
    Final,
    /// Inside a recognised query; buffering its payload.
    Collect,
    /// Inside a recognised query and saw ESC; `\` terminates it.
    CollectEscape,
    /// Inside some other DCS; discarding until the terminator.
    Skip,
    /// Inside a skipped DCS and saw ESC.
    SkipEscape,
}

/// Upper bound on a buffered query payload. Real queries are a few bytes; the
/// cap bounds memory if one is never terminated.
const DCS_QUERY_MAX_PAYLOAD: usize = 256;

impl DcsQueryScanner {
    /// Advance by one byte, returning a query when `byte` completes one.
    fn step(&mut self, byte: u8) -> Option<TerminalQuery> {
        const ESC: u8 = 0x1b;
        const CAN: u8 = 0x18;
        const SUB: u8 = 0x1a;
        // CAN and SUB abort any control string.
        if matches!(byte, CAN | SUB) {
            self.state = DcsState::Ground;
            return None;
        }
        match self.state {
            DcsState::Ground => {
                if byte == ESC {
                    self.state = DcsState::Escape;
                }
            }
            DcsState::Escape => {
                self.state = match byte {
                    b'P' => DcsState::Entry,
                    ESC => DcsState::Escape,
                    _ => DcsState::Ground,
                };
            }
            DcsState::Entry => match byte {
                b'$' | b'+' => {
                    self.kind = byte;
                    self.state = DcsState::Final;
                }
                ESC => self.state = DcsState::SkipEscape,
                _ => self.state = DcsState::Skip,
            },
            DcsState::Final => match byte {
                b'q' => {
                    self.buf.clear();
                    self.state = DcsState::Collect;
                }
                ESC => self.state = DcsState::SkipEscape,
                _ => self.state = DcsState::Skip,
            },
            DcsState::Collect => match byte {
                ESC => self.state = DcsState::CollectEscape,
                _ if self.buf.len() >= DCS_QUERY_MAX_PAYLOAD => self.state = DcsState::Skip,
                _ => self.buf.push(byte),
            },
            DcsState::CollectEscape => {
                if byte == b'\\' {
                    self.state = DcsState::Ground;
                    let payload = std::mem::take(&mut self.buf);
                    return Some(if self.kind == b'$' {
                        TerminalQuery::RequestSetting(payload)
                    } else {
                        TerminalQuery::Termcap(payload)
                    });
                }
                // ESC followed by anything else cancels the string; that ESC
                // may itself start the next sequence.
                self.state = if byte == ESC {
                    DcsState::Escape
                } else {
                    DcsState::Ground
                };
            }
            DcsState::Skip => {
                if byte == ESC {
                    self.state = DcsState::SkipEscape;
                }
            }
            DcsState::SkipEscape => {
                self.state = match byte {
                    b'\\' => DcsState::Ground,
                    ESC => DcsState::SkipEscape,
                    _ => DcsState::Skip,
                };
            }
        }
        None
    }
}

/// Streaming matcher for XTWINOPS 16 (`ESC [ 16 t`), the cell size query
/// alacritty leaves unanswered. It is a fixed sequence, so the matcher only
/// tracks how much of it it has seen; an ESC anywhere starts it over.
#[derive(Clone, Debug, Default)]
struct CellSizeQueryScanner {
    matched: usize,
}

const CELL_SIZE_QUERY: &[u8] = b"\x1b[16t";

impl CellSizeQueryScanner {
    fn step(&mut self, byte: u8) -> Option<TerminalQuery> {
        if byte == CELL_SIZE_QUERY[self.matched] {
            self.matched += 1;
        } else {
            self.matched = usize::from(byte == CELL_SIZE_QUERY[0]);
        }
        if self.matched < CELL_SIZE_QUERY.len() {
            return None;
        }
        self.matched = 0;
        Some(TerminalQuery::CellSize)
    }
}

/// What follows `ESC P` in a tmux passthrough sequence.
const TMUX_PASSTHROUGH_PREFIX: &[u8] = b"tmux;";

/// Streaming scanner that takes sequences for the host terminal out of PTY
/// output:
///
/// - tmux passthrough (`ESC P tmux; <payload> ESC \`), returned unwrapped.
///   Inside the payload every ESC is doubled, so `ESC ESC` stands for one ESC
///   and `ESC \` ends the sequence; like tmux, an ESC followed by anything
///   else is dropped. CAN or SUB, escaped or not, cancels the sequence
///   instead, as it does any DCS, and runs as a control.
/// - kitty graphics commands sent without the wrapper (`ESC _ G ... ESC \`),
///   returned whole. The emulator would drop them, and programs only send them
///   when they believe the terminal shows images, so they go the same way as
///   wrapped ones. Like any APC, one ends at the first ESC, CAN or SUB; ended
///   by anything but `ESC \`, it is dropped.
///
/// Bytes that might begin either (`ESC`, `ESC P`, `ESC P tm`, `ESC _`, ...) are
/// held back until it is clear whether they do, possibly across read chunks,
/// so the backend never sees part of one. Everything else passes through
/// unchanged and in order.
#[derive(Clone, Debug, Default)]
struct PassthroughScanner {
    state: PassthroughState,
    /// Bytes held back while they might still begin a passthrough.
    held: Vec<u8>,
    payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassthroughKind {
    /// `ESC P tmux; ... ESC \`.
    Tmux,
    /// `ESC _ G ... ESC \`.
    KittyGraphics,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PassthroughState {
    #[default]
    Ground,
    /// Holding an ESC.
    Escape,
    /// Holding `ESC P` and this many bytes of [`TMUX_PASSTHROUGH_PREFIX`].
    TmuxPrefix(usize),
    /// Holding `ESC _`.
    ApcPrefix,
    /// Inside a sequence, collecting its payload.
    Collect(PassthroughKind),
    /// Inside a sequence and saw ESC.
    CollectEscape(PassthroughKind),
    /// Inside a sequence over [`MAX_PASSTHROUGH_BYTES`], dropping it.
    Discard(PassthroughKind),
    /// Inside a dropped sequence and saw ESC.
    DiscardEscape(PassthroughKind),
}

impl PassthroughScanner {
    /// Advance by one byte. Bytes meant for the backend are appended to
    /// `forward`; returns the bytes for the host terminal when `byte` ends a
    /// sequence.
    fn step(&mut self, byte: u8, forward: &mut Vec<u8>) -> Option<Vec<u8>> {
        const ESC: u8 = 0x1b;
        const CAN: u8 = 0x18;
        const SUB: u8 = 0x1a;
        use PassthroughKind::{KittyGraphics, Tmux};
        match self.state {
            PassthroughState::Ground => {
                if byte == ESC {
                    self.held.push(byte);
                    self.state = PassthroughState::Escape;
                } else {
                    forward.push(byte);
                }
            }
            PassthroughState::Escape => {
                self.state = match byte {
                    b'P' => PassthroughState::TmuxPrefix(0),
                    b'_' => PassthroughState::ApcPrefix,
                    _ => return self.release_and_step(byte, forward),
                };
                self.held.push(byte);
            }
            PassthroughState::TmuxPrefix(matched) => {
                if byte != TMUX_PASSTHROUGH_PREFIX[matched] {
                    return self.release_and_step(byte, forward);
                }
                if matched + 1 == TMUX_PASSTHROUGH_PREFIX.len() {
                    self.held.clear();
                    self.payload.clear();
                    self.state = PassthroughState::Collect(Tmux);
                } else {
                    self.held.push(byte);
                    self.state = PassthroughState::TmuxPrefix(matched + 1);
                }
            }
            PassthroughState::ApcPrefix => {
                if byte != b'G' {
                    return self.release_and_step(byte, forward);
                }
                self.held.clear();
                self.payload.clear();
                self.payload.extend_from_slice(b"\x1b_G");
                self.state = PassthroughState::Collect(KittyGraphics);
            }
            PassthroughState::Collect(kind) => match byte {
                ESC => self.state = PassthroughState::CollectEscape(kind),
                // CAN and SUB abort a DCS or APC, and run as controls.
                CAN | SUB => {
                    self.payload.clear();
                    self.state = PassthroughState::Ground;
                    forward.push(byte);
                }
                _ => self.collect(kind, byte),
            },
            PassthroughState::CollectEscape(kind) => {
                if byte == b'\\' {
                    self.state = PassthroughState::Ground;
                    let mut payload = std::mem::take(&mut self.payload);
                    if kind == KittyGraphics {
                        payload.extend_from_slice(b"\x1b\\");
                    }
                    return Some(payload);
                }
                match kind {
                    Tmux if matches!(byte, CAN | SUB) => {
                        self.payload.clear();
                        self.state = PassthroughState::Ground;
                        forward.push(byte);
                    }
                    // `ESC ESC` is an escaped ESC; tmux drops an ESC before
                    // anything else.
                    Tmux => {
                        self.state = PassthroughState::Collect(Tmux);
                        self.collect(Tmux, byte);
                    }
                    // The ESC ended the command early: drop it, and let the
                    // ESC begin whatever follows.
                    KittyGraphics => {
                        self.payload.clear();
                        return self.restart_after_escape(byte, forward);
                    }
                }
            }
            PassthroughState::Discard(kind) => match byte {
                ESC => self.state = PassthroughState::DiscardEscape(kind),
                CAN | SUB => {
                    self.state = PassthroughState::Ground;
                    forward.push(byte);
                }
                _ => {}
            },
            PassthroughState::DiscardEscape(kind) => {
                if byte == b'\\' {
                    self.state = PassthroughState::Ground;
                } else if kind == Tmux && matches!(byte, CAN | SUB) {
                    self.state = PassthroughState::Ground;
                    forward.push(byte);
                } else if kind == Tmux {
                    self.state = PassthroughState::Discard(Tmux);
                } else {
                    return self.restart_after_escape(byte, forward);
                }
            }
        }
        None
    }

    fn collect(&mut self, kind: PassthroughKind, byte: u8) {
        if self.payload.len() >= MAX_PASSTHROUGH_BYTES {
            debug!(
                limit = MAX_PASSTHROUGH_BYTES,
                "dropping oversized passthrough sequence"
            );
            self.payload = Vec::new();
            self.state = PassthroughState::Discard(kind);
        } else {
            self.payload.push(byte);
        }
    }

    /// The held bytes turned out not to begin a passthrough: hand them to the
    /// backend and look at `byte` afresh, since it may begin a sequence itself.
    fn release_and_step(&mut self, byte: u8, forward: &mut Vec<u8>) -> Option<Vec<u8>> {
        forward.append(&mut self.held);
        self.state = PassthroughState::Ground;
        self.step(byte, forward)
    }

    /// Continue as if an ESC had just arrived in ordinary output, then `byte`.
    fn restart_after_escape(&mut self, byte: u8, forward: &mut Vec<u8>) -> Option<Vec<u8>> {
        self.held.clear();
        self.held.push(0x1b);
        self.state = PassthroughState::Escape;
        self.step(byte, forward)
    }
}

/// Terminfo capabilities Embers reports through XTGETTCAP, as the decoded
/// strings a terminal would send. `Ms` tells programs such as Neovim that
/// OSC 52 clipboard writes work; `Smulx` and `Setulc` that underline styles
/// and colors do.
const TERMCAP_ANSWERS: &[(&str, &str)] = &[
    ("Ms", "\x1b]52;%p1%s;%p2%s\x07"),
    ("Smulx", "\x1b[4:%p1%dm"),
    (
        "Setulc",
        "\x1b[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
    ),
];

fn hex_encode(bytes: &[u8], out: &mut String) {
    use std::fmt::Write as _;
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
}

fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

/// XTGETTCAP reply: one `DCS 1 + r name=value ST` per known name and
/// `DCS 0 + r name ST` per unknown one, in request order.
fn termcap_reply(payload: &[u8]) -> String {
    let mut reply = String::new();
    for hex_name in payload.split(|&byte| byte == b';') {
        let decoded = hex_decode(hex_name).unwrap_or_default();
        let value = TERMCAP_ANSWERS
            .iter()
            .find(|(known, _)| known.as_bytes() == decoded.as_slice())
            .map(|(_, value)| *value);
        // Echo the name back as hex, normalized to upper case.
        let mut name = String::new();
        hex_encode(&decoded, &mut name);
        match value {
            Some(value) => {
                reply.push_str("\x1bP1+r");
                reply.push_str(&name);
                reply.push('=');
                hex_encode(value.as_bytes(), &mut reply);
            }
            None => {
                reply.push_str("\x1bP0+r");
                reply.push_str(&name);
            }
        }
        reply.push_str("\x1b\\");
    }
    reply
}

/// The SGR parameters (without `CSI` and `m`) that recreate `template`'s pen,
/// for a DECRQSS reply. Always starts with `0` so the report is absolute.
fn sgr_report(template: &Cell) -> String {
    use std::fmt::Write as _;
    let mut params = String::from("0");
    let flags = template.flags;
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if flags.contains(flag) {
            params.push(';');
            params.push_str(code);
        }
    }
    for (color, base) in [
        (Some(template.fg), 38),
        (Some(template.bg), 48),
        (template.underline_color(), 58),
    ] {
        match color.map(map_color) {
            None | Some(TermColor::Default) => {}
            Some(TermColor::Indexed(index)) => {
                let _ = write!(params, ";{base}:5:{index}");
            }
            Some(TermColor::Rgb { r, g, b }) => {
                let _ = write!(params, ";{base}:2::{r}:{g}:{b}");
            }
        }
    }
    params
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

/// Map the active kitty keyboard `TermMode` bits to the compact bitfield the
/// client encoder consumes. Bit positions follow the kitty protocol's own flag
/// numbering (bit 0 = disambiguate, bit 3 = report-all-keys) so the byte never
/// reuses a spec bit position for a different flag.
fn kitty_keyboard_mode(mode: TermMode) -> u8 {
    let mut bits = 0;
    if mode.contains(TermMode::DISAMBIGUATE_ESC_CODES) {
        bits |= 0b0000_0001;
    }
    if mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC) {
        bits |= 0b0000_1000;
    }
    bits
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
    clipboard: Option<ClipboardWrite>,
    /// The PTY size, for the text area and cell size reports.
    size: PtySize,
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
            // XTWINOPS 14, the text area in pixels. Like the cell size report,
            // only answered once the pixel size is known.
            Event::TextAreaSizeRequest(format) => {
                // Unknown until a client that knows its terminal's cell size
                // has sized the pane.
                if let Some(cell) = state.size.cell_size() {
                    let reply = format(WindowSize {
                        num_lines: state.size.rows,
                        num_cols: state.size.cols,
                        cell_width: cell.width,
                        cell_height: cell.height,
                    });
                    state.pty_write.extend_from_slice(reply.as_bytes());
                }
            }
            // OSC 52 stores. Loads (clipboard queries) never get here: the
            // emulator runs with `Osc52::OnlyCopy`, so it drops them unanswered.
            Event::ClipboardStore(clipboard, text) => {
                if text.len() > MAX_CLIPBOARD_WRITE_BYTES {
                    debug!(
                        bytes = text.len(),
                        "dropping oversized OSC 52 clipboard write"
                    );
                    return;
                }
                let selection = match clipboard {
                    ClipboardType::Clipboard => ClipboardSelection::Clipboard,
                    ClipboardType::Selection => ClipboardSelection::Primary,
                };
                state.clipboard = Some(ClipboardWrite { selection, text });
            }
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
        let events = Arc::new(Mutex::new(BackendEventState {
            size,
            ..BackendEventState::default()
        }));
        let dimensions = BackendSize {
            columns: size.cols as usize,
            screen_lines: size.rows as usize,
        };
        let config = Config {
            scrolling_history: max_scrollback_lines,
            // Enable the kitty keyboard protocol so inner apps (e.g. nvim) can
            // push/pop/query disambiguation flags; the reply rides Event::PtyWrite.
            kitty_keyboard: true,
            // Accept OSC 52 clipboard writes (forwarded to attached clients) but
            // deny reads: answering one would hand whatever is on the clipboard
            // to any program that asks, including one on a remote host.
            osc52: Osc52::OnlyCopy,
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
        let mut links = LineLinks::default();

        for column in 0..content_len {
            let cell = &row[Column(column)];
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            let fg = map_color(cell.fg);
            let bg = map_color(cell.bg);
            let attrs = map_attrs(flags);
            let underline_color = cell.underline_color().map_or(TermColor::Default, map_color);
            let link = links.index_of(cell.hyperlink());

            // `emit_cell_text` always writes at least one byte (a char or a
            // space), so every cell contributes to exactly one run.
            let start = text.len();
            emit_cell_text(cell, &mut text);
            let byte_len = u32::try_from(text.len() - start).unwrap_or(u32::MAX);

            if fg != TermColor::Default
                || bg != TermColor::Default
                || attrs != CellAttrs::empty()
                || underline_color != TermColor::Default
                || link != 0
            {
                styled = true;
            }

            match runs.last_mut() {
                Some(last)
                    if last.fg == fg
                        && last.bg == bg
                        && last.attrs == attrs
                        && last.underline_color == underline_color
                        && last.link == link =>
                {
                    last.len += byte_len;
                }
                _ => runs.push(StyledRun {
                    len: byte_len,
                    fg,
                    bg,
                    attrs,
                    underline_color,
                    link,
                }),
            }
        }

        // A line that is entirely default-styled coalesces to a single default
        // run; drop it so the on-wire invariant is "empty runs = plain line".
        if !styled {
            runs.clear();
        }

        SnapshotLine {
            text,
            runs,
            links: links.table,
        }
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
            keyboard_mode: kitty_keyboard_mode(mode),
        }
    }

    fn viewport_top_line(&self) -> u64 {
        let grid = self.term.grid();
        grid.history_size().saturating_sub(grid.display_offset()) as u64
    }

    // (kitty keyboard bitfield extracted below as a free function)

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
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .size = size;
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
                keyboard_mode: metadata.keyboard_mode,
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
            keyboard_mode: modes.keyboard_mode,
            cursor: self.cursor_state(),
        }
    }

    fn take_events(&mut self) -> BackendEvents {
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
        BackendEvents {
            activity,
            replies: std::mem::take(&mut state.pty_write),
            clipboard: state.clipboard.take(),
        }
    }

    fn answer_query(&mut self, query: &TerminalQuery) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reply = match query {
            // DECRQSS for SGR. Neovim sets `4:3` and asks this to learn whether
            // the terminal keeps curly underlines before it uses them (and
            // underline colors) under a TERM without `Smulx`.
            TerminalQuery::RequestSetting(setting) if setting == b"m" => {
                let pen = sgr_report(&self.term.grid().cursor.template);
                format!("\x1bP1$r{pen}m\x1b\\")
            }
            // Any other setting: the "invalid request" reply.
            TerminalQuery::RequestSetting(_) => "\x1bP0$r\x1b\\".to_owned(),
            TerminalQuery::Termcap(payload) => termcap_reply(payload),
            // Only answered once the pixel size is known: a program that gets
            // no answer falls back to its own guess, while a zero would break
            // its arithmetic.
            TerminalQuery::CellSize => match events.size.cell_size() {
                Some(cell) => format!("\x1b[6;{};{}t", cell.height, cell.width),
                None => return,
            },
        };
        events.pty_write.extend_from_slice(reply.as_bytes());
    }

    fn cursor_position(&self) -> CursorPosition {
        let point = self.term.grid().cursor.point;
        CursorPosition {
            row: u16::try_from(point.line.0).unwrap_or(0),
            col: u16::try_from(point.column.0).unwrap_or(u16::MAX),
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

/// Per-line hyperlink table built during the row walk.
#[derive(Default)]
struct LineLinks {
    table: Vec<Hyperlink>,
    /// The emulator link most recently resolved and its index, so the cells of
    /// one link (the common case) skip the table search.
    last: Option<(AlacrittyHyperlink, u16)>,
}

impl LineLinks {
    /// 1-based index of `link` in this line's table (`0` for no link), adding
    /// it on first sight.
    fn index_of(&mut self, link: Option<AlacrittyHyperlink>) -> u16 {
        let Some(link) = link else {
            return 0;
        };
        if let Some((last, index)) = &self.last
            && *last == link
        {
            return *index;
        }
        let mapped = Hyperlink {
            id: Some(link.id().to_owned()),
            uri: link.uri().to_owned(),
        };
        let index = match self.table.iter().position(|known| *known == mapped) {
            Some(position) => position + 1,
            None => {
                self.table.push(mapped);
                self.table.len()
            }
        };
        // A row holds at most `u16::MAX` cells, so the index always fits.
        let index = u16::try_from(index).unwrap_or(u16::MAX);
        self.last = Some((link, index));
        index
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
    // The emulator keeps at most one underline flag set per cell (SGR 4:n
    // replaces the previous style), so each maps onto its own exclusive flag.
    for (flag, attr) in [
        (Flags::UNDERLINE, CellAttrs::UNDERLINE),
        (Flags::DOUBLE_UNDERLINE, CellAttrs::DOUBLE_UNDERLINE),
        (Flags::UNDERCURL, CellAttrs::CURLY_UNDERLINE),
        (Flags::DOTTED_UNDERLINE, CellAttrs::DOTTED_UNDERLINE),
        (Flags::DASHED_UNDERLINE, CellAttrs::DASHED_UNDERLINE),
    ] {
        if flags.contains(flag) {
            attrs.insert(attr);
        }
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
        AlacrittyTerminalBackend, BackendDamage, BackendEvents, BackendMetadata,
        BackendScrollbackSlice, RawByteRouter, TerminalBackend, TerminalQuery,
    };
    use crate::config::DEFAULT_MAX_SCROLLBACK_LINES;
    use embers_core::{
        ActivityState, CellAttrs, ClipboardSelection, ClipboardWrite, CursorPosition, CursorShape,
        MAX_PASSTHROUGH_BYTES, PassthroughWrite, PtySize, SnapshotLine, TermColor,
        TerminalSnapshot,
    };

    fn backend(size: PtySize) -> AlacrittyTerminalBackend {
        AlacrittyTerminalBackend::new(size, DEFAULT_MAX_SCROLLBACK_LINES)
    }

    #[derive(Default)]
    struct StubBackend {
        ingested: Vec<u8>,
        /// Each query with how many bytes had been ingested when it was asked.
        queries: Vec<(usize, TerminalQuery)>,
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

        fn take_events(&mut self) -> BackendEvents {
            BackendEvents {
                activity: ActivityState::Activity,
                replies: Vec::new(),
                clipboard: None,
            }
        }

        fn answer_query(&mut self, query: &TerminalQuery) {
            self.queries.push((self.ingested.len(), query.clone()));
        }

        /// The column is the number of bytes ingested so far, so tests can
        /// tell where in the stream a cursor was read.
        fn cursor_position(&self) -> CursorPosition {
            CursorPosition {
                row: 0,
                col: u16::try_from(self.ingested.len()).unwrap_or(u16::MAX),
            }
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
    fn underline_styles_keep_their_shape() {
        let size = PtySize::new(8, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        // Single, double, curly, dotted, dashed: one cell each.
        backend.ingest_bytes(b"\x1b[4:1ma\x1b[4:2mb\x1b[4:3mc\x1b[4:4md\x1b[4:5me\x1b[0m");

        let line = first_line(&backend, size);
        assert_eq!(line.text, "abcde");
        let attrs: Vec<_> = line.runs.iter().map(|run| run.attrs).collect();
        assert_eq!(
            attrs,
            vec![
                CellAttrs(CellAttrs::UNDERLINE),
                CellAttrs(CellAttrs::DOUBLE_UNDERLINE),
                CellAttrs(CellAttrs::CURLY_UNDERLINE),
                CellAttrs(CellAttrs::DOTTED_UNDERLINE),
                CellAttrs(CellAttrs::DASHED_UNDERLINE),
            ]
        );
    }

    #[test]
    fn underline_color_is_carried_semantically() {
        let size = PtySize::new(8, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        // Curly underline in true-color red, then an indexed underline color,
        // then SGR 59 resets the underline color back to the foreground.
        backend.ingest_bytes(b"\x1b[4:3;58:2::255:0:0mab\x1b[58:5:12mc\x1b[59md\x1b[0m");

        let line = first_line(&backend, size);
        assert_eq!(line.text, "abcd");
        assert_eq!(line.runs.len(), 3);
        assert_eq!(line.runs[0].len, 2);
        assert!(line.runs[0].attrs.contains(CellAttrs::CURLY_UNDERLINE));
        assert_eq!(
            line.runs[0].underline_color,
            TermColor::Rgb { r: 255, g: 0, b: 0 }
        );
        assert_eq!(line.runs[1].underline_color, TermColor::Indexed(12));
        assert_eq!(line.runs[2].underline_color, TermColor::Default);
        assert!(line.runs[2].attrs.contains(CellAttrs::CURLY_UNDERLINE));
    }

    #[test]
    fn osc8_hyperlinks_become_per_line_link_table() {
        let size = PtySize::new(20, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(
            b"\x1b]8;id=a;https://a.example\x1b\\one\x1b]8;;\x1b\\ \
              \x1b]8;;https://b.example\x07two\x1b]8;;\x07 \
              \x1b]8;id=a;https://a.example\x1b\\three\x1b]8;;\x1b\\",
        );

        let line = first_line(&backend, size);
        assert_eq!(line.text, "one two three");
        assert_eq!(line.links.len(), 2, "a repeated link is listed once");
        assert_eq!(line.links[0].id.as_deref(), Some("a"));
        assert_eq!(line.links[0].uri, "https://a.example");
        assert_eq!(line.links[1].uri, "https://b.example");

        let spans: Vec<_> = line.runs.iter().map(|run| (run.len, run.link)).collect();
        assert_eq!(spans, vec![(3, 1), (1, 0), (3, 2), (1, 0), (5, 1)]);
        assert_eq!(
            line.link(&line.runs[4]).map(|link| link.uri.as_str()),
            Some("https://a.example")
        );
        // Linked text with otherwise default styling is still a styled line.
        assert!(line.runs.iter().all(|run| run.attrs == CellAttrs::empty()));
    }

    #[test]
    fn plain_lines_carry_no_link_table() {
        let size = PtySize::new(8, 1);
        let mut backend = backend(size);
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[31mred\x1b[0m");

        let line = first_line(&backend, size);
        assert!(line.links.is_empty());
        assert!(line.runs.iter().all(|run| run.link == 0));
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
    fn kitty_keyboard_mode_is_pushed_and_popped() {
        let mut backend = backend(PtySize::new(10, 2));
        assert_eq!(backend.metadata().keyboard_mode, 0);

        // Push disambiguate-esc-codes (flags = 1).
        backend.ingest_bytes(b"\x1b[>1u");
        assert_eq!(backend.metadata().keyboard_mode & 0b0000_0001, 0b0000_0001);

        // Pop restores the previous (empty) mode.
        backend.ingest_bytes(b"\x1b[<u");
        assert_eq!(backend.metadata().keyboard_mode, 0);
    }

    #[test]
    fn bell_activity_is_consumed_separately_from_metadata() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]0;embers\x07\x07");

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_events().activity, ActivityState::Bell);

        let metadata = backend.metadata();
        assert_eq!(metadata.title.as_deref(), Some("embers"));
        assert_eq!(backend.take_events().activity, ActivityState::Activity);
    }

    #[test]
    fn da1_query_produces_device_attributes_reply() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[c");

        let reply = backend.take_events().replies;
        let text = String::from_utf8(reply).expect("reply is utf8");
        assert!(text.starts_with("\x1b[?"), "reply: {text:?}");
        assert!(text.ends_with('c'), "reply: {text:?}");

        // Drained: a second call returns nothing.
        assert!(backend.take_events().replies.is_empty());
    }

    #[test]
    fn dsr_cursor_position_report_reports_row_and_column() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"ab\x1b[6n");

        let reply = backend.take_events().replies;
        let text = String::from_utf8(reply).expect("reply is utf8");
        // Cursor sits after "ab" on row 1: CPR is ESC [ <row> ; <col> R.
        assert_eq!(text, "\x1b[1;3R", "reply: {text:?}");
    }

    #[test]
    fn cell_and_text_area_size_reports_use_the_pixel_size() {
        let size = PtySize {
            cols: 10,
            rows: 2,
            pixel_width: 90,
            pixel_height: 36,
        };
        let mut backend = backend(size);
        let mut router = RawByteRouter::default();

        let _ = router.route_output(&mut backend, b"\x1b[16t\x1b[14t\x1b[18t");

        assert_eq!(
            String::from_utf8(backend.take_events().replies).expect("utf8"),
            "\x1b[6;18;9t\x1b[4;36;90t\x1b[8;2;10t"
        );

        // A resize carries the new pixel size into the reports.
        backend.resize(PtySize {
            cols: 20,
            rows: 4,
            pixel_width: 200,
            pixel_height: 80,
        });
        let _ = router.route_output(&mut backend, b"\x1b[16t\x1b[14t");
        assert_eq!(
            String::from_utf8(backend.take_events().replies).expect("utf8"),
            "\x1b[6;20;10t\x1b[4;80;200t"
        );
    }

    #[test]
    fn pixel_size_reports_wait_for_a_known_pixel_size() {
        let mut backend = backend(PtySize::new(10, 2));
        let mut router = RawByteRouter::default();

        let _ = router.route_output(&mut backend, b"\x1b[16t\x1b[14t\x1b[5n");

        // Only the status report: no made-up pixel sizes.
        assert_eq!(backend.take_events().replies, b"\x1b[0n");
    }

    #[test]
    fn decrqm_reports_bracketed_paste_mode() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b[?2004h\x1b[?2004$p");

        let reply = backend.take_events().replies;
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
        let events = backend.take_events();
        assert_eq!(events.activity, ActivityState::Bell);
        assert!(
            !events.replies.is_empty(),
            "device-attributes reply should survive"
        );
    }

    #[test]
    fn osc52_store_surfaces_as_a_clipboard_write() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        // "hi" in base64, to the clipboard; then "yo" to the primary selection,
        // which replaces it within one drain.
        backend.ingest_bytes(b"\x1b]52;c;aGk=\x07");
        assert_eq!(
            backend.take_events().clipboard,
            Some(ClipboardWrite {
                selection: ClipboardSelection::Clipboard,
                text: "hi".to_owned(),
            })
        );
        assert_eq!(backend.take_events().clipboard, None, "drained");

        backend.ingest_bytes(b"\x1b]52;c;aGk=\x1b\\\x1b]52;p;eW8=\x1b\\");
        assert_eq!(
            backend.take_events().clipboard,
            Some(ClipboardWrite {
                selection: ClipboardSelection::Primary,
                text: "yo".to_owned(),
            })
        );
    }

    #[test]
    fn router_splits_output_after_each_dcs_query() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        // A DECRQSS split across chunks, then an XTGETTCAP, then a sixel DCS
        // (not a query) and trailing text.
        router.route_output(&mut backend, b"a\x1b[4:3m\x1bP$");
        router.route_output(&mut backend, b"qm\x1b\\b\x1bP+q4D73;5463\x1b\\");
        router.route_output(&mut backend, b"\x1bPq#0;2;0;0;0\x1b\\c");

        assert_eq!(
            backend.queries,
            vec![
                (
                    b"a\x1b[4:3m\x1bP$qm\x1b\\".len(),
                    TerminalQuery::RequestSetting(b"m".to_vec())
                ),
                (
                    b"a\x1b[4:3m\x1bP$qm\x1b\\b\x1bP+q4D73;5463\x1b\\".len(),
                    TerminalQuery::Termcap(b"4D73;5463".to_vec())
                ),
            ]
        );
        // Every byte still reaches the backend, in order.
        assert!(backend.ingested.ends_with(b"\x1b\\c"));
        assert_eq!(
            backend.ingested.len(),
            b"a\x1b[4:3m\x1bP$qm\x1b\\b\x1bP+q4D73;5463\x1b\\\x1bPq#0;2;0;0;0\x1b\\c".len()
        );
    }

    #[test]
    fn router_splits_output_after_each_cell_size_query() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        // Split across chunks, and next to near misses.
        router.route_output(&mut backend, b"a\x1b[1\x1b[16");
        router.route_output(&mut backend, b"tb\x1b[161t\x1b[16t");

        assert_eq!(
            backend.queries,
            vec![
                (b"a\x1b[1\x1b[16t".len(), TerminalQuery::CellSize),
                (
                    b"a\x1b[1\x1b[16tb\x1b[161t\x1b[16t".len(),
                    TerminalQuery::CellSize
                ),
            ]
        );
    }

    /// `payload` wrapped for tmux passthrough, with every ESC doubled.
    fn tmux_wrap(payload: &[u8]) -> Vec<u8> {
        let mut wrapped = b"\x1bPtmux;".to_vec();
        for &byte in payload {
            if byte == 0x1b {
                wrapped.push(0x1b);
            }
            wrapped.push(byte);
        }
        wrapped.extend_from_slice(b"\x1b\\");
        wrapped
    }

    const KITTY_IMAGE: &[u8] = b"\x1b_Gf=100,a=T;iVBORw0KGgo=\x1b\\";

    #[test]
    fn router_unwraps_tmux_passthrough_and_keeps_it_from_the_backend() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let mut output = b"ab".to_vec();
        output.extend(tmux_wrap(KITTY_IMAGE));
        output.extend_from_slice(b"cd");

        let writes = router.route_output(&mut backend, &output);

        assert_eq!(
            writes,
            vec![PassthroughWrite {
                bytes: KITTY_IMAGE.to_vec(),
                cursor: CursorPosition { row: 0, col: 2 },
            }]
        );
        assert_eq!(backend.ingested, b"abcd");
    }

    #[test]
    fn router_reassembles_passthrough_split_at_every_byte() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let mut output = b"a".to_vec();
        output.extend(tmux_wrap(b"\x1b]9;done\x07"));
        output.extend(tmux_wrap(KITTY_IMAGE));
        output.extend_from_slice(b"\x1b[1mb");

        let mut writes = Vec::new();
        for byte in &output {
            writes.extend(router.route_output(&mut backend, std::slice::from_ref(byte)));
        }

        let payloads: Vec<_> = writes.iter().map(|write| write.bytes.clone()).collect();
        assert_eq!(
            payloads,
            vec![b"\x1b]9;done\x07".to_vec(), KITTY_IMAGE.to_vec()]
        );
        assert_eq!(writes[0].cursor.col, 1);
        assert_eq!(backend.ingested, b"a\x1b[1mb");
    }

    #[test]
    fn router_leaves_other_dcs_sequences_to_the_backend() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        // A sixel image, a DCS that starts like the tmux prefix but is not it,
        // and a DECRQSS query, which must still be answered.
        let output = b"\x1bPq#0;2;0;0;0\x1b\\\x1bPtmx\x1b\\\x1b\x1bP$qm\x1b\\";

        let writes = router.route_output(&mut backend, output);

        assert!(writes.is_empty());
        assert_eq!(backend.ingested, output);
        assert_eq!(
            backend.queries,
            vec![(output.len(), TerminalQuery::RequestSetting(b"m".to_vec()))]
        );
    }

    #[test]
    fn router_drops_oversized_passthrough_whole() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let mut output = b"a".to_vec();
        output.extend(tmux_wrap(&vec![b'x'; MAX_PASSTHROUGH_BYTES + 1]));
        output.push(b'b');
        output.extend(tmux_wrap(b"\x1b]9;small\x07"));

        let writes = router.route_output(&mut backend, &output);

        // Nothing of the oversized one is forwarded or reaches the backend, and
        // the scanner recovers for the next sequence.
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].bytes, b"\x1b]9;small\x07");
        assert_eq!(backend.ingested, b"ab");

        // Exactly at the cap still goes through.
        let writes =
            router.route_output(&mut backend, &tmux_wrap(&vec![b'y'; MAX_PASSTHROUGH_BYTES]));
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].bytes.len(), MAX_PASSTHROUGH_BYTES);
    }

    #[test]
    fn router_ends_tmux_passthrough_at_can_or_sub() {
        for control in [0x18_u8, 0x1a] {
            // Cancelled while collecting, after an escaped ESC, and while
            // dropping an oversized payload.
            let collecting = b"\x1bPtmux;abc".to_vec();
            let after_escape = b"\x1bPtmux;abc\x1b".to_vec();
            let mut discarding = b"\x1bPtmux;".to_vec();
            discarding.extend(vec![b'x'; MAX_PASSTHROUGH_BYTES + 1]);
            for start in [collecting, after_escape, discarding] {
                let mut router = RawByteRouter::default();
                let mut backend = StubBackend::default();
                let mut output = start;
                output.push(control);
                output.extend_from_slice(b"visible text");

                let writes = router.route_output(&mut backend, &output);

                assert!(writes.is_empty(), "{control:#x}: {writes:?}");
                let mut expected = vec![control];
                expected.extend_from_slice(b"visible text");
                assert_eq!(backend.ingested, expected, "{control:#x}");

                // The scanner is back in ground state for the next sequence.
                let writes = router.route_output(&mut backend, &tmux_wrap(b"\x1b]9;next\x07"));
                assert_eq!(writes.len(), 1, "{control:#x}");
                assert_eq!(writes[0].bytes, b"\x1b]9;next\x07");
            }
        }
    }

    #[test]
    fn router_forwards_raw_kitty_graphics_commands_whole() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let mut output = b"ab".to_vec();
        output.extend_from_slice(KITTY_IMAGE);
        output.extend_from_slice(b"cd");

        // Split at every byte: the APC may straddle read chunks.
        let mut writes = Vec::new();
        for byte in &output {
            writes.extend(router.route_output(&mut backend, std::slice::from_ref(byte)));
        }

        assert_eq!(
            writes,
            vec![PassthroughWrite {
                bytes: KITTY_IMAGE.to_vec(),
                cursor: CursorPosition { row: 0, col: 2 },
            }]
        );
        assert_eq!(backend.ingested, b"abcd");
    }

    #[test]
    fn router_leaves_other_apc_strings_and_broken_graphics_to_the_backend() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        // An APC that is not a graphics command goes to the emulator, which
        // ignores it.
        let writes = router.route_output(&mut backend, b"\x1b_Xother\x1b\\");
        assert!(writes.is_empty());
        assert_eq!(backend.ingested, b"\x1b_Xother\x1b\\");

        // A graphics command cut short by another escape sequence is dropped;
        // the sequence that cut it short still runs.
        backend.ingested.clear();
        let writes = router.route_output(&mut backend, b"\x1b_Gf=100;AAAA\x1b[1mx");
        assert!(writes.is_empty());
        assert_eq!(backend.ingested, b"\x1b[1mx");
    }

    #[test]
    fn router_drops_oversized_raw_kitty_graphics_commands() {
        let mut router = RawByteRouter::default();
        let mut backend = StubBackend::default();
        let mut output = b"\x1b_G".to_vec();
        output.extend(vec![b'A'; MAX_PASSTHROUGH_BYTES]);
        output.extend_from_slice(b"\x1b\\z");
        output.extend_from_slice(KITTY_IMAGE);

        let writes = router.route_output(&mut backend, &output);

        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].bytes, KITTY_IMAGE);
        assert_eq!(backend.ingested, b"z");
    }

    #[test]
    fn passthrough_records_the_emulated_cursor_and_never_touches_the_screen() {
        let mut backend = backend(PtySize::new(20, 4));
        let mut router = RawByteRouter::default();
        let _ = backend.take_damage();

        // The payload carries a clear-screen that must not run in the pane.
        let mut output = b"keep\x1b[3;5H".to_vec();
        output.extend(tmux_wrap(b"\x1b[2J\x1b]9;hi\x07"));
        let writes = router.route_output(&mut backend, &output);

        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].cursor, CursorPosition { row: 2, col: 4 });
        assert_eq!(first_line(&backend, PtySize::new(20, 4)).text, "keep");
        // Hiding the cursor (as Neovim does while it draws) keeps its position.
        let writes = router.route_output(
            &mut backend,
            &[b"\x1b[?25l".as_slice(), &tmux_wrap(b"x")].concat(),
        );
        assert_eq!(writes[0].cursor, CursorPosition { row: 2, col: 4 });
    }

    #[test]
    fn decrqss_sgr_reports_the_current_pen() {
        let mut backend = backend(PtySize::new(10, 2));
        let mut router = RawByteRouter::default();
        let _ = backend.take_damage();

        // Neovim's undercurl probe: set a curly underline, then ask for SGR.
        router.route_output(&mut backend, b"\x1b[0m\x1b[4:3m\x1bP$qm\x1b\\");
        let reply = String::from_utf8(backend.take_events().replies).expect("utf8");
        assert_eq!(reply, "\x1bP1$r0;4:3m\x1b\\");

        router.route_output(
            &mut backend,
            b"\x1b[0;1;38;5;208;58:2::255:0:0m\x1bP$qm\x1b\\\x1b[0m\x1bP$qm\x1b\\",
        );
        let reply = String::from_utf8(backend.take_events().replies).expect("utf8");
        assert_eq!(
            reply,
            "\x1bP1$r0;1;38:5:208;58:2::255:0:0m\x1b\\\x1bP1$r0m\x1b\\"
        );

        // Settings other than SGR get the "invalid request" reply.
        router.route_output(&mut backend, b"\x1bP$qr\x1b\\");
        let reply = String::from_utf8(backend.take_events().replies).expect("utf8");
        assert_eq!(reply, "\x1bP0$r\x1b\\");
    }

    #[test]
    fn xtgettcap_advertises_clipboard_and_underline_capabilities() {
        let mut backend = backend(PtySize::new(10, 2));
        let mut router = RawByteRouter::default();
        let _ = backend.take_damage();

        // "Ms" (as Neovim sends it) and an unknown "TN".
        router.route_output(&mut backend, b"\x1bP+q4D73;544e\x1b\\");
        let reply = String::from_utf8(backend.take_events().replies).expect("utf8");
        let mut expected_ms = String::from("\x1bP1+r4D73=");
        for byte in b"\x1b]52;%p1%s;%p2%s\x07" {
            expected_ms.push_str(&format!("{byte:02X}"));
        }
        expected_ms.push_str("\x1b\\");
        assert_eq!(reply, format!("{expected_ms}\x1bP0+r544E\x1b\\"));
    }

    #[test]
    fn osc52_query_is_denied_without_a_reply() {
        let mut backend = backend(PtySize::new(10, 2));
        let _ = backend.take_damage();

        backend.ingest_bytes(b"\x1b]52;c;?\x07");

        let events = backend.take_events();
        assert_eq!(events.clipboard, None);
        assert!(events.replies.is_empty(), "replies: {:?}", events.replies);
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
