use std::path::PathBuf;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, SessionId, SplitDirection,
    Timestamp,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub root_node: NodeId,
    pub floating: Vec<FloatingId>,
    pub focused_leaf: Option<NodeId>,
    pub focused_floating: Option<FloatingId>,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub id: BufferId,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub state: BufferState,
    pub attachment: BufferAttachment,
    pub pty_size: PtySize,
    pub activity: ActivityState,
    pub last_snapshot_seq: u64,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunningBuffer {
    pub pid: Option<u32>,
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
}

impl TabEntry {
    pub fn new(title: impl Into<String>, child: NodeId) -> Self {
        Self {
            title: title.into(),
            child,
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
