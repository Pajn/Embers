use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, MuxError, NodeId, PtySize, Result,
    SessionId, Size, SplitDirection, Timestamp,
};
use serde::{Deserialize, Serialize};

use crate::model::{
    Buffer, BufferAttachment, BufferKind, BufferState, BufferViewNode, BufferViewState,
    ExitedBuffer, FloatingWindow, HelperBuffer, HelperBufferScope, InterruptedBuffer, Node,
    Session, SplitNode, TabEntry, TabsNode,
};
use crate::state::ServerState;

const LEGACY_FORMAT_VERSION: u32 = 0;
const FIRST_VERSIONED_FORMAT_VERSION: u32 = 1;
pub const CURRENT_FORMAT_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedWorkspace {
    #[serde(default)]
    pub format_version: Option<u32>,
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
    #[serde(default)]
    pub zoomed_node: Option<u64>,
    /// The viewport a client last drew the session in, so panes created
    /// after a restart, before a client attaches, still start at that size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Size>,
    /// The base index of the client that last showed the session, so the
    /// CLI numbers windows the same way after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_index: Option<u32>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedBuffer {
    pub id: u64,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    /// Options set on the buffer (`embers buffer set-option`), such as
    /// `remain-on-exit`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_options: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_command: Option<Vec<String>>,
    #[serde(default)]
    pub runtime_socket_path: Option<PathBuf>,
    pub state: PersistedBufferState,
    pub attachment: PersistedBufferAttachment,
    pub pty_size: PtySize,
    /// Saved, but a change to it alone does not call for a save (see
    /// [`same_saved_state`]). Output sequence numbers are not saved at all:
    /// they only mean something to the keeper that produced them, and are
    /// taken from the keeper again when one is attached.
    pub activity: PersistedActivityState,
    #[serde(default)]
    pub kind: PersistedBufferKind,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedBufferKind {
    #[default]
    Pty,
    Helper {
        source_buffer_id: u64,
        scope: PersistedHelperBufferScope,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lines: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PersistedHelperBufferScope {
    Full,
    Visible,
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
            let persisted = load_current_workspace(persisted)?;
            Ok(Some(ServerState::from_persisted(persisted)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub fn save_workspace(path: &Path, state: &ServerState) -> Result<()> {
    write_workspace(path, &encode_workspace(state)?)
}

/// Serialize `state` into the bytes [`write_workspace`] stores.
#[cfg(test)]
pub fn encode_workspace(state: &ServerState) -> Result<Vec<u8>> {
    encode_persisted_workspace(&state.to_persisted())
}

/// Serialize `workspace` into the bytes [`write_workspace`] stores.
pub fn encode_persisted_workspace(workspace: &PersistedWorkspace) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(workspace).map_err(|error| MuxError::internal(error.to_string()))
}

/// Whether `a` and `b` save the same workspace apart from buffer activity,
/// which changes with every bit of output and is not worth a write of its
/// own; it is saved along with the next change that is.
pub fn same_saved_state(a: &PersistedWorkspace, b: &PersistedWorkspace) -> bool {
    // Destructured so a new field cannot be left out of the comparison.
    let PersistedWorkspace {
        format_version,
        sessions,
        buffers,
        nodes,
        floating,
        next_session_id,
        next_buffer_id,
        next_node_id,
        next_floating_id,
    } = a;
    *format_version == b.format_version
        && *sessions == b.sessions
        && *nodes == b.nodes
        && *floating == b.floating
        && *next_session_id == b.next_session_id
        && *next_buffer_id == b.next_buffer_id
        && *next_node_id == b.next_node_id
        && *next_floating_id == b.next_floating_id
        && buffers.len() == b.buffers.len()
        && buffers.iter().zip(&b.buffers).all(|(a, b)| {
            PersistedBuffer {
                activity: b.activity,
                ..a.clone()
            } == *b
        })
}

/// Atomically replace the workspace file at `path` with `bytes`: write a temp
/// file next to it, fsync it, rename it over `path`, then fsync the directory.
/// A crash at any point leaves either the old file or the new one, never a
/// partial write.
pub fn write_workspace(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let (temp_path, mut file) = open_workspace_temp_file(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(error.into());
    }
    drop(file);
    if let Err(error) = validate_workspace_temp_path(&temp_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error.into());
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(())
}

fn open_workspace_temp_file(path: &Path) -> Result<(PathBuf, fs::File)> {
    const MAX_ATTEMPTS: u32 = 1024;

    let pid = std::process::id();
    for attempt in 0..MAX_ATTEMPTS {
        let temp_path = workspace_temp_path(path, pid, attempt);
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        match options.open(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(MuxError::internal(format!(
        "failed to allocate a temporary workspace file next to {}",
        path.display()
    )))
}

fn workspace_temp_path(path: &Path, pid: u32, attempt: u32) -> PathBuf {
    let file_name = path.file_name().unwrap_or_else(|| OsStr::new("workspace"));
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".tmp.{pid}.{attempt}"));
    path.with_file_name(temp_name)
}

fn validate_workspace_temp_path(temp_path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(temp_path)?;
    if metadata.file_type().is_symlink() {
        return Err(MuxError::internal(format!(
            "refusing to rename symlink temp workspace file {}",
            temp_path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(MuxError::internal(format!(
            "refusing to rename non-file temp workspace path {}",
            temp_path.display()
        )));
    }
    Ok(())
}

fn load_current_workspace(mut workspace: PersistedWorkspace) -> Result<PersistedWorkspace> {
    let version = workspace.format_version.unwrap_or(LEGACY_FORMAT_VERSION);
    if version != CURRENT_FORMAT_VERSION {
        return migrate_workspace(workspace, version);
    }
    workspace.format_version = Some(CURRENT_FORMAT_VERSION);
    Ok(workspace)
}

fn migrate_workspace(
    mut workspace: PersistedWorkspace,
    version: u32,
) -> Result<PersistedWorkspace> {
    match version {
        LEGACY_FORMAT_VERSION | FIRST_VERSIONED_FORMAT_VERSION => {
            let mut legacy_zoomed_nodes = BTreeMap::<u64, Vec<u64>>::new();
            for node in &workspace.nodes {
                if let PersistedNode::BufferView {
                    id,
                    session_id,
                    zoomed: true,
                    ..
                } = node
                {
                    legacy_zoomed_nodes
                        .entry(*session_id)
                        .or_default()
                        .push(*id);
                }
            }
            for session in &mut workspace.sessions {
                if session.zoomed_node.is_none() {
                    match legacy_zoomed_nodes.get(&session.id).map(Vec::as_slice) {
                        Some([]) | None => {}
                        Some([node_id]) => session.zoomed_node = Some(*node_id),
                        Some(node_ids) => {
                            return Err(MuxError::internal(format!(
                                "workspace session {} has multiple legacy zoomed nodes: {:?}",
                                session.id, node_ids
                            )));
                        }
                    }
                }
            }
            workspace.format_version = Some(CURRENT_FORMAT_VERSION);
            Ok(workspace)
        }
        _ => Err(MuxError::internal(format!(
            "unsupported workspace format version {version}"
        ))),
    }
}

pub fn persisted_session(session: &Session) -> PersistedSession {
    PersistedSession {
        id: session.id.0,
        name: session.name.clone(),
        root_node: session.root_node.0,
        floating: session.floating.iter().map(|id| id.0).collect(),
        focused_leaf: session.focused_leaf.map(|id| id.0),
        focused_floating: session.focused_floating.map(|id| id.0),
        zoomed_node: session.zoomed_node.map(|id| id.0),
        viewport: session.viewport,
        base_index: session.base_index,
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
        zoomed_node: session.zoomed_node.map(NodeId),
        viewport: session.viewport,
        base_index: session.base_index,
        focus_history: Vec::new(),
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
        user_options: buffer.user_options.clone(),
        foreground_command: buffer.foreground_command.clone(),
        runtime_socket_path: buffer.runtime_socket_path().cloned(),
        state: persisted_buffer_state(&buffer.state),
        attachment: persisted_buffer_attachment(&buffer.attachment),
        pty_size: buffer.pty_size,
        activity: persisted_activity(buffer.activity),
        kind: persisted_buffer_kind(&buffer.kind),
        created_at_ms: timestamp_to_millis(buffer.created_at),
    }
}

pub fn restored_buffer(buffer: PersistedBuffer) -> Result<Buffer> {
    let mut restored = Buffer::new(
        BufferId(buffer.id),
        buffer.title,
        buffer.command,
        buffer.cwd,
        buffer.env,
    );
    restored.user_options = buffer.user_options;
    restored.foreground_command = buffer.foreground_command;
    restored.set_runtime_socket_path(buffer.runtime_socket_path);
    restored.state = restored_buffer_state(buffer.state)?;
    restored.attachment = restored_buffer_attachment(buffer.attachment);
    restored.pty_size = buffer.pty_size;
    restored.activity = restored_activity(buffer.activity);
    restored.kind = restored_buffer_kind(buffer.kind);
    restored.created_at = timestamp_from_millis(buffer.created_at_ms)?;
    // Created by an earlier server: whatever request created it is long
    // done, so an exited restored buffer is cleaned up even if never shown.
    restored.ever_attached = true;
    Ok(restored)
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
                .map(|tab| TabEntry::new(tab.title, NodeId(tab.child)))
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
        PersistedBufferState::Running { pid } => BufferState::Interrupted(InterruptedBuffer {
            last_known_pid: pid,
        }),
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

fn persisted_buffer_kind(kind: &BufferKind) -> PersistedBufferKind {
    match kind {
        BufferKind::Pty => PersistedBufferKind::Pty,
        BufferKind::Helper(helper) => PersistedBufferKind::Helper {
            source_buffer_id: helper.source_buffer_id.0,
            scope: persisted_helper_scope(helper.scope),
            lines: Vec::new(),
        },
    }
}

fn restored_buffer_kind(kind: PersistedBufferKind) -> BufferKind {
    match kind {
        PersistedBufferKind::Pty => BufferKind::Pty,
        PersistedBufferKind::Helper {
            source_buffer_id,
            scope,
            lines,
        } => BufferKind::Helper(HelperBuffer {
            source_buffer_id: BufferId(source_buffer_id),
            scope: restored_helper_scope(scope),
            lines,
        }),
    }
}

fn persisted_helper_scope(scope: HelperBufferScope) -> PersistedHelperBufferScope {
    match scope {
        HelperBufferScope::Full => PersistedHelperBufferScope::Full,
        HelperBufferScope::Visible => PersistedHelperBufferScope::Visible,
    }
}

fn restored_helper_scope(scope: PersistedHelperBufferScope) -> HelperBufferScope {
    match scope {
        PersistedHelperBufferScope::Full => HelperBufferScope::Full,
        PersistedHelperBufferScope::Visible => HelperBufferScope::Visible,
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

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn save_workspace_writes_current_format_version() {
        let tempdir = tempdir().expect("tempdir");
        let workspace_path = tempdir.path().join("workspace.json");
        let mut state = ServerState::new();
        let _ = state.create_session("main");

        save_workspace(&workspace_path, &state).expect("workspace saves");

        let persisted: PersistedWorkspace =
            serde_json::from_slice(&fs::read(&workspace_path).expect("workspace bytes"))
                .expect("workspace json");
        assert_eq!(persisted.format_version, Some(CURRENT_FORMAT_VERSION));
    }

    #[test]
    fn only_changes_worth_saving_count_as_changes() {
        let mut state = ServerState::new();
        let _ = state.create_session("main");
        let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
        let saved = state.to_persisted();

        // Output: a new sequence number and activity.
        let buffer = state.buffers.get_mut(&buffer_id).expect("buffer");
        buffer.last_snapshot_seq += 100;
        buffer.activity = ActivityState::Bell;
        assert!(same_saved_state(&saved, &state.to_persisted()));
        let encoded = String::from_utf8(encode_workspace(&state).expect("encodes")).unwrap();
        assert!(
            !encoded.contains("last_snapshot_seq"),
            "output sequence numbers are not saved: {encoded}"
        );

        // A title is what a restore brings back.
        state.buffers.get_mut(&buffer_id).expect("buffer").title = "vim".to_owned();
        assert!(!same_saved_state(&saved, &state.to_persisted()));
    }

    #[test]
    fn session_viewport_round_trips_through_persistence() {
        let tempdir = tempdir().expect("tempdir");
        let workspace_path = tempdir.path().join("workspace.json");
        let mut state = ServerState::new();
        let session_id = state.create_session("main");
        let viewport = Size {
            width: 120,
            height: 40,
        };
        state
            .set_session_viewport(session_id, viewport)
            .expect("viewport is set");
        state
            .set_session_base_index(session_id, Some(1))
            .expect("base index is set");

        save_workspace(&workspace_path, &state).expect("workspace saves");
        let restored = load_workspace(&workspace_path)
            .expect("workspace loads")
            .expect("workspace exists");

        assert_eq!(
            restored.session(session_id).expect("session").viewport,
            Some(viewport)
        );
        assert_eq!(
            restored.session(session_id).expect("session").base_index,
            Some(1)
        );
    }

    #[test]
    fn load_current_workspace_migrates_unversioned_workspace() {
        let workspace = PersistedWorkspace {
            format_version: None,
            sessions: Vec::new(),
            buffers: Vec::new(),
            nodes: Vec::new(),
            floating: Vec::new(),
            next_session_id: 1,
            next_buffer_id: 1,
            next_node_id: 1,
            next_floating_id: 1,
        };

        let migrated = load_current_workspace(workspace).expect("legacy workspace migrates");
        assert_eq!(migrated.format_version, Some(CURRENT_FORMAT_VERSION));
    }

    #[test]
    fn load_current_workspace_migrates_v1_workspace() {
        let workspace: PersistedWorkspace = serde_json::from_str(
            r#"
            {
              "format_version": 1,
              "sessions": [
                {
                  "id": 1,
                  "name": "alpha",
                  "root_node": 10,
                  "floating": [],
                  "focused_leaf": 10,
                  "focused_floating": null,
                  "created_at_ms": 1234
                }
              ],
              "buffers": [
                {
                  "id": 20,
                  "title": "shell",
                  "command": ["sh"],
                  "cwd": null,
                  "env": {},
                  "runtime_socket_path": null,
                  "state": { "kind": "created" },
                  "attachment": { "kind": "detached" },
                  "pty_size": {
                    "cols": 80,
                    "rows": 24,
                    "pixel_width": 0,
                    "pixel_height": 0
                  },
                  "activity": "idle",
                  "last_snapshot_seq": 0,
                  "created_at_ms": 5678
                }
              ],
              "nodes": [
                {
                  "kind": "buffer_view",
                  "id": 10,
                  "session_id": 1,
                  "parent": null,
                  "buffer_id": 20,
                  "focused": true,
                  "zoomed": true,
                  "follow_output": true,
                  "last_render_size": {
                    "cols": 80,
                    "rows": 24,
                    "pixel_width": 0,
                    "pixel_height": 0
                  }
                }
              ],
              "floating": [],
              "next_session_id": 2,
              "next_buffer_id": 21,
              "next_node_id": 11,
              "next_floating_id": 1
            }
            "#,
        )
        .expect("deserialize v1 workspace fixture");

        let migrated = load_current_workspace(workspace).expect("v1 workspace migrates");
        assert_eq!(migrated.format_version, Some(CURRENT_FORMAT_VERSION));
        assert_eq!(migrated.sessions[0].zoomed_node, Some(10));
        assert_eq!(migrated.buffers[0].kind, PersistedBufferKind::Pty);
    }

    #[test]
    fn load_current_workspace_rejects_unknown_format_versions() {
        let workspace = PersistedWorkspace {
            format_version: Some(CURRENT_FORMAT_VERSION + 1),
            sessions: Vec::new(),
            buffers: Vec::new(),
            nodes: Vec::new(),
            floating: Vec::new(),
            next_session_id: 1,
            next_buffer_id: 1,
            next_node_id: 1,
            next_floating_id: 1,
        };

        let error = load_current_workspace(workspace).expect_err("unknown version should fail");
        assert_eq!(
            error.to_string(),
            format!(
                "internal error: unsupported workspace format version {}",
                CURRENT_FORMAT_VERSION + 1
            )
        );
    }

    #[test]
    fn helper_buffer_kind_round_trips_through_persistence() {
        for scope in [HelperBufferScope::Full, HelperBufferScope::Visible] {
            let kind = BufferKind::Helper(HelperBuffer {
                source_buffer_id: BufferId(42),
                scope,
                lines: vec!["alpha".to_owned(), "beta".to_owned()],
            });

            let persisted = persisted_buffer_kind(&kind);
            let restored = restored_buffer_kind(persisted.clone());

            assert_eq!(
                restored,
                BufferKind::Helper(HelperBuffer {
                    source_buffer_id: BufferId(42),
                    scope,
                    lines: Vec::new(),
                })
            );
            assert_eq!(restored_helper_scope(persisted_helper_scope(scope)), scope);
            assert_eq!(
                persisted,
                PersistedBufferKind::Helper {
                    source_buffer_id: 42,
                    scope: persisted_helper_scope(scope),
                    lines: Vec::new(),
                }
            );
        }
    }

    #[test]
    fn helper_buffer_kind_restores_legacy_lines_when_present() {
        let restored = restored_buffer_kind(PersistedBufferKind::Helper {
            source_buffer_id: 42,
            scope: PersistedHelperBufferScope::Visible,
            lines: vec!["alpha".to_owned(), "beta".to_owned()],
        });

        assert_eq!(
            restored,
            BufferKind::Helper(HelperBuffer {
                source_buffer_id: BufferId(42),
                scope: HelperBufferScope::Visible,
                lines: vec!["alpha".to_owned(), "beta".to_owned()],
            })
        );
    }

    #[test]
    fn load_current_workspace_rejects_duplicate_legacy_zoomed_nodes() {
        let workspace = PersistedWorkspace {
            format_version: Some(FIRST_VERSIONED_FORMAT_VERSION),
            sessions: vec![PersistedSession {
                id: 1,
                name: "alpha".to_owned(),
                root_node: 10,
                floating: Vec::new(),
                focused_leaf: Some(10),
                focused_floating: None,
                zoomed_node: None,
                viewport: None,
                base_index: None,
                created_at_ms: 1234,
            }],
            buffers: Vec::new(),
            nodes: vec![
                PersistedNode::BufferView {
                    id: 10,
                    session_id: 1,
                    parent: None,
                    buffer_id: 20,
                    focused: true,
                    zoomed: true,
                    follow_output: true,
                    last_render_size: PtySize::new(80, 24),
                },
                PersistedNode::BufferView {
                    id: 11,
                    session_id: 1,
                    parent: None,
                    buffer_id: 21,
                    focused: false,
                    zoomed: true,
                    follow_output: true,
                    last_render_size: PtySize::new(80, 24),
                },
            ],
            floating: Vec::new(),
            next_session_id: 2,
            next_buffer_id: 22,
            next_node_id: 12,
            next_floating_id: 1,
        };

        let error = load_current_workspace(workspace)
            .expect_err("duplicate legacy zoomed nodes should be rejected");
        assert!(
            error.to_string().contains("multiple legacy zoomed nodes"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn restored_running_buffers_become_interrupted() {
        let state = restored_buffer_state(PersistedBufferState::Running { pid: Some(42) })
            .expect("state restores");
        assert_eq!(
            state,
            BufferState::Interrupted(InterruptedBuffer {
                last_known_pid: Some(42),
            })
        );
    }
}
