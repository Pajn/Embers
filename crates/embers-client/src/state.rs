use std::collections::{BTreeMap, BTreeSet};

use embers_core::{BufferId, NodeId, SessionId};
use embers_protocol::NodeRecordKind;
use embers_protocol::{
    BufferRecord, ServerEvent, SessionRecord, SessionSnapshot, VisibleSnapshotResponse,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchState {
    pub query: String,
    pub active_match_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Character,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionPoint {
    pub line: u64,
    pub column: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionState {
    pub kind: SelectionKind,
    pub anchor: SelectionPoint,
    pub cursor: SelectionPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferViewState {
    pub buffer_id: BufferId,
    pub follow_output: bool,
    pub scroll_top_line: u64,
    pub visible_line_count: u16,
    pub total_line_count: u64,
    pub alternate_screen: bool,
    pub search_state: Option<SearchState>,
    pub selection_state: Option<SelectionState>,
}

impl Default for BufferViewState {
    fn default() -> Self {
        Self {
            buffer_id: BufferId(0),
            follow_output: true,
            scroll_top_line: 0,
            visible_line_count: 0,
            total_line_count: 0,
            alternate_screen: false,
            search_state: None,
            selection_state: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientState {
    pub sessions: BTreeMap<SessionId, SessionRecord>,
    pub buffers: BTreeMap<BufferId, BufferRecord>,
    pub nodes: BTreeMap<NodeId, embers_protocol::NodeRecord>,
    pub floating: BTreeMap<embers_core::FloatingId, embers_protocol::FloatingRecord>,
    pub snapshots: BTreeMap<BufferId, VisibleSnapshotResponse>,
    pub view_state: BTreeMap<NodeId, BufferViewState>,
    pub dirty_sessions: BTreeSet<SessionId>,
    pub invalidated_buffers: BTreeSet<BufferId>,
}

impl ClientState {
    pub fn apply_session_snapshot(&mut self, snapshot: SessionSnapshot) {
        let SessionSnapshot {
            session,
            nodes,
            buffers,
            floating,
        } = snapshot;
        let session_id = session.id;
        let previous_node_ids = self.session_node_ids(session_id);
        let previous_attached_buffers = self.attached_buffers_for_nodes(&previous_node_ids);
        let current_node_ids = nodes.iter().map(|node| node.id).collect::<BTreeSet<_>>();
        let current_buffer_ids = buffers
            .iter()
            .map(|buffer| buffer.id)
            .collect::<BTreeSet<_>>();
        let current_floating_ids = floating
            .iter()
            .map(|window| window.id)
            .collect::<BTreeSet<_>>();

        self.sessions.insert(session_id, session);
        self.nodes.retain(|node_id, node| {
            node.session_id != session_id || current_node_ids.contains(node_id)
        });
        self.floating.retain(|floating_id, window| {
            window.session_id != session_id || current_floating_ids.contains(floating_id)
        });
        self.view_state
            .retain(|node_id, _| !previous_node_ids.contains(node_id) || current_node_ids.contains(node_id));

        for node in nodes {
            self.nodes.insert(node.id, node);
        }

        for buffer in buffers {
            self.buffers.insert(buffer.id, buffer);
        }

        for window in floating {
            self.floating.insert(window.id, window);
        }

        for buffer_id in previous_attached_buffers.difference(&current_buffer_ids) {
            if let Some(buffer) = self.buffers.get_mut(buffer_id) {
                buffer.attachment_node_id = None;
            }
        }

        self.sync_view_states_for_nodes(&current_node_ids);
        self.dirty_sessions.remove(&session_id);
    }

    pub fn apply_detached_buffers(&mut self, buffers: Vec<BufferRecord>) {
        let current_detached = self
            .buffers
            .values()
            .filter(|buffer| buffer.attachment_node_id.is_none())
            .map(|buffer| buffer.id)
            .collect::<BTreeSet<_>>();
        let incoming_ids = buffers
            .iter()
            .map(|buffer| buffer.id)
            .collect::<BTreeSet<_>>();

        for buffer_id in current_detached.difference(&incoming_ids) {
            self.buffers.remove(buffer_id);
            self.snapshots.remove(buffer_id);
            self.view_state.retain(|_, state| state.buffer_id != *buffer_id);
            self.invalidated_buffers.remove(buffer_id);
        }

        for buffer in buffers {
            self.buffers.insert(buffer.id, buffer);
        }
    }

    pub fn apply_buffer_snapshot(&mut self, snapshot: VisibleSnapshotResponse) {
        if let Some(buffer) = self.buffers.get_mut(&snapshot.buffer_id) {
            buffer.last_snapshot_seq = snapshot.sequence;
            buffer.pty_size = snapshot.size;
            if let Some(title) = &snapshot.title {
                buffer.title = title.clone();
            }
            if let Some(cwd) = &snapshot.cwd {
                buffer.cwd = Some(cwd.clone());
            }
        }

        let buffer_id = snapshot.buffer_id;
        self.invalidated_buffers.remove(&snapshot.buffer_id);
        self.snapshots.insert(snapshot.buffer_id, snapshot);
        self.sync_view_states_for_buffer(buffer_id);
    }

    pub fn apply_event(&mut self, event: &ServerEvent) {
        match event {
            ServerEvent::SessionCreated(event) => {
                self.sessions
                    .insert(event.session.id, event.session.clone());
                self.dirty_sessions.insert(event.session.id);
            }
            ServerEvent::SessionClosed(event) => self.remove_session(event.session_id),
            ServerEvent::BufferCreated(event) => {
                self.buffers.insert(event.buffer.id, event.buffer.clone());
            }
            ServerEvent::BufferDetached(event) => {
                if let Some(buffer) = self.buffers.get_mut(&event.buffer_id) {
                    buffer.attachment_node_id = None;
                }
            }
            ServerEvent::NodeChanged(event) => {
                self.dirty_sessions.insert(event.session_id);
            }
            ServerEvent::FloatingChanged(event) => {
                self.dirty_sessions.insert(event.session_id);
            }
            ServerEvent::FocusChanged(event) => {
                if let Some(session) = self.sessions.get_mut(&event.session_id) {
                    session.focused_leaf_id = event.focused_leaf_id;
                    session.focused_floating_id = event.focused_floating_id;
                }
            }
            ServerEvent::RenderInvalidated(event) => {
                self.invalidated_buffers.insert(event.buffer_id);
            }
        }
    }

    pub fn remove_session(&mut self, session_id: SessionId) {
        let node_ids = self.session_node_ids(session_id);
        self.sessions.remove(&session_id);
        self.nodes.retain(|_, node| node.session_id != session_id);
        self.floating
            .retain(|_, window| window.session_id != session_id);
        self.view_state.retain(|node_id, _| !node_ids.contains(node_id));
        self.detach_buffers_for_nodes(&node_ids);
        self.dirty_sessions.remove(&session_id);
    }

    pub fn view_state(&self, node_id: NodeId) -> Option<&BufferViewState> {
        self.view_state.get(&node_id)
    }

    fn sync_view_states_for_buffer(&mut self, buffer_id: BufferId) {
        let node_ids = self
            .nodes
            .values()
            .filter(|node| {
                matches!(node.kind, NodeRecordKind::BufferView)
                    && node
                        .buffer_view
                        .as_ref()
                        .is_some_and(|view| view.buffer_id == buffer_id)
            })
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let node_ids = node_ids.into_iter().collect::<BTreeSet<_>>();
        self.sync_view_states_for_nodes(&node_ids);
    }

    fn sync_view_states_for_nodes(&mut self, node_ids: &BTreeSet<NodeId>) {
        for node_id in node_ids {
            let Some(node) = self.nodes.get(node_id) else {
                continue;
            };
            if node.kind != NodeRecordKind::BufferView {
                continue;
            }
            let Some(buffer_view) = node.buffer_view.as_ref() else {
                continue;
            };
            let snapshot = self.snapshots.get(&buffer_view.buffer_id);
            let visible_line_count = buffer_view.last_render_size.rows;
            let total_line_count = snapshot
                .map(|snapshot| snapshot.total_lines.max(u64::from(visible_line_count)))
                .unwrap_or_else(|| u64::from(visible_line_count));
            let alternate_screen = snapshot.is_some_and(|snapshot| snapshot.alternate_screen);
            let initial_top_line = snapshot
                .map(|snapshot| snapshot.viewport_top_line)
                .unwrap_or_else(|| bottom_top_line(total_line_count, visible_line_count));

            match self.view_state.get_mut(node_id) {
                Some(state) if state.buffer_id == buffer_view.buffer_id => {
                    state.visible_line_count = visible_line_count;
                    state.total_line_count = total_line_count;
                    state.alternate_screen = alternate_screen;
                    if !alternate_screen {
                        state.scroll_top_line = if state.follow_output {
                            bottom_top_line(total_line_count, visible_line_count)
                        } else {
                            clamp_top_line(
                                state.scroll_top_line,
                                total_line_count,
                                visible_line_count,
                            )
                        };
                    }
                }
                Some(state) => {
                    let search_state = state.search_state.clone();
                    let selection_state = state.selection_state.clone();
                    *state = BufferViewState {
                        buffer_id: buffer_view.buffer_id,
                        follow_output: buffer_view.follow_output,
                        scroll_top_line: initial_top_line,
                        visible_line_count,
                        total_line_count,
                        alternate_screen,
                        search_state,
                        selection_state,
                    };
                }
                None => {
                    self.view_state.insert(
                        *node_id,
                        BufferViewState {
                            buffer_id: buffer_view.buffer_id,
                            follow_output: buffer_view.follow_output,
                            scroll_top_line: initial_top_line,
                            visible_line_count,
                            total_line_count,
                            alternate_screen,
                            search_state: None,
                            selection_state: None,
                        },
                    );
                }
            }
        }
    }

    fn session_node_ids(&self, session_id: SessionId) -> BTreeSet<NodeId> {
        self.nodes
            .values()
            .filter(|node| node.session_id == session_id)
            .map(|node| node.id)
            .collect()
    }

    fn attached_buffers_for_nodes(&self, node_ids: &BTreeSet<NodeId>) -> BTreeSet<BufferId> {
        self.buffers
            .values()
            .filter_map(|buffer| {
                let node_id = buffer.attachment_node_id?;
                node_ids.contains(&node_id).then_some(buffer.id)
            })
            .collect()
    }

    fn detach_buffers_for_nodes(&mut self, node_ids: &BTreeSet<NodeId>) {
        for buffer in self.buffers.values_mut() {
            if buffer
                .attachment_node_id
                .is_some_and(|node_id| node_ids.contains(&node_id))
            {
                buffer.attachment_node_id = None;
            }
        }
    }
}

fn bottom_top_line(total_line_count: u64, visible_line_count: u16) -> u64 {
    total_line_count.saturating_sub(u64::from(visible_line_count))
}

fn clamp_top_line(scroll_top_line: u64, total_line_count: u64, visible_line_count: u16) -> u64 {
    scroll_top_line.min(bottom_top_line(total_line_count, visible_line_count))
}
