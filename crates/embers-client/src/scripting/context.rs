use std::collections::{BTreeMap, BTreeSet};

use embers_core::{ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, Rect, SessionId};
use embers_protocol::{BufferRecordState, NodeRecordKind};

use crate::input::NORMAL_MODE;
use crate::{ClientState, PresentationModel, TabsFrame};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Context {
    current_mode: String,
    event: Option<EventInfo>,
    current_session_id: Option<SessionId>,
    current_node_id: Option<NodeId>,
    current_buffer_id: Option<BufferId>,
    current_floating_id: Option<FloatingId>,
    sessions: BTreeMap<SessionId, SessionRef>,
    buffers: BTreeMap<BufferId, BufferRef>,
    nodes: BTreeMap<NodeId, NodeRef>,
    floating: BTreeMap<FloatingId, FloatingRef>,
}

impl Context {
    pub fn from_state(state: &ClientState, presentation: Option<&PresentationModel>) -> Self {
        Self::from_state_with_mode(state, presentation, NORMAL_MODE)
    }

    pub fn from_state_with_mode(
        state: &ClientState,
        presentation: Option<&PresentationModel>,
        current_mode: impl Into<String>,
    ) -> Self {
        let current_mode = current_mode.into();
        let current_session_id = presentation.map(|presentation| presentation.session_id);
        let current_node_id = presentation
            .and_then(|presentation| presentation.focused_leaf())
            .map(|leaf| leaf.node_id);
        let current_buffer_id = presentation.and_then(PresentationModel::focused_buffer_id);
        let current_floating_id = presentation.and_then(PresentationModel::focused_floating_id);

        let visible_buffer_ids = presentation
            .map(|presentation| {
                presentation
                    .leaves
                    .iter()
                    .map(|leaf| leaf.buffer_id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let geometry_by_node = presentation.map(geometry_by_node).unwrap_or_default();
        let visible_node_ids = visible_node_ids(state, &geometry_by_node);
        let focused_leaf_ids = state
            .sessions
            .values()
            .filter_map(|session| session.focused_leaf_id)
            .collect::<BTreeSet<_>>();
        let session_root_ids = state
            .sessions
            .values()
            .map(|session| session.root_node_id)
            .collect::<BTreeSet<_>>();
        let floating_root_ids = state
            .floating
            .values()
            .map(|floating| floating.root_node_id)
            .collect::<BTreeSet<_>>();

        let sessions = state
            .sessions
            .values()
            .map(|session| {
                (
                    session.id,
                    SessionRef {
                        id: session.id,
                        name: session.name.clone(),
                        root_node_id: session.root_node_id,
                        floating_ids: session.floating_ids.clone(),
                        focused_leaf_id: session.focused_leaf_id,
                        focused_floating_id: session.focused_floating_id,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let nodes = state
            .nodes
            .values()
            .map(|node| {
                let child_ids = node
                    .split
                    .as_ref()
                    .map(|split| split.child_ids.clone())
                    .or_else(|| {
                        node.tabs
                            .as_ref()
                            .map(|tabs| tabs.tabs.iter().map(|tab| tab.child_id).collect())
                    })
                    .unwrap_or_default();
                let buffer_id = node
                    .buffer_view
                    .as_ref()
                    .map(|buffer_view| buffer_view.buffer_id);
                let split_direction = node.split.as_ref().map(|split| split.direction);
                let split_weights = node.split.as_ref().map(|split| split.sizes.clone());
                let active_tab_index = node.tabs.as_ref().map(|tabs| tabs.active);
                let tab_titles = node
                    .tabs
                    .as_ref()
                    .map(|tabs| tabs.tabs.iter().map(|tab| tab.title.clone()).collect())
                    .unwrap_or_default();
                (
                    node.id,
                    NodeRef {
                        id: node.id,
                        session_id: node.session_id,
                        kind: node.kind,
                        parent_id: node.parent_id,
                        child_ids,
                        geometry: geometry_by_node.get(&node.id).copied(),
                        is_root: session_root_ids.contains(&node.id),
                        is_floating_root: floating_root_ids.contains(&node.id),
                        is_focused: focused_leaf_ids.contains(&node.id),
                        visible: visible_node_ids.contains(&node.id),
                        buffer_id,
                        split_direction,
                        split_weights,
                        active_tab_index,
                        tab_titles,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let buffers = state
            .buffers
            .values()
            .map(|buffer| {
                let session_id = buffer
                    .attachment_node_id
                    .and_then(|node_id| state.nodes.get(&node_id).map(|node| node.session_id));
                let snapshot_lines = state
                    .snapshots
                    .get(&buffer.id)
                    .map(|snapshot| snapshot.lines.clone())
                    .unwrap_or_default();
                (
                    buffer.id,
                    BufferRef {
                        id: buffer.id,
                        title: buffer.title.clone(),
                        command: buffer.command.clone(),
                        cwd: buffer.cwd.clone(),
                        pid: buffer.pid,
                        env: buffer.env.clone(),
                        state: buffer.state,
                        activity: buffer.activity,
                        attachment_node_id: buffer.attachment_node_id,
                        session_id,
                        visible: visible_buffer_ids.contains(&buffer.id),
                        exit_code: buffer.exit_code,
                        tty_path: None,
                        snapshot_lines,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let floating = state
            .floating
            .values()
            .map(|window| {
                (
                    window.id,
                    FloatingRef {
                        id: window.id,
                        session_id: window.session_id,
                        root_node_id: window.root_node_id,
                        title: window.title.clone(),
                        geometry: window.geometry,
                        focused: window.focused,
                        visible: window.visible,
                        close_on_empty: window.close_on_empty,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            current_mode,
            event: None,
            current_session_id,
            current_node_id,
            current_buffer_id,
            current_floating_id,
            sessions,
            buffers,
            nodes,
            floating,
        }
    }

    pub fn with_event(mut self, event: EventInfo) -> Self {
        self.event = Some(event);
        self
    }

    pub fn current_mode(&self) -> &str {
        &self.current_mode
    }

    pub fn event(&self) -> Option<EventInfo> {
        self.event.clone()
    }

    pub fn current_session(&self) -> Option<SessionRef> {
        self.current_session_id
            .and_then(|session_id| self.sessions.get(&session_id).cloned())
    }

    pub fn current_node(&self) -> Option<NodeRef> {
        self.current_node_id
            .and_then(|node_id| self.nodes.get(&node_id).cloned())
    }

    pub fn current_buffer(&self) -> Option<BufferRef> {
        self.current_buffer_id
            .and_then(|buffer_id| self.buffers.get(&buffer_id).cloned())
    }

    pub fn current_floating(&self) -> Option<FloatingRef> {
        self.current_floating_id
            .and_then(|floating_id| self.floating.get(&floating_id).cloned())
    }

    pub fn sessions(&self) -> Vec<SessionRef> {
        self.sessions.values().cloned().collect()
    }

    pub fn find_buffer(&self, buffer_id: BufferId) -> Option<BufferRef> {
        self.buffers.get(&buffer_id).cloned()
    }

    pub fn find_node(&self, node_id: NodeId) -> Option<NodeRef> {
        self.nodes.get(&node_id).cloned()
    }

    pub fn find_floating(&self, floating_id: FloatingId) -> Option<FloatingRef> {
        self.floating.get(&floating_id).cloned()
    }

    pub fn detached_buffers(&self) -> Vec<BufferRef> {
        self.buffers
            .values()
            .filter(|buffer| buffer.is_detached())
            .cloned()
            .collect()
    }

    pub fn visible_buffers(&self) -> Vec<BufferRef> {
        self.buffers
            .values()
            .filter(|buffer| buffer.visible)
            .cloned()
            .collect()
    }

    pub fn visible_floating(&self) -> Vec<FloatingRef> {
        self.floating
            .values()
            .filter(|floating| floating.visible)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventInfo {
    pub name: String,
    pub session_id: Option<SessionId>,
    pub buffer_id: Option<BufferId>,
    pub node_id: Option<NodeId>,
    pub floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRef {
    pub id: SessionId,
    pub name: String,
    pub root_node_id: NodeId,
    pub floating_ids: Vec<FloatingId>,
    pub focused_leaf_id: Option<NodeId>,
    pub focused_floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferRef {
    pub id: BufferId,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    pub env: BTreeMap<String, String>,
    pub state: BufferRecordState,
    pub activity: ActivityState,
    pub attachment_node_id: Option<NodeId>,
    pub session_id: Option<SessionId>,
    pub visible: bool,
    pub exit_code: Option<i32>,
    pub tty_path: Option<String>,
    pub snapshot_lines: Vec<String>,
}

impl BufferRef {
    pub fn node_id(&self) -> Option<NodeId> {
        self.attachment_node_id
    }

    pub fn process_name(&self) -> Option<String> {
        let command = self.command.first()?;
        Some(
            std::path::Path::new(command)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(command)
                .to_owned(),
        )
    }

    pub fn env_hint(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    pub fn snapshot_text(&self, limit: usize) -> String {
        if limit == 0 {
            return String::new();
        }
        let start = self.snapshot_lines.len().saturating_sub(limit);
        self.snapshot_lines[start..].join("\n")
    }

    pub fn history_text(&self) -> String {
        self.snapshot_lines.join("\n")
    }

    pub fn is_attached(&self) -> bool {
        self.attachment_node_id.is_some()
    }

    pub fn is_detached(&self) -> bool {
        self.attachment_node_id.is_none()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, BufferRecordState::Running)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub id: NodeId,
    pub session_id: SessionId,
    pub kind: NodeRecordKind,
    pub parent_id: Option<NodeId>,
    pub child_ids: Vec<NodeId>,
    pub geometry: Option<Rect>,
    pub is_root: bool,
    pub is_floating_root: bool,
    pub is_focused: bool,
    pub visible: bool,
    pub buffer_id: Option<BufferId>,
    pub split_direction: Option<embers_core::SplitDirection>,
    pub split_weights: Option<Vec<u16>>,
    pub active_tab_index: Option<u32>,
    pub tab_titles: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingRef {
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
pub struct TabBarContext {
    pub node_id: NodeId,
    pub is_root: bool,
    pub active: usize,
    pub mode: String,
    pub viewport_width: u16,
    pub tabs: Vec<TabInfo>,
}

impl TabBarContext {
    pub fn from_frame(frame: &TabsFrame, mode: impl Into<String>, viewport_width: u16) -> Self {
        Self {
            node_id: frame.node_id,
            is_root: frame.is_root,
            active: frame.active,
            mode: mode.into(),
            viewport_width,
            tabs: frame
                .tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| TabInfo {
                    index,
                    title: tab.title.clone(),
                    active: tab.active,
                    has_activity: matches!(
                        tab.activity,
                        ActivityState::Activity | ActivityState::Bell
                    ),
                    has_bell: matches!(tab.activity, ActivityState::Bell),
                    buffer_count: tab.buffer_count,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub index: usize,
    pub title: String,
    pub active: bool,
    pub has_activity: bool,
    pub has_bell: bool,
    pub buffer_count: usize,
}

fn geometry_by_node(presentation: &PresentationModel) -> BTreeMap<NodeId, Rect> {
    let mut geometry = BTreeMap::new();
    for tabs in &presentation.tab_bars {
        geometry.insert(tabs.node_id, tabs.rect);
    }
    for leaf in &presentation.leaves {
        geometry.insert(leaf.node_id, leaf.rect);
    }
    geometry
}

fn visible_node_ids(
    state: &ClientState,
    geometry_by_node: &BTreeMap<NodeId, Rect>,
) -> BTreeSet<NodeId> {
    let mut visible = BTreeSet::new();
    for node_id in geometry_by_node.keys().copied() {
        let mut current = Some(node_id);
        while let Some(node_id) = current {
            if !visible.insert(node_id) {
                break;
            }
            current = state.nodes.get(&node_id).and_then(|node| node.parent_id);
        }
    }
    visible
}
