use std::collections::BTreeMap;
use std::num::NonZeroU64;

use embers_core::{
    ActivityState, BufferId, ClipboardSelection, CursorState, FloatGeometry, FloatingId, NodeId,
    PassthroughWrite, PtySize, RequestId, SessionId, Size, SnapshotLine, SplitDirection, WireError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub request_id: RequestId,
    pub payload: String,
}

/// Stop the server and end every buffer's program and runtime keeper, unlike
/// a clean shutdown, which leaves keepers running for the next server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillServerRequest {
    pub request_id: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingResponse {
    pub request_id: RequestId,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    Create {
        request_id: RequestId,
        name: String,
    },
    List {
        request_id: RequestId,
    },
    Get {
        request_id: RequestId,
        session_id: SessionId,
    },
    Close {
        request_id: RequestId,
        session_id: SessionId,
        force: bool,
    },
    Rename {
        request_id: RequestId,
        session_id: SessionId,
        name: String,
    },
    AddRootTab {
        request_id: RequestId,
        session_id: SessionId,
        title: String,
        buffer_id: Option<BufferId>,
        child_node_id: Option<NodeId>,
    },
    SelectRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: u32,
    },
    RenameRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: u32,
        title: String,
    },
    CloseRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: u32,
    },
    /// Turn tmux-style `synchronize-panes` on, off, or flip it for a window
    /// (a root tab).
    SetRootTabSynchronizePanes {
        request_id: RequestId,
        session_id: SessionId,
        index: u32,
        mode: SynchronizePanesMode,
    },
    /// Records the area an attached client draws the session in, and whether
    /// it gives each pane a title row, so panes created without a client can
    /// still be spawned at their drawn size.
    SetViewport {
        request_id: RequestId,
        session_id: SessionId,
        viewport: Size,
        pane_titles: bool,
    },
    /// Records the window base index (tmux `base-index`) of the client that
    /// shows the session, so the CLI numbers windows the way that client
    /// does. `None` means the client sets none.
    SetBaseIndex {
        request_id: RequestId,
        session_id: SessionId,
        base_index: Option<u32>,
    },
    /// tmux `set-environment`: set `name` to `value`, or unset it when `value`
    /// is `None`, in a session's environment, or in the global environment when
    /// `session_id` is `None`. New buffers created for a session get the global
    /// environment, then the session's, then their own `env`.
    SetEnvironment {
        request_id: RequestId,
        session_id: Option<SessionId>,
        name: String,
        value: Option<String>,
    },
    /// tmux `show-environment`: a session's environment, or the global one when
    /// `session_id` is `None`. Answered with `ServerResponse::Environment`.
    ShowEnvironment {
        request_id: RequestId,
        session_id: Option<SessionId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynchronizePanesMode {
    Toggle,
    On,
    Off,
}

impl SessionRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::List { request_id }
            | Self::Get { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::Rename { request_id, .. }
            | Self::AddRootTab { request_id, .. }
            | Self::SelectRootTab { request_id, .. }
            | Self::RenameRootTab { request_id, .. }
            | Self::CloseRootTab { request_id, .. }
            | Self::SetRootTabSynchronizePanes { request_id, .. }
            | Self::SetViewport { request_id, .. }
            | Self::SetBaseIndex { request_id, .. }
            | Self::SetEnvironment { request_id, .. }
            | Self::ShowEnvironment { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferRequest {
    Create {
        request_id: RequestId,
        title: Option<String>,
        command: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        /// The PTY size to start the command at: the size the buffer will be
        /// drawn at once placed. `None` uses the server default.
        size: Option<PtySize>,
        /// The session the buffer is created for. Its environment (see
        /// `SessionRequest::SetEnvironment`) is applied under `env`.
        session_id: Option<SessionId>,
    },
    List {
        request_id: RequestId,
        session_id: Option<SessionId>,
        attached_only: bool,
        detached_only: bool,
    },
    Get {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Inspect {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Detach {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Kill {
        request_id: RequestId,
        buffer_id: BufferId,
        force: bool,
    },
    Capture {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    CaptureVisible {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    ScrollbackSlice {
        request_id: RequestId,
        buffer_id: BufferId,
        start_line: u64,
        line_count: u32,
    },
    GetLocation {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Reveal {
        request_id: RequestId,
        buffer_id: BufferId,
        client_id: Option<NonZeroU64>,
    },
    OpenHistory {
        request_id: RequestId,
        buffer_id: BufferId,
        scope: BufferHistoryScope,
        placement: BufferHistoryPlacement,
        client_id: Option<NonZeroU64>,
    },
    StartPipe {
        request_id: RequestId,
        buffer_id: BufferId,
        command: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
    },
    StopPipe {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    SetUserOption {
        request_id: RequestId,
        buffer_id: BufferId,
        key: String,
        value: Option<String>,
    },
}

impl BufferRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::List { request_id, .. }
            | Self::Get { request_id, .. }
            | Self::Inspect { request_id, .. }
            | Self::Detach { request_id, .. }
            | Self::Kill { request_id, .. }
            | Self::Capture { request_id, .. }
            | Self::CaptureVisible { request_id, .. }
            | Self::ScrollbackSlice { request_id, .. }
            | Self::GetLocation { request_id, .. }
            | Self::Reveal { request_id, .. }
            | Self::OpenHistory { request_id, .. }
            | Self::StartPipe { request_id, .. }
            | Self::StopPipe { request_id, .. }
            | Self::SetUserOption { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferHistoryScope {
    Full,
    Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferHistoryPlacement {
    Tab,
    Floating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferPipeState {
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferPipeStopReason {
    Requested,
    PipeExited,
    WriteFailed,
    BufferExited,
    RuntimeInterrupted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferPipeRecord {
    pub command: Vec<String>,
    pub state: BufferPipeState,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stop_reason: Option<BufferPipeStopReason>,
}

impl BufferPipeRecord {
    pub fn validate(&self, field: &str) -> std::result::Result<(), String> {
        if self.command.is_empty() {
            return Err(format!("{field}.command must not be empty"));
        }
        if self.command[0].is_empty() {
            return Err(format!("{field}.command first segment must not be empty"));
        }
        match self.state {
            BufferPipeState::Running => {
                if self.stop_reason.is_some() {
                    return Err(format!("{field}.stop_reason requires a stopped pipe"));
                }
                if self.exit_code.is_some() {
                    return Err(format!("{field}.exit_code requires a stopped pipe"));
                }
            }
            BufferPipeState::Stopped => {
                if self.stop_reason.is_none() {
                    return Err(format!("{field}.stop_reason required for stopped pipe"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferLocationAttachment {
    Detached,
    Session {
        session_id: SessionId,
        node_id: NodeId,
    },
    Floating {
        session_id: SessionId,
        node_id: NodeId,
        floating_id: FloatingId,
    },
}

impl BufferLocationAttachment {
    pub const fn session_id(self) -> Option<SessionId> {
        match self {
            Self::Detached => None,
            Self::Session { session_id, .. } | Self::Floating { session_id, .. } => {
                Some(session_id)
            }
        }
    }

    pub const fn node_id(self) -> Option<NodeId> {
        match self {
            Self::Detached => None,
            Self::Session { node_id, .. } | Self::Floating { node_id, .. } => Some(node_id),
        }
    }

    pub const fn floating_id(self) -> Option<FloatingId> {
        match self {
            Self::Floating { floating_id, .. } => Some(floating_id),
            Self::Detached | Self::Session { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLocation {
    pub buffer_id: BufferId,
    pub attachment: BufferLocationAttachment,
}

impl BufferLocation {
    pub const fn detached(buffer_id: BufferId) -> Self {
        Self {
            buffer_id,
            attachment: BufferLocationAttachment::Detached,
        }
    }

    pub const fn session(buffer_id: BufferId, session_id: SessionId, node_id: NodeId) -> Self {
        Self {
            buffer_id,
            attachment: BufferLocationAttachment::Session {
                session_id,
                node_id,
            },
        }
    }

    pub const fn floating(
        buffer_id: BufferId,
        session_id: SessionId,
        node_id: NodeId,
        floating_id: FloatingId,
    ) -> Self {
        Self {
            buffer_id,
            attachment: BufferLocationAttachment::Floating {
                session_id,
                node_id,
                floating_id,
            },
        }
    }

    pub const fn session_id(self) -> Option<SessionId> {
        self.attachment.session_id()
    }

    pub const fn node_id(self) -> Option<NodeId> {
        self.attachment.node_id()
    }

    pub const fn floating_id(self) -> Option<FloatingId> {
        self.attachment.floating_id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeRequest {
    GetTree {
        request_id: RequestId,
        session_id: SessionId,
    },
    Split {
        request_id: RequestId,
        leaf_node_id: NodeId,
        direction: SplitDirection,
        new_buffer_id: BufferId,
    },
    CreateSplit {
        request_id: RequestId,
        session_id: SessionId,
        direction: SplitDirection,
        child_node_ids: Vec<NodeId>,
        sizes: Vec<u16>,
    },
    CreateTabs {
        request_id: RequestId,
        session_id: SessionId,
        child_node_ids: Vec<NodeId>,
        titles: Vec<String>,
        active: u32,
    },
    ReplaceNode {
        request_id: RequestId,
        node_id: NodeId,
        child_node_id: NodeId,
    },
    WrapInSplit {
        request_id: RequestId,
        node_id: NodeId,
        child_node_id: NodeId,
        direction: SplitDirection,
        insert_before: bool,
    },
    WrapInTabs {
        request_id: RequestId,
        node_id: NodeId,
        title: String,
    },
    AddTab {
        request_id: RequestId,
        tabs_node_id: NodeId,
        title: String,
        buffer_id: Option<BufferId>,
        child_node_id: Option<NodeId>,
        index: u32,
    },
    SelectTab {
        request_id: RequestId,
        tabs_node_id: NodeId,
        index: u32,
    },
    Focus {
        request_id: RequestId,
        session_id: SessionId,
        node_id: NodeId,
    },
    /// Focus the pane focused before the current one in the session's
    /// focused window (tmux `select-pane -l`).
    FocusLast {
        request_id: RequestId,
        session_id: SessionId,
    },
    Close {
        request_id: RequestId,
        node_id: NodeId,
    },
    MoveBufferToNode {
        request_id: RequestId,
        buffer_id: BufferId,
        target_leaf_node_id: NodeId,
    },
    Resize {
        request_id: RequestId,
        node_id: NodeId,
        sizes: Vec<u16>,
    },
    Zoom {
        request_id: RequestId,
        node_id: NodeId,
    },
    Unzoom {
        request_id: RequestId,
        session_id: SessionId,
    },
    ToggleZoom {
        request_id: RequestId,
        node_id: NodeId,
    },
    SwapSiblings {
        request_id: RequestId,
        first_node_id: NodeId,
        second_node_id: NodeId,
    },
    BreakNode {
        request_id: RequestId,
        node_id: NodeId,
        destination: NodeBreakDestination,
    },
    JoinBufferAtNode {
        request_id: RequestId,
        node_id: NodeId,
        buffer_id: BufferId,
        placement: NodeJoinPlacement,
    },
    MoveNodeBefore {
        request_id: RequestId,
        node_id: NodeId,
        sibling_node_id: NodeId,
    },
    MoveNodeAfter {
        request_id: RequestId,
        node_id: NodeId,
        sibling_node_id: NodeId,
    },
}

impl NodeRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::GetTree { request_id, .. }
            | Self::Split { request_id, .. }
            | Self::CreateSplit { request_id, .. }
            | Self::CreateTabs { request_id, .. }
            | Self::ReplaceNode { request_id, .. }
            | Self::WrapInSplit { request_id, .. }
            | Self::WrapInTabs { request_id, .. }
            | Self::AddTab { request_id, .. }
            | Self::SelectTab { request_id, .. }
            | Self::Focus { request_id, .. }
            | Self::FocusLast { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::MoveBufferToNode { request_id, .. }
            | Self::Resize { request_id, .. }
            | Self::Zoom { request_id, .. }
            | Self::Unzoom { request_id, .. }
            | Self::ToggleZoom { request_id, .. }
            | Self::SwapSiblings { request_id, .. }
            | Self::BreakNode { request_id, .. }
            | Self::JoinBufferAtNode { request_id, .. }
            | Self::MoveNodeBefore { request_id, .. }
            | Self::MoveNodeAfter { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeBreakDestination {
    Tab,
    Floating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeJoinPlacement {
    Left,
    Right,
    Up,
    Down,
    TabBefore,
    TabAfter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloatingRequest {
    Create {
        request_id: RequestId,
        session_id: SessionId,
        root_node_id: Option<NodeId>,
        buffer_id: Option<BufferId>,
        geometry: FloatGeometry,
        title: Option<String>,
        focus: bool,
        close_on_empty: bool,
    },
    Close {
        request_id: RequestId,
        floating_id: FloatingId,
    },
    Move {
        request_id: RequestId,
        floating_id: FloatingId,
        geometry: FloatGeometry,
    },
    Focus {
        request_id: RequestId,
        floating_id: FloatingId,
    },
}

impl FloatingRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::Move { request_id, .. }
            | Self::Focus { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputRequest {
    Send {
        request_id: RequestId,
        buffer_id: BufferId,
        bytes: Vec<u8>,
    },
    Resize {
        request_id: RequestId,
        buffer_id: BufferId,
        cols: u16,
        rows: u16,
    },
}

impl InputRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Send { request_id, .. } | Self::Resize { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscribeRequest {
    pub request_id: RequestId,
    pub session_id: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsubscribeRequest {
    pub request_id: RequestId,
    pub subscription_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientRequest {
    List {
        request_id: RequestId,
    },
    Get {
        request_id: RequestId,
        client_id: Option<NonZeroU64>,
    },
    Detach {
        request_id: RequestId,
        client_id: Option<NonZeroU64>,
    },
    Switch {
        request_id: RequestId,
        client_id: Option<NonZeroU64>,
        session_id: SessionId,
    },
    /// tmux `display-message`: show `message` in the status line of
    /// `client_id`, or of every client whose current session is `session_id`,
    /// for `duration_ms` (0 for the client's default).
    DisplayMessage {
        request_id: RequestId,
        client_id: Option<NonZeroU64>,
        session_id: Option<SessionId>,
        message: String,
        duration_ms: u32,
    },
}

impl ClientRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::List { request_id }
            | Self::Get { request_id, .. }
            | Self::Detach { request_id, .. }
            | Self::Switch { request_id, .. }
            | Self::DisplayMessage { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    Ping(PingRequest),
    Session(SessionRequest),
    Buffer(BufferRequest),
    Node(NodeRequest),
    Floating(FloatingRequest),
    Input(InputRequest),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    Client(ClientRequest),
    KillServer(KillServerRequest),
}

impl ClientMessage {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Ping(request) => request.request_id,
            Self::KillServer(request) => request.request_id,
            Self::Session(request) => request.request_id(),
            Self::Buffer(request) => request.request_id(),
            Self::Node(request) => request.request_id(),
            Self::Floating(request) => request.request_id(),
            Self::Input(request) => request.request_id(),
            Self::Subscribe(request) => request.request_id,
            Self::Unsubscribe(request) => request.request_id,
            Self::Client(request) => request.request_id(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferRecordState {
    Created,
    Running,
    Interrupted,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionId,
    pub name: String,
    pub root_node_id: NodeId,
    pub floating_ids: Vec<FloatingId>,
    pub focused_leaf_id: Option<NodeId>,
    pub focused_floating_id: Option<FloatingId>,
    pub zoomed_node_id: Option<NodeId>,
    /// The area the session was last drawn in by an attached client; see
    /// `SessionRequest::SetViewport`.
    pub viewport: Option<Size>,
    /// The window base index of the client that last showed the session;
    /// see `SessionRequest::SetBaseIndex`.
    pub base_index: Option<u32>,
    /// Whether the client that last drew the session gave each pane a title
    /// row; see `SessionRequest::SetViewport`.
    pub pane_titles: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferRecordKind {
    Pty,
    Helper,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferRecord {
    pub id: BufferId,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub pipe: Option<BufferPipeRecord>,
    pub kind: BufferRecordKind,
    pub state: BufferRecordState,
    pub pid: Option<u32>,
    pub attachment_node_id: Option<NodeId>,
    pub read_only: bool,
    pub helper_source_buffer_id: Option<BufferId>,
    pub helper_scope: Option<BufferHistoryScope>,
    pub pty_size: PtySize,
    pub activity: ActivityState,
    pub last_snapshot_seq: u64,
    pub exit_code: Option<i32>,
    pub env: BTreeMap<String, String>,
    pub user_options: BTreeMap<String, String>,
    /// Command line of the job in the foreground of the buffer's PTY when that
    /// is not the buffer's own process, for example a program its shell runs.
    /// `None` while the buffer's own process is in the foreground.
    pub foreground_command: Option<Vec<String>>,
}

impl BufferRecord {
    /// Command line of what runs in the foreground of the buffer now: the
    /// job its shell runs, or the buffer's own command when that is in the
    /// foreground (a shell at its prompt, or a program started directly).
    pub fn current_command(&self) -> &[String] {
        self.foreground_command.as_deref().unwrap_or(&self.command)
    }

    /// Name of the program in the foreground of the buffer, like tmux's
    /// `pane_current_command`. See [`program_name`].
    pub fn process_name(&self) -> Option<String> {
        program_name(self.current_command())
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(pipe) = &self.pipe {
            pipe.validate("buffer_record.pipe")?;
        }
        match self.kind {
            BufferRecordKind::Pty => {
                if self.helper_source_buffer_id.is_some() {
                    return Err(
                        "buffer_record.kind=pty cannot set helper_source_buffer_id".to_owned()
                    );
                }
                if self.helper_scope.is_some() {
                    return Err("buffer_record.kind=pty cannot set helper_scope".to_owned());
                }
            }
            BufferRecordKind::Helper => {
                if self.pipe.is_some() {
                    return Err("buffer_record.kind=helper cannot set pipe".to_owned());
                }
                if self.helper_source_buffer_id.is_some() ^ self.helper_scope.is_some() {
                    return Err(
                        "buffer_record.kind=helper must set helper_source_buffer_id and helper_scope together"
                            .to_owned(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// Name of the program a command line runs: the file name of its first word,
/// without the dash a login shell starts with (`-zsh` is `zsh`). `None` for
/// an empty command.
pub fn program_name(command: &[String]) -> Option<String> {
    let program = command.first()?;
    let program = program.strip_prefix('-').unwrap_or(program);
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    (!name.is_empty()).then(|| name.to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRecordKind {
    BufferView,
    Split,
    Tabs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferViewRecord {
    pub buffer_id: BufferId,
    pub focused: bool,
    pub zoomed: bool,
    pub follow_output: bool,
    pub last_render_size: PtySize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitRecord {
    pub direction: SplitDirection,
    pub child_ids: Vec<NodeId>,
    pub sizes: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabRecord {
    pub title: String,
    pub child_id: NodeId,
    /// Whether input typed into one pane of this window goes to all of its
    /// panes. Only meaningful on root tabs.
    pub synchronize_panes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsRecord {
    pub active: u32,
    pub tabs: Vec<TabRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRecord {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeRecordKind,
    pub buffer_view: Option<BufferViewRecord>,
    pub split: Option<SplitRecord>,
    pub tabs: Option<TabsRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingRecord {
    pub id: FloatingId,
    pub session_id: SessionId,
    pub root_node_id: NodeId,
    pub title: Option<String>,
    pub geometry: FloatGeometry,
    pub focused: bool,
    pub visible: bool,
    pub close_on_empty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRecord {
    pub id: u64,
    pub current_session_id: Option<SessionId>,
    pub subscribed_all_sessions: bool,
    pub subscribed_session_ids: Vec<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session: SessionRecord,
    pub nodes: Vec<NodeRecord>,
    pub buffers: Vec<BufferRecord>,
    pub floating: Vec<FloatingRecord>,
}

/// Buffers shown in the subtree of `nodes` rooted at `root`, including `root`
/// itself when it is a buffer view, in node order.
pub fn buffer_ids_in_subtree<'a>(
    nodes: impl IntoIterator<Item = &'a NodeRecord>,
    root: NodeId,
) -> Vec<BufferId> {
    let nodes = nodes.into_iter().collect::<Vec<_>>();
    let parents = nodes
        .iter()
        .map(|node| (node.id, node.parent_id))
        .collect::<BTreeMap<_, _>>();
    let in_subtree = |mut node_id: NodeId| {
        // Bounded by the node count, so a malformed parent cycle cannot hang.
        for _ in 0..=parents.len() {
            if node_id == root {
                return true;
            }
            match parents.get(&node_id).copied().flatten() {
                Some(parent) => node_id = parent,
                None => return false,
            }
        }
        false
    };
    nodes
        .iter()
        .filter_map(|node| {
            let view = node.buffer_view.as_ref()?;
            in_subtree(node.id).then_some(view.buffer_id)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OkResponse {
    pub request_id: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorResponse {
    pub request_id: Option<RequestId>,
    pub error: WireError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionsResponse {
    pub request_id: RequestId,
    pub sessions: Vec<SessionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshotResponse {
    pub request_id: RequestId,
    pub snapshot: SessionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuffersResponse {
    pub request_id: RequestId,
    pub buffers: Vec<BufferRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferResponse {
    pub request_id: RequestId,
    pub buffer: BufferRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingListResponse {
    pub request_id: RequestId,
    pub floating: Vec<FloatingRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingResponse {
    pub request_id: RequestId,
    pub floating: FloatingRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionAckResponse {
    pub request_id: RequestId,
    pub subscription_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientsResponse {
    pub request_id: RequestId,
    pub clients: Vec<ClientRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientResponse {
    pub request_id: RequestId,
    pub client: ClientRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferLocationResponse {
    pub request_id: RequestId,
    pub location: BufferLocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentResponse {
    pub request_id: RequestId,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferWithLocationResponse {
    pub(crate) request_id: RequestId,
    pub(crate) buffer: BufferRecord,
    pub(crate) location: BufferLocation,
    pub(crate) at_root_tab: bool,
}

impl BufferWithLocationResponse {
    pub fn new(
        request_id: RequestId,
        buffer: BufferRecord,
        location: BufferLocation,
        at_root_tab: bool,
    ) -> std::result::Result<Self, String> {
        if buffer.id != location.buffer_id {
            return Err(
                "buffer_with_location_response.buffer.id must equal location.buffer_id".to_owned(),
            );
        }
        buffer.validate()?;
        match (buffer.attachment_node_id, location.node_id()) {
            (Some(buffer_node_id), Some(location_node_id))
                if buffer_node_id == location_node_id => {}
            (Some(buffer_node_id), Some(location_node_id)) => {
                return Err(format!(
                    "buffer_with_location_response.buffer.attachment_node_id {buffer_node_id} must equal location node_id {location_node_id}"
                ));
            }
            (Some(buffer_node_id), None) => {
                return Err(format!(
                    "buffer_with_location_response.buffer.attachment_node_id {buffer_node_id} requires an attached location"
                ));
            }
            (None, Some(location_node_id)) => {
                return Err(format!(
                    "buffer_with_location_response.location node_id {location_node_id} requires buffer.attachment_node_id"
                ));
            }
            (None, None) => {}
        }
        if at_root_tab
            && !matches!(
                location.attachment,
                BufferLocationAttachment::Session { .. }
            )
        {
            return Err(
                "buffer_with_location_response.at_root_tab requires a session location".to_owned(),
            );
        }
        Ok(Self {
            request_id,
            buffer,
            location,
            at_root_tab,
        })
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub fn buffer(&self) -> &BufferRecord {
        &self.buffer
    }

    pub fn location(&self) -> &BufferLocation {
        &self.location
    }

    pub fn at_root_tab(&self) -> bool {
        self.at_root_tab
    }

    pub fn into_parts(self) -> (RequestId, BufferRecord, BufferLocation, bool) {
        (
            self.request_id,
            self.buffer,
            self.location,
            self.at_root_tab,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotResponse {
    pub request_id: RequestId,
    pub buffer_id: BufferId,
    pub sequence: u64,
    pub size: PtySize,
    pub lines: Vec<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleSnapshotResponse {
    pub request_id: RequestId,
    pub buffer_id: BufferId,
    pub sequence: u64,
    pub size: PtySize,
    pub lines: Vec<SnapshotLine>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub viewport_top_line: u64,
    pub total_lines: u64,
    pub alternate_screen: bool,
    pub mouse_reporting: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub keyboard_mode: u8,
    pub cursor: Option<CursorState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollbackSliceResponse {
    pub request_id: RequestId,
    pub buffer_id: BufferId,
    pub start_line: u64,
    pub total_lines: u64,
    pub lines: Vec<SnapshotLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerResponse {
    Pong(PingResponse),
    Ok(OkResponse),
    Error(ErrorResponse),
    Sessions(SessionsResponse),
    SessionSnapshot(SessionSnapshotResponse),
    Buffers(BuffersResponse),
    Buffer(BufferResponse),
    FloatingList(FloatingListResponse),
    Floating(FloatingResponse),
    SubscriptionAck(SubscriptionAckResponse),
    Clients(ClientsResponse),
    Client(ClientResponse),
    BufferLocation(BufferLocationResponse),
    BufferWithLocation(BufferWithLocationResponse),
    Environment(EnvironmentResponse),
    Snapshot(SnapshotResponse),
    VisibleSnapshot(VisibleSnapshotResponse),
    ScrollbackSlice(ScrollbackSliceResponse),
}

impl ServerResponse {
    pub fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::Pong(response) => Some(response.request_id),
            Self::Ok(response) => Some(response.request_id),
            Self::Error(response) => response.request_id,
            Self::Sessions(response) => Some(response.request_id),
            Self::SessionSnapshot(response) => Some(response.request_id),
            Self::Buffers(response) => Some(response.request_id),
            Self::Buffer(response) => Some(response.request_id),
            Self::FloatingList(response) => Some(response.request_id),
            Self::Floating(response) => Some(response.request_id),
            Self::SubscriptionAck(response) => Some(response.request_id),
            Self::Clients(response) => Some(response.request_id),
            Self::Client(response) => Some(response.request_id),
            Self::BufferLocation(response) => Some(response.request_id),
            Self::BufferWithLocation(response) => Some(response.request_id()),
            Self::Environment(response) => Some(response.request_id),
            Self::Snapshot(response) => Some(response.request_id),
            Self::VisibleSnapshot(response) => Some(response.request_id),
            Self::ScrollbackSlice(response) => Some(response.request_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCreatedEvent {
    pub session: SessionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClosedEvent {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferCreatedEvent {
    pub buffer: BufferRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferPipeChangedEvent {
    pub session_id: Option<SessionId>,
    pub buffer: BufferRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferDetachedEvent {
    pub buffer_id: BufferId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeChangedEvent {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingChangedEvent {
    pub session_id: SessionId,
    pub floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusChangedEvent {
    pub session_id: SessionId,
    pub focused_leaf_id: Option<NodeId>,
    pub focused_floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderInvalidatedEvent {
    pub buffer_id: BufferId,
}

/// A clipboard write (OSC 52) from a program in a buffer. Unlike other events it
/// is not broadcast to subscribers: the server sends it only to the clients
/// whose current session shows the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardEvent {
    pub buffer_id: BufferId,
    pub selection: ClipboardSelection,
    pub text: String,
}

/// Sequences a program in a buffer sent for the host terminal (tmux
/// passthrough and unwrapped kitty graphics commands), in order. Like [`ClipboardEvent`] it goes only to the clients
/// whose current session contains the buffer; each decides from its own layout
/// whether the pane is on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassthroughEvent {
    pub buffer_id: BufferId,
    pub writes: Vec<PassthroughWrite>,
}

/// A message to show in a client's status line (`display-message`). Sent only
/// to the clients the request named, not broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayMessageEvent {
    pub message: String,
    /// How long to show it; 0 for the client's default.
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRenamedEvent {
    pub session_id: SessionId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientChangedEvent {
    pub client: ClientRecord,
    pub previous_session_id: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEvent {
    SessionCreated(SessionCreatedEvent),
    SessionClosed(SessionClosedEvent),
    SessionRenamed(SessionRenamedEvent),
    BufferCreated(BufferCreatedEvent),
    BufferPipeChanged(BufferPipeChangedEvent),
    BufferDetached(BufferDetachedEvent),
    NodeChanged(NodeChangedEvent),
    FloatingChanged(FloatingChangedEvent),
    FocusChanged(FocusChangedEvent),
    RenderInvalidated(RenderInvalidatedEvent),
    ClientChanged(ClientChangedEvent),
    Clipboard(ClipboardEvent),
    DisplayMessage(DisplayMessageEvent),
    Passthrough(PassthroughEvent),
    /// The server is stopping on purpose (`kill-server`, or a signal asking it
    /// to): every connection is about to close. Sent to every connection, so a
    /// client can tell this from the server crashing.
    ServerShutdown,
}

impl ServerEvent {
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionCreated(event) => Some(event.session.id),
            Self::SessionClosed(event) => Some(event.session_id),
            Self::SessionRenamed(event) => Some(event.session_id),
            Self::BufferCreated(_) => None,
            Self::BufferPipeChanged(event) => event.session_id,
            Self::BufferDetached(_) => None,
            Self::NodeChanged(event) => Some(event.session_id),
            Self::FloatingChanged(event) => Some(event.session_id),
            Self::FocusChanged(event) => Some(event.session_id),
            Self::RenderInvalidated(_) => None,
            Self::ClientChanged(event) => event.client.current_session_id,
            Self::Clipboard(_)
            | Self::DisplayMessage(_)
            | Self::Passthrough(_)
            | Self::ServerShutdown => None,
        }
    }

    pub fn all_session_ids(&self) -> Vec<SessionId> {
        match self {
            Self::ClientChanged(event) => {
                let mut ids = Vec::new();
                if let Some(prev) = event.previous_session_id {
                    ids.push(prev);
                }
                if let Some(curr) = event.client.current_session_id {
                    ids.push(curr);
                }
                ids
            }
            _ => self.session_id().into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEnvelope {
    Response(ServerResponse),
    Event(ServerEvent),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(command: &[&str], foreground: Option<&[&str]>) -> BufferRecord {
        let owned = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        BufferRecord {
            id: BufferId(1),
            title: "shell".to_owned(),
            command: owned(command),
            cwd: None,
            pipe: None,
            kind: BufferRecordKind::Pty,
            state: BufferRecordState::Running,
            pid: None,
            attachment_node_id: None,
            read_only: false,
            helper_source_buffer_id: None,
            helper_scope: None,
            pty_size: PtySize::new(80, 24),
            activity: ActivityState::Idle,
            last_snapshot_seq: 0,
            exit_code: None,
            env: BTreeMap::new(),
            user_options: BTreeMap::new(),
            foreground_command: foreground.map(owned),
        }
    }

    #[test]
    fn process_name_is_the_foreground_program_without_path_or_login_dash() {
        assert_eq!(
            buffer(&["/bin/zsh"], None).process_name().as_deref(),
            Some("zsh")
        );
        assert_eq!(
            buffer(&["-zsh"], None).process_name().as_deref(),
            Some("zsh")
        );
        assert_eq!(
            buffer(&["/bin/zsh"], Some(&["/usr/bin/nvim", "notes.txt"]))
                .process_name()
                .as_deref(),
            Some("nvim")
        );
        assert_eq!(buffer(&[], None).process_name(), None);
    }
}
