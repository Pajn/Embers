use embers_core::{MuxError, Result};
use embers_protocol::{
    BufferHistoryScope, BufferLocation, BufferRecord, BufferRecordKind, BufferRecordState,
    BufferViewRecord, FloatingRecord, NodeRecord, NodeRecordKind, SessionRecord, SessionSnapshot,
    SplitRecord, TabRecord, TabsRecord,
};

use crate::model::{
    Buffer, BufferAttachment, BufferKind, BufferState, FloatingWindow, HelperBufferScope, Node,
    Session,
};
use crate::state::ServerState;

pub fn session_record(session: &Session) -> SessionRecord {
    SessionRecord {
        id: session.id,
        name: session.name.clone(),
        root_node_id: session.root_node,
        floating_ids: session.floating.clone(),
        focused_leaf_id: session.focused_leaf,
        focused_floating_id: session.focused_floating,
        zoomed_node_id: session.zoomed_node,
    }
}

pub fn buffer_record(buffer: &Buffer) -> BufferRecord {
    let (state, pid, exit_code) = match &buffer.state {
        BufferState::Created => (BufferRecordState::Created, None, None),
        BufferState::Running(running) => (BufferRecordState::Running, running.pid, None),
        BufferState::Interrupted(interrupted) => (
            BufferRecordState::Interrupted,
            interrupted.last_known_pid,
            None,
        ),
        BufferState::Exited(exited) => (BufferRecordState::Exited, None, exited.exit_code),
    };
    let (kind, read_only, helper_source_buffer_id, helper_scope) = match &buffer.kind {
        BufferKind::Pty => (BufferRecordKind::Pty, false, None, None),
        BufferKind::Helper(helper) => (
            BufferRecordKind::Helper,
            true,
            Some(helper.source_buffer_id),
            Some(match helper.scope {
                HelperBufferScope::Full => BufferHistoryScope::Full,
                HelperBufferScope::Visible => BufferHistoryScope::Visible,
            }),
        ),
    };

    BufferRecord {
        id: buffer.id,
        title: buffer.title.clone(),
        command: buffer.command.clone(),
        cwd: buffer
            .cwd
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        kind,
        state,
        pid,
        attachment_node_id: match buffer.attachment {
            BufferAttachment::Attached(node_id) => Some(node_id),
            BufferAttachment::Detached => None,
        },
        read_only,
        helper_source_buffer_id,
        helper_scope,
        pty_size: buffer.pty_size,
        activity: buffer.activity,
        last_snapshot_seq: buffer.last_snapshot_seq,
        exit_code,
        env: buffer.env.clone(),
    }
}

pub fn buffer_location(
    state: &ServerState,
    buffer_id: embers_core::BufferId,
) -> Result<BufferLocation> {
    let buffer = state.buffer(buffer_id)?;
    let node_id = match buffer.attachment {
        BufferAttachment::Attached(node_id) => Some(node_id),
        BufferAttachment::Detached => None,
    };
    let session_id = node_id
        .map(|node_id| state.node(node_id).map(|node| node.session_id()))
        .transpose()?;
    let floating_id = node_id
        .map(|node_id| state.floating_id_for_node(node_id))
        .transpose()?
        .flatten();

    Ok(BufferLocation {
        buffer_id,
        session_id,
        node_id,
        floating_id,
    })
}

pub fn node_record(node: &Node) -> NodeRecord {
    match node {
        Node::BufferView(view) => NodeRecord {
            id: view.id,
            session_id: view.session_id,
            parent_id: view.parent,
            kind: NodeRecordKind::BufferView,
            buffer_view: Some(BufferViewRecord {
                buffer_id: view.buffer_id,
                focused: view.view.focused,
                zoomed: view.view.zoomed,
                follow_output: view.view.follow_output,
                last_render_size: view.view.last_render_size,
            }),
            split: None,
            tabs: None,
        },
        Node::Split(split) => NodeRecord {
            id: split.id,
            session_id: split.session_id,
            parent_id: split.parent,
            kind: NodeRecordKind::Split,
            buffer_view: None,
            split: Some(SplitRecord {
                direction: split.direction,
                child_ids: split.children.clone(),
                sizes: split.sizes.clone(),
            }),
            tabs: None,
        },
        Node::Tabs(tabs) => NodeRecord {
            id: tabs.id,
            session_id: tabs.session_id,
            parent_id: tabs.parent,
            kind: NodeRecordKind::Tabs,
            buffer_view: None,
            split: None,
            tabs: Some(TabsRecord {
                active: u32::try_from(tabs.active)
                    .expect("server tab indices fit into the protocol width"),
                tabs: tabs
                    .tabs
                    .iter()
                    .map(|tab| TabRecord {
                        title: tab.title.clone(),
                        child_id: tab.child,
                    })
                    .collect(),
            }),
        },
    }
}

pub fn floating_record(floating: &FloatingWindow) -> FloatingRecord {
    FloatingRecord {
        id: floating.id,
        session_id: floating.session_id,
        root_node_id: floating.root_node,
        title: floating.title.clone(),
        geometry: floating.geometry,
        focused: floating.focused,
        visible: floating.visible,
        close_on_empty: floating.close_on_empty,
    }
}

pub fn session_snapshot(
    state: &ServerState,
    session_id: embers_core::SessionId,
) -> Result<SessionSnapshot> {
    let session = state.session(session_id)?;
    let nodes = state
        .session_node_ids(session_id)?
        .into_iter()
        .map(|node_id| state.node(node_id).map(node_record))
        .collect::<Result<Vec<_>>>()?;
    let buffers = state
        .session_buffer_ids(session_id)?
        .into_iter()
        .map(|buffer_id| state.buffer(buffer_id).map(buffer_record))
        .collect::<Result<Vec<_>>>()?;
    let floating = session
        .floating
        .iter()
        .map(|floating_id| state.floating_window(*floating_id).map(floating_record))
        .collect::<Result<Vec<_>>>()?;

    if !nodes.iter().any(|node| node.id == session.root_node) {
        return Err(MuxError::conflict(format!(
            "session snapshot for {} is missing its root node {}",
            session_id, session.root_node
        )));
    }

    Ok(SessionSnapshot {
        session: session_record(session),
        nodes,
        buffers,
        floating,
    })
}
