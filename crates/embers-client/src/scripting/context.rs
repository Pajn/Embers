use std::collections::{BTreeMap, BTreeSet};

use embers_core::{ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, Rect, SessionId};
use embers_protocol::{BufferRecordState, NodeRecordKind};

use crate::{ClientState, PresentationModel, TabsFrame};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Context {
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
        let visible_node_ids = geometry_by_node.keys().copied().collect::<BTreeSet<_>>();

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
                let tab_titles = node
                    .tabs
                    .as_ref()
                    .map(|tabs| tabs.tabs.iter().map(|tab| tab.title.clone()).collect())
                    .unwrap_or_default();
                let buffer_id = node.buffer_view.as_ref().map(|buffer_view| buffer_view.buffer_id);
                (
                    node.id,
                    NodeRef {
                        id: node.id,
                        session_id: node.session_id,
                        parent_id: node.parent_id,
                        kind: node.kind,
                        child_ids,
                        geometry: geometry_by_node.get(&node.id).copied(),
                        tab_titles,
                        active_tab: node.tabs.as_ref().map(|tabs| tabs.active),
                        buffer_id,
                        visible: visible_node_ids.contains(&node.id),
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
                (
                    buffer.id,
                    BufferRef {
                        id: buffer.id,
                        title: buffer.title.clone(),
                        command: buffer.command.clone(),
                        cwd: buffer.cwd.clone(),
                        state: buffer.state,
                        attachment_node_id: buffer.attachment_node_id,
                        activity: buffer.activity,
                        exit_code: buffer.exit_code,
                        session_id,
                        visible: visible_buffer_ids.contains(&buffer.id),
                        detached: buffer.attachment_node_id.is_none(),
                        tty_path: None,
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

    pub fn detached_buffers(&self) -> Vec<BufferRef> {
        self.buffers
            .values()
            .filter(|buffer| buffer.detached)
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
            .filter(|window| window.visible)
            .cloned()
            .collect()
    }
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
    pub state: BufferRecordState,
    pub attachment_node_id: Option<NodeId>,
    pub activity: ActivityState,
    pub exit_code: Option<i32>,
    pub session_id: Option<SessionId>,
    pub visible: bool,
    pub detached: bool,
    pub tty_path: Option<String>,
}

impl BufferRef {
    pub fn process_name(&self) -> Option<String> {
        self.command.first().map(|command| {
            command
                .rsplit('/')
                .next()
                .unwrap_or(command)
                .to_owned()
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeRecordKind,
    pub child_ids: Vec<NodeId>,
    pub geometry: Option<Rect>,
    pub tab_titles: Vec<String>,
    pub active_tab: Option<usize>,
    pub buffer_id: Option<BufferId>,
    pub visible: bool,
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
    pub tabs: Vec<TabStateRef>,
}

impl TabBarContext {
    pub fn from_frame(frame: &TabsFrame) -> Self {
        Self {
            node_id: frame.node_id,
            is_root: frame.is_root,
            active: frame.active,
            tabs: frame
                .tabs
                .iter()
                .map(|tab| TabStateRef {
                    title: tab.title.clone(),
                    active: tab.active,
                    activity: tab.activity,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabStateRef {
    pub title: String,
    pub active: bool,
    pub activity: ActivityState,
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
