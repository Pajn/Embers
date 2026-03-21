use std::collections::{BTreeMap, BTreeSet};

use embers_core::{BufferId, NodeId, SessionId};
use embers_protocol::{
    BufferRecord, ServerEvent, SessionRecord, SessionSnapshot, SnapshotResponse,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientState {
    pub sessions: BTreeMap<SessionId, SessionRecord>,
    pub buffers: BTreeMap<BufferId, BufferRecord>,
    pub nodes: BTreeMap<NodeId, embers_protocol::NodeRecord>,
    pub floating: BTreeMap<embers_core::FloatingId, embers_protocol::FloatingRecord>,
    pub snapshots: BTreeMap<BufferId, SnapshotResponse>,
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
            self.invalidated_buffers.remove(buffer_id);
        }

        for buffer in buffers {
            self.buffers.insert(buffer.id, buffer);
        }
    }

    pub fn apply_buffer_snapshot(&mut self, snapshot: SnapshotResponse) {
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

        self.invalidated_buffers.remove(&snapshot.buffer_id);
        self.snapshots.insert(snapshot.buffer_id, snapshot);
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
        self.detach_buffers_for_nodes(&node_ids);
        self.dirty_sessions.remove(&session_id);
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
