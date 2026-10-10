use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, SessionId, Size,
    SplitDirection, Timestamp,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub root_node: NodeId,
    pub floating: Vec<FloatingId>,
    pub focused_leaf: Option<NodeId>,
    pub focused_floating: Option<FloatingId>,
    pub zoomed_node: Option<NodeId>,
    /// The area an attached client last drew this session in. Panes created
    /// without a client use it to start at their drawn size.
    pub viewport: Option<Size>,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperBuffer {
    pub source_buffer_id: BufferId,
    pub scope: HelperBufferScope,
    pub lines: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperBufferScope {
    Full,
    Visible,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BufferKind {
    #[default]
    Pty,
    Helper(HelperBuffer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferPipe {
    pub command: Vec<String>,
    pub state: BufferPipeState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferPipeState {
    Running {
        pid: Option<u32>,
    },
    Stopped {
        exit_code: Option<i32>,
        reason: BufferPipeStopReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferPipeStopReason {
    Requested,
    PipeExited,
    WriteFailed,
    BufferExited,
    RuntimeInterrupted,
}

#[derive(Clone)]
pub struct Buffer {
    pub id: BufferId,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub user_options: BTreeMap<String, String>,
    /// Command line of the job last seen in the foreground of this buffer's
    /// PTY when that was not the buffer's own process, for example a program
    /// its shell runs. Saved so a restore can decide whether to run it again.
    pub foreground_command: Option<Vec<String>>,
    runtime_socket_path: Option<PathBuf>,
    pub state: BufferState,
    pub attachment: BufferAttachment,
    pub pty_size: PtySize,
    pub activity: ActivityState,
    pub last_snapshot_seq: u64,
    pub kind: BufferKind,
    pub pipe: Option<BufferPipe>,
    pub created_at: Timestamp,
    /// Whether the buffer has been shown in a view since it was created. A
    /// buffer is created detached and attached by a later request, so an exit
    /// before its first attachment does not remove it (see
    /// [`ServerState::dispose_exited_buffers`](crate::ServerState::dispose_exited_buffers)).
    /// Not persisted: restored buffers start out as never shown.
    pub ever_attached: bool,
}

/// Buffer option that keeps an exited buffer in its view instead of closing
/// the view, like tmux's `remain-on-exit`: `on` always, `failed` only when the
/// command did not exit with status 0. Any other value, or none, means off.
pub const REMAIN_ON_EXIT_OPTION: &str = "remain-on-exit";

/// Buffer option Neovim sets on its own pane, like tmux's `@pane-is-vim`, so
/// smart navigation can tell it runs there. It describes the program running
/// in the buffer rather than the pane, so it is dropped when a restore starts
/// a fresh process in the buffer.
pub const IS_VIM_OPTION: &str = "is-vim";

impl Buffer {
    pub(crate) fn new(
        id: BufferId,
        title: impl Into<String>,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            command,
            cwd,
            env,
            user_options: BTreeMap::new(),
            foreground_command: None,
            runtime_socket_path: None,
            state: BufferState::Created,
            attachment: BufferAttachment::Detached,
            pty_size: PtySize::new(80, 24),
            activity: ActivityState::Idle,
            last_snapshot_seq: 0,
            kind: BufferKind::Pty,
            pipe: None,
            created_at: Timestamp::now(),
            ever_attached: false,
        }
    }

    /// Whether this buffer stays in its view after its command exits, per its
    /// [`REMAIN_ON_EXIT_OPTION`].
    pub fn remains_on_exit(&self) -> bool {
        match self
            .user_options
            .get(REMAIN_ON_EXIT_OPTION)
            .map(String::as_str)
        {
            Some("on") => true,
            Some("failed") => match &self.state {
                BufferState::Exited(exited) => exited.exit_code != Some(0),
                _ => true,
            },
            _ => false,
        }
    }

    pub(crate) fn runtime_socket_path(&self) -> Option<&PathBuf> {
        self.runtime_socket_path.as_ref()
    }

    pub(crate) fn set_runtime_socket_path(&mut self, runtime_socket_path: Option<PathBuf>) {
        self.runtime_socket_path = runtime_socket_path;
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Buffer")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("command", &self.command)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("user_options", &self.user_options)
            .field("foreground_command", &self.foreground_command)
            .field("state", &self.state)
            .field("attachment", &self.attachment)
            .field("pty_size", &self.pty_size)
            .field("activity", &self.activity)
            .field("last_snapshot_seq", &self.last_snapshot_seq)
            .field("kind", &self.kind)
            .field("pipe", &self.pipe)
            .field("created_at", &self.created_at)
            .field("ever_attached", &self.ever_attached)
            .finish()
    }
}

// `ever_attached` is left out: it is bookkeeping that is not persisted, so a
// saved and restored buffer still compares equal to the original.
impl PartialEq for Buffer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.title == other.title
            && self.command == other.command
            && self.cwd == other.cwd
            && self.env == other.env
            && self.user_options == other.user_options
            && self.state == other.state
            && self.attachment == other.attachment
            && self.pty_size == other.pty_size
            && self.activity == other.activity
            && self.last_snapshot_seq == other.last_snapshot_seq
            && self.kind == other.kind
            && self.pipe == other.pipe
            && self.created_at == other.created_at
    }
}

impl Eq for Buffer {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunningBuffer {
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterruptedBuffer {
    pub last_known_pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitedBuffer {
    pub exit_code: Option<i32>,
    pub exited_at: Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BufferState {
    #[default]
    Created,
    Running(RunningBuffer),
    Interrupted(InterruptedBuffer),
    Exited(ExitedBuffer),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BufferAttachment {
    Attached(NodeId),
    #[default]
    Detached,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    BufferView(BufferViewNode),
    Split(SplitNode),
    Tabs(TabsNode),
}

impl Node {
    pub fn id(&self) -> NodeId {
        match self {
            Self::BufferView(node) => node.id,
            Self::Split(node) => node.id,
            Self::Tabs(node) => node.id,
        }
    }

    pub fn session_id(&self) -> SessionId {
        match self {
            Self::BufferView(node) => node.session_id,
            Self::Split(node) => node.session_id,
            Self::Tabs(node) => node.session_id,
        }
    }

    pub fn parent(&self) -> Option<NodeId> {
        match self {
            Self::BufferView(node) => node.parent,
            Self::Split(node) => node.parent,
            Self::Tabs(node) => node.parent,
        }
    }

    pub fn set_parent(&mut self, parent: Option<NodeId>) {
        match self {
            Self::BufferView(node) => node.parent = parent,
            Self::Split(node) => node.parent = parent,
            Self::Tabs(node) => node.parent = parent,
        }
    }

    pub fn child_ids(&self) -> Vec<NodeId> {
        match self {
            Self::BufferView(_) => Vec::new(),
            Self::Split(node) => node.children.clone(),
            Self::Tabs(node) => node.tabs.iter().map(|tab| tab.child).collect(),
        }
    }

    pub fn last_focused_descendant(&self) -> Option<NodeId> {
        match self {
            Self::BufferView(node) => node.view.focused.then_some(node.id),
            Self::Split(node) => node.last_focused_descendant,
            Self::Tabs(node) => node.last_focused_descendant,
        }
    }

    pub fn set_last_focused_descendant(&mut self, leaf_id: Option<NodeId>) {
        match self {
            Self::BufferView(_) => {}
            Self::Split(node) => node.last_focused_descendant = leaf_id,
            Self::Tabs(node) => node.last_focused_descendant = leaf_id,
        }
    }

    pub fn as_buffer_view(&self) -> Option<&BufferViewNode> {
        match self {
            Self::BufferView(node) => Some(node),
            _ => None,
        }
    }

    pub fn as_split(&self) -> Option<&SplitNode> {
        match self {
            Self::Split(node) => Some(node),
            _ => None,
        }
    }

    pub fn as_tabs(&self) -> Option<&TabsNode> {
        match self {
            Self::Tabs(node) => Some(node),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferViewNode {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent: Option<NodeId>,
    pub buffer_id: BufferId,
    pub view: BufferViewState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferViewState {
    pub focused: bool,
    pub zoomed: bool,
    pub follow_output: bool,
    pub last_render_size: PtySize,
}

impl Default for BufferViewState {
    fn default() -> Self {
        Self {
            focused: false,
            zoomed: false,
            follow_output: true,
            last_render_size: PtySize::new(80, 24),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitNode {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent: Option<NodeId>,
    pub direction: SplitDirection,
    pub children: Vec<NodeId>,
    pub sizes: Vec<u16>,
    pub last_focused_descendant: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsNode {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent: Option<NodeId>,
    pub tabs: Vec<TabEntry>,
    pub active: usize,
    pub last_focused_descendant: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabEntry {
    pub title: String,
    pub child: NodeId,
    /// tmux `synchronize-panes` for the window this tab holds. Only set on
    /// root tabs; the client fans typed input out to the window's panes.
    pub synchronize_panes: bool,
}

impl TabEntry {
    pub fn new(title: impl Into<String>, child: NodeId) -> Self {
        Self {
            title: title.into(),
            child,
            synchronize_panes: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingWindow {
    pub id: FloatingId,
    pub session_id: SessionId,
    pub root_node: NodeId,
    pub title: Option<String>,
    pub geometry: FloatGeometry,
    pub focused: bool,
    pub visible: bool,
    pub close_on_empty: bool,
    pub last_focused_leaf: Option<NodeId>,
}
