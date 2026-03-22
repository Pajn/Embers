use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, MuxError, NodeId, PtySize, Result,
    SessionId, SplitDirection, Timestamp,
};
use serde::{Deserialize, Serialize};

use crate::model::{
    Buffer, BufferAttachment, BufferState, BufferViewNode, BufferViewState, ExitedBuffer,
    FloatingWindow, InterruptedBuffer, Node, RunningBuffer, Session, SplitNode, TabEntry, TabsNode,
};
use crate::state::ServerState;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedWorkspace {
    pub sessions: Vec<PersistedSession>,
    pub buffers: Vec<PersistedBuffer>,
    pub nodes: Vec<PersistedNode>,
    pub floating: Vec<PersistedFloatingWindow>,
    pub next_session_id: u64,
    pub next_buffer_id: u64,
    pub next_node_id: u64,
    pub next_floating_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedSession {
    pub id: u64,
    pub name: String,
    pub root_node: u64,
    pub floating: Vec<u64>,
    pub focused_leaf: Option<u64>,
    pub focused_floating: Option<u64>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedBuffer {
    pub id: u64,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub state: PersistedBufferState,
    pub attachment: PersistedBufferAttachment,
    pub pty_size: PtySize,
    pub activity: PersistedActivityState,
    pub last_snapshot_seq: u64,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedBufferState {
    Created,
    Running {
        pid: Option<u32>,
    },
    Interrupted {
        last_known_pid: Option<u32>,
    },
    Exited {
        exit_code: Option<i32>,
        exited_at_ms: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedBufferAttachment {
    Attached { node_id: u64 },
    Detached,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedNode {
    BufferView {
        id: u64,
        session_id: u64,
        parent: Option<u64>,
        buffer_id: u64,
        focused: bool,
        zoomed: bool,
        follow_output: bool,
        last_render_size: PtySize,
    },
    Split {
        id: u64,
        session_id: u64,
        parent: Option<u64>,
        direction: PersistedSplitDirection,
        children: Vec<u64>,
        sizes: Vec<u16>,
        last_focused_descendant: Option<u64>,
    },
    Tabs {
        id: u64,
        session_id: u64,
        parent: Option<u64>,
        tabs: Vec<PersistedTabEntry>,
        active: usize,
        last_focused_descendant: Option<u64>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedTabEntry {
    pub title: String,
    pub child: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PersistedSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PersistedActivityState {
    Idle,
    Activity,
    Bell,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedFloatingWindow {
    pub id: u64,
    pub session_id: u64,
    pub root_node: u64,
    pub title: Option<String>,
    pub geometry: FloatGeometry,
    pub focused: bool,
    pub visible: bool,
    pub close_on_empty: bool,
    pub last_focused_leaf: Option<u64>,
}

pub fn load_workspace(path: &Path) -> Result<Option<ServerState>> {
    match fs::read(path) {
        Ok(bytes) => {
            let persisted: PersistedWorkspace = serde_json::from_slice(&bytes)
                .map_err(|error| MuxError::internal(error.to_string()))?;
            Ok(Some(ServerState::from_persisted(persisted)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save_workspace(path: &Path, state: &ServerState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&state.to_persisted())
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let temp_path = path.with_extension("tmp");
    {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp_path, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn persisted_session(session: &Session) -> PersistedSession {
    PersistedSession {
        id: session.id.0,
        name: session.name.clone(),
        root_node: session.root_node.0,
        floating: session.floating.iter().map(|id| id.0).collect(),
        focused_leaf: session.focused_leaf.map(|id| id.0),
        focused_floating: session.focused_floating.map(|id| id.0),
        created_at_ms: timestamp_to_millis(session.created_at),
    }
}

pub fn restored_session(session: PersistedSession) -> Result<Session> {
    Ok(Session {
        id: SessionId(session.id),
        name: session.name,
        root_node: NodeId(session.root_node),
        floating: session.floating.into_iter().map(FloatingId).collect(),
        focused_leaf: session.focused_leaf.map(NodeId),
        focused_floating: session.focused_floating.map(FloatingId),
        created_at: timestamp_from_millis(session.created_at_ms)?,
    })
}

pub fn persisted_buffer(buffer: &Buffer) -> PersistedBuffer {
    PersistedBuffer {
        id: buffer.id.0,
        title: buffer.title.clone(),
        command: buffer.command.clone(),
        cwd: buffer.cwd.clone(),
        env: buffer.env.clone(),
        state: persisted_buffer_state(&buffer.state),
        attachment: persisted_buffer_attachment(&buffer.attachment),
        pty_size: buffer.pty_size,
        activity: persisted_activity(buffer.activity),
        last_snapshot_seq: buffer.last_snapshot_seq,
        created_at_ms: timestamp_to_millis(buffer.created_at),
    }
}

pub fn restored_buffer(buffer: PersistedBuffer) -> Result<Buffer> {
    Ok(Buffer {
        id: BufferId(buffer.id),
        title: buffer.title,
        command: buffer.command,
        cwd: buffer.cwd,
        env: buffer.env,
        state: restored_buffer_state(buffer.state)?,
        attachment: restored_buffer_attachment(buffer.attachment),
        pty_size: buffer.pty_size,
        activity: restored_activity(buffer.activity),
        last_snapshot_seq: buffer.last_snapshot_seq,
        created_at: timestamp_from_millis(buffer.created_at_ms)?,
    })
}

pub fn persisted_node(node: &Node) -> PersistedNode {
    match node {
        Node::BufferView(node) => PersistedNode::BufferView {
            id: node.id.0,
            session_id: node.session_id.0,
            parent: node.parent.map(|id| id.0),
            buffer_id: node.buffer_id.0,
            focused: node.view.focused,
            zoomed: node.view.zoomed,
            follow_output: node.view.follow_output,
            last_render_size: node.view.last_render_size,
        },
        Node::Split(node) => PersistedNode::Split {
            id: node.id.0,
            session_id: node.session_id.0,
            parent: node.parent.map(|id| id.0),
            direction: persisted_split_direction(node.direction),
            children: node.children.iter().map(|id| id.0).collect(),
            sizes: node.sizes.clone(),
            last_focused_descendant: node.last_focused_descendant.map(|id| id.0),
        },
        Node::Tabs(node) => PersistedNode::Tabs {
            id: node.id.0,
            session_id: node.session_id.0,
            parent: node.parent.map(|id| id.0),
            tabs: node
                .tabs
                .iter()
                .map(|tab| PersistedTabEntry {
                    title: tab.title.clone(),
                    child: tab.child.0,
                })
                .collect(),
            active: node.active,
            last_focused_descendant: node.last_focused_descendant.map(|id| id.0),
        },
    }
}

pub fn restored_node(node: PersistedNode) -> Node {
    match node {
        PersistedNode::BufferView {
            id,
            session_id,
            parent,
            buffer_id,
            focused,
            zoomed,
            follow_output,
            last_render_size,
        } => Node::BufferView(BufferViewNode {
            id: NodeId(id),
            session_id: SessionId(session_id),
            parent: parent.map(NodeId),
            buffer_id: BufferId(buffer_id),
            view: BufferViewState {
                focused,
                zoomed,
                follow_output,
                last_render_size,
            },
        }),
        PersistedNode::Split {
            id,
            session_id,
            parent,
            direction,
            children,
            sizes,
            last_focused_descendant,
        } => Node::Split(SplitNode {
            id: NodeId(id),
            session_id: SessionId(session_id),
            parent: parent.map(NodeId),
            direction: restored_split_direction(direction),
            children: children.into_iter().map(NodeId).collect(),
            sizes,
            last_focused_descendant: last_focused_descendant.map(NodeId),
        }),
        PersistedNode::Tabs {
            id,
            session_id,
            parent,
            tabs,
            active,
            last_focused_descendant,
        } => Node::Tabs(TabsNode {
            id: NodeId(id),
            session_id: SessionId(session_id),
            parent: parent.map(NodeId),
            tabs: tabs
                .into_iter()
                .map(|tab| TabEntry {
                    title: tab.title,
                    child: NodeId(tab.child),
                })
                .collect(),
            active,
            last_focused_descendant: last_focused_descendant.map(NodeId),
        }),
    }
}

pub fn persisted_floating(window: &FloatingWindow) -> PersistedFloatingWindow {
    PersistedFloatingWindow {
        id: window.id.0,
        session_id: window.session_id.0,
        root_node: window.root_node.0,
        title: window.title.clone(),
        geometry: window.geometry,
        focused: window.focused,
        visible: window.visible,
        close_on_empty: window.close_on_empty,
        last_focused_leaf: window.last_focused_leaf.map(|id| id.0),
    }
}

pub fn restored_floating(window: PersistedFloatingWindow) -> FloatingWindow {
    FloatingWindow {
        id: FloatingId(window.id),
        session_id: SessionId(window.session_id),
        root_node: NodeId(window.root_node),
        title: window.title,
        geometry: window.geometry,
        focused: window.focused,
        visible: window.visible,
        close_on_empty: window.close_on_empty,
        last_focused_leaf: window.last_focused_leaf.map(NodeId),
    }
}

fn persisted_buffer_state(state: &BufferState) -> PersistedBufferState {
    match state {
        BufferState::Created => PersistedBufferState::Created,
        BufferState::Running(running) => PersistedBufferState::Running { pid: running.pid },
        BufferState::Interrupted(interrupted) => PersistedBufferState::Interrupted {
            last_known_pid: interrupted.last_known_pid,
        },
        BufferState::Exited(exited) => PersistedBufferState::Exited {
            exit_code: exited.exit_code,
            exited_at_ms: timestamp_to_millis(exited.exited_at),
        },
    }
}

fn restored_buffer_state(state: PersistedBufferState) -> Result<BufferState> {
    Ok(match state {
        PersistedBufferState::Created => BufferState::Created,
        PersistedBufferState::Running { pid } => BufferState::Running(RunningBuffer { pid }),
        PersistedBufferState::Interrupted { last_known_pid } => {
            BufferState::Interrupted(InterruptedBuffer { last_known_pid })
        }
        PersistedBufferState::Exited {
            exit_code,
            exited_at_ms,
        } => BufferState::Exited(ExitedBuffer {
            exit_code,
            exited_at: timestamp_from_millis(exited_at_ms)?,
        }),
    })
}

fn persisted_buffer_attachment(attachment: &BufferAttachment) -> PersistedBufferAttachment {
    match attachment {
        BufferAttachment::Attached(node_id) => {
            PersistedBufferAttachment::Attached { node_id: node_id.0 }
        }
        BufferAttachment::Detached => PersistedBufferAttachment::Detached,
    }
}

fn restored_buffer_attachment(attachment: PersistedBufferAttachment) -> BufferAttachment {
    match attachment {
        PersistedBufferAttachment::Attached { node_id } => {
            BufferAttachment::Attached(NodeId(node_id))
        }
        PersistedBufferAttachment::Detached => BufferAttachment::Detached,
    }
}

fn persisted_split_direction(direction: SplitDirection) -> PersistedSplitDirection {
    match direction {
        SplitDirection::Horizontal => PersistedSplitDirection::Horizontal,
        SplitDirection::Vertical => PersistedSplitDirection::Vertical,
    }
}

fn restored_split_direction(direction: PersistedSplitDirection) -> SplitDirection {
    match direction {
        PersistedSplitDirection::Horizontal => SplitDirection::Horizontal,
        PersistedSplitDirection::Vertical => SplitDirection::Vertical,
    }
}

fn persisted_activity(activity: ActivityState) -> PersistedActivityState {
    match activity {
        ActivityState::Idle => PersistedActivityState::Idle,
        ActivityState::Activity => PersistedActivityState::Activity,
        ActivityState::Bell => PersistedActivityState::Bell,
    }
}

fn restored_activity(activity: PersistedActivityState) -> ActivityState {
    match activity {
        PersistedActivityState::Idle => ActivityState::Idle,
        PersistedActivityState::Activity => ActivityState::Activity,
        PersistedActivityState::Bell => ActivityState::Bell,
    }
}

fn timestamp_to_millis(timestamp: Timestamp) -> u64 {
    timestamp
        .0
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn timestamp_from_millis(millis: u64) -> Result<Timestamp> {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(millis))
        .map(Timestamp)
        .ok_or_else(|| MuxError::internal(format!("timestamp overflow for milliseconds: {millis}")))
}
