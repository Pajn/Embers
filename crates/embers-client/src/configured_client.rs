use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use embers_core::{
    ActivityState, BufferId, ClipboardSelection, CursorPosition, CursorShape, FloatGeometry,
    FloatingId, MuxError, NodeId, PassthroughWrite, Point, PtySize, Result, SessionId, Size,
};
use embers_protocol::{
    BufferLocation, BufferLocationAttachment, BufferRecord, BufferRecordState, BufferRequest,
    BufferResponse, ClientMessage, FloatingRequest, InputRequest, NodeRequest, PassthroughEvent,
    ServerEvent, ServerResponse, SessionRequest, buffer_ids_in_subtree,
};

use crate::RenderGrid;
use crate::chooser::{Chooser, ChooserItem, ChooserScope, chooser_area, window_position};
use crate::client::MuxClient;
use crate::command_prompt::{
    CommandContext, CommandPrompt, CommandRunner, StatusKeys, record_history,
};
use crate::config::ConfigManager;
use crate::controller::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::copy_mode::{
    CapturedText, CopyModeState, CopyMotion, CopyViewport, JumpKind, MotionOutcome, MouseSelection,
    apply_motion, jump, next_match_index, word_bounds,
};
use crate::grid::{CellStyle, GridCursor};
use crate::host_terminal::{HostCapabilities, PendingAttachQueries};
use crate::input::{
    COPY_MODE, FallbackPolicy, HINTS_MODE, InputResolution, InputState, KeyToken, NORMAL_MODE,
    PROMPT_MODE, SEARCH_MODE, SELECT_MODE, TREE_MODE, resolve_key,
};
use crate::presentation::{
    LayoutOptions, LeafFrame, NavigationDirection, PaneTitles, PresentationModel, RootTabBar,
    StatusLineMode,
};
use crate::pty_sizes::PtySizeSync;
use crate::renderer::Renderer;
use crate::scripting::{
    Action, BarSpec, Context, DetachOnDestroy, EventInfo, FloatingAnchor, FloatingGeometrySpec,
    FloatingSize, MouseSettings, NotifyLevel, PassthroughMode, TabBarContext, TreeSpec,
};
use crate::spawn_size::{PanePlacement, spawn_pty_size_with};
use crate::split_resize::SplitResize;
use crate::state::{
    HintsState, SearchMatch, SearchState, SelectionKind, SelectionPoint, SelectionState,
};
use crate::transport::Transport;

const WHEEL_SCROLL_LINES: u64 = 3;
/// How long `display-message` text stays in the status line unless the
/// request gives its own time.
pub const DEFAULT_DISPLAY_TIME: Duration = Duration::from_secs(3);
const MAX_EXPANDED_ACTIONS: usize = 256;
/// Most events `process_ready_events_timeout` handles before the caller gets
/// to draw, so a flood of output cannot hold off input and frames for long.
const MAX_EVENT_BATCH: usize = 256;
/// Most passthrough bytes held for the next frame. Frames follow events
/// promptly, so this only bounds a client that has stopped drawing.
const MAX_PENDING_PASSTHROUGH_BYTES: usize = 32 * 1024 * 1024;

/// Where on the host screen a passthrough write from `leaf` goes: the pane's
/// `cursor`, clamped into the pane's content area. `None` when the pane shows
/// no content or that cell is off screen.
fn passthrough_cursor(
    leaf: &LeafFrame,
    cursor: CursorPosition,
    viewport: Size,
) -> Option<(u16, u16)> {
    let pty = leaf.pty_size()?;
    let origin = leaf.content_rect.origin;
    let x = i64::from(origin.x) + i64::from(cursor.col.min(pty.cols - 1));
    let y = i64::from(origin.y) + i64::from(cursor.row.min(pty.rows - 1));
    let x = u16::try_from(x).ok().filter(|x| *x < viewport.width)?;
    let y = u16::try_from(y).ok().filter(|y| *y < viewport.height)?;
    Some((x, y))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchPrompt {
    node_id: NodeId,
    query: String,
    /// `?` rather than `/`.
    backward: bool,
    /// Mode to return to once the prompt is committed or cancelled.
    return_mode: String,
}

/// A `confirm-before` question waiting for its answer.
struct PendingConfirm {
    /// The question, as shown in the status line.
    prompt: String,
    /// What runs if the answer is `y`.
    actions: Vec<Action>,
    /// Mode to return to once answered.
    return_mode: String,
}

/// A `display-message` shown in the status line until `until`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusMessage {
    text: String,
    until: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DividerDrag {
    split_node_id: NodeId,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedTreeBuffer {
    Existing(BufferId),
    NewlySpawned(BufferId),
}

impl ResolvedTreeBuffer {
    fn id(self) -> BufferId {
        match self {
            Self::Existing(buffer_id) | Self::NewlySpawned(buffer_id) => buffer_id,
        }
    }

    fn created_by_helper(self) -> Option<BufferId> {
        match self {
            Self::Existing(_) => None,
            Self::NewlySpawned(buffer_id) => Some(buffer_id),
        }
    }
}

pub struct ConfiguredClient<T> {
    client: MuxClient<T>,
    config: ConfigManager,
    input_state: InputState,
    notifications: Vec<String>,
    /// Until when the latest notification shows over the tab bar.
    notification_until: Option<Instant>,
    active_session_id: Option<SessionId>,
    previous_session_id: Option<SessionId>,
    viewport: Option<Size>,
    pty_sizes: PtySizeSync,
    /// The viewport and whether panes have title rows, last reported to the
    /// server, so they are only sent again when they change.
    reported_viewport: Option<(SessionId, Size, bool)>,
    /// The base index last reported to the server, and for which session.
    reported_base_index: Option<(SessionId, Option<u32>)>,
    search_prompt: Option<SearchPrompt>,
    command_prompt: Option<CommandPrompt>,
    /// An open `confirm-before` question, shown in the status line.
    confirm: Option<PendingConfirm>,
    /// Command lines run from the prompt, oldest first.
    prompt_history: VecDeque<String>,
    command_runner: Option<Arc<dyn CommandRunner>>,
    chooser: Option<Chooser>,
    status_message: Option<StatusMessage>,
    hints_node: Option<NodeId>,
    copy: CopyModeState,
    /// The divider being dragged with the mouse, from the press on it until
    /// the button is released.
    divider_drag: Option<DividerDrag>,
    terminal_output: VecDeque<Vec<u8>>,
    /// Passthrough writes from panes, oldest first, waiting for the next frame.
    pending_passthrough: Vec<(BufferId, PassthroughWrite)>,
    pending_passthrough_bytes: usize,
    /// The buffer whose passthrough write last reached the host terminal: the
    /// pane a host reply to a query sent through passthrough is meant for.
    last_passthrough_buffer: Option<BufferId>,
    /// The client's attach queries whose late replies are still expected.
    pending_attach_queries: PendingAttachQueries,
    /// Host terminal output that must follow the frame `render_session` last
    /// returned (see [`ConfiguredClient::drain_frame_output`]).
    frame_output: Vec<u8>,
    /// What the host terminal was detected to support, before config
    /// overrides.
    detected_host: HostCapabilities,
    socket_path: Option<PathBuf>,
    /// Notifications produced by background tasks (e.g. a failed `run_shell`
    /// child), drained into `notifications` on the next input/event tick.
    background_notifications: Arc<Mutex<Vec<String>>>,
    /// Set once the session this client showed was destroyed and
    /// `detach-on-destroy` left it nowhere to go (see [`Self::should_exit`]).
    exit_requested: bool,
}

impl<T> ConfiguredClient<T>
where
    T: Transport,
{
    pub fn new(client: MuxClient<T>, config: ConfigManager) -> Self {
        Self {
            client,
            config,
            input_state: InputState::default(),
            notifications: Vec::new(),
            notification_until: None,
            active_session_id: None,
            previous_session_id: None,
            viewport: None,
            pty_sizes: PtySizeSync::default(),
            reported_viewport: None,
            reported_base_index: None,
            search_prompt: None,
            command_prompt: None,
            confirm: None,
            prompt_history: VecDeque::new(),
            command_runner: None,
            chooser: None,
            status_message: None,
            hints_node: None,
            copy: CopyModeState::default(),
            divider_drag: None,
            terminal_output: VecDeque::new(),
            pending_passthrough: Vec::new(),
            pending_passthrough_bytes: 0,
            last_passthrough_buffer: None,
            pending_attach_queries: PendingAttachQueries::default(),
            frame_output: Vec::new(),
            detected_host: HostCapabilities::default(),
            socket_path: None,
            background_notifications: Arc::new(Mutex::new(Vec::new())),
            exit_requested: false,
        }
    }

    /// Whether the interactive client should exit: the session it showed was
    /// destroyed and, per the `detach-on-destroy` setting, it detaches rather
    /// than moving to another session, or no other session is left.
    pub fn should_exit(&self) -> bool {
        self.exit_requested
    }

    /// Record the socket this client is attached to, so `run_shell` children can
    /// drive Embers via `$EMBERS_SOCKET` and the `embers` CLI (tmux `run-shell`).
    pub fn set_socket_path(&mut self, socket_path: PathBuf) {
        self.socket_path = Some(socket_path);
    }

    /// Install what runs command lines from the command prompt and
    /// `action.run_command`. Without one, those report that commands are
    /// unavailable.
    pub fn set_command_runner(&mut self, runner: Arc<dyn CommandRunner>) {
        self.command_runner = Some(runner);
    }

    /// The open session/window/pane picker, if any.
    pub fn chooser(&self) -> Option<&Chooser> {
        self.chooser.as_ref()
    }

    /// The open command prompt, if any.
    pub fn command_prompt(&self) -> Option<&CommandPrompt> {
        self.command_prompt.as_ref()
    }

    /// Where the terminal cursor belongs in the status line, as a display
    /// column, while the command prompt is open.
    pub fn status_cursor(&self) -> Option<u16> {
        let column = match (&self.command_prompt, &self.confirm) {
            (Some(prompt), _) => prompt.cursor_column(),
            (None, Some(confirm)) => UnicodeWidthStr::width(confirm.prompt.as_str()),
            (None, None) => return None,
        };
        Some(u16::try_from(column).unwrap_or(u16::MAX))
    }

    pub fn client(&self) -> &MuxClient<T> {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut MuxClient<T> {
        &mut self.client
    }

    pub fn config(&self) -> &ConfigManager {
        &self.config
    }

    pub fn notifications(&self) -> &[String] {
        &self.notifications
    }

    /// The window base index (tmux `base-index`): the config's
    /// `set_base_index`, else `EMBERS_BASE_INDEX`, else `None` for numbering
    /// from 0.
    pub fn base_index(&self) -> Option<u32> {
        self.config
            .active_script()
            .loaded_config()
            .base_index
            .or_else(crate::numbering::base_index_from_env)
    }

    /// The name of the active input mode (e.g. "normal", "hints").
    pub fn current_mode(&self) -> &str {
        self.input_state.current_mode()
    }

    /// The session the client is currently focused on, if any.
    pub fn active_session_id(&self) -> Option<SessionId> {
        self.active_session_id
    }

    /// The session the client was focused on before the current one, used by
    /// `last_session` (tmux `switch-client -l`).
    pub fn previous_session_id(&self) -> Option<SessionId> {
        self.previous_session_id
    }

    /// Show `text` in the status line for `duration` (tmux `display-message`).
    /// Control characters are replaced, so the text cannot reach the host
    /// terminal as escape sequences.
    pub fn show_message(&mut self, text: &str, duration: Duration) {
        self.status_message = Some(StatusMessage {
            text: crate::grid::replace_controls(text).into_owned(),
            until: Instant::now() + duration,
        });
    }

    /// The `display-message` text in the status line, if it has not expired.
    pub fn status_message(&self) -> Option<&str> {
        self.status_message
            .as_ref()
            .filter(|message| message.until > Instant::now())
            .map(|message| message.text.as_str())
    }

    /// Drop a `display-message` or warning whose time is up. Returns `true`
    /// when one was dropped, so the caller redraws the status line.
    pub fn expire_status_message(&mut self) -> bool {
        let now = Instant::now();
        let mut expired = false;
        if self
            .status_message
            .as_ref()
            .is_some_and(|message| message.until <= now)
        {
            self.status_message = None;
            expired = true;
        }
        if self.notification_until.is_some_and(|until| until <= now) {
            self.notification_until = None;
            expired = true;
        }
        expired
    }

    pub fn drain_terminal_output(&mut self) -> Vec<Vec<u8>> {
        self.terminal_output.drain(..).collect()
    }

    /// Host terminal output to write right after the frame `render_session`
    /// returned, unlike [`ConfiguredClient::drain_terminal_output`], which goes
    /// before it: passthrough sequences from programs in panes. Each one from a
    /// pane on screen starts by moving the cursor to that pane's cursor, so the
    /// caller puts its own cursor back afterwards. Empty when there is nothing
    /// to write.
    pub fn drain_frame_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.frame_output)
    }

    /// Record what detection found the host terminal supports. Until this is
    /// called the client assumes it supports everything.
    pub fn set_host_capabilities(&mut self, detected: HostCapabilities) {
        self.detected_host = detected;
    }

    /// What the host terminal supports: what detection found, with the
    /// config's `set_terminal_features` overrides applied.
    pub fn host_capabilities(&self) -> HostCapabilities {
        self.detected_host.with_overrides(
            self.config
                .active_script()
                .loaded_config()
                .terminal_features,
        )
    }

    /// Which panes may write to the host terminal through passthrough: the
    /// config's choice, or none when the host does not support passthrough.
    fn passthrough_mode(&self) -> PassthroughMode {
        if !self.host_capabilities().passthrough {
            return PassthroughMode::Off;
        }
        self.config.active_script().loaded_config().passthrough
    }

    /// Hold passthrough writes from `buffer_id` for the next frame. The pane is
    /// refreshed first, so that frame shows everything the program printed
    /// before them.
    async fn queue_passthrough(&mut self, event: &PassthroughEvent) {
        if self.passthrough_mode() == PassthroughMode::Off || self.active_session_id.is_none() {
            return;
        }
        let buffer_id = event.buffer_id;
        if let Err(error) = self.client.refresh_buffer_snapshot(buffer_id).await {
            warn!(%error, %buffer_id, "refreshing a pane for passthrough failed");
        }
        for write in &event.writes {
            if self.pending_passthrough_bytes + write.bytes.len() > MAX_PENDING_PASSTHROUGH_BYTES {
                warn!(%buffer_id, "too much passthrough waiting for a frame; dropping");
                continue;
            }
            self.pending_passthrough_bytes += write.bytes.len();
            self.pending_passthrough.push((buffer_id, write.clone()));
        }
    }

    /// Turn the held passthrough writes into output for the frame drawn from
    /// `presentation`. A write from a pane on screen goes to that pane's
    /// cursor (tmux `allow-passthrough on`); one from a pane that is not is
    /// written where the cursor is with `all`, and dropped otherwise.
    fn flush_passthrough(&mut self, presentation: &PresentationModel) {
        if self.pending_passthrough.is_empty() {
            return;
        }
        let mode = self.passthrough_mode();
        let pending = std::mem::take(&mut self.pending_passthrough);
        self.pending_passthrough_bytes = 0;
        let mut output = Vec::new();
        for (buffer_id, write) in pending {
            let cursor = presentation
                .leaves
                .iter()
                .find(|leaf| leaf.buffer_id == buffer_id)
                .and_then(|leaf| passthrough_cursor(leaf, write.cursor, presentation.viewport));
            match (mode, cursor) {
                (PassthroughMode::On | PassthroughMode::All, Some((x, y))) => {
                    output.extend_from_slice(
                        format!("\x1b[{};{}H", u32::from(y) + 1, u32::from(x) + 1).as_bytes(),
                    );
                    output.extend_from_slice(&write.bytes);
                }
                (PassthroughMode::All, None) => output.extend_from_slice(&write.bytes),
                _ => continue,
            }
            self.last_passthrough_buffer = Some(buffer_id);
        }
        if output.is_empty() {
            return;
        }
        // Hide the cursor while it jumps around, and leave the pen at its
        // default: the next frame assumes both.
        self.frame_output.extend_from_slice(b"\x1b[?25l");
        self.frame_output.extend_from_slice(&output);
        self.frame_output.extend_from_slice(b"\x1b[0m");
    }

    /// Emit an OSC 2 window-title update naming the given session, so the host
    /// terminal's tab/window title follows the active session (tmux
    /// `set-titles-string '#S'`). Called by the interactive client on attach; it
    /// also fires on session switch and rename. No reset is emitted on exit —
    /// terminals restore their own title, matching tmux.
    pub fn emit_terminal_title(&mut self, session_id: SessionId) {
        let Some(session_name) = self
            .client
            .state()
            .sessions
            .get(&session_id)
            .map(|session| session.name.clone())
        else {
            return;
        };
        // Strip control characters so a session name can't terminate the OSC 2
        // sequence early or inject its own escape sequences into the host terminal.
        let title: String = session_name.chars().filter(|ch| !ch.is_control()).collect();
        self.terminal_output
            .push_back(format!("\x1b]2;{title}\x07").into_bytes());
    }

    /// The status line's text: what [`Self::status_text`] shows, else the
    /// latest warning, else the session and how to quit.
    pub fn status_line(&self, session_id: SessionId, socket_path: &Path) -> String {
        if let Some(text) = self.status_text(session_id) {
            return text;
        }
        let session_name = self.session_name(session_id);
        match self.notifications.last() {
            Some(message) => format!("[{session_name}] {message}"),
            None => format!("[{session_name}] {}  ctrl-q quit", socket_path.display()),
        }
    }

    /// What the status line has to show right now: the command prompt, a
    /// `confirm-before` question, the copy-mode search prompt, a
    /// `display-message`, or a warning less than the display time old. `None`
    /// when there is nothing, so the root tab bar shows instead.
    pub fn status_text(&self, session_id: SessionId) -> Option<String> {
        if let Some(prompt) = &self.command_prompt {
            return Some(prompt.display());
        }
        if let Some(confirm) = &self.confirm {
            return Some(confirm.prompt.clone());
        }
        let session_name = self.session_name(session_id);
        if let Some(prompt) = &self.search_prompt {
            let prefix = if prompt.backward { '?' } else { '/' };
            return Some(format!("[{session_name}] {prefix}{}", prompt.query));
        }
        if let Some(message) = self.status_message() {
            return Some(message.to_owned());
        }
        let now = Instant::now();
        self.notifications
            .last()
            .filter(|_| self.notification_until.is_some_and(|until| until > now))
            .map(|message| format!("[{session_name}] {message}"))
    }

    fn session_name(&self, session_id: SessionId) -> &str {
        self.client
            .state()
            .sessions
            .get(&session_id)
            .map(|session| session.name.as_str())
            .unwrap_or("<missing>")
    }

    /// Draw [`Self::status_text`] over `presentation`'s status row of `grid`,
    /// with the cursor at the command prompt's.
    fn draw_status_overlay(
        &self,
        grid: &mut RenderGrid,
        presentation: &PresentationModel,
        session_id: SessionId,
    ) {
        let (Some(row), Some(text)) = (presentation.status_row(), self.status_text(session_id))
        else {
            return;
        };
        let style = CellStyle::default().with_reverse();
        grid.put_str_styled(0, row, &" ".repeat(usize::from(grid.width())), style);
        grid.put_str_styled(0, row, &crate::grid::replace_controls(&text), style);
        if let Some(column) = self.status_cursor() {
            grid.set_cursor(Some(GridCursor {
                x: column.min(grid.width().saturating_sub(1)),
                y: row,
                shape: CursorShape::Beam,
            }));
        }
    }

    pub async fn handle_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        key: KeyEvent,
    ) -> Result<()> {
        self.drain_background_notifications();
        self.pty_sizes.note_user_activity();
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;

        // Under report-all-keys the host terminal sends CSI-u for every key,
        // including ordinary typed text. Normalise an unmodified text key to its
        // legacy form up front so every downstream path — binding tokenization,
        // hints label matching, and search input — treats both encodings alike.
        let key = normalize_text_key(key);

        if self.input_state.current_mode() == PROMPT_MODE {
            if let Some(confirm) = self.confirm.take() {
                return self
                    .answer_confirm(session_id, viewport, confirm, key)
                    .await;
            }
            return self.handle_prompt_key(session_id, key).await;
        }

        if self.input_state.current_mode() == TREE_MODE {
            return self.handle_chooser_key(session_id, viewport, key).await;
        }

        if self.input_state.current_mode() == SEARCH_MODE {
            return self
                .handle_search_key(session_id, viewport, &presentation, key)
                .await;
        }

        if self.input_state.current_mode() == HINTS_MODE {
            return self
                .handle_hints_key(session_id, viewport, &presentation, key)
                .await;
        }

        if self.in_copy_mode() {
            // `f`/`F`/`t`/`T` take the next key as their target; any key that is
            // not a character cancels the jump.
            if let Some((kind, count)) = self.copy.pending_jump.take() {
                if let KeyEvent::Char(target) = key {
                    let leaf = self.focused_leaf(&presentation)?;
                    self.copy.last_jump = Some((kind, target));
                    return self
                        .copy_jump(leaf.node_id, kind, target, count, false)
                        .await;
                }
                return Ok(());
            }
            if let KeyEvent::Char(ch) = key
                && self.accepts_count_digit(ch)
            {
                self.copy
                    .push_count_digit(ch.to_digit(10).expect("checked digit"));
                return Ok(());
            }
        }

        match key {
            KeyEvent::Bytes(bytes) => {
                if self.current_fallback_policy() != FallbackPolicy::Passthrough {
                    return Ok(());
                }
                let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                self.send_typed_input(&presentation, buffer_id, session_id, |_, _| {
                    Ok(bytes.clone())
                })
                .await
            }
            other => {
                let token = key_event_to_token(other)?;
                match resolve_key(
                    &self.config.active_script().loaded_config().bindings,
                    &self.config.active_script().loaded_config().modes,
                    &mut self.input_state,
                    token,
                ) {
                    InputResolution::ExactMatch(binding) => {
                        // A count typed before the binding applies to the first
                        // motion it runs.
                        self.copy.active_count = self.copy.pending_count.take();
                        if self.should_passthrough_binding_in_alternate_screen(
                            &presentation,
                            &binding.target,
                        ) {
                            self.copy.active_count = None;
                            let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                            return self
                                .send_typed_input(
                                    &presentation,
                                    buffer_id,
                                    session_id,
                                    |client, target| {
                                        sequence_to_bytes(
                                            &binding.sequence,
                                            client.buffer_keyboard_mode(target),
                                        )
                                    },
                                )
                                .await;
                        }
                        let result = self
                            .execute_actions(
                                Some(session_id),
                                Some(viewport),
                                binding.target.clone(),
                            )
                            .await;
                        self.copy.active_count = None;
                        result
                    }
                    InputResolution::PrefixMatch => Ok(()),
                    InputResolution::Unmatched {
                        sequence,
                        fallback_policy,
                        ..
                    } => {
                        self.copy.pending_count = None;
                        match fallback_policy {
                            FallbackPolicy::Passthrough => {
                                let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                                self.send_typed_input(
                                    &presentation,
                                    buffer_id,
                                    session_id,
                                    |client, target| {
                                        sequence_to_bytes(
                                            &sequence,
                                            client.buffer_keyboard_mode(target),
                                        )
                                    },
                                )
                                .await
                            }
                            FallbackPolicy::Ignore => Ok(()),
                        }
                    }
                }
            }
        }
    }

    pub async fn handle_paste(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        bytes: Vec<u8>,
    ) -> Result<()> {
        self.pty_sizes.note_user_activity();
        self.set_active_view(session_id, viewport);
        if self.input_state.current_mode() == PROMPT_MODE {
            if let Some(prompt) = &mut self.command_prompt {
                prompt.insert(&String::from_utf8_lossy(&bytes));
            }
            return Ok(());
        }
        if self.input_state.current_mode() == TREE_MODE {
            if let Some(chooser) = &mut self.chooser
                && chooser.editing_filter
            {
                chooser.filter.push_str(&String::from_utf8_lossy(&bytes));
            }
            return Ok(());
        }
        if self.input_state.current_mode() == SEARCH_MODE {
            return self.handle_search_paste(session_id, viewport, bytes).await;
        }
        if self.current_fallback_policy() != FallbackPolicy::Passthrough {
            return Ok(());
        }

        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let buffer_id = self.resolve_buffer_id(None, &presentation)?;
        self.send_typed_input(&presentation, buffer_id, session_id, |client, target| {
            Ok(
                if client
                    .client
                    .state()
                    .snapshots
                    .get(&target)
                    .is_some_and(|snapshot| snapshot.bracketed_paste)
                {
                    let mut wrapped = b"\x1b[200~".to_vec();
                    wrapped.extend_from_slice(&bytes);
                    wrapped.extend_from_slice(b"\x1b[201~");
                    wrapped
                } else {
                    bytes.clone()
                },
            )
        })
        .await
    }

    pub async fn handle_mouse(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        event: MouseEvent,
    ) -> Result<()> {
        self.pty_sizes.note_user_activity();
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let point = Point {
            x: i32::from(event.column),
            y: i32::from(event.row),
        };
        let settings = self.config.active_script().loaded_config().mouse;
        if self
            .handle_divider_drag(session_id, &presentation, point, event.kind, settings)
            .await?
        {
            return Ok(());
        }
        // A drag selection stays with the pane it started in, wherever the
        // pointer goes.
        if self.copy.mouse.is_some()
            && matches!(
                event.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Release(_)
            )
        {
            return self
                .continue_mouse_selection(&presentation, point, event.kind)
                .await;
        }
        let Some(target_leaf) = self.mouse_target_leaf(&presentation, point).cloned() else {
            return Ok(());
        };

        let target_snapshot = self.client.state().snapshots.get(&target_leaf.buffer_id);
        let mouse_reporting = target_snapshot.is_some_and(|snapshot| snapshot.mouse_reporting);
        let point_in_content = target_leaf.content_rect.contains(point);

        match event.kind {
            MouseEventKind::WheelUp | MouseEventKind::WheelDown => {
                if mouse_reporting && settings.wheel_forward && point_in_content {
                    return self
                        .send_bytes_to_buffer(
                            target_leaf.buffer_id,
                            session_id,
                            encode_mouse_event(&target_leaf, event)?,
                        )
                        .await;
                }
                if settings.wheel_scroll && !self.view_is_alternate_screen(target_leaf.node_id) {
                    // Scrolling replaces this pane's visible_lines directly (not via
                    // a snapshot apply, which would invalidate hints), so hint labels
                    // captured against the old viewport would map to the wrong
                    // content. Clear them when the hinted pane itself is scrolled;
                    // scrolling a different pane leaves the hinted pane untouched.
                    if self.hints_node == Some(target_leaf.node_id) {
                        self.cancel_active_hints();
                    }
                    let delta = match event.kind {
                        MouseEventKind::WheelUp => -(WHEEL_SCROLL_LINES as i64),
                        MouseEventKind::WheelDown => WHEEL_SCROLL_LINES as i64,
                        _ => 0,
                    };
                    return self.scroll_view_by(target_leaf.node_id, delta).await;
                }
                Ok(())
            }
            MouseEventKind::Press(_) | MouseEventKind::Release(_) | MouseEventKind::Drag(_) => {
                if settings.click_focus && !target_leaf.focused {
                    // Focus is moving to a different pane; a hints overlay on the
                    // previously focused node is now stale, so tear it down.
                    self.cancel_active_hints();
                    self.focus_node(session_id, target_leaf.node_id).await?;
                }
                if settings.click_forward && mouse_reporting && point_in_content {
                    self.send_bytes_to_buffer(
                        target_leaf.buffer_id,
                        session_id,
                        encode_mouse_event(&target_leaf, event)?,
                    )
                    .await?;
                } else if settings.select
                    && !mouse_reporting
                    && point_in_content
                    && event.kind == MouseEventKind::Press(MouseButton::Left)
                {
                    self.begin_mouse_selection(&target_leaf, point).await?;
                }
                Ok(())
            }
        }
    }

    pub async fn handle_focus_event(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        focused: bool,
    ) -> Result<()> {
        if focused {
            self.pty_sizes.note_user_activity();
        }
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let buffer_id = self.resolve_buffer_id(None, &presentation)?;
        if !self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .is_some_and(|snapshot| snapshot.focus_reporting)
        {
            return Ok(());
        }

        let bytes = if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        };
        self.send_bytes_to_buffer(buffer_id, session_id, bytes)
            .await
    }

    /// Record the attach queries the host had not answered when attaching
    /// stopped waiting, so their late replies are dropped (see
    /// [`ConfiguredClient::handle_host_reply`]).
    pub fn set_pending_attach_queries(&mut self, pending: PendingAttachQueries) {
        self.pending_attach_queries = pending;
    }

    /// Deliver `bytes`, a reply from the host terminal (a DCS, APC or OSC
    /// string, or a CSI report), to the pane it is meant for. A reply is not
    /// typed input: it is never copied to synchronized panes, inserted into
    /// the command prompt or chooser filter, or matched against bindings.
    /// A reply to one of the client's own attach queries that is still
    /// pending is dropped, and the query marked answered (see
    /// [`PendingAttachQueries`]); an identical reply after that is a pane's.
    ///
    /// The host only answers what it was sent, and the only queries that do
    /// not come from the client come from panes through passthrough, so a
    /// reply goes to the pane whose passthrough write last reached the host.
    /// When there is none, or its buffer is gone, it goes to the focused pane.
    pub async fn handle_host_reply(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        bytes: Vec<u8>,
    ) -> Result<()> {
        if self
            .pending_attach_queries
            .take_reply(&bytes, Instant::now())
        {
            debug!("dropped a reply to the attach queries");
            return Ok(());
        }
        self.set_active_view(session_id, viewport);
        let source = self
            .last_passthrough_buffer
            .filter(|buffer_id| self.client.state().buffers.contains_key(buffer_id));
        let buffer_id = match source {
            Some(buffer_id) => buffer_id,
            None => {
                let presentation = self.prepare_presentation(session_id, viewport).await?;
                self.resolve_buffer_id(None, &presentation)?
            }
        };
        self.send_input(session_id, vec![(buffer_id, bytes)]).await
    }

    pub async fn process_next_event(&mut self) -> Result<ServerEvent> {
        let event = self.next_event().await?;
        self.handle_event(&event).await?;
        Ok(event)
    }

    pub async fn process_next_event_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<ServerEvent>> {
        let Some(event) = tokio::time::timeout(timeout, self.client.next_event())
            .await
            .ok()
            .transpose()?
        else {
            return Ok(None);
        };
        self.handle_event(&event).await?;
        Ok(Some(event))
    }

    /// Waits up to `timeout` for a server event, then handles it together with
    /// every event that has already arrived behind it, and returns the handled
    /// events in order.
    ///
    /// Handling one event per frame lets a busy pane's `RenderInvalidated`
    /// events arrive faster than they are drawn, so a layout change queued
    /// behind them, and the PTY resizes the next frame sends for it, would wait
    /// for the whole backlog. Within a batch only the last `RenderInvalidated`
    /// of each buffer is handled: it refreshes to the same latest snapshot the
    /// earlier ones would have.
    pub async fn process_ready_events_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Vec<ServerEvent>> {
        let Some(first) = tokio::time::timeout(timeout, self.client.next_event())
            .await
            .ok()
            .transpose()?
        else {
            return Ok(Vec::new());
        };
        let mut batch = vec![first];
        while batch.len() < MAX_EVENT_BATCH
            && let Some(event) = self.client.ready_event().await?
        {
            batch.push(event);
        }
        let batch = coalesce_render_invalidations(batch);
        for event in &batch {
            self.handle_event(event).await?;
        }
        Ok(batch)
    }

    pub async fn next_event(&mut self) -> Result<ServerEvent> {
        self.client.next_event().await
    }

    pub async fn handle_event(&mut self, event: &ServerEvent) -> Result<()> {
        // The server is stopping on purpose and about to close the
        // connection: nothing is left to ask it, so just leave.
        if matches!(event, ServerEvent::ServerShutdown) {
            self.exit_requested = true;
            return Ok(());
        }
        self.drain_background_notifications();
        let previous_render_activity = match event {
            ServerEvent::RenderInvalidated(render) => self
                .client
                .state()
                .buffers
                .get(&render.buffer_id)
                .map(|buffer| buffer.activity),
            _ => None,
        };
        let detached_session_id = matches!(event, ServerEvent::BufferDetached(_))
            .then(|| self.event_session_id(event))
            .flatten();
        self.client.handle_event(event).await?;
        if let ServerEvent::RenderInvalidated(event) = event {
            self.client.refresh_buffer_snapshot(event.buffer_id).await?;
        }
        // Keep last-session tracking in sync when our own session changes through
        // the server (e.g. another client or a server-driven switch).
        if let ServerEvent::ClientChanged(changed) = event
            && self.client.cached_client_id() == Some(changed.client.id)
        {
            match changed.client.current_session_id {
                Some(session_id) => {
                    self.set_active_session(session_id);
                    self.emit_terminal_title(session_id);
                }
                // The session this client showed was destroyed: move on as
                // detach-on-destroy says.
                None if changed.previous_session_id.is_some_and(|session_id| {
                    !self.client.state().sessions.contains_key(&session_id)
                }) =>
                {
                    self.leave_destroyed_session().await?
                }
                // The server detached this client from every session; drop the
                // now-stale active session instead of leaving it dangling.
                None => self.clear_active_session(),
            }
        }
        if let ServerEvent::DisplayMessage(message) = event {
            self.show_message(&message.message, display_duration(message.duration_ms));
        }
        // A program in a pane set the clipboard (OSC 52): pass it on to the
        // host terminal unless the config turned that off.
        if let ServerEvent::Clipboard(clipboard) = event
            && self.config.active_script().loaded_config().clipboard
        {
            self.enqueue_osc52(clipboard.selection, &clipboard.text);
        }
        if let ServerEvent::Passthrough(passthrough) = event {
            self.queue_passthrough(passthrough).await;
        }
        // Follow the active session's name in the host terminal title.
        if let ServerEvent::SessionRenamed(renamed) = event
            && self.active_session_id == Some(renamed.session_id)
        {
            self.emit_terminal_title(renamed.session_id);
        }
        // A server-driven focus change (e.g. another client) that moves focus off
        // our hints node orphans the overlay, so drop it and leave hints mode.
        if let ServerEvent::FocusChanged(focus) = event
            && self.active_session_id == Some(focus.session_id)
            && self.hints_node.is_some()
            && focus.focused_leaf_id != self.hints_node
        {
            self.cancel_active_hints();
        }
        let session_id = detached_session_id.or_else(|| self.event_session_id(event));

        let mut event_names = vec![event_name(event).to_owned()];
        if let ServerEvent::RenderInvalidated(render) = event
            && previous_render_activity != Some(ActivityState::Bell)
            && self
                .client
                .state()
                .buffers
                .get(&render.buffer_id)
                .is_some_and(|buffer| buffer.activity == ActivityState::Bell)
        {
            event_names.push("buffer_bell".to_owned());
        }

        for event_name in event_names {
            let context = self.context_for(
                session_id,
                self.viewport,
                Some(event_info(&event_name, event, session_id)),
            );
            match self
                .config
                .active_script()
                .dispatch_event(&event_name, context)
            {
                Ok(actions) if !actions.is_empty() => {
                    self.execute_actions(session_id, self.viewport, actions)
                        .await?;
                }
                Ok(_) => {}
                Err(error) => self.record_notification(error.to_string()),
            }
        }
        Ok(())
    }

    pub async fn render_session(
        &mut self,
        session_id: SessionId,
        viewport: Size,
    ) -> Result<RenderGrid> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let mut custom_bars = BTreeMap::<embers_core::NodeId, BarSpec>::new();
        let mut recorded_formatter_error = false;
        for tabs in presentation
            .tab_bars
            .iter()
            .filter(|tabs| tabs.rect.size.height > 0)
        {
            let bar_context =
                TabBarContext::from_frame(tabs, self.input_state.current_mode(), viewport.width)
                    .with_base_index(self.base_index())
                    .with_session_name(self.session_name(session_id));
            let result = self.config.active_script().format_tab_bar(bar_context);

            match result {
                Ok(Some(bar)) => {
                    custom_bars.insert(tabs.node_id, bar);
                }
                Ok(None) => {}
                Err(error) if !recorded_formatter_error => {
                    recorded_formatter_error = true;
                    // Every frame fails the same way; showing it once, for
                    // the display time, lets the default bar show after.
                    let message = error.to_string();
                    if self.notifications.last() != Some(&message) {
                        self.record_notification(message);
                    }
                }
                Err(_) => {}
            }
        }

        let renderer = Renderer::new(
            self.config
                .active_script()
                .loaded_config()
                .theme
                .pane_borders,
        );
        let mut grid =
            renderer.render_with_tab_bars(self.client.state(), &presentation, &custom_bars);
        self.flush_passthrough(&presentation);
        if let Some(chooser) = &mut self.chooser {
            let rows = chooser.rows(self.client.state(), self.active_session_id, {
                self.config
                    .active_script()
                    .loaded_config()
                    .base_index
                    .or_else(crate::numbering::base_index_from_env)
            });
            chooser.render(&mut grid, chooser_area(viewport), &rows);
            grid.set_cursor(None);
        }
        if self.status_line_mode() == StatusLineMode::Overlay {
            self.draw_status_overlay(&mut grid, &presentation, session_id);
        }
        Ok(grid)
    }

    /// The part of a `terminal`-sized screen the session is drawn in. With
    /// the status line drawn over the root tab bar (the default), all of it,
    /// since the status line never takes a row from the panes; with a
    /// separate status line, all but the last row, which the caller draws
    /// [`Self::status_line`] in.
    pub fn content_viewport(&self, terminal: Size) -> Size {
        self.status_line_mode().content_viewport(terminal)
    }

    fn status_line_mode(&self) -> StatusLineMode {
        self.config.active_script().loaded_config().status_line
    }

    pub fn reload_config(&mut self) -> Result<()> {
        let current_mode = self.input_state.current_mode().to_owned();
        self.config
            .reload()
            .map_err(|error| MuxError::invalid_input(error.to_string()))?;
        self.finish_config_reload(&current_mode);
        Ok(())
    }

    pub fn reload_config_if_changed(&mut self) -> Result<bool> {
        match self.config.reload_if_changed() {
            Ok(false) => Ok(false),
            Ok(true) => {
                let current_mode = self.input_state.current_mode().to_owned();
                self.finish_config_reload(&current_mode);
                Ok(true)
            }
            Err(error) => {
                let message = error.to_string();
                self.record_notification(message.clone());
                Err(MuxError::invalid_input(message))
            }
        }
    }

    fn finish_config_reload(&mut self, current_mode: &str) {
        if self
            .config
            .active_script()
            .loaded_config()
            .modes
            .contains_key(current_mode)
        {
            self.input_state.clear_pending();
        } else {
            self.input_state.set_mode(NORMAL_MODE);
            self.search_prompt = None;
            self.command_prompt = None;
            self.confirm = None;
            self.chooser = None;
            // Also drop any active hints overlay so it can't outlive its mode.
            if let Some(node_id) = self.hints_node.take()
                && let Some(state) = self.client.state_mut().view_state_mut(node_id)
            {
                state.hints_state = None;
            }
        }
    }

    async fn execute_actions(
        &mut self,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
        actions: Vec<Action>,
    ) -> Result<()> {
        let mut pending = VecDeque::from(actions);
        let mut expansions = 0usize;
        let mut current_session_id = session_id;
        let current_viewport = viewport;
        while let Some(action) = pending.pop_front() {
            let result = match action {
                Action::Noop => Ok(()),
                Action::Chain(actions) => {
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::RunNamedAction { name } => {
                    match self.config.active_script().run_named_action(
                        &name,
                        self.context_for(current_session_id, current_viewport, None),
                    ) {
                        Ok(actions) => {
                            prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                        }
                        Err(error) => Err(MuxError::invalid_input(error.to_string())),
                    }
                }
                Action::EnterMode { mode } => {
                    let actions = self
                        .transition_mode(mode, current_session_id, current_viewport)
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::LeaveMode => {
                    let actions = self
                        .transition_mode(
                            NORMAL_MODE.to_owned(),
                            current_session_id,
                            current_viewport,
                        )
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::ToggleMode { mode } => {
                    let next_mode = if self.input_state.current_mode() == mode {
                        NORMAL_MODE.to_owned()
                    } else {
                        mode
                    };
                    let actions = self
                        .transition_mode(next_mode, current_session_id, current_viewport)
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::ClearPendingKeys => {
                    self.input_state.clear_pending();
                    Ok(())
                }
                Action::Notify { level, message } => {
                    self.record_notification(format_notification(level, &message));
                    Ok(())
                }
                Action::FocusBuffer { buffer_id } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::GetLocation {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                            }))
                            .await?,
                        "buffer focus",
                    )?;
                    let (session_id, node_id) =
                        Self::attached_buffer_location(Some(buffer_id), location, "buffer focus")?;
                    self.focus_node_with_shortcut(
                        session_id,
                        node_id,
                        self.active_session_id == Some(session_id),
                    )
                    .await?;
                    current_session_id = Some(session_id);
                    self.set_active_session(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                Action::RevealBuffer { buffer_id } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::Reveal {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                                client_id: None,
                            }))
                            .await?,
                        "buffer reveal",
                    )?;
                    let (session_id, _) =
                        Self::attached_buffer_location(Some(buffer_id), location, "buffer reveal")?;
                    current_session_id = Some(session_id);
                    self.set_active_session(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                Action::OpenBufferHistory {
                    buffer_id,
                    scope,
                    placement,
                } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::OpenHistory {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                                scope,
                                placement,
                                client_id: None,
                            }))
                            .await?,
                        "buffer history",
                    )?;
                    let (session_id, _) =
                        Self::attached_buffer_location(None, location, "buffer history")?;
                    current_session_id = Some(session_id);
                    self.set_active_session(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                Action::SwitchSession { name } => match self.resolve_session_by_name(&name) {
                    Some(target) => {
                        self.perform_session_switch(target).await?;
                        current_session_id = Some(target);
                        Ok(())
                    }
                    None => {
                        self.record_notification(format_notification(
                            NotifyLevel::Warn,
                            &format!("no session named '{name}'"),
                        ));
                        Ok(())
                    }
                },
                Action::LastSession => match self
                    .previous_session_id
                    .filter(|id| self.client.state().sessions.contains_key(id))
                {
                    Some(target) => {
                        self.perform_session_switch(target).await?;
                        current_session_id = Some(target);
                        Ok(())
                    }
                    None => {
                        // Unset, or the previous session has since closed.
                        self.previous_session_id = None;
                        self.record_notification(format_notification(
                            NotifyLevel::Info,
                            "no previous session",
                        ));
                        Ok(())
                    }
                },
                Action::NextSession => match self.cycled_session(true) {
                    Some(target) => {
                        self.perform_session_switch(target).await?;
                        current_session_id = Some(target);
                        Ok(())
                    }
                    None => Ok(()),
                },
                Action::PrevSession => match self.cycled_session(false) {
                    Some(target) => {
                        self.perform_session_switch(target).await?;
                        current_session_id = Some(target);
                        Ok(())
                    }
                    None => Ok(()),
                },
                Action::RunShell { command } => {
                    self.spawn_run_shell(command, current_session_id);
                    Ok(())
                }
                Action::SetEnvironment {
                    global,
                    name,
                    value,
                } => match (global, current_session_id) {
                    (false, None) => Err(MuxError::invalid_input(
                        "set_environment needs a current session",
                    )),
                    (global, session_id) => self
                        .client
                        .request_message(ClientMessage::Session(SessionRequest::SetEnvironment {
                            request_id: self.client.next_request_id(),
                            session_id: session_id.filter(|_| !global),
                            name,
                            value,
                        }))
                        .await
                        .map(|_| ()),
                },
                Action::CommandPrompt {
                    prompt,
                    initial,
                    template,
                } => {
                    let label =
                        self.expand_format(current_session_id, prompt.as_deref().unwrap_or(":"));
                    let initial = self.expand_format(current_session_id, &initial);
                    // Return to copy mode when opened from it; the other
                    // modes that read keys themselves are left for normal.
                    let mode = self.input_state.current_mode();
                    let return_mode =
                        if mode == SEARCH_MODE || mode == HINTS_MODE || mode == PROMPT_MODE {
                            NORMAL_MODE.to_owned()
                        } else {
                            mode.to_owned()
                        };
                    self.search_prompt = None;
                    let prompt = CommandPrompt::new(label, initial, template, return_mode);
                    self.command_prompt = Some(
                        match self.config.active_script().loaded_config().status_keys {
                            StatusKeys::Vi => prompt.with_vi_keys(),
                            StatusKeys::Emacs => prompt,
                        },
                    );
                    self.input_state.set_mode(PROMPT_MODE);
                    Ok(())
                }
                Action::Confirm { prompt, actions } => {
                    let prompt = self.expand_format(current_session_id, &prompt);
                    let mode = self.input_state.current_mode();
                    let return_mode =
                        if mode == SEARCH_MODE || mode == HINTS_MODE || mode == PROMPT_MODE {
                            NORMAL_MODE.to_owned()
                        } else {
                            mode.to_owned()
                        };
                    self.search_prompt = None;
                    self.command_prompt = None;
                    self.confirm = Some(PendingConfirm {
                        prompt,
                        actions,
                        return_mode,
                    });
                    self.input_state.set_mode(PROMPT_MODE);
                    Ok(())
                }
                Action::ChooseTree { scope } => {
                    self.open_chooser(scope, current_session_id);
                    Ok(())
                }
                Action::RunCommand { command } => {
                    self.run_command_line(current_session_id, &command).await;
                    Ok(())
                }
                Action::DisplayMessage {
                    message,
                    duration_ms,
                } => {
                    let text = self.expand_format(current_session_id, &message);
                    self.show_message(&text, display_duration(duration_ms.unwrap_or(0)));
                    Ok(())
                }
                Action::EnterHints { action } => match (current_session_id, current_viewport) {
                    (Some(session_id), Some(viewport)) => {
                        let presentation = self.prepare_presentation(session_id, viewport).await?;
                        self.enter_hints_mode(action, &presentation)
                    }
                    _ => Ok(()),
                },
                action => {
                    match self
                        .execute_without_presentation(current_session_id, action)
                        .await
                    {
                        Ok(None) => Ok(()),
                        Ok(Some(action)) => {
                            if matches!(action, Action::UnzoomNode { .. })
                                && current_session_id.is_none()
                            {
                                return Err(MuxError::invalid_input(format!(
                                    "cannot execute action {action:?} without current_session_id"
                                )));
                            }
                            let mut missing = Vec::new();
                            if current_session_id.is_none() {
                                missing.push("current_session_id");
                            }
                            if current_viewport.is_none() {
                                missing.push("current_viewport");
                            }
                            if !missing.is_empty() {
                                return Err(MuxError::invalid_input(format!(
                                    "cannot execute action {action:?} without {}",
                                    missing.join(" and ")
                                )));
                            }
                            let session_id = current_session_id.expect("checked current session");
                            let viewport = current_viewport.expect("checked current viewport");
                            let presentation =
                                self.prepare_presentation(session_id, viewport).await?;
                            self.execute_action(session_id, viewport, &presentation, action)
                                .await
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                self.record_notification(error.to_string());
            }
        }
        Ok(())
    }

    async fn execute_without_presentation(
        &mut self,
        current_session_id: Option<SessionId>,
        action: Action,
    ) -> Result<Option<Action>> {
        match action {
            Action::CloseFloating {
                floating_id: Some(floating_id),
            } => {
                self.close_floating(floating_id, false).await?;
                Ok(None)
            }
            Action::KillFloating {
                floating_id: Some(floating_id),
            } => {
                self.close_floating(floating_id, true).await?;
                Ok(None)
            }
            Action::CloseView {
                node_id: Some(node_id),
            } => {
                self.close_view(node_id, false).await?;
                Ok(None)
            }
            Action::KillView {
                node_id: Some(node_id),
            } => {
                self.close_view(node_id, true).await?;
                Ok(None)
            }
            Action::KillBuffer {
                buffer_id: Some(buffer_id),
            } => {
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        force: false,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::DetachBuffer {
                buffer_id: Some(buffer_id),
            } => {
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveBufferToNode { buffer_id, node_id } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        target_leaf_node_id: node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::ZoomNode {
                node_id: Some(node_id),
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Zoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::UnzoomNode {
                session_id: target_session_id,
            } => {
                let Some(session_id) = target_session_id.or(current_session_id) else {
                    return Ok(Some(Action::UnzoomNode {
                        session_id: target_session_id,
                    }));
                };
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Unzoom {
                        request_id: self.client.next_request_id(),
                        session_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::ToggleZoomNode {
                node_id: Some(node_id),
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::ToggleZoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::SwapSiblingNodes {
                first_node_id: Some(first_node_id),
                second_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SwapSiblings {
                        request_id: self.client.next_request_id(),
                        first_node_id,
                        second_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::BreakNode {
                node_id: Some(node_id),
                destination,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::BreakNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        destination,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::JoinBufferAtNode {
                node_id: Some(node_id),
                buffer_id,
                placement,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::JoinBufferAtNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        buffer_id,
                        placement,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveNodeBefore {
                node_id: Some(node_id),
                sibling_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveNodeAfter {
                node_id: Some(node_id),
                sibling_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeAfter {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            other => Ok(Some(other)),
        }
    }

    async fn transition_mode(
        &mut self,
        mode: String,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
    ) -> Result<Vec<Action>> {
        if !self
            .config
            .active_script()
            .loaded_config()
            .modes
            .contains_key(&mode)
        {
            return Err(MuxError::invalid_input(format!("unknown mode '{mode}'")));
        }

        let previous_mode = self.input_state.current_mode().to_owned();
        if previous_mode == mode {
            return Ok(Vec::new());
        }

        let mut actions = self
            .config
            .active_script()
            .run_leave_hook(&previous_mode, self.context_for(session_id, viewport, None))
            .map_err(|error| MuxError::invalid_input(error.to_string()))?;
        self.input_state.set_mode(mode.clone());
        actions.extend(
            self.config
                .active_script()
                .run_enter_hook(&mode, self.context_for(session_id, viewport, None))
                .map_err(|error| {
                    self.input_state.set_mode(previous_mode.clone());
                    MuxError::invalid_input(error.to_string())
                })?,
        );

        // `enter_mode("copy")` and `leave_mode()` behave like
        // `enter_copy_mode()` and `exit_copy_mode()` for the views involved.
        let copy_modes = [COPY_MODE, SELECT_MODE];
        if copy_modes.contains(&mode.as_str()) {
            if let Some((session_id, viewport)) = session_id.zip(viewport)
                && let Ok(presentation) = self.project(session_id, viewport)
                && let Some(leaf) = presentation.focused_leaf()
            {
                self.ensure_copy_cursor(leaf.node_id);
            }
        } else if copy_modes.contains(&previous_mode.as_str()) && mode != SEARCH_MODE {
            self.clear_copy_views().await?;
        }
        Ok(actions)
    }

    /// Close a view. With `kill`, also end the programs of the buffers it
    /// showed; otherwise they keep running, detached.
    async fn close_view(&mut self, node_id: NodeId, kill: bool) -> Result<()> {
        let shown = buffer_ids_in_subtree(self.client.state().nodes.values(), node_id);
        self.client
            .request_message(ClientMessage::Node(NodeRequest::Close {
                request_id: self.client.next_request_id(),
                node_id,
            }))
            .await?;
        if kill {
            self.kill_detached_buffers(shown).await?;
        }
        self.client.resync_all_sessions().await
    }

    /// Close a floating window. With `kill`, also end the programs of the
    /// buffers it showed; otherwise they keep running, detached.
    async fn close_floating(&mut self, floating_id: FloatingId, kill: bool) -> Result<()> {
        let shown = self
            .client
            .state()
            .floating
            .get(&floating_id)
            .map(|floating| {
                buffer_ids_in_subtree(self.client.state().nodes.values(), floating.root_node_id)
            })
            .unwrap_or_default();
        self.client
            .request_message(ClientMessage::Floating(FloatingRequest::Close {
                request_id: self.client.next_request_id(),
                floating_id,
            }))
            .await?;
        if kill {
            self.kill_detached_buffers(shown).await?;
        }
        self.client.resync_all_sessions().await
    }

    /// End the programs of buffers whose views were just closed. Each is
    /// detached by then; one that is attached again by the time it is checked
    /// is shown somewhere else and keeps running, and one whose program already
    /// ended needs nothing. One left `Interrupted` (its keeper is gone) has no
    /// program, but is killed too so the server removes it.
    async fn kill_detached_buffers(&mut self, buffer_ids: Vec<BufferId>) -> Result<()> {
        for buffer_id in buffer_ids {
            let buffer = match self
                .client
                .request_message(ClientMessage::Buffer(BufferRequest::Get {
                    request_id: self.client.next_request_id(),
                    buffer_id,
                }))
                .await
            {
                Ok(ServerResponse::Buffer(response)) => response.buffer,
                Ok(_) => continue,
                // Gone already: a history view's buffer goes with its view.
                Err(error) if error.is_not_found() => continue,
                Err(error) => return Err(error),
            };
            if buffer.attachment_node_id.is_some()
                || !matches!(
                    buffer.state,
                    BufferRecordState::Running | BufferRecordState::Interrupted
                )
            {
                continue;
            }
            match self
                .client
                .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                    request_id: self.client.next_request_id(),
                    buffer_id,
                    force: false,
                }))
                .await
            {
                Ok(_) => {}
                Err(error) if error.is_not_found() || error.is_conflict() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn execute_action(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        presentation: &PresentationModel,
        action: Action,
    ) -> Result<()> {
        match action {
            Action::FocusDirection { direction } => {
                let Some(node_id) = presentation.focus_target(direction) else {
                    return Ok(());
                };
                self.focus_node(session_id, node_id).await
            }
            Action::LastPane => self.focus_last_pane(session_id).await,
            Action::ScrollLineUp => {
                let leaf = self.focused_leaf(presentation)?;
                self.scroll_view_by(leaf.node_id, -1).await
            }
            Action::ScrollLineDown => {
                let leaf = self.focused_leaf(presentation)?;
                self.scroll_view_by(leaf.node_id, 1).await
            }
            Action::ScrollPageUp => {
                let leaf = self.focused_leaf(presentation)?;
                let page = self
                    .client
                    .state()
                    .view_state(leaf.node_id)
                    .map(|state| i64::from(state.visible_line_count.max(1)))
                    .unwrap_or(1);
                self.scroll_view_by(leaf.node_id, -page).await
            }
            Action::ScrollPageDown => {
                let leaf = self.focused_leaf(presentation)?;
                let page = self
                    .client
                    .state()
                    .view_state(leaf.node_id)
                    .map(|state| i64::from(state.visible_line_count.max(1)))
                    .unwrap_or(1);
                self.scroll_view_by(leaf.node_id, page).await
            }
            Action::ScrollToTop => {
                let leaf = self.focused_leaf(presentation)?;
                self.set_view_scroll_top(leaf.node_id, 0).await
            }
            Action::ScrollToBottom | Action::FollowOutput => {
                let leaf = self.focused_leaf(presentation)?;
                self.follow_output_for_view(leaf.node_id).await
            }
            Action::EnterSearchMode => self.enter_search_mode(presentation, false).await,
            Action::EnterSearchBackward => self.enter_search_mode(presentation, true).await,
            Action::SearchNext => {
                let count = self.copy.take_count();
                self.navigate_search(presentation, true, count).await
            }
            Action::SearchPrev => {
                let count = self.copy.take_count();
                self.navigate_search(presentation, false, count).await
            }
            Action::CommitSearch => self.commit_search_prompt(session_id, viewport).await,
            Action::CancelSearch => self.cancel_search_prompt(session_id, viewport).await,
            Action::EnterCopyMode => {
                let leaf = self.focused_leaf(presentation)?;
                if self.ensure_copy_cursor(leaf.node_id) && !self.in_copy_mode() {
                    self.input_state.set_mode(COPY_MODE);
                }
                Ok(())
            }
            Action::ExitCopyMode => self.exit_copy_mode().await,
            Action::CopyMotion { motion } => {
                let leaf = self.focused_leaf(presentation)?;
                let count = self.copy.take_count();
                self.copy_motion(leaf.node_id, motion, count).await
            }
            Action::CopyJump { kind } => {
                let leaf = self.focused_leaf(presentation)?;
                let count = self.copy.take_count();
                if self.ensure_copy_cursor(leaf.node_id) {
                    self.enter_copy_input_mode();
                    self.copy.pending_jump = Some((kind, count));
                }
                Ok(())
            }
            Action::CopyJumpAgain | Action::CopyJumpReverse => {
                let leaf = self.focused_leaf(presentation)?;
                let count = self.copy.take_count();
                let Some((kind, target)) = self.copy.last_jump else {
                    return Ok(());
                };
                let kind = if action == Action::CopyJumpReverse {
                    kind.reversed()
                } else {
                    kind
                };
                self.copy_jump(leaf.node_id, kind, target, count, true)
                    .await
            }
            Action::EnterSelect { kind } => self.enter_select_mode(presentation, kind),
            Action::SelectMove { direction } => {
                let leaf = self.focused_leaf(presentation)?;
                let motion = match direction {
                    NavigationDirection::Left => CopyMotion::Left,
                    NavigationDirection::Right => CopyMotion::Right,
                    NavigationDirection::Up => CopyMotion::Up,
                    NavigationDirection::Down => CopyMotion::Down,
                };
                let count = self.copy.take_count();
                self.copy_motion(leaf.node_id, motion, count).await
            }
            Action::SelectOtherEnd => {
                let leaf = self.focused_leaf(presentation)?;
                self.select_other_end(leaf.node_id).await
            }
            Action::CopySelection => {
                let leaf = self.focused_leaf(presentation)?;
                self.copy_selection(leaf.node_id).await
            }
            Action::CancelSelection => {
                let leaf = self.focused_leaf(presentation)?;
                self.cancel_selection(leaf.node_id);
                Ok(())
            }
            Action::SelectTab {
                tabs_node_id,
                index,
            } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if index >= tabs.tabs.len() {
                    return Err(MuxError::invalid_input(format!(
                        "tab index {index} is out of range for {} tabs",
                        tabs.tabs.len()
                    )));
                }
                let index = u32::try_from(index).map_err(|_| {
                    MuxError::invalid_input(format!("tab index {index} exceeds protocol limits"))
                })?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::SelectWindow { number } => {
                let count = self
                    .client
                    .state()
                    .sessions
                    .get(&session_id)
                    .and_then(|session| self.client.state().nodes.get(&session.root_node_id))
                    .map_or(1, |root| {
                        root.tabs.as_ref().map_or(1, |tabs| tabs.tabs.len())
                    });
                let Some(index) = crate::numbering::window_position(
                    number,
                    count,
                    self.base_index().unwrap_or(0),
                ) else {
                    self.record_notification(format_notification(
                        NotifyLevel::Warn,
                        &format!("no window {number}"),
                    ));
                    return Ok(());
                };
                self.client
                    .request_message(ClientMessage::Session(SessionRequest::SelectRootTab {
                        request_id: self.client.next_request_id(),
                        session_id,
                        index: u32::try_from(index).map_err(|_| {
                            MuxError::invalid_input("window index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::NextTab { tabs_node_id } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if tabs.tabs.is_empty() {
                    return Ok(());
                }
                let index = (tabs.active + 1) % tabs.tabs.len();
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index: u32::try_from(index).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::PrevTab { tabs_node_id } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if tabs.tabs.is_empty() {
                    return Ok(());
                }
                let index = if tabs.active == 0 {
                    tabs.tabs.len() - 1
                } else {
                    tabs.active - 1
                };
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index: u32::try_from(index).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::SplitCurrent {
                direction,
                new_child,
            } => {
                let focused_leaf = presentation
                    .focused_leaf()
                    .ok_or_else(|| MuxError::invalid_input("no focused leaf to split"))?;
                let placement = PanePlacement::Split {
                    leaf_id: focused_leaf.node_id,
                    direction,
                };
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, new_child, placement)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::Split {
                        request_id: self.client.next_request_id(),
                        leaf_node_id: focused_leaf.node_id,
                        direction,
                        new_buffer_id: buffer.id(),
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "split pane",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::OpenFloating { spec } => {
                let geometry = resolve_floating_geometry(spec.geometry, viewport);
                let buffer = self
                    .resolve_tree_buffer(
                        session_id,
                        presentation,
                        spec.tree,
                        PanePlacement::Floating { geometry },
                    )
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer.id()),
                        geometry,
                        title: spec.title,
                        focus: spec.focus,
                        close_on_empty: spec.close_on_empty,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "open floating window",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::DetachBuffer { buffer_id } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::KillBuffer { buffer_id } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        force: false,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::SendKeys {
                buffer_id: Some(buffer_id),
                keys,
            } => {
                let mode = self.buffer_keyboard_mode(buffer_id);
                self.send_bytes_to_buffer(buffer_id, session_id, sequence_to_bytes(&keys, mode)?)
                    .await
            }
            Action::SendKeys {
                buffer_id: None,
                keys,
            } => {
                let buffer_id = self.resolve_buffer_id(None, presentation)?;
                self.send_typed_input(presentation, buffer_id, session_id, |client, target| {
                    sequence_to_bytes(&keys, client.buffer_keyboard_mode(target))
                })
                .await
            }
            Action::SendBytes {
                buffer_id: Some(buffer_id),
                bytes,
            } => {
                self.send_bytes_to_buffer(buffer_id, session_id, bytes)
                    .await
            }
            Action::SendBytes {
                buffer_id: None,
                bytes,
            } => {
                let buffer_id = self.resolve_buffer_id(None, presentation)?;
                self.send_typed_input(
                    presentation,
                    buffer_id,
                    session_id,
                    |_, _| Ok(bytes.clone()),
                )
                .await
            }
            Action::CloseFloating { floating_id } => {
                let floating_id = focused_floating_target(floating_id, presentation)?;
                self.close_floating(floating_id, false).await
            }
            Action::KillFloating { floating_id } => {
                let floating_id = focused_floating_target(floating_id, presentation)?;
                self.close_floating(floating_id, true).await
            }
            Action::CloseView { node_id } => {
                let node_id = focused_view_target(node_id, presentation)?;
                self.close_view(node_id, false).await
            }
            Action::KillView { node_id } => {
                let node_id = focused_view_target(node_id, presentation)?;
                self.close_view(node_id, true).await
            }
            Action::InsertTabAfter {
                tabs_node_id,
                title,
                child,
            } => {
                self.insert_tab(session_id, presentation, tabs_node_id, title, child, true)
                    .await
            }
            Action::InsertTabBefore {
                tabs_node_id,
                title,
                child,
            } => {
                self.insert_tab(session_id, presentation, tabs_node_id, title, child, false)
                    .await
            }
            Action::NewWindow { title, child } => {
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, child, PanePlacement::RootTab)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Session(SessionRequest::AddRootTab {
                        request_id: self.client.next_request_id(),
                        session_id,
                        title: title.unwrap_or_else(|| "window".to_owned()),
                        buffer_id: Some(buffer.id()),
                        child_node_id: None,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "new window",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::ReplaceNode { node_id, tree } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to replace"))?;
                let placement = PanePlacement::Replace { leaf_id: node_id };
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, tree, placement)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id: buffer.id(),
                        target_leaf_node_id: node_id,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "replace node buffer",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveBufferToNode { buffer_id, node_id } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        target_leaf_node_id: node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveBufferToFloating {
                buffer_id,
                geometry,
                title,
                focus,
                close_on_empty,
            } => {
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer_id),
                        geometry: resolve_floating_geometry(geometry, viewport),
                        title,
                        focus,
                        close_on_empty,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ZoomNode { node_id: None } => {
                let node_id = presentation
                    .focused_leaf()
                    .map(|leaf| leaf.node_id)
                    .ok_or_else(|| MuxError::invalid_input("no focused node to zoom"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Zoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ToggleZoomNode { node_id } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to toggle zoom"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::ToggleZoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ResizePane { direction, cells } => {
                let leaf = self.focused_leaf(presentation)?;
                match presentation.resize_leaf(self.client.state(), leaf.node_id, direction, cells)
                {
                    Some(resize) => self.resize_split(session_id, resize).await,
                    None => Ok(()),
                }
            }
            Action::SetSynchronizePanes { mode } => {
                // Windows are the root tabs; a session whose root is a single
                // untabbed window addresses it as index 0.
                let index = self
                    .client
                    .state()
                    .sessions
                    .get(&session_id)
                    .and_then(|session| self.client.state().nodes.get(&session.root_node_id))
                    .and_then(|root| root.tabs.as_ref())
                    .map_or(0, |tabs| tabs.active);
                self.client
                    .request_message(ClientMessage::Session(
                        SessionRequest::SetRootTabSynchronizePanes {
                            request_id: self.client.next_request_id(),
                            session_id,
                            index,
                            mode,
                        },
                    ))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::SwapSiblingNodes {
                first_node_id,
                second_node_id,
            } => {
                let first_node_id = first_node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to swap"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SwapSiblings {
                        request_id: self.client.next_request_id(),
                        first_node_id,
                        second_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::BreakNode {
                node_id,
                destination,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to break"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::BreakNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        destination,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::JoinBufferAtNode {
                node_id,
                buffer_id,
                placement,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| {
                        MuxError::invalid_input("no focused node to join buffer into")
                    })?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::JoinBufferAtNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        buffer_id,
                        placement,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveNodeBefore {
                node_id,
                sibling_node_id,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to reorder"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveNodeAfter {
                node_id,
                sibling_node_id,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to reorder"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeAfter {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::FocusBuffer { .. }
            | Action::RevealBuffer { .. }
            | Action::OpenBufferHistory { .. }
            | Action::UnzoomNode { .. }
            | Action::ZoomNode { node_id: Some(_) } => Err(MuxError::invalid_input(
                "action should be handled before presentation is required",
            )),
            other => Err(MuxError::invalid_input(format!(
                "action '{other:?}' is not supported by the live executor yet"
            ))),
        }
    }

    /// The buffer `tree` names, spawning it first when the tree asks for a
    /// new one. A new buffer is started at the size it will be drawn at once
    /// placed at `placement`.
    async fn resolve_tree_buffer(
        &self,
        session_id: SessionId,
        presentation: &PresentationModel,
        tree: TreeSpec,
        placement: PanePlacement,
    ) -> Result<ResolvedTreeBuffer> {
        let size = || {
            spawn_pty_size_with(
                self.client.state(),
                session_id,
                presentation.viewport,
                self.layout_options(),
                placement,
            )
        };
        match tree {
            TreeSpec::BufferCurrent => presentation
                .focused_buffer_id()
                .map(ResolvedTreeBuffer::Existing)
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
            TreeSpec::BufferAttach { buffer_id } => Ok(ResolvedTreeBuffer::Existing(buffer_id)),
            TreeSpec::BufferSpawn(spec) => self
                .create_buffer(spec, size(), session_id)
                .await
                .map(ResolvedTreeBuffer::NewlySpawned),
            TreeSpec::BufferEmpty => self
                .create_buffer(
                    crate::scripting::BufferSpawnSpec {
                        title: Some("shell".to_owned()),
                        command: default_shell_command(),
                        cwd: None,
                        env: Default::default(),
                    },
                    size(),
                    session_id,
                )
                .await
                .map(ResolvedTreeBuffer::NewlySpawned),
            other => Err(MuxError::invalid_input(format!(
                "tree '{other:?}' is not supported by the live executor yet"
            ))),
        }
    }

    async fn create_buffer(
        &self,
        spec: crate::scripting::BufferSpawnSpec,
        size: Option<PtySize>,
        session_id: SessionId,
    ) -> Result<BufferId> {
        let response = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Create {
                request_id: self.client.next_request_id(),
                title: spec.title,
                command: spec.command,
                cwd: spec.cwd,
                env: spec.env,
                size,
                session_id: Some(session_id),
            }))
            .await?;

        match response {
            ServerResponse::Buffer(BufferResponse { buffer, .. }) => Ok(buffer.id),
            other => Err(MuxError::protocol(format!(
                "expected buffer response, got {other:?}"
            ))),
        }
    }

    async fn rollback_created_buffer(&mut self, buffer_id: BufferId, operation: &str) {
        if let Err(error) = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                request_id: self.client.next_request_id(),
                buffer_id,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to detach created buffer during rollback"
            );
        }
        if let Err(error) = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                request_id: self.client.next_request_id(),
                buffer_id,
                force: true,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to kill created buffer during rollback"
            );
        }
    }

    async fn send_bytes_to_buffer(
        &mut self,
        buffer_id: BufferId,
        session_id: SessionId,
        bytes: Vec<u8>,
    ) -> Result<()> {
        self.send_input(session_id, vec![(buffer_id, bytes)]).await
    }

    /// Send input typed into the focused pane's `buffer_id`, honouring
    /// synchronize-panes (see `input_targets`). `encode` produces the bytes
    /// for each target, so key encodings and bracketed paste follow what each
    /// pane's program asked for.
    async fn send_typed_input(
        &mut self,
        presentation: &PresentationModel,
        buffer_id: BufferId,
        session_id: SessionId,
        encode: impl Fn(&Self, BufferId) -> Result<Vec<u8>>,
    ) -> Result<()> {
        let sends = self
            .input_targets(presentation, buffer_id)
            .into_iter()
            .map(|target| Ok((target, encode(self, target)?)))
            .collect::<Result<Vec<_>>>()?;
        self.send_input(session_id, sends).await
    }

    /// The buffers input for `buffer_id` goes to: `buffer_id` itself, then,
    /// when it is the focused pane of a window with synchronize-panes on,
    /// every other pane the window shows. Panes in hidden nested tabs or
    /// hidden by zoom are not shown, so they are left out, as are popups:
    /// input typed into a popup is never copied, and popup panes never
    /// receive copies.
    fn input_targets(
        &self,
        presentation: &PresentationModel,
        buffer_id: BufferId,
    ) -> Vec<BufferId> {
        let mut targets = vec![buffer_id];
        let synchronized = presentation
            .focused_leaf()
            .is_some_and(|leaf| leaf.buffer_id == buffer_id && leaf.floating_id.is_none())
            && self.window_synchronized(presentation.session_id);
        if synchronized {
            for leaf in presentation
                .leaves
                .iter()
                .filter(|leaf| leaf.floating_id.is_none())
            {
                if !targets.contains(&leaf.buffer_id) {
                    targets.push(leaf.buffer_id);
                }
            }
        }
        targets
    }

    /// Whether synchronize-panes is on for the session's active window.
    fn window_synchronized(&self, session_id: SessionId) -> bool {
        let state = self.client.state();
        state
            .sessions
            .get(&session_id)
            .and_then(|session| state.nodes.get(&session.root_node_id))
            .and_then(|root| root.tabs.as_ref())
            .and_then(|tabs| tabs.tabs.get(usize::try_from(tabs.active).ok()?))
            .is_some_and(|tab| tab.synchronize_panes)
    }

    /// Send each `(buffer, bytes)` pair, then refresh the buffers that took
    /// input and the session. A pane that refuses its input (see
    /// `write_input`) is skipped and the others still get theirs.
    async fn send_input(
        &mut self,
        session_id: SessionId,
        sends: Vec<(BufferId, Vec<u8>)>,
    ) -> Result<()> {
        let mut sent = Vec::with_capacity(sends.len());
        for (buffer_id, bytes) in sends {
            if self.write_input(buffer_id, bytes).await? {
                sent.push(buffer_id);
            }
        }
        for buffer_id in sent {
            self.client.refresh_buffer_snapshot(buffer_id).await?;
        }
        self.client.resync_session(session_id).await
    }

    async fn resize_split(&mut self, session_id: SessionId, resize: SplitResize) -> Result<()> {
        self.client
            .request_message(ClientMessage::Node(NodeRequest::Resize {
                request_id: self.client.next_request_id(),
                node_id: resize.split_node_id,
                sizes: resize.sizes,
            }))
            .await?;
        self.client.resync_session(session_id).await
    }

    /// Claim mouse events that drag a split divider: a left press on a
    /// divider starts a drag, left drags move the divider to the pointer
    /// wherever it is, and the release ends it. Returns whether the event was
    /// claimed; everything else, including drags that start inside a pane, is
    /// left to the normal pane handling.
    async fn handle_divider_drag(
        &mut self,
        session_id: SessionId,
        presentation: &PresentationModel,
        point: Point,
        kind: MouseEventKind,
        settings: MouseSettings,
    ) -> Result<bool> {
        match kind {
            MouseEventKind::Press(button) => {
                self.divider_drag = None;
                if button != MouseButton::Left || !settings.drag_resize {
                    return Ok(false);
                }
                let Some(divider) = presentation.divider_at(point) else {
                    return Ok(false);
                };
                self.divider_drag = Some(DividerDrag {
                    split_node_id: divider.split_node_id,
                    index: divider.index,
                });
                Ok(true)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(drag) = self.divider_drag else {
                    return Ok(false);
                };
                if let Some(resize) =
                    presentation.drag_divider(drag.split_node_id, drag.index, point)
                {
                    self.resize_split(session_id, resize).await?;
                }
                Ok(true)
            }
            MouseEventKind::Release(_) => Ok(self.divider_drag.take().is_some()),
            _ => Ok(false),
        }
    }

    fn resolve_buffer_id(
        &self,
        buffer_id: Option<BufferId>,
        presentation: &PresentationModel,
    ) -> Result<BufferId> {
        match buffer_id {
            Some(buffer_id) => Ok(buffer_id),
            None => presentation
                .focused_buffer_id()
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
        }
    }

    /// Insert a tab after (or before) the active tab of `tabs_node_id`, or of
    /// the innermost tabs holding the focus. When the focus is in the
    /// session's layout but in no tabs, the session has a single window,
    /// whose root is not tabbed: the tab is then added as a second window,
    /// beside that one, rather than failing.
    async fn insert_tab(
        &mut self,
        session_id: SessionId,
        presentation: &PresentationModel,
        tabs_node_id: Option<NodeId>,
        title: Option<String>,
        child: TreeSpec,
        after: bool,
    ) -> Result<()> {
        if tabs_node_id.is_none()
            && presentation.focused_tabs().is_none()
            && presentation
                .focused_leaf()
                .is_some_and(|leaf| leaf.floating_id.is_none())
        {
            return self
                .insert_beside_only_window(session_id, presentation, title, child, after)
                .await;
        }
        let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
        let placement = PanePlacement::Tab {
            tabs_node_id: tabs.node_id,
        };
        let buffer = self
            .resolve_tree_buffer(session_id, presentation, child, placement)
            .await?;
        let index = if after {
            tabs.active.saturating_add(1)
        } else {
            tabs.active
        };
        let result = self
            .client
            .request_message(ClientMessage::Node(NodeRequest::AddTab {
                request_id: self.client.next_request_id(),
                tabs_node_id: tabs.node_id,
                title: title.unwrap_or_else(|| "tab".to_owned()),
                buffer_id: Some(buffer.id()),
                child_node_id: None,
                index: u32::try_from(index)
                    .map_err(|_| MuxError::invalid_input("tab index exceeds protocol limits"))?,
            }))
            .await;
        rollback_created_buffer_on_error(self, buffer.created_by_helper(), "insert tab", result)
            .await?;
        self.client.resync_all_sessions().await
    }

    /// Add a second window to a session that has one, after it or before it.
    async fn insert_beside_only_window(
        &mut self,
        session_id: SessionId,
        presentation: &PresentationModel,
        title: Option<String>,
        child: TreeSpec,
        after: bool,
    ) -> Result<()> {
        let old_root = self
            .client
            .state()
            .sessions
            .get(&session_id)
            .map(|session| session.root_node_id)
            .ok_or_else(|| MuxError::not_found(format!("session {session_id} is not cached")))?;
        let buffer = self
            .resolve_tree_buffer(session_id, presentation, child, PanePlacement::RootTab)
            .await?;
        let result = self
            .client
            .request_message(ClientMessage::Session(SessionRequest::AddRootTab {
                request_id: self.client.next_request_id(),
                session_id,
                title: title.unwrap_or_else(|| "tab".to_owned()),
                buffer_id: Some(buffer.id()),
                child_node_id: None,
            }))
            .await;
        rollback_created_buffer_on_error(self, buffer.created_by_helper(), "insert tab", result)
            .await?;
        self.client.resync_all_sessions().await?;
        if after {
            return Ok(());
        }
        // The new window was appended; move it in front of the old one.
        let new_window = self
            .client
            .state()
            .sessions
            .get(&session_id)
            .and_then(|session| self.client.state().nodes.get(&session.root_node_id))
            .and_then(|root| root.tabs.as_ref())
            .and_then(|tabs| tabs.tabs.last())
            .map(|tab| tab.child_id)
            .ok_or_else(|| MuxError::protocol("new window was not added to root tabs"))?;
        self.client
            .request_message(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                request_id: self.client.next_request_id(),
                node_id: new_window,
                sibling_node_id: old_root,
            }))
            .await?;
        self.client.resync_all_sessions().await
    }

    fn resolve_tabs_target(
        &self,
        presentation: &PresentationModel,
        tabs_node_id: Option<embers_core::NodeId>,
    ) -> Result<crate::TabsFrame> {
        match tabs_node_id {
            Some(tabs_node_id) => presentation
                .tab_bars
                .iter()
                .find(|tabs| tabs.node_id == tabs_node_id)
                .cloned()
                .or_else(|| {
                    self.client
                        .state()
                        .nodes
                        .get(&tabs_node_id)
                        .and_then(|node| {
                            node.tabs.as_ref().map(|tabs| crate::TabsFrame {
                                node_id: tabs_node_id,
                                rect: embers_core::Rect::default(),
                                tabs: tabs
                                    .tabs
                                    .iter()
                                    .enumerate()
                                    .map(|(index, tab)| crate::TabItem {
                                        title: tab.title.clone(),
                                        child_id: tab.child_id,
                                        active: usize::try_from(tabs.active).ok() == Some(index),
                                        activity: ActivityState::Idle,
                                        buffer_count: crate::presentation::subtree_buffer_count(
                                            self.client.state(),
                                            tab.child_id,
                                        ),
                                        synchronize_panes: tab.synchronize_panes,
                                        pane: None,
                                    })
                                    .collect(),
                                active: usize::try_from(tabs.active).unwrap_or(0),
                                is_root: self
                                    .client
                                    .state()
                                    .sessions
                                    .values()
                                    .any(|session| session.root_node_id == tabs_node_id),
                                floating_id: None,
                            })
                        })
                })
                .ok_or_else(|| MuxError::invalid_input(format!("node {tabs_node_id} is not tabs"))),
            None => presentation
                .focused_tabs()
                .cloned()
                .ok_or_else(|| MuxError::invalid_input("no focused tabs to select from")),
        }
    }

    /// Project `session_id` the way this client draws it, with the root tab
    /// bar and pane title rows where the config puts them.
    fn project(&self, session_id: SessionId, viewport: Size) -> Result<PresentationModel> {
        PresentationModel::project_with(
            self.client.state(),
            session_id,
            viewport,
            self.layout_options(),
        )
    }

    /// How the config has sessions laid out: the root tab bar and the panes'
    /// title rows.
    pub fn layout_options(&self) -> LayoutOptions {
        LayoutOptions {
            tab_bar: self.root_tab_bar(),
            pane_titles: self.pane_titles(),
        }
    }

    /// How the config has the session's root tab bar drawn.
    pub fn root_tab_bar(&self) -> RootTabBar {
        let config = self.config.active_script().loaded_config();
        RootTabBar {
            position: config.tab_bar_position,
            visible: config.tab_bar_visible,
        }
    }

    /// Where the config has panes draw their title rows.
    pub fn pane_titles(&self) -> PaneTitles {
        self.config.active_script().loaded_config().pane_titles
    }

    async fn prepare_presentation(
        &mut self,
        session_id: SessionId,
        viewport: Size,
    ) -> Result<PresentationModel> {
        let mut presentation = self.project(session_id, viewport)?;
        let invalidated = presentation
            .leaves
            .iter()
            .filter(|leaf| {
                self.client
                    .state()
                    .invalidated_buffers
                    .contains(&leaf.buffer_id)
            })
            .map(|leaf| leaf.buffer_id)
            .collect::<Vec<_>>();
        for buffer_id in invalidated {
            self.client.refresh_buffer_snapshot(buffer_id).await?;
        }
        if !self.client.state().invalidated_buffers.is_empty() {
            presentation = self.project(session_id, viewport)?;
        }
        self.refresh_local_viewports(&presentation).await?;
        Ok(presentation)
    }

    fn context_for(
        &self,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
        event: Option<EventInfo>,
    ) -> Context {
        let context = if let Some((session_id, viewport)) = session_id.zip(viewport)
            && let Ok(presentation) = self.project(session_id, viewport)
        {
            Context::from_state_with_mode(
                self.client.state(),
                Some(&presentation),
                self.input_state.current_mode(),
                None,
                None,
                None,
                None,
            )
        } else {
            Context::from_state_with_mode(
                self.client.state(),
                None,
                self.input_state.current_mode(),
                session_id,
                None,
                None,
                None,
            )
        };
        if let Some(event) = event {
            context.with_event(event)
        } else {
            context
        }
    }

    fn event_session_id(&self, event: &ServerEvent) -> Option<SessionId> {
        event.session_id().or_else(|| match event {
            ServerEvent::BufferCreated(event) => self.session_id_for_buffer_record(&event.buffer),
            ServerEvent::BufferPipeChanged(event) => {
                self.session_id_for_buffer_record(&event.buffer)
            }
            ServerEvent::BufferDetached(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::RenderInvalidated(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::Clipboard(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::Passthrough(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::DisplayMessage(_) => self.active_session_id,
            ServerEvent::ServerShutdown => None,
            ServerEvent::SessionCreated(_)
            | ServerEvent::SessionClosed(_)
            | ServerEvent::SessionRenamed(_)
            | ServerEvent::NodeChanged(_)
            | ServerEvent::FloatingChanged(_)
            | ServerEvent::FocusChanged(_) => None,
            ServerEvent::ClientChanged(event) => event.client.current_session_id,
        })
    }

    fn session_id_for_buffer_record(&self, buffer: &BufferRecord) -> Option<SessionId> {
        buffer
            .attachment_node_id
            .and_then(|node_id| self.session_id_for_node(node_id))
            .or_else(|| self.session_id_for_buffer(buffer.id))
    }

    fn session_id_for_buffer(&self, buffer_id: BufferId) -> Option<SessionId> {
        let state = self.client.state();
        state
            .buffers
            .get(&buffer_id)
            .and_then(|buffer| buffer.attachment_node_id)
            .and_then(|node_id| state.nodes.get(&node_id))
            .map(|node| node.session_id)
            .or_else(|| {
                state.nodes.values().find_map(|node| {
                    node.buffer_view
                        .as_ref()
                        .filter(|view| view.buffer_id == buffer_id)
                        .map(|_| node.session_id)
                })
            })
    }

    fn session_id_for_node(&self, node_id: NodeId) -> Option<SessionId> {
        self.client
            .state()
            .nodes
            .get(&node_id)
            .map(|node| node.session_id)
    }

    fn buffer_location_from_response(
        response: ServerResponse,
        context: &str,
    ) -> Result<BufferLocation> {
        match response {
            ServerResponse::BufferLocation(response) => Ok(response.location),
            ServerResponse::BufferWithLocation(response) => {
                let (_, _, location, _) = response.into_parts();
                Ok(location)
            }
            other => Err(MuxError::protocol(format!(
                "expected {context} response, got {other:?}"
            ))),
        }
    }

    fn attached_buffer_location(
        expected_buffer_id: Option<BufferId>,
        location: BufferLocation,
        context: &str,
    ) -> Result<(SessionId, NodeId)> {
        if let Some(expected_buffer_id) = expected_buffer_id
            && location.buffer_id != expected_buffer_id
        {
            return Err(MuxError::protocol(format!(
                "{context} returned location for buffer {} while acting on buffer {expected_buffer_id}",
                location.buffer_id
            )));
        }

        match location.attachment {
            BufferLocationAttachment::Session {
                session_id,
                node_id,
            }
            | BufferLocationAttachment::Floating {
                session_id,
                node_id,
                ..
            } => Ok((session_id, node_id)),
            BufferLocationAttachment::Detached => Err(MuxError::conflict(format!(
                "{context} returned a detached location for buffer {}",
                expected_buffer_id.unwrap_or(location.buffer_id)
            ))),
        }
    }

    fn set_active_view(&mut self, session_id: SessionId, viewport: Size) {
        self.set_active_session(session_id);
        self.viewport = Some(viewport);
    }

    /// Update the active session, remembering the prior one so `last_session`
    /// (tmux `switch-client -l`) can toggle back to it.
    fn set_active_session(&mut self, session_id: SessionId) {
        if self.active_session_id != Some(session_id) {
            // The overlay is anchored to a node in the old session; it can't
            // survive the switch, so tear it down before we move on.
            self.cancel_active_hints();
            // Only record the current session as `previous` when switching
            // directly between sessions. When reattaching after a detach (active
            // is None), keep the session `clear_active_session` already stored so
            // `last_session` can still toggle back to it.
            if let Some(active) = self.active_session_id {
                self.previous_session_id = Some(active);
            }
            self.active_session_id = Some(session_id);
        }
    }

    /// Leave the active session after the server destroyed it, as the
    /// `detach-on-destroy` setting says: switch to another session, or ask the
    /// interactive client to exit when it detaches or no session is left.
    async fn leave_destroyed_session(&mut self) -> Result<()> {
        let Some(destroyed) = self.active_session_id.take() else {
            return Ok(());
        };
        self.cancel_active_hints();
        let sessions = &self.client.state().sessions;
        let remaining = sessions.keys().copied().collect::<Vec<_>>();
        let previous = self
            .previous_session_id
            .filter(|session_id| sessions.contains_key(session_id));
        let target = match self
            .config
            .active_script()
            .loaded_config()
            .detach_on_destroy
        {
            DetachOnDestroy::On => None,
            DetachOnDestroy::Off => previous.or_else(|| remaining.first().copied()),
            DetachOnDestroy::Previous => remaining
                .iter()
                .rev()
                .find(|session_id| **session_id < destroyed)
                .or(remaining.last())
                .copied(),
            DetachOnDestroy::Next => remaining
                .iter()
                .find(|session_id| **session_id > destroyed)
                .or(remaining.first())
                .copied(),
        };
        // `last_session` keeps toggling to the session before the destroyed
        // one, unless that is where the client goes now.
        self.previous_session_id = previous.filter(|session_id| Some(*session_id) != target);
        let Some(target) = target else {
            self.exit_requested = true;
            return Ok(());
        };
        self.client.switch_current_session(target).await?;
        self.active_session_id = Some(target);
        Ok(())
    }

    /// Drop the active session (e.g. the server detached this client), keeping
    /// the prior one so `last_session` can still toggle back to it.
    fn clear_active_session(&mut self) {
        if let Some(session_id) = self.active_session_id.take() {
            self.cancel_active_hints();
            self.previous_session_id = Some(session_id);
        }
    }

    /// Drop any active hints overlay. Used when the active session changes or is
    /// detached, so a stale overlay can't linger on a node we're leaving.
    fn cancel_active_hints(&mut self) {
        let Some(node_id) = self.hints_node.take() else {
            return;
        };
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.hints_state = None;
        }
        // Fully leave hints mode so the next key is processed normally instead of
        // being routed to `handle_hints_key` for an overlay that no longer exists.
        self.input_state.set_mode(NORMAL_MODE);
    }

    fn resolve_session_by_name(&self, name: &str) -> Option<SessionId> {
        self.client
            .state()
            .sessions
            .values()
            .find(|session| session.name == name)
            .map(|session| session.id)
    }

    /// The next (or previous) session id in list-sessions order, wrapping around.
    /// Returns `None` when there are no sessions or only the current one.
    fn cycled_session(&self, forward: bool) -> Option<SessionId> {
        let ids: Vec<SessionId> = self.client.state().sessions.keys().copied().collect();
        if ids.len() < 2 {
            return None;
        }
        let current = self.active_session_id?;
        let index = ids.iter().position(|id| *id == current)?;
        let len = ids.len();
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        Some(ids[next])
    }

    async fn perform_session_switch(&mut self, session_id: SessionId) -> Result<()> {
        self.client.switch_current_session(session_id).await?;
        self.set_active_session(session_id);
        self.client.resync_all_sessions().await
    }

    /// The kitty keyboard mode negotiated by the given buffer's inner program,
    /// used so passthrough keys are re-encoded the way that program expects.
    fn buffer_keyboard_mode(&self, buffer_id: BufferId) -> u8 {
        self.client
            .state()
            .snapshots
            .get(&buffer_id)
            .map(|snapshot| snapshot.keyboard_mode)
            .unwrap_or(0)
    }

    /// The working directory of the focused buffer in the given session, if known.
    fn focused_buffer_cwd(&self, session_id: Option<SessionId>) -> Option<PathBuf> {
        let session_id = session_id.or(self.active_session_id)?;
        let state = self.client.state();
        let session = state.sessions.get(&session_id)?;
        let node = state.nodes.get(&session.focused_leaf_id?)?;
        let buffer_id = node.buffer_view.as_ref()?.buffer_id;
        state
            .buffers
            .get(&buffer_id)?
            .cwd
            .as_ref()
            .map(PathBuf::from)
    }

    /// Spawn a `run_shell` child detached in the client's login context.
    ///
    /// Output is discarded (unlike tmux, which shows it in a pane — capture into
    /// a popup with `buffer_spawn` instead). `$EMBERS_SOCKET` is injected so the
    /// child can drive Embers via the CLI. The client never awaits the child; a
    /// background waiter surfaces a warning on non-zero exit.
    fn spawn_run_shell(&mut self, command: Vec<String>, session_id: Option<SessionId>) {
        let Some((program, args)) = command.split_first() else {
            return;
        };
        let mut process = tokio::process::Command::new(program);
        process
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(cwd) = self.focused_buffer_cwd(session_id) {
            process.current_dir(cwd);
        }
        if let Some(socket_path) = &self.socket_path {
            process.env("EMBERS_SOCKET", socket_path);
        }
        // So `embers` commands the child runs number windows like this client.
        if let Some(base_index) = self.base_index() {
            process.env(crate::numbering::BASE_INDEX_ENV_VAR, base_index.to_string());
        }

        // Identify the command by its executable only. The full argument list
        // may carry secrets (tokens, passwords) and these notifications are
        // logged at WARN and shown in the status line.
        let program_name = program.clone();
        let sink = Arc::clone(&self.background_notifications);
        match process.spawn() {
            Ok(mut child) => {
                tokio::spawn(async move {
                    match child.wait().await {
                        Ok(status) if status.success() => {}
                        Ok(status) => {
                            push_background_notification(
                                &sink,
                                format_notification(
                                    NotifyLevel::Warn,
                                    &format!("run-shell `{program_name}` exited with {status}"),
                                ),
                            );
                        }
                        Err(error) => {
                            push_background_notification(
                                &sink,
                                format_notification(
                                    NotifyLevel::Warn,
                                    &format!("run-shell `{program_name}` failed: {error}"),
                                ),
                            );
                        }
                    }
                });
            }
            Err(error) => {
                self.record_notification(format_notification(
                    NotifyLevel::Warn,
                    &format!("run-shell `{program_name}` could not start: {error}"),
                ));
            }
        }
    }

    /// Move notifications produced by background tasks into the visible queue.
    /// Returns `true` when at least one notification was surfaced, so callers
    /// (e.g. the idle poll loop) know a redraw is warranted.
    pub fn drain_background_notifications(&mut self) -> bool {
        let drained: Vec<String> = {
            let mut pending = self
                .background_notifications
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if pending.is_empty() {
                return false;
            }
            std::mem::take(&mut *pending)
        };
        for message in drained {
            self.record_notification(message);
        }
        true
    }

    fn current_fallback_policy(&self) -> FallbackPolicy {
        self.config
            .active_script()
            .loaded_config()
            .modes
            .get(self.input_state.current_mode())
            .map(|mode| mode.fallback_policy)
            .unwrap_or(FallbackPolicy::Ignore)
    }

    fn focused_leaf<'a>(&self, presentation: &'a PresentationModel) -> Result<&'a LeafFrame> {
        presentation
            .focused_leaf()
            .ok_or_else(|| MuxError::invalid_input("no focused leaf"))
    }

    fn mouse_target_leaf<'a>(
        &self,
        presentation: &'a PresentationModel,
        point: Point,
    ) -> Option<&'a LeafFrame> {
        if let Some(floating) = presentation.floating_at(point) {
            return presentation.leaves.iter().rev().find(|leaf| {
                leaf.floating_id == Some(floating.floating_id) && leaf.rect.contains(point)
            });
        }
        presentation.leaf_at(point)
    }

    fn view_is_alternate_screen(&self, node_id: NodeId) -> bool {
        self.client
            .state()
            .view_state(node_id)
            .is_some_and(|state| state.alternate_screen)
    }

    fn should_passthrough_binding_in_alternate_screen(
        &self,
        presentation: &PresentationModel,
        actions: &[Action],
    ) -> bool {
        // When the focused pane is acting like a live terminal surface, prefer
        // forwarding local search/select/navigation bindings to the program
        // instead of stealing keys that fullscreen apps expect to receive.
        let Some(leaf) = presentation.focused_leaf() else {
            return false;
        };
        let Some(view_state) = self.client.state().view_state(leaf.node_id) else {
            return false;
        };
        // Copy mode itself never forwards: its bindings must keep working (and
        // `q` must still leave) if focus moves onto a fullscreen app.
        (!self.in_copy_mode()
            && view_state.alternate_screen
            && actions.iter().all(action_is_local_terminal_action))
            || (self.input_state.current_mode() == NORMAL_MODE
                && view_state.follow_output
                && view_state.search_state.is_none()
                && view_state.selection_state.is_none()
                && actions.iter().all(action_requires_local_context))
    }

    /// Resize the PTYs of the session's visible buffers to the content areas
    /// `render_session` draws for them at `viewport`, sending a request only for
    /// buffers whose size changed (see `PtySizeSync` for the multi-client
    /// policy). The server answers with `RenderInvalidated`, which triggers the
    /// redraw at the new size. A failed resize (e.g. the buffer exited
    /// meanwhile) only leaves that PTY at its old size, so it is logged and
    /// left for a later sync to retry rather than returned.
    ///
    /// The viewport itself is reported to the server too, with whether panes
    /// have title rows, so panes created from the CLI can be spawned at the
    /// size this client will draw them. The CLI does not know whether the
    /// root tab bar is hidden, so the viewport reported is one in which it is
    /// shown and the panes keep their sizes
    /// ([`PresentationModel::layout_viewport`]). Title rows cannot be made up
    /// for that way, since every pane has one, so they are reported as such.
    pub async fn sync_pty_sizes(&mut self, session_id: SessionId, viewport: Size) -> Result<()> {
        let presentation = self.project(session_id, viewport)?;
        self.report_viewport(session_id, presentation.layout_viewport())
            .await;
        self.report_base_index(session_id).await;
        for leaf in &presentation.leaves {
            if let Some(size) = leaf.pty_size() {
                self.client
                    .state_mut()
                    .set_view_drawn_rows(leaf.node_id, size.rows);
            }
        }
        for (buffer_id, size) in self.pty_sizes.plan(self.client.state(), &presentation) {
            let result = self
                .client
                .request_message(ClientMessage::Input(InputRequest::Resize {
                    request_id: self.client.next_request_id(),
                    buffer_id,
                    cols: size.cols,
                    rows: size.rows,
                }))
                .await;
            if let Err(error) = result {
                warn!("failed to resize buffer {buffer_id}: {error}");
                self.pty_sizes.resize_failed(buffer_id);
            }
        }
        Ok(())
    }

    /// Tell the server the base index this client numbers windows from, so
    /// `embers` commands run without `--base-index` or `EMBERS_BASE_INDEX`
    /// number them the same way. Sent when it differs from what the server
    /// has for the session.
    async fn report_base_index(&mut self, session_id: SessionId) {
        let base_index = self.base_index();
        let known = self
            .client
            .state()
            .sessions
            .get(&session_id)
            .map(|session| session.base_index);
        if self.reported_base_index == Some((session_id, base_index))
            || known.is_none_or(|known| known == base_index)
        {
            return;
        }
        let result = self
            .client
            .request_message(ClientMessage::Session(SessionRequest::SetBaseIndex {
                request_id: self.client.next_request_id(),
                session_id,
                base_index,
            }))
            .await;
        match result {
            Ok(_) => self.reported_base_index = Some((session_id, base_index)),
            // Only CLI window numbers depend on it; retried on the next sync.
            Err(error) => warn!("failed to report base index of session {session_id}: {error}"),
        }
    }

    async fn report_viewport(&mut self, session_id: SessionId, viewport: Size) {
        let pane_titles = self.pane_titles().shown();
        if self.reported_viewport == Some((session_id, viewport, pane_titles))
            || viewport.width == 0
            || viewport.height == 0
        {
            return;
        }
        let result = self
            .client
            .request_message(ClientMessage::Session(SessionRequest::SetViewport {
                request_id: self.client.next_request_id(),
                session_id,
                viewport,
                pane_titles,
            }))
            .await;
        match result {
            Ok(_) => self.reported_viewport = Some((session_id, viewport, pane_titles)),
            // Only the size of panes created later depends on it, so a
            // failure is retried on the next sync rather than returned.
            Err(error) => warn!("failed to report viewport of session {session_id}: {error}"),
        }
    }

    async fn refresh_local_viewports(&mut self, presentation: &PresentationModel) -> Result<()> {
        let refreshes = presentation
            .leaves
            .iter()
            .filter_map(|leaf| {
                let state = self.client.state().view_state(leaf.node_id)?;
                if state.alternate_screen
                    || state.follow_output
                    || state.visible_line_count == 0
                    || !state.visible_lines.is_empty()
                {
                    return None;
                }
                Some((leaf.node_id, state.scroll_top_line))
            })
            .collect::<Vec<_>>();

        for (node_id, scroll_top_line) in refreshes {
            self.fetch_view_slice(node_id, scroll_top_line).await?;
        }
        Ok(())
    }

    async fn scroll_view_by(&mut self, node_id: NodeId, delta: i64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        let current = i128::from(state.scroll_top_line);
        let delta = i128::from(delta);
        // Negative deltas scroll up by subtracting the absolute value; positive deltas
        // scroll down by adding. We do the math in i128 with saturating ops so
        // unsigned_abs/subtraction cannot underflow or overflow before clamping at 0.
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs() as i128)
        } else {
            current.saturating_add(delta)
        };
        let next = next.max(0) as u64;
        self.set_view_scroll_top(node_id, next).await
    }

    async fn set_view_scroll_top(&mut self, node_id: NodeId, scroll_top_line: u64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        let bottom = state
            .total_line_count
            .saturating_sub(u64::from(state.visible_line_count));
        let scroll_top_line = scroll_top_line.min(bottom);
        // A view in copy mode stays frozen even when scrolled to the bottom.
        let in_copy_mode = state.copy_cursor.is_some();
        if scroll_top_line == bottom && !in_copy_mode {
            return self.follow_output_for_view(node_id).await;
        }

        self.fetch_view_slice(node_id, scroll_top_line).await?;
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.follow_output = false;
        }
        // Like tmux, scrolling in copy mode drags the cursor along so it stays
        // on screen.
        if let Some((cursor, top, rows)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state.copy_cursor.map(|cursor| {
                    (
                        cursor,
                        state.scroll_top_line,
                        u64::from(state.visible_line_count.max(1)),
                    )
                })
            })
        {
            let line = cursor.line.clamp(top, top + rows - 1);
            if line != cursor.line {
                self.set_copy_point(node_id, SelectionPoint { line, ..cursor });
            }
        }
        Ok(())
    }

    async fn follow_output_for_view(&mut self, node_id: NodeId) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        self.client
            .state_mut()
            .set_view_follow_output(node_id, true);
        Ok(())
    }

    async fn fetch_view_slice(&mut self, node_id: NodeId, scroll_top_line: u64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.visible_line_count == 0 {
            return Ok(());
        }
        let response = self
            .client
            .capture_scrollback_slice(
                state.buffer_id,
                scroll_top_line,
                u32::from(state.visible_line_count),
            )
            .await?;
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.total_line_count = response
                .total_lines
                .max(u64::from(state.visible_line_count));
        }
        self.client
            .state_mut()
            .set_view_visible_lines(node_id, scroll_top_line, response.lines);
        Ok(())
    }

    /// Expand `template` as a format for the focused pane of `session_id`, or
    /// leave it as is without a session.
    fn expand_format(&self, session_id: Option<SessionId>, template: &str) -> String {
        session_id
            .or(self.active_session_id)
            .and_then(|session_id| {
                crate::format::FormatContext::from_state(
                    self.client.state(),
                    session_id,
                    None,
                    self.base_index(),
                )
            })
            .map(|context| crate::format::expand_format(template, &context))
            .unwrap_or_else(|| template.to_owned())
    }

    fn open_chooser(&mut self, scope: ChooserScope, session_id: Option<SessionId>) {
        let mode = self.input_state.current_mode();
        let return_mode = if [SEARCH_MODE, HINTS_MODE, PROMPT_MODE, TREE_MODE].contains(&mode) {
            NORMAL_MODE.to_owned()
        } else {
            mode.to_owned()
        };
        self.search_prompt = None;
        self.command_prompt = None;
        self.confirm = None;
        self.chooser = Some(Chooser::open(
            scope,
            self.client.state(),
            session_id.or(self.active_session_id),
            self.base_index(),
            return_mode,
        ));
        self.input_state.set_mode(TREE_MODE);
    }

    fn close_chooser(&mut self) {
        if let Some(chooser) = self.chooser.take() {
            self.input_state.set_mode(chooser.return_mode);
        } else {
            self.input_state.set_mode(NORMAL_MODE);
        }
    }

    async fn handle_chooser_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        key: KeyEvent,
    ) -> Result<()> {
        let base_index = self.base_index();
        let rows = match &self.chooser {
            Some(chooser) => chooser.rows(self.client.state(), self.active_session_id, base_index),
            None => {
                self.input_state.set_mode(NORMAL_MODE);
                return Ok(());
            }
        };
        let chooser = self.chooser.as_mut().expect("checked chooser");

        if let Some(item) = chooser.confirm_kill.take() {
            if matches!(key, KeyEvent::Char('y' | 'Y')) {
                self.kill_chooser_item(item).await;
            }
            return Ok(());
        }

        if chooser.editing_filter {
            match key {
                KeyEvent::Char(ch) => chooser.filter.push(ch),
                KeyEvent::Bytes(bytes) => chooser.filter.push_str(&String::from_utf8_lossy(&bytes)),
                KeyEvent::Backspace | KeyEvent::Ctrl('h') => {
                    if chooser.filter.pop().is_none() {
                        chooser.editing_filter = false;
                    }
                }
                KeyEvent::Ctrl('u') => chooser.filter.clear(),
                KeyEvent::Escape => {
                    chooser.filter.clear();
                    chooser.editing_filter = false;
                }
                KeyEvent::Up => chooser.move_by(&rows, -1),
                KeyEvent::Down => chooser.move_by(&rows, 1),
                // Enter ends filtering; a second Enter chooses.
                KeyEvent::Enter | KeyEvent::Tab => chooser.editing_filter = false,
                _ => {}
            }
            return Ok(());
        }

        let page = isize::try_from(chooser_area(viewport).size.height.saturating_sub(3).max(1))
            .unwrap_or(1);
        match key {
            KeyEvent::Char('j') | KeyEvent::Down | KeyEvent::Ctrl('n') => chooser.move_by(&rows, 1),
            KeyEvent::Char('k') | KeyEvent::Up | KeyEvent::Ctrl('p') => chooser.move_by(&rows, -1),
            KeyEvent::PageDown | KeyEvent::Ctrl('d') => chooser.move_by(&rows, page),
            KeyEvent::PageUp | KeyEvent::Ctrl('u') => chooser.move_by(&rows, -page),
            KeyEvent::Char('g') | KeyEvent::Home => chooser.select_first(&rows),
            KeyEvent::Char('G') | KeyEvent::End => chooser.select_last(&rows),
            KeyEvent::Char('l' | '+') | KeyEvent::Right => chooser.expand(&rows),
            KeyEvent::Char('h' | '-') | KeyEvent::Left => chooser.collapse(&rows),
            KeyEvent::Char('/') => chooser.editing_filter = true,
            KeyEvent::Char('x') => match chooser.target(self.client.state(), &rows) {
                Some(item) if !item.exists(self.client.state()) => self.report_gone_item(item),
                target => chooser.confirm_kill = target,
            },
            KeyEvent::Escape if !chooser.filter.is_empty() => chooser.filter.clear(),
            KeyEvent::Char('q') | KeyEvent::Escape | KeyEvent::Ctrl('c') => self.close_chooser(),
            KeyEvent::Enter => {
                let selected = chooser.target(self.client.state(), &rows);
                self.close_chooser();
                if let Some(item) = selected {
                    self.choose_item(session_id, item).await?;
                }
            }
            // Any other text starts a filter, so typing a name finds it.
            KeyEvent::Char(ch) if !ch.is_control() => {
                chooser.editing_filter = true;
                chooser.filter.push(ch);
            }
            _ => {}
        }
        Ok(())
    }

    /// Say that a picker item closed before it could be acted on.
    fn report_gone_item(&mut self, item: ChooserItem) {
        self.show_message(
            &format!("{} no longer exists", item.kind()),
            DEFAULT_DISPLAY_TIME,
        );
    }

    /// Switch to the chosen session, window or pane, like Enter in tmux
    /// `choose-tree`. An item that has closed is reported, not replaced.
    async fn choose_item(&mut self, session_id: SessionId, item: ChooserItem) -> Result<()> {
        if !item.exists(self.client.state()) {
            self.report_gone_item(item);
            return Ok(());
        }
        let target_session = item.session_id();
        if target_session != session_id || self.active_session_id != Some(target_session) {
            self.perform_session_switch(target_session).await?;
        }
        match item {
            ChooserItem::Session(_) => Ok(()),
            ChooserItem::Window(session_id, window_id) => {
                let state = self.client.state();
                let has_root_tabs = state
                    .sessions
                    .get(&session_id)
                    .and_then(|session| state.nodes.get(&session.root_node_id))
                    .is_some_and(|root| root.tabs.is_some());
                let position = window_position(state, session_id, window_id);
                let Some(position) = position.filter(|_| has_root_tabs) else {
                    if position.is_none() {
                        self.report_gone_item(item);
                    }
                    return Ok(());
                };
                self.client
                    .request_message(ClientMessage::Session(SessionRequest::SelectRootTab {
                        request_id: self.client.next_request_id(),
                        session_id,
                        index: u32::try_from(position).map_err(|_| {
                            MuxError::invalid_input("window index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            ChooserItem::Pane(session_id, node_id) => self.focus_node(session_id, node_id).await,
        }
    }

    /// Kill a picker item with the same commands as the CLI (`kill-session`,
    /// `kill-window`, `kill-pane`), so it behaves exactly as they do. An item
    /// that has closed is reported, not replaced: a window is killed by the
    /// number it has now.
    async fn kill_chooser_item(&mut self, item: ChooserItem) {
        if !item.exists(self.client.state()) {
            self.report_gone_item(item);
            return;
        }
        let base_index = self.base_index().unwrap_or(0);
        let argv: Vec<String> = match item {
            ChooserItem::Session(_) => vec!["kill-session".to_owned()],
            ChooserItem::Window(session_id, window_id) => {
                let Some(position) = window_position(self.client.state(), session_id, window_id)
                else {
                    self.report_gone_item(item);
                    return;
                };
                vec![
                    "kill-window".to_owned(),
                    "-t".to_owned(),
                    crate::numbering::window_number(position, Some(base_index)).to_string(),
                ]
            }
            ChooserItem::Pane(_, node_id) => {
                vec!["kill-pane".to_owned(), "-t".to_owned(), node_id.to_string()]
            }
        };
        let Some(runner) = self.command_runner.clone() else {
            self.show_message(
                "commands are not available in this client",
                DEFAULT_DISPLAY_TIME,
            );
            return;
        };
        let context = CommandContext {
            session_id: Some(item.session_id()),
            client_id: self.client.cached_client_id(),
            // An explicit base index makes the window number unambiguous.
            base_index: Some(base_index),
        };
        if let Err(error) = runner.run(argv, context).await {
            self.show_message(&error.to_string(), DEFAULT_DISPLAY_TIME);
        }
    }

    /// Answer a `confirm-before` question: `y` runs its actions, any other
    /// key cancels it, as in tmux.
    async fn answer_confirm(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        confirm: PendingConfirm,
        key: KeyEvent,
    ) -> Result<()> {
        self.input_state.set_mode(confirm.return_mode);
        if key != KeyEvent::Char('y') {
            return Ok(());
        }
        self.execute_actions(Some(session_id), Some(viewport), confirm.actions)
            .await
    }

    async fn handle_prompt_key(&mut self, session_id: SessionId, key: KeyEvent) -> Result<()> {
        let Some(prompt) = self.command_prompt.as_mut() else {
            self.input_state.set_mode(NORMAL_MODE);
            return Ok(());
        };
        let history = &self.prompt_history;
        if prompt.vi_key(&key, history) {
            return Ok(());
        }
        match key {
            KeyEvent::Enter => return self.commit_command_prompt(session_id).await,
            KeyEvent::Escape | KeyEvent::Ctrl('c') | KeyEvent::Ctrl('g') => {
                let return_mode = prompt.return_mode.clone();
                self.command_prompt = None;
                self.input_state.set_mode(return_mode);
            }
            KeyEvent::Char(ch) => prompt.insert(ch.encode_utf8(&mut [0; 4])),
            KeyEvent::Bytes(bytes) => prompt.insert(&String::from_utf8_lossy(&bytes)),
            KeyEvent::Backspace | KeyEvent::Ctrl('h') => prompt.backspace(),
            KeyEvent::Delete | KeyEvent::Ctrl('d') => prompt.delete(),
            KeyEvent::Left | KeyEvent::Ctrl('b') => prompt.left(),
            KeyEvent::Right | KeyEvent::Ctrl('f') => prompt.right(),
            KeyEvent::Home | KeyEvent::Ctrl('a') => prompt.home(),
            KeyEvent::End | KeyEvent::Ctrl('e') => prompt.end(),
            KeyEvent::Ctrl('u') => prompt.kill_to_start(),
            KeyEvent::Ctrl('k') => prompt.kill_to_end(),
            KeyEvent::Ctrl('w') => prompt.delete_word(),
            KeyEvent::Up | KeyEvent::Ctrl('p') => prompt.history_up(history),
            KeyEvent::Down | KeyEvent::Ctrl('n') => prompt.history_down(history),
            _ => {}
        }
        Ok(())
    }

    async fn commit_command_prompt(&mut self, session_id: SessionId) -> Result<()> {
        let Some(prompt) = self.command_prompt.take() else {
            return Ok(());
        };
        self.input_state.set_mode(prompt.return_mode.clone());
        // Only lines typed as commands are worth recalling; rename prompts
        // and other templates are not.
        if prompt.template.is_none() {
            record_history(&mut self.prompt_history, &prompt.input);
        }
        self.run_command_line(Some(session_id), &prompt.command_line())
            .await;
        Ok(())
    }

    /// Run `line` through the command runner and show its output, or its
    /// error, in the status line.
    async fn run_command_line(&mut self, session_id: Option<SessionId>, line: &str) {
        let argv = match shell_words::split(line) {
            Ok(argv) => argv,
            Err(error) => {
                self.show_message(&format!("{error}"), DEFAULT_DISPLAY_TIME);
                return;
            }
        };
        if argv.is_empty() {
            return;
        }
        // The picker belongs to this client, so its commands open it here
        // rather than going through the runner.
        let scope = match argv[0].as_str() {
            "choose-tree" if argv[1..].iter().any(|arg| arg == "-s") => {
                Some(ChooserScope::Sessions)
            }
            "choose-tree" if argv[1..].iter().any(|arg| arg == "-w") => Some(ChooserScope::Windows),
            "choose-tree" => Some(ChooserScope::Tree),
            "choose-window" => Some(ChooserScope::Windows),
            "choose-session" => Some(ChooserScope::Sessions),
            _ => None,
        };
        if let Some(scope) = scope {
            self.open_chooser(scope, session_id);
            return;
        }
        let Some(runner) = self.command_runner.clone() else {
            self.show_message(
                "commands are not available in this client",
                DEFAULT_DISPLAY_TIME,
            );
            return;
        };
        let context = CommandContext {
            session_id: session_id.or(self.active_session_id),
            client_id: self.client.cached_client_id(),
            base_index: self.base_index(),
        };
        match runner.run(argv, context).await {
            Ok(output) => {
                let output = output.lines().collect::<Vec<_>>().join("; ");
                if !output.trim().is_empty() {
                    self.show_message(&output, DEFAULT_DISPLAY_TIME);
                }
            }
            Err(error) => self.show_message(&error.to_string(), DEFAULT_DISPLAY_TIME),
        }
    }

    /// Open the search prompt. The prompt starts empty, as in vim and tmux;
    /// committing it empty clears the search.
    async fn enter_search_mode(
        &mut self,
        presentation: &PresentationModel,
        backward: bool,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        if self.view_is_alternate_screen(leaf.node_id) {
            return Ok(());
        }

        // Searching from copy mode returns there, so the cursor can land on
        // the match.
        let return_mode = if self.in_copy_mode() {
            self.input_state.current_mode().to_owned()
        } else {
            NORMAL_MODE.to_owned()
        };
        self.search_prompt = Some(SearchPrompt {
            node_id: leaf.node_id,
            query: String::new(),
            backward,
            return_mode,
        });
        self.input_state.set_mode(SEARCH_MODE);
        Ok(())
    }

    async fn handle_search_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        presentation: &PresentationModel,
        key: KeyEvent,
    ) -> Result<()> {
        if self.search_prompt.is_none() {
            return self.enter_search_mode(presentation, false).await;
        }

        match key {
            KeyEvent::Char(ch) => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push(ch);
                }
                Ok(())
            }
            KeyEvent::Tab => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push('\t');
                }
                Ok(())
            }
            KeyEvent::Backspace | KeyEvent::Ctrl('h') => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.pop();
                }
                Ok(())
            }
            KeyEvent::Enter => {
                self.execute_actions(Some(session_id), Some(viewport), vec![Action::CommitSearch])
                    .await
            }
            KeyEvent::Escape => {
                self.execute_actions(Some(session_id), Some(viewport), vec![Action::CancelSearch])
                    .await
            }
            KeyEvent::Bytes(bytes) => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(())
            }
            KeyEvent::Ctrl(_)
            | KeyEvent::Alt(_)
            | KeyEvent::Up
            | KeyEvent::Down
            | KeyEvent::Left
            | KeyEvent::Right
            | KeyEvent::Home
            | KeyEvent::End
            | KeyEvent::Insert
            | KeyEvent::Delete
            | KeyEvent::PageUp
            | KeyEvent::PageDown
            | KeyEvent::Key { .. } => Ok(()),
        }
    }

    async fn handle_search_paste(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        bytes: Vec<u8>,
    ) -> Result<()> {
        if self.search_prompt.is_none() {
            return self.cancel_search_prompt(session_id, viewport).await;
        }
        if let Some(prompt) = &mut self.search_prompt {
            prompt.query.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok(())
    }

    fn enter_hints_mode(
        &mut self,
        on_select: Option<String>,
        presentation: &PresentationModel,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        let node_id = leaf.node_id;
        let buffer_id = leaf.buffer_id;
        if self.view_is_alternate_screen(node_id) {
            return Ok(());
        }

        // Scan the same lines the renderer draws: the local visible lines when
        // present, otherwise the buffer's visible snapshot, positioned by the
        // same follow-output offset the overlay uses.
        let (lines, top_line) = {
            let state = self.client.state();
            let view_state = state.view_state(node_id);
            let rendered: Vec<embers_core::SnapshotLine> = match view_state
                .map(|view| view.visible_lines.as_slice())
                .filter(|lines| !lines.is_empty())
            {
                Some(lines) => lines.to_vec(),
                None => state
                    .snapshots
                    .get(&buffer_id)
                    .map(|snapshot| snapshot.lines.clone())
                    .unwrap_or_default(),
            };
            let scroll_top = view_state.map(|view| view.scroll_top_line).unwrap_or(0);
            let follow = view_state.map(|view| view.follow_output).unwrap_or(true);
            let content_rows = usize::from(leaf.content_rect.size.height);
            let position = crate::renderer::follow_output_position(
                &rendered,
                content_rows,
                scroll_top,
                follow,
            );
            let displayed = rendered[position.display_offset.min(rendered.len())..].to_vec();
            (displayed, position.displayed_top_line)
        };
        if lines.is_empty() {
            self.record_notification(format_notification(NotifyLevel::Info, "no hints found"));
            return Ok(());
        }

        let pattern_sources = &self.config.active_script().loaded_config().hints.patterns;
        let patterns = if pattern_sources.is_empty() {
            crate::hints::default_patterns()
        } else {
            crate::hints::compile_patterns(pattern_sources)
        };
        let matches = crate::hints::scan(&lines, &patterns, top_line);
        if matches.is_empty() {
            self.record_notification(format_notification(NotifyLevel::Info, "no hints found"));
            return Ok(());
        }

        let Some(state) = self.client.state_mut().view_state_mut(node_id) else {
            // No view state to attach the overlay to; don't enter a hints mode
            // that would have no matches.
            return Ok(());
        };
        state.hints_state = Some(HintsState {
            matches,
            typed: String::new(),
            on_select,
        });
        self.hints_node = Some(node_id);
        self.input_state.set_mode(HINTS_MODE);
        Ok(())
    }

    async fn handle_hints_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        _presentation: &PresentationModel,
        key: KeyEvent,
    ) -> Result<()> {
        let Some(node_id) = self.hints_node else {
            self.input_state.set_mode(NORMAL_MODE);
            return Ok(());
        };
        // The overlay can be invalidated out from under us — e.g. a fresh
        // snapshot clears hints_state because the visible content changed. If
        // that happened, tear the overlay down and drop back to NORMAL_MODE so
        // we don't stay stuck in HINTS_MODE swallowing every non-Escape key.
        let overlay_active = self
            .client
            .state()
            .view_state(node_id)
            .is_some_and(|state| state.hints_state.is_some());
        if !overlay_active {
            self.cancel_hints(node_id);
            return Ok(());
        }
        match key {
            KeyEvent::Escape => {
                self.cancel_hints(node_id);
                Ok(())
            }
            KeyEvent::Backspace | KeyEvent::Ctrl('h') => {
                if let Some(state) = self
                    .client
                    .state_mut()
                    .view_state_mut(node_id)
                    .and_then(|state| state.hints_state.as_mut())
                {
                    state.typed.pop();
                }
                Ok(())
            }
            KeyEvent::Char(ch) => {
                self.hints_type_char(session_id, viewport, node_id, ch)
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Append a character to the typed hint prefix, selecting on a unique full
    /// match and ignoring characters that match no label.
    async fn hints_type_char(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        node_id: NodeId,
        ch: char,
    ) -> Result<()> {
        let (selected, extend) = {
            let Some(state) = self
                .client
                .state()
                .view_state(node_id)
                .and_then(|state| state.hints_state.as_ref())
            else {
                return Ok(());
            };
            let mut typed = state.typed.clone();
            typed.push(ch);
            let any_prefix = state
                .matches
                .iter()
                .any(|hint| hint.label.starts_with(&typed));
            if !any_prefix {
                // Not a valid continuation; ignore the keystroke.
                (None, None)
            } else if let Some(hint) = state.matches.iter().find(|hint| hint.label == typed) {
                (Some(hint.clone()), None)
            } else {
                (None, Some(typed))
            }
        };

        if let Some(typed) = extend {
            if let Some(state) = self
                .client
                .state_mut()
                .view_state_mut(node_id)
                .and_then(|state| state.hints_state.as_mut())
            {
                state.typed = typed;
            }
            return Ok(());
        }
        if let Some(hint) = selected {
            return self.select_hint(session_id, viewport, node_id, hint).await;
        }
        Ok(())
    }

    fn cancel_hints(&mut self, node_id: NodeId) {
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.hints_state = None;
        }
        self.hints_node = None;
        self.input_state.set_mode(NORMAL_MODE);
    }

    async fn select_hint(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        node_id: NodeId,
        hint: crate::state::HintMatch,
    ) -> Result<()> {
        let on_select = self
            .client
            .state()
            .view_state(node_id)
            .and_then(|state| state.hints_state.as_ref())
            .and_then(|state| state.on_select.clone());
        self.cancel_hints(node_id);

        match on_select {
            Some(name) => {
                let context = self
                    .context_for(Some(session_id), Some(viewport), None)
                    .with_hint_selection(hint.text.clone());
                match self.config.active_script().run_named_action(&name, context) {
                    Ok(actions) => {
                        self.execute_actions(Some(session_id), Some(viewport), actions)
                            .await
                    }
                    Err(error) => {
                        self.record_notification(error.to_string());
                        Ok(())
                    }
                }
            }
            None => {
                // Report only the length, not the copied text: record_notification
                // logs at WARN and the status line renders it, so raw hint text
                // (possibly a secret or control chars) must not leak in.
                let copied = hint.text.chars().count();
                self.enqueue_clipboard(hint.text);
                self.record_notification(format_notification(
                    NotifyLevel::Info,
                    &format!("copied {copied} characters"),
                ));
                Ok(())
            }
        }
    }

    async fn commit_search_prompt(&mut self, session_id: SessionId, viewport: Size) -> Result<()> {
        let Some(prompt) = self.search_prompt.take() else {
            return self.cancel_search_prompt(session_id, viewport).await;
        };
        self.input_state.set_mode(prompt.return_mode.clone());
        let Some(buffer_id) = self
            .client
            .state()
            .view_state(prompt.node_id)
            .map(|state| state.buffer_id)
        else {
            return Ok(());
        };

        let matches = if prompt.query.is_empty() {
            Vec::new()
        } else {
            let snapshot = self.client.capture_buffer(buffer_id).await?;
            compute_search_matches(&snapshot.lines, &prompt.query)
        };

        if let Some(state) = self.client.state_mut().view_state_mut(prompt.node_id) {
            if prompt.query.is_empty() {
                state.search_state = None;
            } else {
                state.search_state = Some(SearchState {
                    query: prompt.query.clone(),
                    matches,
                    active_match_index: None,
                });
            }
        }
        self.copy.search_backward = prompt.backward;

        match self.initial_search_index(prompt.node_id, !prompt.backward) {
            Some(index) => self.jump_to_search_index(prompt.node_id, index).await,
            None => Ok(()),
        }
    }

    /// The match a newly committed search lands on: the next one in the search
    /// direction from the copy-mode cursor (not counting a match under it), or
    /// without a cursor the first match (forward) or the last (backward).
    fn initial_search_index(&self, node_id: NodeId, forward: bool) -> Option<usize> {
        let matches = &self
            .client
            .state()
            .view_state(node_id)?
            .search_state
            .as_ref()?
            .matches;
        match self.copy_cursor_of(node_id) {
            Some(cursor) => next_match_index(matches, cursor, forward),
            None if matches.is_empty() => None,
            None if forward => Some(0),
            None => Some(matches.len() - 1),
        }
    }

    async fn cancel_search_prompt(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
    ) -> Result<()> {
        let return_mode = self
            .search_prompt
            .take()
            .map_or_else(|| NORMAL_MODE.to_owned(), |prompt| prompt.return_mode);
        self.input_state.set_mode(return_mode);
        Ok(())
    }

    /// `n` (`forward`) or `N`, `count` times. After a backward search the
    /// directions swap, as in vim. In copy mode the next match is found from
    /// the cursor; otherwise from the active match.
    async fn navigate_search(
        &mut self,
        presentation: &PresentationModel,
        forward: bool,
        count: u32,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        if self.view_is_alternate_screen(leaf.node_id) {
            return Ok(());
        }
        let forward = forward != self.copy.search_backward;
        let mut cursor = self.copy_cursor_of(leaf.node_id);
        let Some(search_state) = self
            .client
            .state()
            .view_state(leaf.node_id)
            .and_then(|state| state.search_state.as_ref())
        else {
            return Ok(());
        };
        let matches = &search_state.matches;
        if matches.is_empty() {
            return Ok(());
        }
        // Moving `count` matches wraps around, so a full lap is a no-op.
        let steps = (count.max(1) as usize - 1) % matches.len() + 1;
        let mut current = search_state.active_match_index;
        for _ in 0..steps {
            let next = match cursor {
                Some(at) => next_match_index(matches, at, forward).unwrap_or(0),
                None => {
                    let current = current.unwrap_or(0);
                    if forward {
                        (current + 1) % matches.len()
                    } else if current == 0 {
                        matches.len() - 1
                    } else {
                        current - 1
                    }
                }
            };
            current = Some(next);
            cursor = cursor.map(|_| SelectionPoint {
                line: matches[next].line,
                column: matches[next].start_column,
            });
        }
        match current {
            Some(index) => self.jump_to_search_index(leaf.node_id, index).await,
            None => Ok(()),
        }
    }

    async fn jump_to_search_index(&mut self, node_id: NodeId, index: usize) -> Result<()> {
        let Some((selected, current_top, visible_line_count)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state.search_state.as_ref().and_then(|search| {
                    search
                        .matches
                        .get(index)
                        .copied()
                        .map(|selected| (selected, state.scroll_top_line, state.visible_line_count))
                })
            })
        else {
            return Ok(());
        };

        let visible_line_count = u64::from(visible_line_count.max(1));
        let new_top = if selected.line < current_top {
            selected.line
        } else if selected.line >= current_top.saturating_add(visible_line_count) {
            selected
                .line
                .saturating_sub(visible_line_count.saturating_sub(1))
        } else {
            current_top
        };
        if new_top != current_top
            || self
                .client
                .state()
                .view_state(node_id)
                .is_some_and(|state| state.follow_output)
        {
            self.fetch_view_slice(node_id, new_top).await?;
        }

        let in_copy_mode = if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            if let Some(search) = &mut state.search_state {
                search.active_match_index = Some(index);
            }
            state.follow_output = false;
            state.copy_cursor.is_some()
        } else {
            false
        };
        // In copy mode the cursor (and a selection's free end) moves onto the
        // match, as in tmux.
        if in_copy_mode {
            self.set_copy_point(
                node_id,
                SelectionPoint {
                    line: selected.line,
                    column: selected.start_column,
                },
            );
        }
        Ok(())
    }

    /// `v`/`V`/`C-v`: start a selection of `kind` at the copy-mode cursor,
    /// entering copy mode first if needed. With a selection already active, the
    /// same kind clears it and another kind switches to it, as in vim.
    fn enter_select_mode(
        &mut self,
        presentation: &PresentationModel,
        kind: SelectionKind,
    ) -> Result<()> {
        let node_id = self.focused_leaf(presentation)?.node_id;
        let active_kind = self
            .client
            .state()
            .view_state(node_id)
            .and_then(|state| state.selection_state.as_ref())
            .map(|selection| selection.kind);
        if let Some(active_kind) = active_kind {
            if active_kind == kind {
                self.cancel_selection(node_id);
            } else if let Some(selection) = self
                .client
                .state_mut()
                .view_state_mut(node_id)
                .and_then(|state| state.selection_state.as_mut())
            {
                selection.kind = kind;
            }
            return Ok(());
        }

        if !self.ensure_copy_cursor(node_id) {
            return Ok(());
        }
        if let Some(state) = self.client.state_mut().view_state_mut(node_id)
            && let Some(start) = state.copy_cursor
        {
            state.selection_state = Some(SelectionState {
                kind,
                anchor: start,
                cursor: start,
            });
        }
        self.input_state.set_mode(SELECT_MODE);
        Ok(())
    }

    /// Where copy mode puts its cursor on entry: the program's cursor when the
    /// view is following output, else the top-left of the view.
    fn selection_origin(&self, node_id: NodeId) -> SelectionPoint {
        let Some(state) = self.client.state().view_state(node_id) else {
            return SelectionPoint::default();
        };
        let fallback = SelectionPoint {
            line: state.scroll_top_line,
            column: 0,
        };
        if !state.follow_output {
            return fallback;
        }
        self.client
            .state()
            .snapshots
            .get(&state.buffer_id)
            .and_then(|snapshot| {
                snapshot.cursor.map(|cursor| SelectionPoint {
                    line: state.scroll_top_line + u64::from(cursor.position.row),
                    column: cursor.position.col,
                })
            })
            .unwrap_or(fallback)
    }

    fn in_copy_mode(&self) -> bool {
        matches!(self.input_state.current_mode(), COPY_MODE | SELECT_MODE)
    }

    /// Switch input to copy mode, unless it is already in copy or select mode.
    fn enter_copy_input_mode(&mut self) {
        if !self.in_copy_mode() {
            self.input_state.set_mode(COPY_MODE);
        }
    }

    /// Whether a typed digit extends the count prefix rather than running a
    /// binding. `0` only continues a count (on its own it is a motion), and a
    /// digit the mode binds itself keeps its binding.
    fn accepts_count_digit(&self, ch: char) -> bool {
        let Some(digit) = ch.to_digit(10) else {
            return false;
        };
        if !self.input_state.pending_sequence().is_empty() {
            return false;
        }
        if self.copy.pending_count.is_some() {
            return true;
        }
        digit != 0
            && !self
                .config
                .active_script()
                .loaded_config()
                .bindings
                .get(self.input_state.current_mode())
                .is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|binding| binding.sequence.first() == Some(&KeyToken::Char(ch)))
                })
    }

    /// The point copy-mode motions move: a selection's free end, else the copy
    /// cursor.
    fn copy_cursor_of(&self, node_id: NodeId) -> Option<SelectionPoint> {
        let state = self.client.state().view_state(node_id)?;
        state
            .selection_state
            .as_ref()
            .map(|selection| selection.cursor)
            .or(state.copy_cursor)
    }

    fn set_copy_point(&mut self, node_id: NodeId, point: SelectionPoint) {
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.copy_cursor = Some(point);
            if let Some(selection) = &mut state.selection_state {
                selection.cursor = point;
            }
        }
    }

    /// Put the view in copy mode if it is not already: freeze it and place the
    /// cursor where the program's cursor is. Returns `false` for views copy mode
    /// does not support (the alternate screen).
    fn ensure_copy_cursor(&mut self, node_id: NodeId) -> bool {
        let Some(state) = self.client.state().view_state(node_id) else {
            return false;
        };
        if state.alternate_screen {
            return false;
        }
        if state.copy_cursor.is_some() {
            return true;
        }
        let origin = state
            .selection_state
            .as_ref()
            .map(|selection| selection.cursor)
            .unwrap_or_else(|| self.selection_origin(node_id));
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.copy_cursor = Some(origin);
            state.follow_output = false;
        }
        true
    }

    fn copy_viewport(&self, node_id: NodeId) -> Option<CopyViewport> {
        let state = self.client.state().view_state(node_id)?;
        let cols = self
            .client
            .state()
            .snapshots
            .get(&state.buffer_id)
            .map(|snapshot| snapshot.size.cols)
            .filter(|cols| *cols > 0)
            .unwrap_or(u16::MAX);
        Some(CopyViewport {
            top: state.scroll_top_line,
            rows: u64::from(state.visible_line_count.max(1)),
            total: state.total_line_count.max(1),
            cols,
        })
    }

    /// Capture the view's buffer text for motions, reusing the last capture
    /// while the buffer's snapshot is unchanged.
    async fn load_copy_text(&mut self, node_id: NodeId) -> Result<()> {
        let Some(buffer_id) = self
            .client
            .state()
            .view_state(node_id)
            .map(|state| state.buffer_id)
        else {
            return Ok(());
        };
        let sequence = self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .map_or(0, |snapshot| snapshot.sequence);
        if self
            .copy
            .text
            .as_ref()
            .is_some_and(|text| text.buffer_id == buffer_id && text.sequence == sequence)
        {
            return Ok(());
        }
        let snapshot = self.client.capture_buffer(buffer_id).await?;
        self.copy.text = Some(CapturedText {
            buffer_id,
            sequence,
            lines: snapshot.lines,
        });
        Ok(())
    }

    fn copy_lines(&self) -> &[String] {
        self.copy
            .text
            .as_ref()
            .map_or(&[], |text| text.lines.as_slice())
    }

    /// The text of one history line: from the rendered view when it is on
    /// screen, else from the captured buffer.
    async fn copy_line_text(&mut self, node_id: NodeId, line: u64) -> Result<String> {
        if let Some(text) = self.client.state().view_state(node_id).and_then(|state| {
            let row = usize::try_from(line.checked_sub(state.scroll_top_line)?).ok()?;
            state.visible_lines.get(row).map(|line| line.text.clone())
        }) {
            return Ok(text);
        }
        self.load_copy_text(node_id).await?;
        Ok(usize::try_from(line)
            .ok()
            .and_then(|line| self.copy_lines().get(line))
            .cloned()
            .unwrap_or_default())
    }

    async fn copy_motion(&mut self, node_id: NodeId, motion: CopyMotion, count: u32) -> Result<()> {
        if !self.ensure_copy_cursor(node_id) {
            return Ok(());
        }
        self.enter_copy_input_mode();
        if motion.needs_text() {
            self.load_copy_text(node_id).await?;
        }
        let (Some(cursor), Some(viewport)) =
            (self.copy_cursor_of(node_id), self.copy_viewport(node_id))
        else {
            return Ok(());
        };
        let repeat = if motion.repeats() { count.max(1) } else { 1 };
        let lines = self.copy_lines();
        let mut outcome = MotionOutcome {
            cursor,
            top: viewport.top,
        };
        for _ in 0..repeat {
            let next = apply_motion(
                motion,
                outcome.cursor,
                CopyViewport {
                    top: outcome.top,
                    ..viewport
                },
                lines,
            );
            if next == outcome {
                break;
            }
            outcome = next;
        }
        self.apply_copy_outcome(node_id, viewport.top, outcome)
            .await
    }

    /// Scroll to `outcome.top` if it moved and put the cursor at
    /// `outcome.cursor`.
    async fn apply_copy_outcome(
        &mut self,
        node_id: NodeId,
        old_top: u64,
        outcome: MotionOutcome,
    ) -> Result<()> {
        if outcome.top != old_top {
            self.fetch_view_slice(node_id, outcome.top).await?;
        }
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.follow_output = false;
        }
        self.set_copy_point(node_id, outcome.cursor);
        Ok(())
    }

    async fn copy_jump(
        &mut self,
        node_id: NodeId,
        kind: JumpKind,
        target: char,
        count: u32,
        repeat: bool,
    ) -> Result<()> {
        if !self.ensure_copy_cursor(node_id) {
            return Ok(());
        }
        self.enter_copy_input_mode();
        self.load_copy_text(node_id).await?;
        let Some(cursor) = self.copy_cursor_of(node_id) else {
            return Ok(());
        };
        let line = usize::try_from(cursor.line)
            .ok()
            .and_then(|line| self.copy_lines().get(line))
            .map_or("", String::as_str);
        let mut column = cursor.column;
        let mut repeat = repeat;
        for _ in 0..count.max(1) {
            match jump(
                kind,
                target,
                SelectionPoint { column, ..cursor },
                line,
                repeat,
            ) {
                Some(next) => column = next,
                None => break,
            }
            // Later steps of a counted `t`/`T` must move past the previous stop.
            repeat = true;
        }
        self.set_copy_point(node_id, SelectionPoint { column, ..cursor });
        Ok(())
    }

    /// `o`: swap the selection's ends so motions move the other one.
    async fn select_other_end(&mut self, node_id: NodeId) -> Result<()> {
        let Some(cursor) = self
            .client
            .state_mut()
            .view_state_mut(node_id)
            .and_then(|state| state.selection_state.as_mut())
            .map(|selection| {
                std::mem::swap(&mut selection.anchor, &mut selection.cursor);
                selection.cursor
            })
        else {
            return Ok(());
        };
        let Some(viewport) = self.copy_viewport(node_id) else {
            return Ok(());
        };
        let outcome = MotionOutcome {
            cursor,
            top: viewport.reveal(viewport.top, cursor.line),
        };
        self.apply_copy_outcome(node_id, viewport.top, outcome)
            .await
    }

    /// Leave copy mode on every view that is in it and return to `normal` mode.
    async fn exit_copy_mode(&mut self) -> Result<()> {
        self.clear_copy_views().await?;
        if self.in_copy_mode() {
            self.input_state.set_mode(NORMAL_MODE);
        }
        Ok(())
    }

    /// Take every view out of copy mode: drop cursors, selections and search
    /// highlights, and follow output again.
    async fn clear_copy_views(&mut self) -> Result<()> {
        let node_ids = self
            .client
            .state()
            .view_state
            .iter()
            .filter(|(_, state)| state.copy_cursor.is_some() || state.selection_state.is_some())
            .map(|(node_id, _)| *node_id)
            .collect::<Vec<_>>();
        for node_id in node_ids {
            if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
                state.copy_cursor = None;
                state.selection_state = None;
                state.search_state = None;
            }
            self.follow_output_for_view(node_id).await?;
        }
        self.copy.clear_pending();
        self.copy.mouse = None;
        Ok(())
    }

    /// Copy the selection, if any, to the clipboard and leave copy mode (tmux
    /// `copy-pipe-and-cancel`).
    async fn copy_selection(&mut self, node_id: NodeId) -> Result<()> {
        if let Some((buffer_id, selection_state)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state
                    .selection_state
                    .clone()
                    .map(|selection| (state.buffer_id, selection))
            })
        {
            let snapshot = self.client.capture_buffer(buffer_id).await?;
            let copied = serialize_selection(&snapshot.lines, &selection_state);
            self.enqueue_clipboard(copied);
        }
        self.exit_copy_mode().await
    }

    /// Clear the selection and stay in copy mode with the cursor where the
    /// selection's free end was.
    fn cancel_selection(&mut self, node_id: NodeId) {
        let in_copy_mode = self
            .client
            .state_mut()
            .view_state_mut(node_id)
            .is_some_and(|state| {
                if let Some(selection) = state.selection_state.take() {
                    state.copy_cursor = Some(selection.cursor);
                }
                state.copy_cursor.is_some()
            });
        self.input_state
            .set_mode(if in_copy_mode { COPY_MODE } else { NORMAL_MODE });
    }

    /// A left-button press in a pane that has not requested the mouse: a
    /// double click copies the word under the pointer, a triple click the line,
    /// and a single press may become a drag selection.
    async fn begin_mouse_selection(&mut self, leaf: &LeafFrame, point: Point) -> Result<()> {
        let Some(state) = self.client.state().view_state(leaf.node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        let origin = leaf.content_rect.origin;
        let row = u64::try_from(point.y - origin.y).unwrap_or(0);
        let at = SelectionPoint {
            line: state.scroll_top_line.saturating_add(row),
            column: u16::try_from(point.x - origin.x).unwrap_or(0),
        };
        match self.copy.clicks.press(leaf.node_id, at, Instant::now()) {
            2 => {
                let text = self.copy_line_text(leaf.node_id, at.line).await?;
                if let Some((start, end)) = word_bounds(&text, at.column) {
                    let anchor = SelectionPoint {
                        column: start,
                        ..at
                    };
                    let cursor = SelectionPoint { column: end, ..at };
                    self.select_and_copy(leaf.node_id, SelectionKind::Character, anchor, cursor)
                        .await?;
                }
            }
            3 => {
                self.select_and_copy(leaf.node_id, SelectionKind::Line, at, at)
                    .await?;
            }
            _ => {
                self.copy.mouse = Some(MouseSelection {
                    node_id: leaf.node_id,
                    anchor: at,
                    dragging: false,
                });
            }
        }
        Ok(())
    }

    async fn select_and_copy(
        &mut self,
        node_id: NodeId,
        kind: SelectionKind,
        anchor: SelectionPoint,
        cursor: SelectionPoint,
    ) -> Result<()> {
        if !self.ensure_copy_cursor(node_id) {
            return Ok(());
        }
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.selection_state = Some(SelectionState {
                kind,
                anchor,
                cursor,
            });
            state.copy_cursor = Some(cursor);
        }
        self.copy_selection(node_id).await
    }

    /// Drag and release events after a press that may start a selection. The
    /// first drag enters copy mode with a character selection from the press
    /// point; later drags extend it, scrolling a line at a time while the
    /// pointer is above or below the pane. Release copies the selection and
    /// leaves copy mode, as tmux's `MouseDragEnd1Pane` does by default.
    async fn continue_mouse_selection(
        &mut self,
        presentation: &PresentationModel,
        point: Point,
        kind: MouseEventKind,
    ) -> Result<()> {
        let Some(mut drag) = self.copy.mouse.take() else {
            return Ok(());
        };
        if let MouseEventKind::Release(_) = kind {
            if drag.dragging {
                return self.copy_selection(drag.node_id).await;
            }
            return Ok(());
        }
        let Some(leaf) = presentation
            .leaves
            .iter()
            .find(|leaf| leaf.node_id == drag.node_id)
        else {
            return Ok(());
        };
        let Some(viewport) = self.copy_viewport(drag.node_id) else {
            return Ok(());
        };
        let rows = i64::try_from(viewport.rows).unwrap_or(i64::MAX);
        let origin = leaf.content_rect.origin;
        let row = i64::from(point.y) - i64::from(origin.y);
        let top = if row < 0 {
            viewport.top.saturating_sub(1)
        } else if row >= rows {
            viewport.reveal(viewport.top, viewport.top.saturating_add(viewport.rows))
        } else {
            viewport.top
        };
        let width = i64::from(leaf.content_rect.size.width.max(1));
        let at = SelectionPoint {
            line: top
                .saturating_add(u64::try_from(row.clamp(0, rows - 1)).unwrap_or(0))
                .min(viewport.total.saturating_sub(1)),
            column: u16::try_from((i64::from(point.x) - i64::from(origin.x)).clamp(0, width - 1))
                .unwrap_or(0),
        };
        if !drag.dragging {
            if !self.ensure_copy_cursor(drag.node_id) {
                return Ok(());
            }
            if let Some(state) = self.client.state_mut().view_state_mut(drag.node_id) {
                state.selection_state = Some(SelectionState {
                    kind: SelectionKind::Character,
                    anchor: drag.anchor,
                    cursor: at,
                });
            }
            self.input_state.set_mode(SELECT_MODE);
            drag.dragging = true;
        }
        self.copy.mouse = Some(drag);
        self.apply_copy_outcome(
            drag.node_id,
            viewport.top,
            MotionOutcome { cursor: at, top },
        )
        .await
    }

    fn enqueue_clipboard(&mut self, text: String) {
        self.enqueue_osc52(ClipboardSelection::Clipboard, &text);
    }

    /// Queue an OSC 52 write that puts `text` on the host terminal's clipboard.
    fn enqueue_osc52(&mut self, selection: ClipboardSelection, text: &str) {
        use base64::Engine as _;

        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let parameter = selection.osc52_parameter();
        self.terminal_output
            .push_back(format!("\x1b]52;{parameter};{encoded}\x07").into_bytes());
    }

    /// Focus the pane focused before the current one in the session's
    /// focused window (tmux `select-pane -l`). The server keeps the focus
    /// history; with no last pane this does nothing.
    async fn focus_last_pane(&mut self, session_id: SessionId) -> Result<()> {
        let previous_buffer = self.focused_buffer_for_session(session_id);
        match self
            .client
            .request_message(ClientMessage::Node(NodeRequest::FocusLast {
                request_id: self.client.next_request_id(),
                session_id,
            }))
            .await
        {
            Ok(_) => {}
            Err(error) if error.is_not_found() => return Ok(()),
            Err(error) => return Err(error),
        }
        self.client.resync_session(session_id).await?;

        let new_buffer = self.focused_buffer_for_session(session_id);
        if new_buffer != previous_buffer {
            for (buffer_id, focused) in [(previous_buffer, false), (new_buffer, true)] {
                if let Some(buffer_id) = buffer_id
                    && self.maybe_send_focus_sequence(buffer_id, focused).await?
                {
                    self.client.refresh_buffer_snapshot(buffer_id).await?;
                }
            }
        }
        Ok(())
    }

    async fn focus_node(&mut self, session_id: SessionId, node_id: NodeId) -> Result<()> {
        self.focus_node_with_shortcut(session_id, node_id, true)
            .await
    }

    async fn focus_node_with_shortcut(
        &mut self,
        session_id: SessionId,
        node_id: NodeId,
        allow_same_session_shortcut: bool,
    ) -> Result<()> {
        if allow_same_session_shortcut
            && self
                .client
                .state()
                .sessions
                .get(&session_id)
                .and_then(|session| session.focused_leaf_id)
                == Some(node_id)
        {
            return Ok(());
        }

        let previous_buffer = self
            .active_session_id
            .and_then(|active_session_id| self.focused_buffer_for_session(active_session_id));
        let sent_focus_out = if let Some(buffer_id) = previous_buffer {
            self.maybe_send_focus_sequence(buffer_id, false).await?
        } else {
            false
        };

        self.client
            .request_message(ClientMessage::Node(NodeRequest::Focus {
                request_id: self.client.next_request_id(),
                session_id,
                node_id,
            }))
            .await?;
        self.client.resync_session(session_id).await?;

        let new_buffer = self.focused_buffer_for_session(session_id);
        if new_buffer != previous_buffer {
            let sent_focus_in = if let Some(buffer_id) = new_buffer {
                self.maybe_send_focus_sequence(buffer_id, true).await?
            } else {
                false
            };
            if sent_focus_in && let Some(buffer_id) = new_buffer {
                self.client.refresh_buffer_snapshot(buffer_id).await?;
            }
            if sent_focus_out && let Some(buffer_id) = previous_buffer {
                self.client.refresh_buffer_snapshot(buffer_id).await?;
            }
        }
        Ok(())
    }

    fn focused_buffer_for_session(&self, session_id: SessionId) -> Option<BufferId> {
        self.client
            .state()
            .sessions
            .get(&session_id)
            .and_then(|session| session.focused_leaf_id)
            .and_then(|node_id| self.node_buffer_id(node_id))
    }

    fn node_buffer_id(&self, node_id: NodeId) -> Option<BufferId> {
        self.client
            .state()
            .nodes
            .get(&node_id)
            .and_then(|node| node.buffer_view.as_ref())
            .map(|buffer_view| buffer_view.buffer_id)
    }

    async fn maybe_send_focus_sequence(
        &mut self,
        buffer_id: BufferId,
        focused: bool,
    ) -> Result<bool> {
        if !self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .is_some_and(|snapshot| snapshot.focus_reporting)
        {
            return Ok(false);
        }
        let bytes = if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        };
        self.write_input(buffer_id, bytes).await
    }

    /// Write `bytes` to `buffer_id`'s program and report whether it took
    /// them. Like tmux, input for a pane that cannot take it (its program has
    /// exited under `remain-on-exit`, or the buffer is gone) is dropped: the
    /// server's refusal is only logged, so keys, pastes, mouse and focus
    /// reports aimed at a dead pane never end the client. A transport failure
    /// is still returned.
    async fn write_input(&self, buffer_id: BufferId, bytes: Vec<u8>) -> Result<bool> {
        match self
            .client
            .request_message(ClientMessage::Input(InputRequest::Send {
                request_id: self.client.next_request_id(),
                buffer_id,
                bytes,
            }))
            .await
        {
            Ok(_) => Ok(true),
            Err(MuxError::Wire(error)) => {
                debug!("dropped input for buffer {buffer_id}: {error}");
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn record_notification(&mut self, message: impl Into<String>) {
        let message = message.into();
        warn!("{message}");
        self.notifications.push(message);
        self.notification_until = Some(Instant::now() + DEFAULT_DISPLAY_TIME);
        enforce_notification_cap(&mut self.notifications);
    }
}

fn action_is_local_terminal_action(action: &Action) -> bool {
    match action {
        Action::Chain(actions) => actions.iter().all(action_is_local_terminal_action),
        Action::EnterMode { mode } | Action::ToggleMode { mode } => {
            mode == SEARCH_MODE || mode == SELECT_MODE
        }
        Action::ScrollLineUp
        | Action::ScrollLineDown
        | Action::ScrollPageUp
        | Action::ScrollPageDown
        | Action::ScrollToTop
        | Action::ScrollToBottom
        | Action::FollowOutput
        | Action::EnterSearchMode
        | Action::EnterSearchBackward
        | Action::SearchNext
        | Action::SearchPrev
        | Action::CommitSearch
        | Action::CancelSearch
        | Action::CopyMotion { .. }
        | Action::CopyJump { .. }
        | Action::CopyJumpAgain
        | Action::CopyJumpReverse
        | Action::EnterSelect { .. }
        | Action::SelectMove { .. }
        | Action::SelectOtherEnd
        | Action::CopySelection
        | Action::CancelSelection => true,
        _ => false,
    }
}

/// The floating window a close or kill action targets: the given one, or the
/// focused one.
fn focused_floating_target(
    floating_id: Option<FloatingId>,
    presentation: &PresentationModel,
) -> Result<FloatingId> {
    floating_id
        .or_else(|| presentation.focused_floating_id())
        .ok_or_else(|| MuxError::invalid_input("no floating window is focused"))
}

/// The view a close or kill action targets: the given node, or the focused
/// leaf.
fn focused_view_target(
    node_id: Option<NodeId>,
    presentation: &PresentationModel,
) -> Result<NodeId> {
    node_id
        .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
        .ok_or_else(|| MuxError::invalid_input("no focused node to close"))
}

fn action_requires_local_context(action: &Action) -> bool {
    match action {
        Action::Chain(actions) => actions.iter().all(action_requires_local_context),
        Action::EnterSearchMode
        | Action::EnterSearchBackward
        | Action::SearchNext
        | Action::SearchPrev
        | Action::EnterSelect { .. } => true,
        Action::EnterMode { mode } | Action::ToggleMode { mode } => {
            mode == SEARCH_MODE || mode == SELECT_MODE
        }
        _ => false,
    }
}

fn compute_search_matches(lines: &[String], query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let mut search_from = 0;
        while let Some(found) = line[search_from..].find(query) {
            let byte_start = search_from + found;
            let byte_end = byte_start + query.len();
            let start_column = display_width(&line[..byte_start]);
            let end_column =
                start_column.saturating_add(display_width(&line[byte_start..byte_end]));
            matches.push(SearchMatch {
                line: u64::try_from(line_index).unwrap_or(u64::MAX),
                start_column,
                end_column,
            });
            search_from = byte_end.max(search_from + 1);
        }
    }
    matches
}

#[doc(hidden)]
pub fn benchmark_search_matches(lines: &[String], query: &str) -> Vec<SearchMatch> {
    compute_search_matches(lines, query)
}

#[doc(hidden)]
pub fn benchmark_serialize_selection(lines: &[String], selection: &SelectionState) -> String {
    serialize_selection(lines, selection)
}

fn serialize_selection(lines: &[String], selection: &SelectionState) -> String {
    match selection.kind {
        SelectionKind::Line => serialize_line_selection(lines, selection),
        SelectionKind::Block => serialize_block_selection(lines, selection),
        SelectionKind::Character => serialize_character_selection(lines, selection),
    }
}

fn serialize_character_selection(lines: &[String], selection: &SelectionState) -> String {
    let (start, end) = ordered_points(selection.anchor, selection.cursor);
    let mut parts = Vec::new();
    for line_index in start.line..=end.line {
        let line = lines
            .get(usize::try_from(line_index).unwrap_or(usize::MAX))
            .map(String::as_str)
            .unwrap_or("");
        let fragment = if start.line == end.line {
            slice_display_range(line, start.column, end.column.saturating_add(1))
        } else if line_index == start.line {
            slice_display_range(line, start.column, u16::MAX)
        } else if line_index == end.line {
            slice_display_range(line, 0, end.column.saturating_add(1))
        } else {
            line.to_owned()
        };
        parts.push(fragment);
    }
    parts.join("\n")
}

fn serialize_line_selection(lines: &[String], selection: &SelectionState) -> String {
    let start_line = selection.anchor.line.min(selection.cursor.line);
    let end_line = selection.anchor.line.max(selection.cursor.line);
    (start_line..=end_line)
        .map(|line_index| {
            lines
                .get(usize::try_from(line_index).unwrap_or(usize::MAX))
                .cloned()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn serialize_block_selection(lines: &[String], selection: &SelectionState) -> String {
    let start_line = selection.anchor.line.min(selection.cursor.line);
    let end_line = selection.anchor.line.max(selection.cursor.line);
    let start_column = selection.anchor.column.min(selection.cursor.column);
    let end_column = selection
        .anchor
        .column
        .max(selection.cursor.column)
        .saturating_add(1);
    (start_line..=end_line)
        .map(|line_index| {
            let line = lines
                .get(usize::try_from(line_index).unwrap_or(usize::MAX))
                .map(String::as_str)
                .unwrap_or("");
            slice_display_range(line, start_column, end_column)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ordered_points(left: SelectionPoint, right: SelectionPoint) -> (SelectionPoint, SelectionPoint) {
    if (left.line, left.column) <= (right.line, right.column) {
        (left, right)
    } else {
        (right, left)
    }
}

fn slice_display_range(line: &str, start_column: u16, end_column: u16) -> String {
    if start_column >= end_column {
        return String::new();
    }

    let mut column = 0_u16;
    let mut output = String::new();
    for grapheme in UnicodeSegmentation::graphemes(line, true) {
        let width = display_width(grapheme).max(1);
        let next_column = column.saturating_add(width);
        if next_column > start_column && column < end_column {
            output.push_str(grapheme);
        }
        if column >= end_column {
            break;
        }
        column = next_column;
    }
    output
}

fn encode_mouse_event(leaf: &LeafFrame, event: MouseEvent) -> Result<Vec<u8>> {
    let origin_x = clamp_origin(leaf.content_rect.origin.x);
    let origin_y = clamp_origin(leaf.content_rect.origin.y);
    let local_column = event
        .column
        .checked_sub(origin_x)
        .ok_or_else(|| MuxError::invalid_input("mouse event fell outside pane bounds"))?;
    let local_row = event
        .row
        .checked_sub(origin_y)
        .ok_or_else(|| MuxError::invalid_input("mouse event fell outside pane content"))?;

    let mut code = 0_u16;
    if event.modifiers.shift {
        code |= 0b00100;
    }
    if event.modifiers.alt {
        code |= 0b01000;
    }
    if event.modifiers.ctrl {
        code |= 0b10000;
    }

    let suffix = match event.kind {
        MouseEventKind::Press(button) => {
            code |= mouse_button_code(button);
            'M'
        }
        MouseEventKind::Release(button) => {
            code |= button.map(mouse_button_code).unwrap_or(0b11);
            'm'
        }
        MouseEventKind::Drag(button) => {
            code |= 0b100000 | mouse_button_code(button);
            'M'
        }
        MouseEventKind::WheelUp => {
            code |= 0b1_000000;
            'M'
        }
        MouseEventKind::WheelDown => {
            code |= 0b1_000001;
            'M'
        }
    };

    Ok(format!(
        "\x1b[<{code};{};{}{suffix}",
        local_column.saturating_add(1),
        local_row.saturating_add(1)
    )
    .into_bytes())
}

fn mouse_button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn clamp_origin(value: i32) -> u16 {
    u16::try_from(value.max(0)).unwrap_or(u16::MAX)
}

fn display_width(text: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
}

fn prepend_actions(pending: &mut VecDeque<Action>, actions: Vec<Action>) {
    for action in actions.into_iter().rev() {
        pending.push_front(action);
    }
}

fn prepend_actions_with_limit(
    pending: &mut VecDeque<Action>,
    actions: Vec<Action>,
    expansions: &mut usize,
) -> Result<()> {
    *expansions = expansions.saturating_add(actions.len());
    if *expansions > MAX_EXPANDED_ACTIONS {
        return Err(MuxError::invalid_input("action expansion limit reached"));
    }
    prepend_actions(pending, actions);
    Ok(())
}

/// A `display-message` time in milliseconds, 0 meaning the default.
fn display_duration(duration_ms: u32) -> Duration {
    if duration_ms == 0 {
        DEFAULT_DISPLAY_TIME
    } else {
        Duration::from_millis(u64::from(duration_ms))
    }
}

fn format_notification(level: NotifyLevel, message: &str) -> String {
    match level {
        NotifyLevel::Info => message.to_owned(),
        NotifyLevel::Warn => format!("warn: {message}"),
        NotifyLevel::Error => format!("error: {message}"),
    }
}

fn push_background_notification(sink: &Arc<Mutex<Vec<String>>>, message: String) {
    let mut pending = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.push(message);
    enforce_notification_cap(&mut pending);
}

/// Retain only the newest [`MAX_NOTIFICATIONS`] messages.
fn enforce_notification_cap(notifications: &mut Vec<String>) {
    if notifications.len() > MAX_NOTIFICATIONS {
        let overflow = notifications.len() - MAX_NOTIFICATIONS;
        notifications.drain(0..overflow);
    }
}

const MAX_NOTIFICATIONS: usize = 64;

fn resolve_floating_geometry(spec: FloatingGeometrySpec, viewport: Size) -> FloatGeometry {
    let width = resolve_floating_size(spec.width, viewport.width);
    let height = resolve_floating_size(spec.height, viewport.height);
    let max_x = viewport.width.saturating_sub(width);
    let max_y = viewport.height.saturating_sub(height);

    let (base_x, base_y) = match spec.anchor {
        FloatingAnchor::Center => (max_x / 2, max_y / 2),
        FloatingAnchor::TopLeft => (0, 0),
        FloatingAnchor::TopRight => (max_x, 0),
        FloatingAnchor::BottomLeft => (0, max_y),
        FloatingAnchor::BottomRight => (max_x, max_y),
    };

    let x = (i32::from(base_x) + i32::from(spec.offset_x)).clamp(0, i32::from(max_x));
    let y = (i32::from(base_y) + i32::from(spec.offset_y)).clamp(0, i32::from(max_y));

    FloatGeometry::new(x as u16, y as u16, width, height)
}

fn resolve_floating_size(size: FloatingSize, max: u16) -> u16 {
    match size {
        FloatingSize::Cells(cells) => cells.min(max.max(1)),
        FloatingSize::Percent(percent) => {
            let max = u32::from(max.max(1));
            let percent = u32::from(percent.clamp(1, 100));
            ((max * percent) / 100).max(1) as u16
        }
    }
}

fn default_shell_command() -> Vec<String> {
    vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())]
}

fn key_event_to_token(key: KeyEvent) -> Result<KeyToken> {
    match key {
        // A space keypress must match the `<Space>` binding token, which the
        // grammar produces for a literal space.
        KeyEvent::Char(' ') => Ok(KeyToken::Space),
        KeyEvent::Char(ch) => Ok(KeyToken::Char(ch)),
        KeyEvent::Enter => Ok(KeyToken::Enter),
        KeyEvent::Tab => Ok(KeyToken::Tab),
        KeyEvent::Backspace => Ok(KeyToken::Backspace),
        KeyEvent::Escape => Ok(KeyToken::Escape),
        KeyEvent::Ctrl(ch) => Ok(KeyToken::Ctrl(ch.to_ascii_lowercase())),
        // A legacy host sends Alt+Shift+h as ESC H; keep the shift so it
        // matches `<M-H>` and is not taken for `<M-h>`.
        KeyEvent::Alt(ch) if ch.is_ascii_uppercase() => Ok(KeyToken::Key {
            code: crate::input::KeyCode::Char(ch.to_ascii_lowercase()),
            mods: crate::input::Modifiers {
                alt: true,
                shift: true,
                ..crate::input::Modifiers::NONE
            },
        }),
        KeyEvent::Alt(ch) => Ok(KeyToken::Alt(ch)),
        KeyEvent::Up => Ok(KeyToken::Up),
        KeyEvent::Down => Ok(KeyToken::Down),
        KeyEvent::Left => Ok(KeyToken::Left),
        KeyEvent::Right => Ok(KeyToken::Right),
        KeyEvent::Home => Ok(KeyToken::Home),
        KeyEvent::End => Ok(KeyToken::End),
        KeyEvent::Insert => Ok(KeyToken::Insert),
        KeyEvent::Delete => Ok(KeyToken::Delete),
        KeyEvent::PageUp => Ok(KeyToken::PageUp),
        KeyEvent::PageDown => Ok(KeyToken::PageDown),
        KeyEvent::Key { code, mods } => Ok(normalize_key_token(code, mods)),
        KeyEvent::Bytes(_) => Err(MuxError::invalid_input("raw bytes are handled separately")),
    }
}

/// Normalise an unmodified CSI-u report for an ordinary text key (the kitty
/// encoding of typed text) to its legacy `KeyEvent`, so text-consuming modes like
/// search treat both encodings identically. Modified keys and non-text keys are
/// returned unchanged.
fn normalize_text_key(key: KeyEvent) -> KeyEvent {
    use crate::input::KeyCode;
    match key {
        KeyEvent::Key { code, mods } if mods.is_empty() => match code {
            KeyCode::Char(ch) => KeyEvent::Char(ch),
            KeyCode::Tab => KeyEvent::Tab,
            KeyCode::Enter => KeyEvent::Enter,
            KeyCode::Backspace => KeyEvent::Backspace,
            KeyCode::Escape => KeyEvent::Escape,
            other => KeyEvent::Key { code: other, mods },
        },
        other => other,
    }
}

/// Collapse a single-modifier character key to the legacy `Ctrl`/`Alt` token so
/// a CSI-u report (e.g. `\x1b[104;5u` for Ctrl+H) matches the same `<C-h>`
/// binding a legacy control byte would, keeping the two key representations
/// coherent. Everything else stays a `Key` token.
fn normalize_key_token(code: crate::input::KeyCode, mods: crate::input::Modifiers) -> KeyToken {
    use crate::input::{KeyCode, Modifiers};
    if let KeyCode::Char(ch) = code {
        if mods
            == (Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            })
        {
            return KeyToken::Ctrl(ch.to_ascii_lowercase());
        }
        if mods
            == (Modifiers {
                alt: true,
                ..Modifiers::NONE
            })
        {
            return KeyToken::Alt(ch.to_ascii_lowercase());
        }
    }
    // Binding tokens store character keys lowercased (shift is a modifier bit),
    // so lowercase here too for hosts that report the shifted code point.
    let code = match code {
        KeyCode::Char(ch) => KeyCode::Char(ch.to_ascii_lowercase()),
        other => other,
    };
    KeyToken::Key { code, mods }
}

fn sequence_to_bytes(sequence: &[KeyToken], mode: u8) -> Result<Vec<u8>> {
    use crate::input::{KeyCode, Modifiers, encode_key};

    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };

    let mut bytes = Vec::new();
    for token in sequence {
        // Route every supported token through the mode-aware encoder. In legacy
        // or disambiguate-only mode it still emits the plain legacy bytes for
        // unmodified keys; under report-all-keys every key (ordinary characters
        // and special keys like Tab) becomes a CSI-u escape sequence.
        let (code, mods) = match token {
            KeyToken::Char(ch) => (KeyCode::Char(*ch), Modifiers::NONE),
            KeyToken::Space => (KeyCode::Char(' '), Modifiers::NONE),
            KeyToken::Tab => (KeyCode::Tab, Modifiers::NONE),
            KeyToken::Enter => (KeyCode::Enter, Modifiers::NONE),
            KeyToken::Backspace => (KeyCode::Backspace, Modifiers::NONE),
            KeyToken::Escape => (KeyCode::Escape, Modifiers::NONE),
            KeyToken::Ctrl(ch) => (KeyCode::Char(*ch), ctrl),
            KeyToken::Alt(ch) => (KeyCode::Char(*ch), alt),
            KeyToken::Up => (KeyCode::Up, Modifiers::NONE),
            KeyToken::Down => (KeyCode::Down, Modifiers::NONE),
            KeyToken::Left => (KeyCode::Left, Modifiers::NONE),
            KeyToken::Right => (KeyCode::Right, Modifiers::NONE),
            KeyToken::Home => (KeyCode::Home, Modifiers::NONE),
            KeyToken::End => (KeyCode::End, Modifiers::NONE),
            KeyToken::Insert => (KeyCode::Insert, Modifiers::NONE),
            KeyToken::Delete => (KeyCode::Delete, Modifiers::NONE),
            KeyToken::PageUp => (KeyCode::PageUp, Modifiers::NONE),
            KeyToken::PageDown => (KeyCode::PageDown, Modifiers::NONE),
            KeyToken::Key { code, mods } => (*code, *mods),
            KeyToken::Leader => {
                return Err(MuxError::invalid_input(
                    "leader placeholders cannot be sent directly",
                ));
            }
        };
        bytes.extend(encode_key(code, mods, mode));
    }
    Ok(bytes)
}

async fn rollback_created_buffer_on_error<T, U>(
    configured: &mut ConfiguredClient<U>,
    buffer_id: Option<BufferId>,
    operation: &str,
    result: Result<T>,
) -> Result<T>
where
    U: Transport,
{
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(buffer_id) = buffer_id {
                configured
                    .rollback_created_buffer(buffer_id, operation)
                    .await;
            }
            Err(error)
        }
    }
}

/// Drops each `RenderInvalidated` that a later one for the same buffer in
/// `events` supersedes, keeping the order of everything else.
fn coalesce_render_invalidations(events: Vec<ServerEvent>) -> Vec<ServerEvent> {
    let mut seen = BTreeSet::new();
    let mut kept = events
        .into_iter()
        .rev()
        .filter(|event| match event {
            ServerEvent::RenderInvalidated(render) => seen.insert(render.buffer_id),
            _ => true,
        })
        .collect::<Vec<_>>();
    kept.reverse();
    kept
}

fn event_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::SessionCreated(_) => "session_created",
        ServerEvent::SessionClosed(_) => "session_closed",
        ServerEvent::SessionRenamed(_) => "session_renamed",
        ServerEvent::BufferCreated(_) => "buffer_created",
        ServerEvent::BufferPipeChanged(_) => "buffer_pipe_changed",
        ServerEvent::BufferDetached(_) => "buffer_detached",
        ServerEvent::NodeChanged(_) => "node_changed",
        ServerEvent::FloatingChanged(_) => "floating_changed",
        ServerEvent::FocusChanged(_) => "focus_changed",
        ServerEvent::RenderInvalidated(_) => "render_invalidated",
        ServerEvent::ClientChanged(_) => "client_changed",
        ServerEvent::Clipboard(_) => "clipboard",
        ServerEvent::DisplayMessage(_) => "display_message",
        ServerEvent::Passthrough(_) => "passthrough",
        ServerEvent::ServerShutdown => "server_shutdown",
    }
}

fn event_info(
    name: &str,
    event: &ServerEvent,
    fallback_session_id: Option<SessionId>,
) -> EventInfo {
    let mut info = base_event_info(name);
    match event {
        ServerEvent::SessionCreated(event) => info.session_id = Some(event.session.id),
        ServerEvent::SessionClosed(event) => info.session_id = Some(event.session_id),
        ServerEvent::SessionRenamed(event) => info.session_id = Some(event.session_id),
        ServerEvent::BufferCreated(event) => {
            info.buffer_id = Some(event.buffer.id);
            info.node_id = event.buffer.attachment_node_id;
        }
        ServerEvent::BufferPipeChanged(event) => {
            info.session_id = event.session_id;
            info.buffer_id = Some(event.buffer.id);
            info.node_id = event.buffer.attachment_node_id;
        }
        ServerEvent::BufferDetached(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::NodeChanged(event) => info.session_id = Some(event.session_id),
        ServerEvent::FloatingChanged(event) => {
            info.session_id = Some(event.session_id);
            info.floating_id = event.floating_id;
        }
        ServerEvent::FocusChanged(event) => {
            info.session_id = Some(event.session_id);
            info.node_id = event.focused_leaf_id;
            info.floating_id = event.focused_floating_id;
        }
        ServerEvent::RenderInvalidated(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::Clipboard(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::Passthrough(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::DisplayMessage(_) | ServerEvent::ServerShutdown => {}
        ServerEvent::ClientChanged(event) => {
            info.session_id = event.client.current_session_id;
            info.previous_session_id = event.previous_session_id;
            info.client_id = Some(event.client.id);
        }
    }
    if info.session_id.is_none() {
        info.session_id = fallback_session_id;
    }
    info
}

fn base_event_info(name: &str) -> EventInfo {
    EventInfo {
        name: name.to_owned(),
        session_id: None,
        previous_session_id: None,
        client_id: None,
        buffer_id: None,
        node_id: None,
        floating_id: None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use embers_core::{ActivityState, BufferId, FloatingId, NodeId, PtySize, RequestId, SessionId};
    use embers_protocol::{
        BufferLocation, BufferLocationAttachment, BufferRecord, BufferRecordKind,
        BufferRecordState, BufferWithLocationResponse, FloatingChangedEvent, ServerEvent,
        ServerResponse,
    };
    use tempfile::tempdir;

    use super::{ConfiguredClient, SearchPrompt, event_info, key_event_to_token};
    use crate::client::MuxClient;
    use crate::config::{ConfigDiscoveryOptions, ConfigManager};
    use crate::controller::KeyEvent;
    use crate::input::{KeyToken, NORMAL_MODE};
    use crate::testing::FakeTransport;

    #[test]
    fn report_all_mode_routes_plain_keys_through_csi_u() {
        use super::sequence_to_bytes;

        // Compact keyboard-mode bitfield: bit 0 = disambiguate, bit 3 = report-all.
        const DISAMBIGUATE_ONLY: u8 = 0b0000_0001;
        const REPORT_ALL: u8 = 0b0000_1000;

        // Disambiguate-only mode keeps unmodified plain keys as their legacy bytes.
        assert_eq!(
            sequence_to_bytes(&[KeyToken::Char('a')], DISAMBIGUATE_ONLY).unwrap(),
            b"a".to_vec()
        );
        assert_eq!(
            sequence_to_bytes(&[KeyToken::Tab], DISAMBIGUATE_ONLY).unwrap(),
            b"\t".to_vec()
        );

        // Under report-all-keys, even ordinary characters and Tab become CSI-u.
        assert_eq!(
            sequence_to_bytes(&[KeyToken::Char('a')], REPORT_ALL).unwrap(),
            b"\x1b[97u".to_vec()
        );
        assert_eq!(
            sequence_to_bytes(&[KeyToken::Tab], REPORT_ALL).unwrap(),
            b"\x1b[9u".to_vec()
        );
    }

    #[test]
    fn space_keypress_maps_to_the_space_binding_token() {
        // A `<Space>` binding token is produced by the grammar for a literal
        // space, so a space keypress must resolve to it (not Char(' ')).
        assert_eq!(
            key_event_to_token(KeyEvent::Char(' ')).unwrap(),
            KeyToken::Space
        );
        assert_eq!(
            key_event_to_token(KeyEvent::Char('a')).unwrap(),
            KeyToken::Char('a')
        );
    }

    #[test]
    fn csi_u_single_modifier_char_normalizes_to_legacy_token() {
        use crate::input::{KeyCode, Modifiers};
        // A CSI-u Ctrl+H report must resolve to the same token as legacy `<C-h>`
        // so a single binding matches both encodings.
        assert_eq!(
            key_event_to_token(KeyEvent::Key {
                code: KeyCode::Char('h'),
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
            })
            .unwrap(),
            KeyToken::Ctrl('h')
        );
        // Multi-modifier / special keys stay as Key tokens.
        assert_eq!(
            key_event_to_token(KeyEvent::Key {
                code: KeyCode::Tab,
                mods: Modifiers {
                    shift: true,
                    ctrl: true,
                    ..Modifiers::NONE
                },
            })
            .unwrap(),
            KeyToken::Key {
                code: KeyCode::Tab,
                mods: Modifiers {
                    shift: true,
                    ctrl: true,
                    ..Modifiers::NONE
                },
            }
        );
    }

    #[test]
    fn legacy_alt_shift_letters_keep_their_shift() {
        use super::sequence_to_bytes;
        use crate::input::{KeyCode, Modifiers};
        let alt_shift_h = KeyToken::Key {
            code: KeyCode::Char('h'),
            mods: Modifiers {
                alt: true,
                shift: true,
                ..Modifiers::NONE
            },
        };
        // ESC H from a legacy host is Alt+Shift+h, matching `<M-H>`, not `<M-h>`.
        assert_eq!(key_event_to_token(KeyEvent::Alt('H')).unwrap(), alt_shift_h);
        assert_eq!(
            key_event_to_token(KeyEvent::Alt('h')).unwrap(),
            KeyToken::Alt('h')
        );
        // The CSI-u report of the same chord resolves to the same token.
        assert_eq!(
            key_event_to_token(KeyEvent::Key {
                code: KeyCode::Char('h'),
                mods: Modifiers {
                    alt: true,
                    shift: true,
                    ..Modifiers::NONE
                },
            })
            .unwrap(),
            alt_shift_h
        );
        // Unbound, it reaches the pane as ESC H again.
        assert_eq!(sequence_to_bytes(&[alt_shift_h], 0).unwrap(), b"\x1bH");
    }

    #[test]
    fn normalize_text_key_maps_unmodified_csi_u_to_legacy() {
        use super::normalize_text_key;
        use crate::input::{KeyCode, Modifiers};

        // Unmodified CSI-u text keys collapse to their legacy form so search
        // (and other text-consuming modes) treat both encodings identically.
        assert_eq!(
            normalize_text_key(KeyEvent::Key {
                code: KeyCode::Char('a'),
                mods: Modifiers::NONE,
            }),
            KeyEvent::Char('a')
        );
        assert_eq!(
            normalize_text_key(KeyEvent::Key {
                code: KeyCode::Enter,
                mods: Modifiers::NONE,
            }),
            KeyEvent::Enter
        );
        // A modified key stays a Key (not text input)...
        let ctrl_a = KeyEvent::Key {
            code: KeyCode::Char('a'),
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        };
        assert_eq!(normalize_text_key(ctrl_a.clone()), ctrl_a);
        // ...as does a non-text key like PageDown.
        let page_down = KeyEvent::Key {
            code: KeyCode::PageDown,
            mods: Modifiers::NONE,
        };
        assert_eq!(normalize_text_key(page_down.clone()), page_down);
    }

    #[test]
    fn report_all_csi_u_keys_tokenize_as_legacy() {
        use super::{key_event_to_token, normalize_text_key};
        use crate::input::{KeyCode, Modifiers};

        // Under report-all-keys, ordinary chars, Space, and Tab arrive as CSI-u
        // Key events; after the up-front normalization in handle_key they tokenize
        // like their legacy forms, so `a`, `<Space>`, and `<Tab>` bindings (and
        // hints labels, which match on the same Char events) still resolve.
        let token = |code| {
            key_event_to_token(normalize_text_key(KeyEvent::Key {
                code,
                mods: Modifiers::NONE,
            }))
            .unwrap()
        };
        assert_eq!(token(KeyCode::Char('a')), KeyToken::Char('a'));
        assert_eq!(token(KeyCode::Char(' ')), KeyToken::Space);
        assert_eq!(token(KeyCode::Tab), KeyToken::Tab);

        // A modified CSI-u key is not text input; it keeps its modifier token.
        let ctrl_a = KeyEvent::Key {
            code: KeyCode::Char('a'),
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        };
        assert_eq!(
            key_event_to_token(normalize_text_key(ctrl_a)).unwrap(),
            KeyToken::Ctrl('a')
        );
    }

    #[test]
    fn shifted_char_reports_tokenize_lowercase() {
        use super::key_event_to_token;
        use crate::input::{KeyCode, Modifiers};

        // A host reporting the shifted code point (Ctrl+Shift+T as 'T') must
        // produce the same token as the kitty-style lowercase report, which is
        // also how the binding grammar stores <C-S-T>.
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            key_event_to_token(KeyEvent::Key {
                code: KeyCode::Char('T'),
                mods: ctrl_shift,
            })
            .unwrap(),
            KeyToken::Key {
                code: KeyCode::Char('t'),
                mods: ctrl_shift,
            }
        );
        // `<C-T>` names the same key.
        assert_eq!(
            crate::input::parse_key_sequence("<C-T>").unwrap(),
            vec![
                key_event_to_token(KeyEvent::Key {
                    code: KeyCode::Char('t'),
                    mods: ctrl_shift,
                })
                .unwrap()
            ]
        );
    }

    #[test]
    fn attached_buffer_location_accepts_session_and_floating_locations() {
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                Some(BufferId(7)),
                BufferLocation::session(BufferId(7), SessionId(2), NodeId(5)),
                "buffer focus",
            )
            .expect("session attachment should validate"),
            (SessionId(2), NodeId(5))
        );
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                Some(BufferId(7)),
                BufferLocation::floating(BufferId(7), SessionId(2), NodeId(5), FloatingId(9)),
                "buffer reveal",
            )
            .expect("floating attachment should validate"),
            (SessionId(2), NodeId(5))
        );
    }

    #[test]
    fn attached_buffer_location_accepts_history_helper_locations_without_source_id_match() {
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                None,
                BufferLocation::session(BufferId(8), SessionId(2), NodeId(5)),
                "buffer history",
            )
            .expect("history helper attachment should validate"),
            (SessionId(2), NodeId(5))
        );
    }

    #[test]
    fn attached_buffer_location_rejects_mismatched_or_detached_locations() {
        let mismatch = ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
            Some(BufferId(7)),
            BufferLocation::session(BufferId(8), SessionId(2), NodeId(5)),
            "buffer focus",
        )
        .expect_err("mismatched buffer ids should fail");
        let detached = ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
            Some(BufferId(7)),
            BufferLocation {
                buffer_id: BufferId(7),
                attachment: BufferLocationAttachment::Detached,
            },
            "buffer reveal",
        )
        .expect_err("detached locations should fail");

        assert!(
            mismatch
                .to_string()
                .contains("returned location for buffer 8")
        );
        assert!(detached.to_string().contains("detached location"));
    }

    #[test]
    fn buffer_location_from_response_accepts_buffer_with_location() {
        let location = BufferLocation::session(BufferId(8), SessionId(2), NodeId(5));
        let response = ServerResponse::BufferWithLocation(
            BufferWithLocationResponse::new(
                RequestId(1),
                BufferRecord {
                    id: BufferId(8),
                    title: "helper".to_owned(),
                    command: Vec::new(),
                    cwd: None,
                    kind: BufferRecordKind::Helper,
                    state: BufferRecordState::Created,
                    pid: None,
                    attachment_node_id: Some(NodeId(5)),
                    read_only: true,
                    helper_source_buffer_id: Some(BufferId(7)),
                    helper_scope: Some(embers_protocol::BufferHistoryScope::Visible),
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    pipe: None,
                    env: Default::default(),
                    user_options: Default::default(),
                    foreground_command: None,
                },
                location,
                false,
            )
            .expect("buffer and location ids should match"),
        );

        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::buffer_location_from_response(
                response,
                "buffer history",
            )
            .expect("buffer-with-location response should validate"),
            location
        );
    }

    #[test]
    fn floating_changed_event_info_includes_floating_id() {
        let info = event_info(
            "floating_changed",
            &ServerEvent::FloatingChanged(FloatingChangedEvent {
                session_id: SessionId(2),
                floating_id: Some(FloatingId(9)),
            }),
            None,
        );

        assert_eq!(info.session_id, Some(SessionId(2)));
        assert_eq!(info.floating_id, Some(FloatingId(9)));
    }

    #[test]
    fn finish_config_reload_clears_search_prompt_when_mode_falls_back_to_normal() {
        let tempdir = tempdir().expect("create tempdir");
        let config_path = tempdir.path().join("config.rhai");
        fs::write(&config_path, "define_mode(\"custom\");").expect("write custom config");
        let config = ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
        )
        .expect("load config");
        let client = MuxClient::new(FakeTransport::default());
        let mut configured = ConfiguredClient::new(client, config);
        configured.input_state.set_mode("custom".to_owned());
        configured.search_prompt = Some(SearchPrompt {
            node_id: NodeId(7),
            query: "stale".to_owned(),
            backward: false,
            return_mode: NORMAL_MODE.to_owned(),
        });

        fs::write(&config_path, "").expect("rewrite config without custom mode");

        configured.reload_config().expect("reload config");

        assert_eq!(configured.input_state.current_mode(), NORMAL_MODE);
        assert_eq!(configured.search_prompt, None);
    }
}
