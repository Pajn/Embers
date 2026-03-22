use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, IdAllocator, MuxError, NodeId, PtySize,
    Result, SessionId, SplitDirection, Timestamp,
};
use embers_protocol::NodeJoinPlacement;

use crate::model::{
    Buffer, BufferAttachment, BufferKind, BufferState, BufferViewNode, BufferViewState,
    ExitedBuffer, FloatingWindow, HelperBuffer, HelperBufferScope, InterruptedBuffer, Node,
    RunningBuffer, Session, SplitNode, TabEntry, TabsNode,
};
use crate::persist::{
    CURRENT_FORMAT_VERSION, PersistedWorkspace, persisted_buffer, persisted_floating,
    persisted_node, persisted_session, restored_buffer, restored_floating, restored_node,
    restored_session,
};

#[derive(Debug)]
pub struct ServerState {
    pub sessions: BTreeMap<SessionId, Session>,
    pub buffers: BTreeMap<BufferId, Buffer>,
    pub nodes: BTreeMap<NodeId, Node>,
    pub floating: BTreeMap<FloatingId, FloatingWindow>,
    session_ids: IdAllocator<SessionId>,
    buffer_ids: IdAllocator<BufferId>,
    node_ids: IdAllocator<NodeId>,
    floating_ids: IdAllocator<FloatingId>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            buffers: BTreeMap::new(),
            nodes: BTreeMap::new(),
            floating: BTreeMap::new(),
            session_ids: IdAllocator::new(1),
            buffer_ids: IdAllocator::new(1),
            node_ids: IdAllocator::new(1),
            floating_ids: IdAllocator::new(1),
        }
    }

    pub fn from_persisted(workspace: PersistedWorkspace) -> Result<Self> {
        let PersistedWorkspace {
            format_version: _,
            sessions: persisted_sessions,
            buffers: persisted_buffers,
            nodes: persisted_nodes,
            floating: persisted_floating,
            next_session_id,
            next_buffer_id,
            next_node_id,
            next_floating_id,
        } = workspace;

        let mut sessions = BTreeMap::new();
        for persisted_session in persisted_sessions {
            let session = restored_session(persisted_session)?;
            if sessions.insert(session.id, session).is_some() {
                return Err(MuxError::internal(
                    "duplicate session id found while loading persisted workspace",
                ));
            }
        }

        let mut buffers = BTreeMap::new();
        for persisted_buffer in persisted_buffers {
            let buffer = restored_buffer(persisted_buffer)?;
            if buffers.insert(buffer.id, buffer).is_some() {
                return Err(MuxError::internal(
                    "duplicate buffer id found while loading persisted workspace",
                ));
            }
        }

        let mut nodes = BTreeMap::new();
        for persisted_node in persisted_nodes {
            let node = restored_node(persisted_node);
            let node_id = node.id();
            if nodes.insert(node_id, node).is_some() {
                return Err(MuxError::internal(
                    "duplicate node id found while loading persisted workspace",
                ));
            }
        }

        let mut floating = BTreeMap::new();
        for persisted_window in persisted_floating {
            let window = restored_floating(persisted_window);
            if floating.insert(window.id, window).is_some() {
                return Err(MuxError::internal(
                    "duplicate floating id found while loading persisted workspace",
                ));
            }
        }

        for (floating_id, window) in &floating {
            let Some(session) = sessions.get(&window.session_id) else {
                return Err(MuxError::internal(format!(
                    "floating window {floating_id} references unknown session {}",
                    window.session_id
                )));
            };
            if !session.floating.contains(floating_id) {
                return Err(MuxError::internal(format!(
                    "floating window {floating_id} is not referenced by session {}",
                    window.session_id
                )));
            }
        }

        let safe_next_session_id = next_id_after_max(sessions.keys().map(|id| id.0));
        let safe_next_buffer_id = next_id_after_max(buffers.keys().map(|id| id.0));
        let safe_next_node_id = next_id_after_max(nodes.keys().map(|id| id.0));
        let safe_next_floating_id = next_id_after_max(floating.keys().map(|id| id.0));

        let state = Self {
            sessions,
            buffers,
            nodes,
            floating,
            session_ids: IdAllocator::new(next_session_id.max(safe_next_session_id)),
            buffer_ids: IdAllocator::new(next_buffer_id.max(safe_next_buffer_id)),
            node_ids: IdAllocator::new(next_node_id.max(safe_next_node_id)),
            floating_ids: IdAllocator::new(next_floating_id.max(safe_next_floating_id)),
        };
        state.validate()?;
        Ok(state)
    }

    pub fn to_persisted(&self) -> PersistedWorkspace {
        PersistedWorkspace {
            format_version: Some(CURRENT_FORMAT_VERSION),
            sessions: self.sessions.values().map(persisted_session).collect(),
            buffers: self.buffers.values().map(persisted_buffer).collect(),
            nodes: self.nodes.values().map(persisted_node).collect(),
            floating: self.floating.values().map(persisted_floating).collect(),
            next_session_id: next_id_after_max(self.sessions.keys().map(|id| id.0)),
            next_buffer_id: next_id_after_max(self.buffers.keys().map(|id| id.0)),
            next_node_id: next_id_after_max(self.nodes.keys().map(|id| id.0)),
            next_floating_id: next_id_after_max(self.floating.keys().map(|id| id.0)),
        }
    }

    pub fn session(&self, session_id: SessionId) -> Result<&Session> {
        self.sessions
            .get(&session_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown session {session_id}")))
    }

    pub fn buffer(&self, buffer_id: BufferId) -> Result<&Buffer> {
        self.buffers
            .get(&buffer_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown buffer {buffer_id}")))
    }

    pub fn node(&self, node_id: NodeId) -> Result<&Node> {
        self.nodes
            .get(&node_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown node {node_id}")))
    }

    pub fn floating_window(&self, floating_id: FloatingId) -> Result<&FloatingWindow> {
        self.floating
            .get(&floating_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown floating window {floating_id}")))
    }

    pub fn root_node(&self, session_id: SessionId) -> Result<NodeId> {
        Ok(self.session(session_id)?.root_node)
    }

    pub fn root_tabs(&self, session_id: SessionId) -> Result<NodeId> {
        let root_node = self.root_node(session_id)?;
        if matches!(self.node(root_node)?, Node::Tabs(_)) {
            Ok(root_node)
        } else {
            Err(MuxError::conflict(format!(
                "session {session_id} root node {root_node} is not tabs"
            )))
        }
    }

    fn root_tabs_node(&self, session_id: SessionId) -> Result<Option<NodeId>> {
        let root_node = self.root_node(session_id)?;
        Ok(matches!(self.node(root_node)?, Node::Tabs(_)).then_some(root_node))
    }

    fn ensure_root_tabs_container(&mut self, session_id: SessionId) -> Result<NodeId> {
        if let Some(root_tabs) = self.root_tabs_node(session_id)? {
            return Ok(root_tabs);
        }

        let root_node = self.root_node(session_id)?;
        let title = self.default_tab_title(root_node)?;
        self.wrap_node_in_tabs(root_node, title)
    }

    pub fn add_root_tab_from_buffer(
        &mut self,
        session_id: SessionId,
        title: impl Into<String>,
        buffer_id: BufferId,
    ) -> Result<usize> {
        let root_tabs = self.ensure_root_tabs_container(session_id)?;
        let child = self.create_buffer_view(session_id, buffer_id)?;
        match self.add_tab_sibling(root_tabs, title, child) {
            Ok(index) => Ok(index),
            Err(error) => {
                self.discard_buffer_view(child);
                Err(error)
            }
        }
    }

    pub fn add_root_tab_from_subtree(
        &mut self,
        session_id: SessionId,
        title: impl Into<String>,
        child: NodeId,
    ) -> Result<usize> {
        let root_tabs = self.ensure_root_tabs_container(session_id)?;
        self.add_tab_sibling(root_tabs, title, child)
    }

    pub fn select_root_tab(&mut self, session_id: SessionId, index: usize) -> Result<()> {
        if let Some(root_tabs) = self.root_tabs_node(session_id)? {
            return self.switch_tab(root_tabs, index);
        }

        if index == 0 {
            Ok(())
        } else {
            Err(MuxError::not_found(format!(
                "tab index {index} is out of range for session {session_id}"
            )))
        }
    }

    pub fn rename_root_tab(
        &mut self,
        session_id: SessionId,
        index: usize,
        title: impl Into<String>,
    ) -> Result<()> {
        let root_tabs = if let Some(root_tabs) = self.root_tabs_node(session_id)? {
            root_tabs
        } else {
            if index != 0 {
                return Err(MuxError::not_found(format!(
                    "tab index {index} is out of range for session {session_id}"
                )));
            }
            self.ensure_root_tabs_container(session_id)?
        };
        self.rename_tab(root_tabs, index, title)
    }

    pub fn close_root_tab(&mut self, session_id: SessionId, index: usize) -> Result<()> {
        if let Some(root_tabs) = self.root_tabs_node(session_id)? {
            return self.close_tab(root_tabs, index);
        }

        if index == 0 {
            self.clear_session_root(session_id)
        } else {
            Err(MuxError::not_found(format!(
                "tab index {index} is out of range for session {session_id}"
            )))
        }
    }

    pub fn close_session(&mut self, session_id: SessionId) -> Result<()> {
        let session = self.session(session_id)?.clone();
        for floating_id in session.floating.clone() {
            self.close_floating(floating_id)?;
        }
        self.remove_subtree_nodes(session.root_node)?;
        self.sessions.remove(&session_id);
        Ok(())
    }

    pub fn rename_session(&mut self, session_id: SessionId, name: impl Into<String>) -> Result<()> {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return Err(MuxError::invalid_input(
                "session name cannot be empty or whitespace",
            ));
        }
        self.session_mut(session_id)?.name = name;
        Ok(())
    }

    pub fn create_session(&mut self, name: impl Into<String>) -> SessionId {
        let session_id = self.session_ids.next();
        let root_node = self.node_ids.next();
        self.nodes.insert(
            root_node,
            Node::Tabs(TabsNode {
                id: root_node,
                session_id,
                parent: None,
                tabs: Vec::new(),
                active: 0,
                last_focused_descendant: None,
            }),
        );
        self.sessions.insert(
            session_id,
            Session {
                id: session_id,
                name: name.into(),
                root_node,
                floating: Vec::new(),
                focused_leaf: None,
                focused_floating: None,
                zoomed_node: None,
                created_at: Timestamp::now(),
            },
        );
        session_id
    }

    pub fn create_buffer(
        &mut self,
        title: impl Into<String>,
        command: Vec<String>,
        cwd: Option<PathBuf>,
    ) -> BufferId {
        self.create_buffer_with_env(title, command, cwd, BTreeMap::new())
    }

    pub fn create_buffer_with_env(
        &mut self,
        title: impl Into<String>,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> BufferId {
        let buffer_id = self.buffer_ids.next();
        self.buffers
            .insert(buffer_id, Buffer::new(buffer_id, title, command, cwd, env));
        buffer_id
    }

    pub fn create_helper_buffer(
        &mut self,
        title: impl Into<String>,
        source_buffer_id: BufferId,
        scope: HelperBufferScope,
        cwd: Option<PathBuf>,
        lines: Vec<String>,
    ) -> BufferId {
        let buffer_id = self.buffer_ids.next();
        let rows = u16::try_from(lines.len().max(1)).unwrap_or(u16::MAX);
        let mut buffer = Buffer::new(buffer_id, title, Vec::new(), cwd, BTreeMap::new());
        buffer.pty_size = PtySize::new(80, rows);
        buffer.last_snapshot_seq = 1;
        buffer.kind = BufferKind::Helper(HelperBuffer {
            source_buffer_id,
            scope,
            lines,
        });
        self.buffers.insert(buffer_id, buffer);
        buffer_id
    }

    pub fn remove_buffer(&mut self, buffer_id: BufferId) -> Result<Buffer> {
        let buffer = self.buffer(buffer_id)?.clone();
        if !matches!(buffer.attachment, BufferAttachment::Detached) {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} must be detached before removal"
            )));
        }
        self.buffers
            .remove(&buffer_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown buffer {buffer_id}")))
    }

    pub fn mark_buffer_running(&mut self, buffer_id: BufferId, pid: Option<u32>) -> Result<()> {
        let buffer = self.buffer_mut(buffer_id)?;
        if matches!(buffer.state, BufferState::Exited(_)) {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} has already exited"
            )));
        }
        buffer.state = BufferState::Running(RunningBuffer { pid });
        Ok(())
    }

    pub fn set_buffer_runtime_socket_path(
        &mut self,
        buffer_id: BufferId,
        runtime_socket_path: Option<PathBuf>,
    ) -> Result<()> {
        self.buffer_mut(buffer_id)?
            .set_runtime_socket_path(runtime_socket_path);
        Ok(())
    }

    pub fn mark_buffer_interrupted(&mut self, buffer_id: BufferId, pid: Option<u32>) -> Result<()> {
        let buffer = self.buffer_mut(buffer_id)?;
        if matches!(buffer.state, BufferState::Exited(_)) {
            return Ok(());
        }
        buffer.state = BufferState::Interrupted(InterruptedBuffer {
            last_known_pid: pid,
        });
        Ok(())
    }

    pub fn mark_buffer_exited(
        &mut self,
        buffer_id: BufferId,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let buffer = self.buffer_mut(buffer_id)?;
        buffer.state = BufferState::Exited(ExitedBuffer {
            exit_code,
            exited_at: Timestamp::now(),
        });
        Ok(())
    }

    pub fn interrupt_unrecoverable_buffers(&mut self) {
        for buffer in self.buffers.values_mut() {
            buffer.state = match &buffer.state {
                BufferState::Exited(exited) => BufferState::Exited(exited.clone()),
                BufferState::Running(running) => BufferState::Interrupted(InterruptedBuffer {
                    last_known_pid: running.pid,
                }),
                BufferState::Interrupted(interrupted) => {
                    BufferState::Interrupted(interrupted.clone())
                }
                BufferState::Created => BufferState::Interrupted(InterruptedBuffer {
                    last_known_pid: None,
                }),
            };
        }
    }

    pub fn set_buffer_size(&mut self, buffer_id: BufferId, size: PtySize) -> Result<()> {
        self.buffer_mut(buffer_id)?.pty_size = size;
        Ok(())
    }

    pub fn note_buffer_output(&mut self, buffer_id: BufferId) -> Result<u64> {
        let buffer = self.buffer_mut(buffer_id)?;
        buffer.last_snapshot_seq = buffer.last_snapshot_seq.saturating_add(1);
        buffer.activity = ActivityState::Activity;
        Ok(buffer.last_snapshot_seq)
    }

    pub fn set_buffer_title(
        &mut self,
        buffer_id: BufferId,
        title: impl Into<String>,
    ) -> Result<()> {
        self.buffer_mut(buffer_id)?.title = title.into();
        Ok(())
    }

    pub fn set_buffer_activity(
        &mut self,
        buffer_id: BufferId,
        activity: ActivityState,
    ) -> Result<()> {
        self.buffer_mut(buffer_id)?.activity = activity;
        Ok(())
    }

    pub fn create_buffer_view(
        &mut self,
        session_id: SessionId,
        buffer_id: BufferId,
    ) -> Result<NodeId> {
        self.ensure_session_exists(session_id)?;
        self.buffer(buffer_id)?;

        let node_id = self.node_ids.next();
        self.nodes.insert(
            node_id,
            Node::BufferView(BufferViewNode {
                id: node_id,
                session_id,
                parent: None,
                buffer_id,
                view: BufferViewState::default(),
            }),
        );
        if let Err(error) = self.attach_buffer(buffer_id, node_id) {
            self.nodes.remove(&node_id);
            return Err(error);
        }
        Ok(node_id)
    }

    pub fn create_split_node(
        &mut self,
        session_id: SessionId,
        direction: SplitDirection,
        children: Vec<NodeId>,
    ) -> Result<NodeId> {
        self.ensure_session_exists(session_id)?;
        if children.len() < 2 {
            return Err(MuxError::invalid_input(
                "split nodes require at least two children",
            ));
        }

        let mut seen_children = BTreeSet::new();
        let node_id = self.node_ids.next();
        for child in &children {
            self.ensure_node_belongs_to(*child, session_id)?;
            if !seen_children.insert(*child) {
                return Err(MuxError::invalid_input(format!(
                    "split node {node_id} reuses child {child}"
                )));
            }
            if self.node_parent(*child)?.is_some() {
                return Err(MuxError::invalid_input(format!(
                    "split child {child} already has a parent"
                )));
            }
        }
        for child in &children {
            self.set_parent(*child, Some(node_id))?;
        }

        self.nodes.insert(
            node_id,
            Node::Split(SplitNode {
                id: node_id,
                session_id,
                parent: None,
                direction,
                sizes: vec![1; children.len()],
                children,
                last_focused_descendant: None,
            }),
        );
        Ok(node_id)
    }

    pub fn create_tabs_node(
        &mut self,
        session_id: SessionId,
        tabs: Vec<TabEntry>,
        active: usize,
    ) -> Result<NodeId> {
        self.ensure_session_exists(session_id)?;

        let mut seen_children = BTreeSet::new();
        let node_id = self.node_ids.next();
        for tab in &tabs {
            self.ensure_node_belongs_to(tab.child, session_id)?;
            if !seen_children.insert(tab.child) {
                return Err(MuxError::invalid_input(format!(
                    "tabs node {node_id} reuses child {}",
                    tab.child
                )));
            }
            if self.node_parent(tab.child)?.is_some() {
                return Err(MuxError::invalid_input(format!(
                    "tabs child {} already has a parent",
                    tab.child
                )));
            }
        }
        for tab in &tabs {
            self.set_parent(tab.child, Some(node_id))?;
        }

        self.nodes.insert(
            node_id,
            Node::Tabs(TabsNode {
                id: node_id,
                session_id,
                parent: None,
                tabs,
                active: active.min(active.saturating_sub(0)),
                last_focused_descendant: None,
            }),
        );

        if matches!(self.node(node_id)?, Node::Tabs(tabs) if tabs.tabs.is_empty()) {
            if let Node::Tabs(tabs) = self.node_mut(node_id)? {
                tabs.active = 0;
            }
        } else if let Node::Tabs(tabs) = self.node_mut(node_id)? {
            tabs.active = active.min(tabs.tabs.len().saturating_sub(1));
        }

        Ok(node_id)
    }

    pub fn create_floating_window(
        &mut self,
        session_id: SessionId,
        root_node: NodeId,
        geometry: FloatGeometry,
        title: Option<String>,
    ) -> Result<FloatingId> {
        self.create_floating_window_with_options(session_id, root_node, geometry, title, true, true)
    }

    pub fn create_floating_window_with_options(
        &mut self,
        session_id: SessionId,
        root_node: NodeId,
        geometry: FloatGeometry,
        title: Option<String>,
        focus: bool,
        close_on_empty: bool,
    ) -> Result<FloatingId> {
        self.ensure_session_exists(session_id)?;
        self.ensure_node_belongs_to(root_node, session_id)?;
        if self.node_parent(root_node)?.is_some() {
            return Err(MuxError::invalid_input(
                "floating roots must not already have a parent",
            ));
        }
        if self.is_session_root(root_node) {
            return Err(MuxError::invalid_input(
                "session root cannot also become a floating root",
            ));
        }
        if self.floating_id_by_root(root_node).is_some() {
            return Err(MuxError::invalid_input(
                "node is already a floating root".to_owned(),
            ));
        }

        let floating_id = self.floating_ids.next();
        self.floating.insert(
            floating_id,
            FloatingWindow {
                id: floating_id,
                session_id,
                root_node,
                title,
                geometry,
                focused: false,
                visible: true,
                close_on_empty,
                last_focused_leaf: None,
            },
        );
        self.session_mut(session_id)?.floating.push(floating_id);
        if focus {
            self.focus_floating(floating_id)?;
        }
        Ok(floating_id)
    }

    pub fn create_floating_from_buffer(
        &mut self,
        session_id: SessionId,
        buffer_id: BufferId,
        geometry: FloatGeometry,
        title: Option<String>,
    ) -> Result<FloatingId> {
        self.create_floating_from_buffer_with_options(
            session_id, buffer_id, geometry, title, true, true,
        )
    }

    pub fn create_floating_from_buffer_with_options(
        &mut self,
        session_id: SessionId,
        buffer_id: BufferId,
        geometry: FloatGeometry,
        title: Option<String>,
        focus: bool,
        close_on_empty: bool,
    ) -> Result<FloatingId> {
        let root_node = self.create_buffer_view(session_id, buffer_id)?;
        self.create_floating_window_with_options(
            session_id,
            root_node,
            geometry,
            title,
            focus,
            close_on_empty,
        )
    }

    pub fn close_floating(&mut self, floating_id: FloatingId) -> Result<()> {
        let floating = self.remove_floating_window(floating_id)?;
        let session_id = floating.session_id;
        self.remove_subtree_nodes(floating.root_node)?;
        self.heal_focus(session_id)
    }

    pub fn focus_floating(&mut self, floating_id: FloatingId) -> Result<()> {
        let floating = self.floating_window(floating_id)?.clone();
        if let Some(leaf) = self.resolve_floating_focus(floating_id)? {
            self.focus_leaf(floating.session_id, leaf)
        } else {
            Err(MuxError::not_found(format!(
                "floating window {floating_id} has no focusable leaf"
            )))
        }
    }

    pub fn move_floating(
        &mut self,
        floating_id: FloatingId,
        geometry: FloatGeometry,
    ) -> Result<()> {
        self.floating_mut(floating_id)?.geometry = geometry;
        Ok(())
    }

    pub fn add_root_tab(
        &mut self,
        session_id: SessionId,
        title: impl Into<String>,
        child: NodeId,
    ) -> Result<usize> {
        let root_tabs = self.ensure_root_tabs_container(session_id)?;
        self.add_tab_sibling(root_tabs, title, child)
    }

    pub fn add_tab_sibling(
        &mut self,
        tabs_id: NodeId,
        title: impl Into<String>,
        child: NodeId,
    ) -> Result<usize> {
        let append_index = match self.node(tabs_id)? {
            Node::Tabs(tabs) => tabs.tabs.len(),
            _ => return Err(MuxError::invalid_input("node is not a tabs container")),
        };
        self.add_tab_sibling_at(tabs_id, append_index, title, child)
    }

    pub fn add_tab_sibling_at(
        &mut self,
        tabs_id: NodeId,
        index: usize,
        title: impl Into<String>,
        child: NodeId,
    ) -> Result<usize> {
        let session_id = self.node_session_id(tabs_id)?;
        self.ensure_node_belongs_to(child, session_id)?;
        if child == tabs_id {
            return Err(MuxError::invalid_input(
                "tabs container cannot contain itself".to_owned(),
            ));
        }
        if !matches!(self.node(tabs_id)?, Node::Tabs(_)) {
            return Err(MuxError::invalid_input("node is not a tabs container"));
        }
        if self.node_parent(child)?.is_some() {
            return Err(MuxError::invalid_input(
                "new tab child must not already have a parent",
            ));
        }
        if self.is_session_root(child) {
            return Err(MuxError::conflict(
                "session root cannot become a tab child".to_owned(),
            ));
        }
        if self.floating_id_by_root(child).is_some() {
            return Err(MuxError::conflict(
                "floating root cannot become a tab child".to_owned(),
            ));
        }

        let tab_len = match self.node(tabs_id)? {
            Node::Tabs(tabs) => tabs.tabs.len(),
            _ => return Err(MuxError::invalid_input("node is not a tabs container")),
        };
        if index > tab_len {
            return Err(MuxError::not_found(format!(
                "tab insertion index {index} is out of range for node {tabs_id}"
            )));
        }

        self.set_parent(child, Some(tabs_id))?;
        let index = {
            let tabs = match self.node_mut(tabs_id)? {
                Node::Tabs(tabs) => tabs,
                _ => return Err(MuxError::invalid_input("node is not a tabs container")),
            };
            tabs.tabs.insert(index, TabEntry::new(title, child));
            tabs.active = index;
            index
        };

        if let Some(leaf) = self.resolve_focus_candidate(child)? {
            self.focus_leaf(session_id, leaf)?;
        } else {
            self.heal_focus(session_id)?;
        }

        Ok(index)
    }

    pub fn add_tab_from_buffer(
        &mut self,
        tabs_id: NodeId,
        title: impl Into<String>,
        buffer_id: BufferId,
    ) -> Result<usize> {
        let append_index = match self.node(tabs_id)? {
            Node::Tabs(tabs) => tabs.tabs.len(),
            _ => return Err(MuxError::invalid_input("node is not a tabs container")),
        };
        self.add_tab_from_buffer_at(tabs_id, append_index, title, buffer_id)
    }

    pub fn add_tab_from_buffer_at(
        &mut self,
        tabs_id: NodeId,
        index: usize,
        title: impl Into<String>,
        buffer_id: BufferId,
    ) -> Result<usize> {
        if !matches!(self.node(tabs_id)?, Node::Tabs(_)) {
            return Err(MuxError::invalid_input("node is not a tabs container"));
        }
        let session_id = self.node_session_id(tabs_id)?;
        let child = self.create_buffer_view(session_id, buffer_id)?;
        match self.add_tab_sibling_at(tabs_id, index, title, child) {
            Ok(index) => Ok(index),
            Err(error) => {
                self.discard_buffer_view(child);
                Err(error)
            }
        }
    }

    pub fn rename_tab(
        &mut self,
        tabs_id: NodeId,
        index: usize,
        title: impl Into<String>,
    ) -> Result<()> {
        let title = title.into();
        let tabs = match self.node_mut(tabs_id)? {
            Node::Tabs(tabs) => tabs,
            _ => return Err(MuxError::invalid_input("node is not a tabs container")),
        };
        if index >= tabs.tabs.len() {
            return Err(MuxError::not_found(format!(
                "tab index {index} is out of range for node {tabs_id}"
            )));
        }
        tabs.tabs[index].title = title;
        Ok(())
    }

    pub fn wrap_node_in_tabs(
        &mut self,
        node_id: NodeId,
        title: impl Into<String>,
    ) -> Result<NodeId> {
        let session_id = self.node_session_id(node_id)?;
        let old_parent = self.node_parent(node_id)?;
        let tabs_id = self.node_ids.next();

        self.nodes.insert(
            tabs_id,
            Node::Tabs(TabsNode {
                id: tabs_id,
                session_id,
                parent: old_parent,
                tabs: vec![TabEntry::new(title, node_id)],
                active: 0,
                last_focused_descendant: self.node(node_id)?.last_focused_descendant(),
            }),
        );
        self.set_parent(node_id, Some(tabs_id))?;
        self.repoint_owner_reference(session_id, old_parent, node_id, tabs_id)?;

        Ok(tabs_id)
    }

    pub fn wrap_node_in_split(
        &mut self,
        node_id: NodeId,
        direction: SplitDirection,
        sibling: NodeId,
        insert_before: bool,
    ) -> Result<NodeId> {
        let session_id = self.node_session_id(node_id)?;
        self.ensure_node_belongs_to(sibling, session_id)?;
        if node_id == sibling {
            return Err(MuxError::invalid_input(
                "split sibling cannot be the same node".to_owned(),
            ));
        }
        if self.node_parent(sibling)?.is_some() {
            return Err(MuxError::invalid_input(
                "split sibling must not already have a parent".to_owned(),
            ));
        }
        if self.is_session_root(sibling) {
            return Err(MuxError::conflict(
                "session root cannot become a split child".to_owned(),
            ));
        }
        if self.floating_id_by_root(sibling).is_some() {
            return Err(MuxError::conflict(
                "floating root cannot become a split child".to_owned(),
            ));
        }

        let old_parent = self.node_parent(node_id)?;
        let split_id = self.node_ids.next();
        let children = if insert_before {
            vec![sibling, node_id]
        } else {
            vec![node_id, sibling]
        };
        self.nodes.insert(
            split_id,
            Node::Split(SplitNode {
                id: split_id,
                session_id,
                parent: old_parent,
                direction,
                children: children.clone(),
                sizes: vec![1; children.len()],
                last_focused_descendant: self.resolve_focus_candidate(sibling)?,
            }),
        );
        self.set_parent(node_id, Some(split_id))?;
        self.set_parent(sibling, Some(split_id))?;
        self.repoint_owner_reference(session_id, old_parent, node_id, split_id)?;
        if let Some(leaf) = self.resolve_focus_candidate(sibling)? {
            self.focus_leaf(session_id, leaf)?;
        } else {
            self.heal_focus(session_id)?;
        }
        Ok(split_id)
    }

    pub fn split_leaf_with_new_buffer(
        &mut self,
        leaf_id: NodeId,
        direction: SplitDirection,
        new_buffer: BufferId,
    ) -> Result<NodeId> {
        self.ensure_leaf(leaf_id)?;
        let session_id = self.node_session_id(leaf_id)?;
        let old_parent = self.node_parent(leaf_id)?;
        let new_leaf = self.create_buffer_view(session_id, new_buffer)?;
        let split_id = self.node_ids.next();

        self.nodes.insert(
            split_id,
            Node::Split(SplitNode {
                id: split_id,
                session_id,
                parent: old_parent,
                direction,
                children: vec![leaf_id, new_leaf],
                sizes: vec![1, 1],
                last_focused_descendant: Some(new_leaf),
            }),
        );
        self.set_parent(leaf_id, Some(split_id))?;
        self.set_parent(new_leaf, Some(split_id))?;
        self.repoint_owner_reference(session_id, old_parent, leaf_id, split_id)?;
        self.focus_leaf(session_id, new_leaf)?;

        Ok(split_id)
    }

    pub fn resize_split_children(&mut self, split_id: NodeId, sizes: Vec<u16>) -> Result<()> {
        let split = match self.node_mut(split_id)? {
            Node::Split(split) => split,
            _ => return Err(MuxError::invalid_input("node is not a split")),
        };
        if sizes.len() != split.children.len() {
            return Err(MuxError::invalid_input(format!(
                "split {split_id} expected {} sizes but received {}",
                split.children.len(),
                sizes.len()
            )));
        }
        if sizes.contains(&0) {
            return Err(MuxError::invalid_input(
                "split sizes must be greater than zero",
            ));
        }
        split.sizes = sizes;
        Ok(())
    }

    pub fn node_parent(&self, node_id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.node(node_id)?.parent())
    }

    pub fn set_parent(&mut self, node_id: NodeId, parent: Option<NodeId>) -> Result<()> {
        self.node_mut(node_id)?.set_parent(parent);
        Ok(())
    }

    pub fn replace_child(
        &mut self,
        parent_id: NodeId,
        old_child: NodeId,
        new_child: NodeId,
    ) -> Result<()> {
        let session_id = self.node_session_id(parent_id)?;
        self.ensure_node_belongs_to(old_child, session_id)?;
        self.ensure_node_belongs_to(new_child, session_id)?;

        let replaced = match self.node_mut(parent_id)? {
            Node::Split(split) => {
                if let Some(index) = split.children.iter().position(|child| *child == old_child) {
                    split.children[index] = new_child;
                    true
                } else {
                    false
                }
            }
            Node::Tabs(tabs) => {
                if let Some(tab) = tabs.tabs.iter_mut().find(|tab| tab.child == old_child) {
                    tab.child = new_child;
                    true
                } else {
                    false
                }
            }
            Node::BufferView(_) => {
                return Err(MuxError::invalid_input(
                    "buffer views cannot replace child references",
                ));
            }
        };

        if !replaced {
            return Err(MuxError::not_found(format!(
                "node {old_child} is not a child of parent {parent_id}"
            )));
        }

        self.set_parent(old_child, None)?;
        self.set_parent(new_child, Some(parent_id))?;
        Ok(())
    }

    pub fn remove_child(&mut self, parent_id: NodeId, child_id: NodeId) -> Result<()> {
        let removed = match self.node_mut(parent_id)? {
            Node::Split(split) => {
                if let Some(index) = split.children.iter().position(|child| *child == child_id) {
                    split.children.remove(index);
                    if index < split.sizes.len() {
                        split.sizes.remove(index);
                    }
                    true
                } else {
                    false
                }
            }
            Node::Tabs(tabs) => {
                if let Some(index) = tabs.tabs.iter().position(|tab| tab.child == child_id) {
                    tabs.tabs.remove(index);
                    if tabs.tabs.is_empty() {
                        tabs.active = 0;
                    } else if tabs.active > index {
                        tabs.active -= 1;
                    } else if tabs.active >= tabs.tabs.len() {
                        tabs.active = tabs.tabs.len() - 1;
                    }
                    true
                } else {
                    false
                }
            }
            Node::BufferView(_) => {
                return Err(MuxError::invalid_input(
                    "buffer views cannot remove child references",
                ));
            }
        };

        if !removed {
            return Err(MuxError::not_found(format!(
                "node {child_id} is not a child of parent {parent_id}"
            )));
        }

        self.set_parent(child_id, None)?;
        Ok(())
    }

    pub fn resolve_first_leaf(&self, node_id: NodeId) -> Result<Option<NodeId>> {
        match self.node(node_id)? {
            Node::BufferView(_) => Ok(Some(node_id)),
            Node::Split(split) => {
                for child in &split.children {
                    if let Some(leaf) = self.resolve_first_leaf(*child)? {
                        return Ok(Some(leaf));
                    }
                }
                Ok(None)
            }
            Node::Tabs(tabs) => {
                for tab in &tabs.tabs {
                    if let Some(leaf) = self.resolve_first_leaf(tab.child)? {
                        return Ok(Some(leaf));
                    }
                }
                Ok(None)
            }
        }
    }

    pub fn resolve_visible_leaf(&self, node_id: NodeId) -> Result<Option<NodeId>> {
        match self.node(node_id)? {
            Node::BufferView(_) => Ok(Some(node_id)),
            Node::Split(split) => {
                for child in &split.children {
                    if let Some(leaf) = self.resolve_visible_leaf(*child)? {
                        return Ok(Some(leaf));
                    }
                }
                Ok(None)
            }
            Node::Tabs(tabs) => {
                let active_child = tabs
                    .tabs
                    .get(tabs.active)
                    .or_else(|| tabs.tabs.first())
                    .map(|tab| tab.child);
                if let Some(child) = active_child {
                    self.resolve_visible_leaf(child)
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn visible_leaf_ids(&self, node_id: NodeId) -> Result<Vec<NodeId>> {
        let mut leaves = Vec::new();
        self.collect_visible_leaf_ids(node_id, &mut leaves)?;
        Ok(leaves)
    }

    pub fn visible_session_leaves(&self, session_id: SessionId) -> Result<Vec<NodeId>> {
        self.visible_leaf_ids(self.root_node(session_id)?)
    }

    pub fn find_last_focused_descendant(&self, node_id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.node(node_id)?.last_focused_descendant())
    }

    pub fn session_node_ids(&self, session_id: SessionId) -> Result<Vec<NodeId>> {
        let session = self.session(session_id)?;
        let mut seen = BTreeSet::new();
        self.collect_subtree_nodes(session.root_node, &mut seen)?;
        for floating_id in &session.floating {
            let floating = self.floating_window(*floating_id)?;
            self.collect_subtree_nodes(floating.root_node, &mut seen)?;
        }
        Ok(seen.into_iter().collect())
    }

    pub fn session_buffer_ids(&self, session_id: SessionId) -> Result<Vec<BufferId>> {
        let mut buffers = BTreeSet::new();
        for node_id in self.session_node_ids(session_id)? {
            if let Node::BufferView(leaf) = self.node(node_id)? {
                buffers.insert(leaf.buffer_id);
            }
        }
        Ok(buffers.into_iter().collect())
    }

    pub fn attach_buffer(&mut self, buffer_id: BufferId, node_id: NodeId) -> Result<()> {
        self.buffer(buffer_id)?;
        let current_attachment = self.buffer(buffer_id)?.attachment.clone();
        if let BufferAttachment::Attached(existing_view) = current_attachment
            && existing_view != node_id
        {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} is already attached to view {existing_view}"
            )));
        }

        let current_buffer = self.buffer_view_buffer_id(node_id)?;
        if current_buffer != buffer_id {
            if let Some(previous_buffer) = self.buffers.get_mut(&current_buffer)
                && matches!(previous_buffer.attachment, BufferAttachment::Attached(attached) if attached == node_id)
            {
                previous_buffer.attachment = BufferAttachment::Detached;
            }
            match self.node_mut(node_id)? {
                Node::BufferView(leaf) => leaf.buffer_id = buffer_id,
                _ => return Err(MuxError::invalid_input("node is not a buffer view")),
            }
        }

        self.buffer_mut(buffer_id)?.attachment = BufferAttachment::Attached(node_id);
        Ok(())
    }

    pub fn move_buffer_to_leaf(&mut self, buffer_id: BufferId, target_leaf: NodeId) -> Result<()> {
        self.ensure_leaf(target_leaf)?;
        let target_session = self.node_session_id(target_leaf)?;
        let source_view = match self.buffer(buffer_id)?.attachment {
            BufferAttachment::Attached(node_id) => Some(node_id),
            BufferAttachment::Detached => None,
        };

        if source_view == Some(target_leaf) {
            return self.focus_leaf(target_session, target_leaf);
        }

        if let Some(source_view) = source_view {
            let source_session = self.node_session_id(source_view)?;
            if source_session != target_session {
                return Err(MuxError::conflict(
                    "attached buffers must be detached before moving across sessions".to_owned(),
                ));
            }
            self.close_node(source_view)?;
        }

        self.attach_buffer(buffer_id, target_leaf)?;
        self.focus_leaf(target_session, target_leaf)
    }

    pub fn zoom_node(&mut self, node_id: NodeId) -> Result<()> {
        let session_id = self.node_session_id(node_id)?;
        self.session_mut(session_id)?.zoomed_node = Some(node_id);
        Ok(())
    }

    pub fn unzoom_session(&mut self, session_id: SessionId) -> Result<()> {
        self.session_mut(session_id)?.zoomed_node = None;
        Ok(())
    }

    pub fn toggle_zoom_node(&mut self, node_id: NodeId) -> Result<()> {
        let session_id = self.node_session_id(node_id)?;
        let next = if self.session(session_id)?.zoomed_node == Some(node_id) {
            None
        } else {
            Some(node_id)
        };
        self.session_mut(session_id)?.zoomed_node = next;
        Ok(())
    }

    pub fn swap_sibling_nodes(
        &mut self,
        first_node_id: NodeId,
        second_node_id: NodeId,
    ) -> Result<()> {
        if first_node_id == second_node_id {
            return Ok(());
        }
        let parent_id = self.shared_parent(first_node_id, second_node_id)?;
        match self.node_mut(parent_id)? {
            Node::Split(split) => {
                let first = split
                    .children
                    .iter()
                    .position(|child| *child == first_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {first_node_id} is not a child of split {parent_id}"
                        ))
                    })?;
                let second = split
                    .children
                    .iter()
                    .position(|child| *child == second_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {second_node_id} is not a child of split {parent_id}"
                        ))
                    })?;
                split.children.swap(first, second);
                split.sizes.swap(first, second);
            }
            Node::Tabs(tabs) => {
                let first = tabs
                    .tabs
                    .iter()
                    .position(|tab| tab.child == first_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {first_node_id} is not a child of tabs {parent_id}"
                        ))
                    })?;
                let second = tabs
                    .tabs
                    .iter()
                    .position(|tab| tab.child == second_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {second_node_id} is not a child of tabs {parent_id}"
                        ))
                    })?;
                tabs.tabs.swap(first, second);
                match tabs.active {
                    active if active == first => tabs.active = second,
                    active if active == second => tabs.active = first,
                    _ => {}
                }
            }
            Node::BufferView(_) => {
                return Err(MuxError::invalid_input(
                    "buffer views do not have sibling children",
                ));
            }
        }
        Ok(())
    }

    pub fn move_node_before(&mut self, node_id: NodeId, sibling_node_id: NodeId) -> Result<()> {
        self.reorder_sibling_node(node_id, sibling_node_id, true)
    }

    pub fn move_node_after(&mut self, node_id: NodeId, sibling_node_id: NodeId) -> Result<()> {
        self.reorder_sibling_node(node_id, sibling_node_id, false)
    }

    pub fn break_node(&mut self, node_id: NodeId, to_floating: bool) -> Result<()> {
        let session_id = self.node_session_id(node_id)?;
        let title = self.default_tab_title(node_id)?;
        let (old_parent, _) = self.detach_node_from_owner(node_id)?;

        if to_floating {
            self.create_floating_window_with_options(
                session_id,
                node_id,
                FloatGeometry::new(10, 3, 100, 26),
                Some(title),
                true,
                true,
            )?;
        } else {
            let tabs_id = self
                .nearest_tabs_ancestor(old_parent)?
                .unwrap_or(self.ensure_root_tabs_container(session_id)?);
            self.add_tab_sibling(tabs_id, title, node_id)?;
        }

        if let Some(parent_id) = old_parent {
            self.normalize_upwards(parent_id)?;
        }
        self.normalize_zoomed_node(session_id)?;
        Ok(())
    }

    pub fn join_buffer_at_node(
        &mut self,
        node_id: NodeId,
        buffer_id: BufferId,
        placement: NodeJoinPlacement,
    ) -> Result<()> {
        let target_session = self.node_session_id(node_id)?;
        if let BufferAttachment::Attached(source_view) = self.buffer(buffer_id)?.attachment
            && source_view != node_id
        {
            self.close_node(source_view)?;
        }

        let new_view = self.create_buffer_view(target_session, buffer_id)?;
        let result = match placement {
            NodeJoinPlacement::Left => self
                .wrap_node_in_split(node_id, SplitDirection::Vertical, new_view, true)
                .map(|_| ()),
            NodeJoinPlacement::Right => self
                .wrap_node_in_split(node_id, SplitDirection::Vertical, new_view, false)
                .map(|_| ()),
            NodeJoinPlacement::Up => self
                .wrap_node_in_split(node_id, SplitDirection::Horizontal, new_view, true)
                .map(|_| ()),
            NodeJoinPlacement::Down => self
                .wrap_node_in_split(node_id, SplitDirection::Horizontal, new_view, false)
                .map(|_| ()),
            NodeJoinPlacement::TabBefore | NodeJoinPlacement::TabAfter => {
                let title = self.buffer(buffer_id)?.title.clone();
                let tabs_id = if matches!(self.node(node_id)?, Node::Tabs(_)) {
                    node_id
                } else if matches!(
                    self.node_parent(node_id)?
                        .map(|id| self.node(id))
                        .transpose()?,
                    Some(Node::Tabs(_))
                ) {
                    self.node_parent(node_id)?.expect("checked parent exists")
                } else {
                    self.wrap_node_in_tabs(node_id, self.default_tab_title(node_id)?)?
                };
                let insert_index = {
                    let tabs = match self.node(tabs_id)? {
                        Node::Tabs(tabs) => tabs,
                        _ => return Err(MuxError::invalid_input("node is not a tabs container")),
                    };
                    if tabs_id == node_id {
                        let active = tabs.active;
                        match placement {
                            NodeJoinPlacement::TabBefore => active,
                            NodeJoinPlacement::TabAfter => active + 1,
                            _ => unreachable!(),
                        }
                    } else {
                        let current = tabs
                            .tabs
                            .iter()
                            .position(|tab| tab.child == node_id)
                            .ok_or_else(|| {
                                MuxError::not_found(format!(
                                    "node {node_id} is not a child of tabs {tabs_id}"
                                ))
                            })?;
                        match placement {
                            NodeJoinPlacement::TabBefore => current,
                            NodeJoinPlacement::TabAfter => current + 1,
                            _ => unreachable!(),
                        }
                    }
                };
                self.add_tab_sibling_at(tabs_id, insert_index, title, new_view)
                    .map(|_| ())
            }
        };

        if let Err(error) = result {
            self.discard_buffer_view(new_view);
            return Err(error);
        }
        self.normalize_zoomed_node(target_session)?;
        Ok(())
    }

    pub fn detach_buffer(&mut self, buffer_id: BufferId) -> Result<()> {
        match self.buffer(buffer_id)?.attachment {
            BufferAttachment::Attached(node_id) => self.close_node(node_id),
            BufferAttachment::Detached => Ok(()),
        }
    }

    pub fn focus_leaf(&mut self, session_id: SessionId, leaf_id: NodeId) -> Result<()> {
        self.ensure_leaf_belongs_to(leaf_id, session_id)?;
        self.ensure_leaf_is_focusable(session_id, leaf_id)?;
        self.clear_session_focus(session_id)?;
        self.set_leaf_focus(leaf_id, true)?;

        let floating_owner = self.floating_id_for_node(leaf_id)?;
        {
            let session = self.session_mut(session_id)?;
            session.focused_leaf = Some(leaf_id);
            session.focused_floating = floating_owner;
        }

        let floating_ids = self.session(session_id)?.floating.clone();
        for floating_id in floating_ids {
            if let Some(floating) = self.floating.get_mut(&floating_id) {
                floating.focused = Some(floating_id) == floating_owner;
                if floating.focused {
                    floating.last_focused_leaf = Some(leaf_id);
                }
            }
        }

        let mut child = leaf_id;
        while let Some(parent) = self.node_parent(child)? {
            match self.node_mut(parent)? {
                Node::Split(split) => {
                    split.last_focused_descendant = Some(leaf_id);
                }
                Node::Tabs(tabs) => {
                    tabs.last_focused_descendant = Some(leaf_id);
                    if let Some(index) = tabs.tabs.iter().position(|tab| tab.child == child) {
                        tabs.active = index;
                    }
                }
                Node::BufferView(_) => {}
            }
            child = parent;
        }

        Ok(())
    }

    pub fn switch_tab(&mut self, tabs_id: NodeId, index: usize) -> Result<()> {
        let session_id = self.node_session_id(tabs_id)?;
        let child = {
            let tabs = match self.node_mut(tabs_id)? {
                Node::Tabs(tabs) => tabs,
                _ => return Err(MuxError::invalid_input("node is not a tabs container")),
            };
            if index >= tabs.tabs.len() {
                return Err(MuxError::not_found(format!(
                    "tab index {index} is out of range for node {tabs_id}"
                )));
            }
            tabs.active = index;
            tabs.tabs[index].child
        };

        if self.is_node_visible_in_session(session_id, tabs_id)? {
            if let Some(leaf) = self.resolve_focus_candidate(child)? {
                self.focus_leaf(session_id, leaf)?;
            } else {
                self.heal_focus(session_id)?;
            }
        }

        Ok(())
    }

    pub fn close_tab(&mut self, tabs_id: NodeId, index: usize) -> Result<()> {
        let session_id = self.node_session_id(tabs_id)?;
        let child = {
            let tabs = match self.node_mut(tabs_id)? {
                Node::Tabs(tabs) => tabs,
                _ => return Err(MuxError::invalid_input("node is not a tabs container")),
            };
            if index >= tabs.tabs.len() {
                return Err(MuxError::not_found(format!(
                    "tab index {index} is out of range for node {tabs_id}"
                )));
            }
            let child = tabs.tabs[index].child;
            tabs.tabs.remove(index);
            if tabs.tabs.is_empty() {
                tabs.active = 0;
                tabs.last_focused_descendant = None;
            } else if tabs.active > index {
                tabs.active -= 1;
            } else if tabs.active >= tabs.tabs.len() {
                tabs.active = tabs.tabs.len() - 1;
            }
            child
        };

        self.set_parent(child, None)?;
        self.remove_subtree_nodes(child)?;
        self.normalize_upwards(tabs_id)?;
        self.heal_focus(session_id)
    }

    pub fn close_node(&mut self, node_id: NodeId) -> Result<()> {
        let session_id = self.node_session_id(node_id)?;
        if self.is_session_root(node_id) {
            return self.clear_session_root(session_id);
        }

        if let Some(parent) = self.node_parent(node_id)? {
            self.remove_child(parent, node_id)?;
            self.remove_subtree_nodes(node_id)?;
            self.normalize_upwards(parent)?;
        } else if let Some(floating_id) = self.floating_id_by_root(node_id) {
            let floating = self.remove_floating_window(floating_id)?;
            self.remove_subtree_nodes(floating.root_node)?;
        } else {
            return Err(MuxError::invalid_input(format!(
                "node {node_id} has no owning container"
            )));
        }

        self.normalize_zoomed_node(session_id)?;
        self.heal_focus(session_id)
    }

    pub fn replace_node(&mut self, node_id: NodeId, replacement: NodeId) -> Result<()> {
        let session_id = self.node_session_id(node_id)?;
        self.ensure_node_belongs_to(replacement, session_id)?;
        if node_id == replacement {
            return Ok(());
        }
        if self.node_parent(replacement)?.is_some() {
            return Err(MuxError::invalid_input(
                "replacement node must not already have a parent".to_owned(),
            ));
        }
        if self.is_session_root(replacement) {
            return Err(MuxError::conflict(
                "session root cannot become a replacement child".to_owned(),
            ));
        }
        if self.floating_id_by_root(replacement).is_some() {
            return Err(MuxError::conflict(
                "floating root cannot become a replacement child".to_owned(),
            ));
        }

        self.replace_node_in_owner(node_id, replacement)?;
        self.remove_subtree_nodes(node_id)?;
        if let Some(leaf) = self.resolve_focus_candidate(replacement)? {
            self.focus_leaf(session_id, leaf)?;
        } else {
            self.heal_focus(session_id)?;
        }
        self.normalize_zoomed_node(session_id)?;
        Ok(())
    }

    pub fn normalize_upwards(&mut self, start: NodeId) -> Result<()> {
        let mut current = Some(start);
        while let Some(node_id) = current {
            if !self.nodes.contains_key(&node_id) {
                break;
            }

            current = match self.node(node_id)? {
                Node::BufferView(_) => self.node_parent(node_id)?,
                Node::Split(_) => self.normalize_split_node(node_id)?,
                Node::Tabs(_) => self.normalize_tabs_node(node_id)?,
            };
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();

        for session in self.sessions.values() {
            let root = self.node(session.root_node)?;
            if root.parent().is_some() {
                return Err(MuxError::conflict(format!(
                    "session {} root node {} must not have a parent",
                    session.id, session.root_node
                )));
            }

            self.validate_subtree(session.id, session.root_node, None, true, &mut seen)?;

            for floating_id in &session.floating {
                let floating = self.floating_window(*floating_id)?;
                if floating.session_id != session.id {
                    return Err(MuxError::conflict(format!(
                        "floating window {floating_id} belongs to the wrong session"
                    )));
                }
                if floating.root_node == session.root_node {
                    return Err(MuxError::conflict(format!(
                        "floating window {floating_id} reuses the session root"
                    )));
                }
                if self.node_parent(floating.root_node)?.is_some() {
                    return Err(MuxError::conflict(format!(
                        "floating window {floating_id} root {} must not have a parent",
                        floating.root_node
                    )));
                }
                self.validate_subtree(session.id, floating.root_node, None, false, &mut seen)?;
            }

            if let Some(focused_leaf) = session.focused_leaf {
                if !matches!(self.node(focused_leaf)?, Node::BufferView(_)) {
                    return Err(MuxError::conflict(format!(
                        "focused leaf {focused_leaf} is not a buffer view"
                    )));
                }
                if !self.is_node_visible_in_session(session.id, focused_leaf)? {
                    return Err(MuxError::conflict(format!(
                        "focused leaf {focused_leaf} is not visible in session {}",
                        session.id
                    )));
                }
            }

            if let Some(zoomed_node) = session.zoomed_node {
                if !self.nodes.contains_key(&zoomed_node) {
                    return Err(MuxError::conflict(format!(
                        "zoomed node {zoomed_node} is missing from session {}",
                        session.id
                    )));
                }
                if self.node(zoomed_node)?.session_id() != session.id {
                    return Err(MuxError::conflict(format!(
                        "zoomed node {zoomed_node} belongs to the wrong session"
                    )));
                }
            }
        }

        if seen.len() != self.nodes.len() {
            return Err(MuxError::conflict(format!(
                "orphaned node(s) detected: visited {} of {} node(s)",
                seen.len(),
                self.nodes.len()
            )));
        }

        for (buffer_id, buffer) in &self.buffers {
            if let BufferAttachment::Attached(node_id) = buffer.attachment {
                match self.node(node_id)? {
                    Node::BufferView(leaf) if leaf.buffer_id == *buffer_id => {}
                    _ => {
                        return Err(MuxError::conflict(format!(
                            "buffer {buffer_id} attachment does not match view {node_id}"
                        )));
                    }
                }
            }
        }

        for node in self.nodes.values() {
            if let Node::BufferView(leaf) = node {
                match self.buffer(leaf.buffer_id)?.attachment {
                    BufferAttachment::Attached(attached) if attached == leaf.id => {}
                    _ => {
                        return Err(MuxError::conflict(format!(
                            "buffer view {} points at detached buffer {}",
                            leaf.id, leaf.buffer_id
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    fn clear_session_root(&mut self, session_id: SessionId) -> Result<()> {
        let old_root = self.root_node(session_id)?;
        self.remove_subtree_nodes(old_root)?;
        let new_root = self.node_ids.next();
        self.nodes.insert(
            new_root,
            Node::Tabs(TabsNode {
                id: new_root,
                session_id,
                parent: None,
                tabs: Vec::new(),
                active: 0,
                last_focused_descendant: None,
            }),
        );
        self.session_mut(session_id)?.root_node = new_root;
        self.session_mut(session_id)?.zoomed_node = None;
        self.heal_focus(session_id)
    }

    fn heal_focus(&mut self, session_id: SessionId) -> Result<()> {
        let preferred_floating = self
            .session(session_id)?
            .focused_floating
            .filter(|floating_id| self.floating.contains_key(floating_id));

        if let Some(floating_id) = preferred_floating
            && let Some(leaf) = self.resolve_floating_focus(floating_id)?
        {
            return self.focus_leaf(session_id, leaf);
        }

        let root = self.root_node(session_id)?;
        if let Some(leaf) = self.resolve_focus_candidate(root)? {
            return self.focus_leaf(session_id, leaf);
        }

        let floating_ids = self.session(session_id)?.floating.clone();
        for floating_id in floating_ids {
            if let Some(leaf) = self.resolve_floating_focus(floating_id)? {
                return self.focus_leaf(session_id, leaf);
            }
        }

        self.clear_session_focus(session_id)
    }

    fn clear_session_focus(&mut self, session_id: SessionId) -> Result<()> {
        let previous_leaf = self.session(session_id)?.focused_leaf;
        if let Some(previous_leaf) = previous_leaf {
            let _ = self.set_leaf_focus(previous_leaf, false);
        }

        let floating_ids = self.session(session_id)?.floating.clone();
        for floating_id in floating_ids {
            if let Some(floating) = self.floating.get_mut(&floating_id) {
                floating.focused = false;
            }
        }

        let session = self.session_mut(session_id)?;
        session.focused_leaf = None;
        session.focused_floating = None;
        Ok(())
    }

    fn resolve_floating_focus(&self, floating_id: FloatingId) -> Result<Option<NodeId>> {
        let floating = self.floating_window(floating_id)?;
        if !floating.visible {
            return Ok(None);
        }

        if let Some(last_leaf) = floating.last_focused_leaf
            && self.nodes.contains_key(&last_leaf)
            && self.top_root_for_node(last_leaf)? == floating.root_node
            && self.is_node_visible_from(floating.root_node, last_leaf)?
        {
            return Ok(Some(last_leaf));
        }

        self.resolve_focus_candidate(floating.root_node)
    }

    fn resolve_focus_candidate(&self, node_id: NodeId) -> Result<Option<NodeId>> {
        match self.node(node_id)? {
            Node::BufferView(_) => Ok(Some(node_id)),
            Node::Split(split) => {
                if let Some(last_leaf) = split.last_focused_descendant
                    && self.nodes.contains_key(&last_leaf)
                    && self.is_node_visible_from(node_id, last_leaf)?
                {
                    return Ok(Some(last_leaf));
                }
                for child in &split.children {
                    if let Some(leaf) = self.resolve_focus_candidate(*child)? {
                        return Ok(Some(leaf));
                    }
                }
                Ok(None)
            }
            Node::Tabs(tabs) => {
                let active_child = tabs
                    .tabs
                    .get(tabs.active)
                    .or_else(|| tabs.tabs.first())
                    .map(|tab| tab.child);
                if let Some(child) = active_child {
                    self.resolve_focus_candidate(child)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn default_tab_title(&self, node_id: NodeId) -> Result<String> {
        if let Some(leaf_id) = self.resolve_focus_candidate(node_id)? {
            let buffer_id = self.buffer_view_buffer_id(leaf_id)?;
            return Ok(self.buffer(buffer_id)?.title.clone());
        }

        Ok("window".to_owned())
    }

    fn set_leaf_focus(&mut self, leaf_id: NodeId, focused: bool) -> Result<()> {
        match self.node_mut(leaf_id)? {
            Node::BufferView(leaf) => {
                leaf.view.focused = focused;
                Ok(())
            }
            _ => Err(MuxError::invalid_input(format!(
                "node {leaf_id} is not a buffer view"
            ))),
        }
    }

    fn buffer_view_buffer_id(&self, node_id: NodeId) -> Result<BufferId> {
        match self.node(node_id)? {
            Node::BufferView(leaf) => Ok(leaf.buffer_id),
            _ => Err(MuxError::invalid_input(format!(
                "node {node_id} is not a buffer view"
            ))),
        }
    }

    fn node_session_id(&self, node_id: NodeId) -> Result<SessionId> {
        Ok(self.node(node_id)?.session_id())
    }

    fn ensure_session_exists(&self, session_id: SessionId) -> Result<()> {
        let _ = self.session(session_id)?;
        Ok(())
    }

    fn ensure_node_belongs_to(&self, node_id: NodeId, session_id: SessionId) -> Result<()> {
        let node = self.node(node_id)?;
        if node.session_id() != session_id {
            return Err(MuxError::conflict(format!(
                "node {node_id} belongs to session {}, not {}",
                node.session_id(),
                session_id
            )));
        }
        Ok(())
    }

    fn ensure_leaf(&self, node_id: NodeId) -> Result<()> {
        if matches!(self.node(node_id)?, Node::BufferView(_)) {
            Ok(())
        } else {
            Err(MuxError::invalid_input(format!(
                "node {node_id} is not a buffer view"
            )))
        }
    }

    fn ensure_leaf_belongs_to(&self, node_id: NodeId, session_id: SessionId) -> Result<()> {
        self.ensure_node_belongs_to(node_id, session_id)?;
        self.ensure_leaf(node_id)
    }

    fn shared_parent(&self, first_node_id: NodeId, second_node_id: NodeId) -> Result<NodeId> {
        if first_node_id == second_node_id {
            return Err(MuxError::invalid_input(
                "sibling operations require two distinct nodes",
            ));
        }
        let first_parent = self.node_parent(first_node_id)?.ok_or_else(|| {
            MuxError::invalid_input(format!("node {first_node_id} has no parent"))
        })?;
        let second_parent = self.node_parent(second_node_id)?.ok_or_else(|| {
            MuxError::invalid_input(format!("node {second_node_id} has no parent"))
        })?;
        if first_parent != second_parent {
            return Err(MuxError::conflict(
                "node ergonomics are restricted to siblings with the same parent".to_owned(),
            ));
        }
        Ok(first_parent)
    }

    fn reorder_sibling_node(
        &mut self,
        node_id: NodeId,
        sibling_node_id: NodeId,
        before: bool,
    ) -> Result<()> {
        if node_id == sibling_node_id {
            return Ok(());
        }
        let parent_id = self.shared_parent(node_id, sibling_node_id)?;
        match self.node_mut(parent_id)? {
            Node::Split(split) => {
                let from = split
                    .children
                    .iter()
                    .position(|child| *child == node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!("node {node_id} is not in split {parent_id}"))
                    })?;
                let target = split
                    .children
                    .iter()
                    .position(|child| *child == sibling_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {sibling_node_id} is not in split {parent_id}"
                        ))
                    })?;
                let child = split.children.remove(from);
                let size = split.sizes.remove(from);
                let mut insert_at = target;
                if from < target {
                    insert_at = insert_at.saturating_sub(1);
                }
                if !before {
                    insert_at = insert_at.saturating_add(1);
                }
                split.children.insert(insert_at, child);
                split.sizes.insert(insert_at, size);
            }
            Node::Tabs(tabs) => {
                let from = tabs
                    .tabs
                    .iter()
                    .position(|tab| tab.child == node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!("node {node_id} is not in tabs {parent_id}"))
                    })?;
                let target = tabs
                    .tabs
                    .iter()
                    .position(|tab| tab.child == sibling_node_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "node {sibling_node_id} is not in tabs {parent_id}"
                        ))
                    })?;
                let tab = tabs.tabs.remove(from);
                let mut insert_at = target;
                if from < target {
                    insert_at = insert_at.saturating_sub(1);
                }
                if !before {
                    insert_at = insert_at.saturating_add(1);
                }
                tabs.tabs.insert(insert_at, tab);
                if tabs.active == from {
                    tabs.active = insert_at;
                } else if from < tabs.active && insert_at >= tabs.active {
                    tabs.active = tabs.active.saturating_sub(1);
                } else if from > tabs.active && insert_at <= tabs.active {
                    tabs.active = tabs.active.saturating_add(1);
                }
            }
            Node::BufferView(_) => {
                return Err(MuxError::invalid_input(
                    "buffer views do not have sibling children",
                ));
            }
        }
        Ok(())
    }

    fn detach_node_from_owner(
        &mut self,
        node_id: NodeId,
    ) -> Result<(Option<NodeId>, Option<FloatingId>)> {
        if self.is_session_root(node_id) {
            return Err(MuxError::conflict(
                "session root cannot be broken out of its owner".to_owned(),
            ));
        }
        if let Some(parent_id) = self.node_parent(node_id)? {
            self.remove_child(parent_id, node_id)?;
            return Ok((Some(parent_id), None));
        }
        if let Some(floating_id) = self.floating_id_by_root(node_id) {
            let _ = self.remove_floating_window(floating_id)?;
            return Ok((None, Some(floating_id)));
        }
        Err(MuxError::invalid_input(format!(
            "node {node_id} has no owning container"
        )))
    }

    fn nearest_tabs_ancestor(&self, mut node_id: Option<NodeId>) -> Result<Option<NodeId>> {
        while let Some(current) = node_id {
            if matches!(self.node(current)?, Node::Tabs(_)) {
                return Ok(Some(current));
            }
            node_id = self.node_parent(current)?;
        }
        Ok(None)
    }

    fn normalize_zoomed_node(&mut self, session_id: SessionId) -> Result<()> {
        let keep = self.session(session_id)?.zoomed_node.filter(|node_id| {
            self.nodes
                .get(node_id)
                .is_some_and(|node| node.session_id() == session_id)
        });
        self.session_mut(session_id)?.zoomed_node = keep;
        Ok(())
    }

    fn is_session_root(&self, node_id: NodeId) -> bool {
        self.sessions
            .values()
            .any(|session| session.root_node == node_id)
    }

    fn floating_id_by_root(&self, root_node: NodeId) -> Option<FloatingId> {
        self.floating
            .values()
            .find(|floating| floating.root_node == root_node)
            .map(|floating| floating.id)
    }

    pub fn floating_id_for_node(&self, node_id: NodeId) -> Result<Option<FloatingId>> {
        let root = self.top_root_for_node(node_id)?;
        Ok(self.floating_id_by_root(root))
    }

    fn top_root_for_node(&self, node_id: NodeId) -> Result<NodeId> {
        let mut current = node_id;
        while let Some(parent) = self.node_parent(current)? {
            current = parent;
        }
        Ok(current)
    }

    fn is_node_visible_from(&self, root_id: NodeId, node_id: NodeId) -> Result<bool> {
        if !self.subtree_contains(root_id, node_id)? {
            return Ok(false);
        }

        let mut current = node_id;
        while current != root_id {
            let parent = self.node_parent(current)?.ok_or_else(|| {
                MuxError::conflict(format!(
                    "node {node_id} is not rooted at expected root {root_id}"
                ))
            })?;
            if let Node::Tabs(tabs) = self.node(parent)? {
                let active_child = tabs.tabs.get(tabs.active).map(|tab| tab.child);
                if active_child != Some(current) {
                    return Ok(false);
                }
            }
            current = parent;
        }

        Ok(true)
    }

    fn is_node_visible_in_session(&self, session_id: SessionId, node_id: NodeId) -> Result<bool> {
        self.ensure_node_belongs_to(node_id, session_id)?;
        let root = self.top_root_for_node(node_id)?;
        if root == self.session(session_id)?.root_node {
            return self.is_node_visible_from(root, node_id);
        }
        if let Some(floating_id) = self.floating_id_by_root(root) {
            let floating = self.floating_window(floating_id)?;
            return Ok(floating.visible && self.is_node_visible_from(root, node_id)?);
        }
        Ok(false)
    }

    fn subtree_contains(&self, root_id: NodeId, needle: NodeId) -> Result<bool> {
        if root_id == needle {
            return Ok(true);
        }

        for child in self.node(root_id)?.child_ids() {
            if self.subtree_contains(child, needle)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn collect_visible_leaf_ids(&self, node_id: NodeId, leaves: &mut Vec<NodeId>) -> Result<()> {
        match self.node(node_id)? {
            Node::BufferView(_) => leaves.push(node_id),
            Node::Split(split) => {
                for child in &split.children {
                    self.collect_visible_leaf_ids(*child, leaves)?;
                }
            }
            Node::Tabs(tabs) => {
                if let Some(child) = tabs
                    .tabs
                    .get(tabs.active)
                    .or_else(|| tabs.tabs.first())
                    .map(|tab| tab.child)
                {
                    self.collect_visible_leaf_ids(child, leaves)?;
                }
            }
        }
        Ok(())
    }

    fn collect_subtree_nodes(&self, root_id: NodeId, seen: &mut BTreeSet<NodeId>) -> Result<()> {
        if !seen.insert(root_id) {
            return Ok(());
        }

        for child in self.node(root_id)?.child_ids() {
            self.collect_subtree_nodes(child, seen)?;
        }

        Ok(())
    }

    fn repoint_owner_reference(
        &mut self,
        session_id: SessionId,
        owner: Option<NodeId>,
        old_node: NodeId,
        new_node: NodeId,
    ) -> Result<()> {
        if let Some(parent_id) = owner {
            match self.node_mut(parent_id)? {
                Node::Split(split) => {
                    let index = split
                        .children
                        .iter()
                        .position(|child| *child == old_node)
                        .ok_or_else(|| {
                            MuxError::not_found(format!(
                                "node {old_node} is not a child of split {parent_id}"
                            ))
                        })?;
                    split.children[index] = new_node;
                }
                Node::Tabs(tabs) => {
                    let tab = tabs
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.child == old_node)
                        .ok_or_else(|| {
                            MuxError::not_found(format!(
                                "node {old_node} is not a tab child of {parent_id}"
                            ))
                        })?;
                    tab.child = new_node;
                }
                Node::BufferView(_) => {
                    return Err(MuxError::invalid_input(
                        "buffer views cannot own child nodes".to_owned(),
                    ));
                }
            }
            self.set_parent(new_node, Some(parent_id))?;
            return Ok(());
        }

        if self.is_session_root(old_node) {
            self.session_mut(session_id)?.root_node = new_node;
            self.set_parent(new_node, None)?;
            return Ok(());
        }

        if let Some(floating_id) = self.floating_id_by_root(old_node) {
            self.floating_mut(floating_id)?.root_node = new_node;
            self.set_parent(new_node, None)?;
            return Ok(());
        }

        Err(MuxError::conflict(format!(
            "node {old_node} does not have a replaceable owner"
        )))
    }

    fn replace_node_in_owner(&mut self, old_node: NodeId, new_node: NodeId) -> Result<()> {
        let session_id = self.node_session_id(old_node)?;
        let owner = self.node_parent(old_node)?;
        let replacement_focus = self.resolve_focus_candidate(new_node)?;
        if let Some(parent_id) = owner {
            let should_update_focus = match self.node(parent_id)?.last_focused_descendant() {
                Some(leaf_id) if self.nodes.contains_key(&leaf_id) => {
                    self.subtree_contains(old_node, leaf_id)?
                }
                Some(_) => true,
                None => false,
            };
            self.replace_child(parent_id, old_node, new_node)?;
            if should_update_focus {
                self.node_mut(parent_id)?
                    .set_last_focused_descendant(replacement_focus);
            }
            return Ok(());
        }

        if self.is_session_root(old_node) {
            self.session_mut(session_id)?.root_node = new_node;
            self.set_parent(new_node, None)?;
            self.set_parent(old_node, None)?;
            return Ok(());
        }

        if let Some(floating_id) = self.floating_id_by_root(old_node) {
            self.floating_mut(floating_id)?.root_node = new_node;
            self.set_parent(new_node, None)?;
            self.set_parent(old_node, None)?;
            return Ok(());
        }

        Err(MuxError::conflict(format!(
            "node {old_node} does not have a replaceable owner"
        )))
    }

    fn normalize_split_node(&mut self, node_id: NodeId) -> Result<Option<NodeId>> {
        let (children_len, parent) = match self.node(node_id)? {
            Node::Split(split) => (split.children.len(), split.parent),
            _ => return self.node_parent(node_id),
        };

        if children_len == 0 {
            return Err(MuxError::conflict(format!(
                "split node {node_id} cannot be empty after mutation"
            )));
        }

        if children_len == 1 {
            let child = match self.node(node_id)? {
                Node::Split(split) => split.children[0],
                _ => unreachable!(),
            };
            self.replace_node_in_owner(node_id, child)?;
            self.nodes.remove(&node_id);
            return Ok(Some(child));
        }

        if let Node::Split(split) = self.node_mut(node_id)?
            && (split.sizes.len() != split.children.len() || split.sizes.contains(&0))
        {
            split.sizes = vec![1; split.children.len()];
        }

        Ok(parent)
    }

    fn normalize_tabs_node(&mut self, node_id: NodeId) -> Result<Option<NodeId>> {
        let (tabs_len, parent) = match self.node(node_id)? {
            Node::Tabs(tabs) => (tabs.tabs.len(), tabs.parent),
            _ => return self.node_parent(node_id),
        };

        let is_root = self.is_session_root(node_id);
        let floating_owner = self.floating_id_by_root(node_id);

        if tabs_len == 0 {
            if is_root {
                if let Node::Tabs(tabs) = self.node_mut(node_id)? {
                    tabs.active = 0;
                    tabs.last_focused_descendant = None;
                }
                return Ok(parent);
            }

            if let Some(floating_id) = floating_owner {
                let floating = self.remove_floating_window(floating_id)?;
                self.nodes.remove(&floating.root_node);
                return Ok(None);
            }

            self.nodes.remove(&node_id);
            return Ok(parent);
        }

        if tabs_len == 1 && floating_owner.is_none() {
            let child = match self.node(node_id)? {
                Node::Tabs(tabs) => tabs.tabs[0].child,
                _ => unreachable!(),
            };
            self.replace_node_in_owner(node_id, child)?;
            self.nodes.remove(&node_id);
            return Ok(Some(child));
        }

        if let Node::Tabs(tabs) = self.node_mut(node_id)?
            && tabs.active >= tabs.tabs.len()
        {
            tabs.active = tabs.tabs.len() - 1;
        }

        Ok(parent)
    }

    fn remove_subtree_nodes(&mut self, node_id: NodeId) -> Result<()> {
        let children = self.node(node_id)?.child_ids();
        for child in children {
            self.remove_subtree_nodes(child)?;
        }

        if let Node::BufferView(leaf) = self.node(node_id)? {
            self.detach_buffer_raw(leaf.buffer_id)?;
        }

        self.nodes.remove(&node_id);
        Ok(())
    }

    fn detach_buffer_raw(&mut self, buffer_id: BufferId) -> Result<()> {
        self.buffer_mut(buffer_id)?.attachment = BufferAttachment::Detached;
        Ok(())
    }

    fn discard_buffer_view(&mut self, node_id: NodeId) {
        if self.node_parent(node_id).ok().flatten().is_some() && self.close_node(node_id).is_ok() {
            return;
        }

        let buffer_id = match self.node(node_id) {
            Ok(Node::BufferView(leaf)) => Some(leaf.buffer_id),
            _ => None,
        };
        if let Some(buffer_id) = buffer_id {
            let _ = self.detach_buffer_raw(buffer_id);
        }
        self.nodes.remove(&node_id);
    }

    fn ensure_leaf_is_focusable(&self, session_id: SessionId, leaf_id: NodeId) -> Result<()> {
        let root = self.top_root_for_node(leaf_id)?;
        if root == self.session(session_id)?.root_node {
            return Ok(());
        }

        let Some(floating_id) = self.floating_id_by_root(root) else {
            return Err(MuxError::invalid_input(format!(
                "leaf {leaf_id} is not attached to session {session_id} layout"
            )));
        };
        if !self.floating_window(floating_id)?.visible {
            return Err(MuxError::invalid_input(format!(
                "leaf {leaf_id} is inside hidden floating window {floating_id}"
            )));
        }
        Ok(())
    }

    fn remove_floating_window(&mut self, floating_id: FloatingId) -> Result<FloatingWindow> {
        let floating = self
            .floating
            .remove(&floating_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown floating window {floating_id}")))?;
        if let Some(session) = self.sessions.get_mut(&floating.session_id) {
            session
                .floating
                .retain(|candidate| *candidate != floating_id);
            if session.focused_floating == Some(floating_id) {
                session.focused_floating = None;
            }
        }
        Ok(floating)
    }

    fn validate_subtree(
        &self,
        session_id: SessionId,
        node_id: NodeId,
        expected_parent: Option<NodeId>,
        is_session_root: bool,
        seen: &mut BTreeSet<NodeId>,
    ) -> Result<()> {
        let node = self.node(node_id)?;
        if node.session_id() != session_id {
            return Err(MuxError::conflict(format!(
                "node {node_id} must belong to session {session_id}"
            )));
        }
        if node.parent() != expected_parent {
            return Err(MuxError::conflict(format!(
                "node {node_id} has parent {:?}, expected {:?}",
                node.parent(),
                expected_parent
            )));
        }
        if !seen.insert(node_id) {
            return Err(MuxError::conflict(format!(
                "node {node_id} is reachable from multiple owners"
            )));
        }

        match node {
            Node::BufferView(_) => {}
            Node::Split(split) => {
                if split.children.len() < 2 {
                    return Err(MuxError::conflict(format!(
                        "split node {node_id} must have at least two children"
                    )));
                }
                if split.sizes.len() != split.children.len() {
                    return Err(MuxError::conflict(format!(
                        "split node {node_id} has mismatched child weights"
                    )));
                }
                for child in &split.children {
                    self.validate_subtree(session_id, *child, Some(node_id), false, seen)?;
                }
            }
            Node::Tabs(tabs) => {
                if !is_session_root && tabs.tabs.is_empty() {
                    return Err(MuxError::conflict(format!(
                        "tabs node {node_id} must not be empty"
                    )));
                }
                if tabs.tabs.is_empty() {
                    if tabs.active != 0 {
                        return Err(MuxError::conflict(format!(
                            "empty tabs node {node_id} must reset active index to zero"
                        )));
                    }
                } else if tabs.active >= tabs.tabs.len() {
                    return Err(MuxError::conflict(format!(
                        "tabs node {node_id} active index is out of range"
                    )));
                }
                for tab in &tabs.tabs {
                    self.validate_subtree(session_id, tab.child, Some(node_id), false, seen)?;
                }
            }
        }

        Ok(())
    }

    fn session_mut(&mut self, session_id: SessionId) -> Result<&mut Session> {
        self.sessions
            .get_mut(&session_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown session {session_id}")))
    }

    fn buffer_mut(&mut self, buffer_id: BufferId) -> Result<&mut Buffer> {
        self.buffers
            .get_mut(&buffer_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown buffer {buffer_id}")))
    }

    fn node_mut(&mut self, node_id: NodeId) -> Result<&mut Node> {
        self.nodes
            .get_mut(&node_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown node {node_id}")))
    }

    fn floating_mut(&mut self, floating_id: FloatingId) -> Result<&mut FloatingWindow> {
        self.floating
            .get_mut(&floating_id)
            .ok_or_else(|| MuxError::not_found(format!("unknown floating window {floating_id}")))
    }
}

fn next_id_after_max(ids: impl Iterator<Item = u64>) -> u64 {
    match ids.max() {
        Some(max) => max.checked_add(1).unwrap_or_else(|| {
            panic!(
                "next_id_after_max allocator exhaustion: restored max id == u64::MAX, cannot allocate a new id"
            )
        }),
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embers_core::FloatGeometry;

    #[test]
    fn interrupt_unrecoverable_buffers_marks_created_buffers_interrupted() {
        let mut state = ServerState::new();
        let buffer_id = state.create_buffer("detached", vec!["/bin/sh".to_owned()], None);

        assert_eq!(
            state.buffer(buffer_id).expect("buffer exists").state,
            BufferState::Created
        );

        state.interrupt_unrecoverable_buffers();

        assert_eq!(
            state.buffer(buffer_id).expect("buffer exists").state,
            BufferState::Interrupted(InterruptedBuffer {
                last_known_pid: None,
            })
        );
    }

    #[test]
    fn from_persisted_rejects_unreferenced_floating_windows() {
        let mut state = ServerState::new();
        let session_id = state.create_session("main");
        let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
        let floating_id = state
            .create_floating_from_buffer_with_options(
                session_id,
                buffer_id,
                FloatGeometry {
                    x: 1,
                    y: 2,
                    width: 80,
                    height: 24,
                },
                Some("popup".to_owned()),
                true,
                true,
            )
            .expect("floating is created");

        let mut workspace = state.to_persisted();
        workspace
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id.0)
            .expect("session persists")
            .floating
            .retain(|candidate| *candidate != floating_id.0);

        let error = ServerState::from_persisted(workspace)
            .expect_err("unreferenced floating window should be rejected");
        let message = error.to_string();
        assert!(message.contains("floating window"));
        assert!(message.contains("not referenced by session"));
    }
}
