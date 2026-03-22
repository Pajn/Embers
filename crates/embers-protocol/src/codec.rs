use embers_core::{
    ActivityState, BufferId, CursorShape, CursorState, ErrorCode, FloatGeometry, FloatingId,
    NodeId, PtySize, RequestId, SessionId, SplitDirection, WireError,
};
use flatbuffers::FlatBufferBuilder;
use thiserror::Error;

use crate::framing::FrameType;
use crate::generated::embers::protocol as fb;
use crate::types::*;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("flatbuffer decode error: {0}")]
    InvalidFlatbuffer(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("invalid message: {0}")]
    InvalidMessage(&'static str),
    #[error("invalid message: {0}")]
    InvalidMessageOwned(String),
    #[error("frame exceeds max length: {0}")]
    FrameTooLarge(usize),
    #[error("invalid frame type: {0}")]
    InvalidFrameType(u8),
    #[error("unexpected frame type: {0:?}")]
    UnexpectedFrameType(FrameType),
    #[error("frame type {frame_type:?} cannot carry a {envelope_kind}")]
    UnexpectedFrameKind {
        frame_type: FrameType,
        envelope_kind: &'static str,
    },
    #[error("mismatched request id: expected {expected}, got {actual}")]
    MismatchedRequestId {
        expected: RequestId,
        actual: RequestId,
    },
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, ProtocolError> {
    value.ok_or(ProtocolError::InvalidMessage(field))
}

fn create_string_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    values: &[String],
) -> flatbuffers::WIPOffset<flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<&'a str>>> {
    let strings: Vec<_> = values
        .iter()
        .map(|value| builder.create_string(value))
        .collect();
    builder.create_vector(&strings)
}

fn decode_string_map(
    keys: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<&str>>>,
    values: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<&str>>>,
    field: &'static str,
) -> Result<std::collections::BTreeMap<String, String>, ProtocolError> {
    let Some(keys) = keys else {
        return Ok(std::collections::BTreeMap::new());
    };
    let Some(values) = values else {
        return Err(ProtocolError::InvalidMessageOwned(format!(
            "{field} is missing matching values"
        )));
    };
    if keys.len() != values.len() {
        return Err(ProtocolError::InvalidMessageOwned(format!(
            "{field} has mismatched key/value lengths"
        )));
    }
    Ok(keys
        .iter()
        .zip(values.iter())
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect())
}

fn encode_cursor_state<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    cursor: &CursorState,
) -> flatbuffers::WIPOffset<fb::CursorState<'a>> {
    let shape = match cursor.shape {
        CursorShape::Block => fb::CursorShapeWire::Block,
        CursorShape::Underline => fb::CursorShapeWire::Underline,
        CursorShape::Beam => fb::CursorShapeWire::Beam,
    };
    fb::CursorState::create(
        builder,
        &fb::CursorStateArgs {
            row: cursor.position.row,
            col: cursor.position.col,
            shape,
        },
    )
}

fn encode_buffer_history_scope(scope: BufferHistoryScope) -> fb::BufferHistoryScopeWire {
    match scope {
        BufferHistoryScope::Full => fb::BufferHistoryScopeWire::Full,
        BufferHistoryScope::Visible => fb::BufferHistoryScopeWire::Visible,
    }
}

fn decode_buffer_history_scope(
    scope: fb::BufferHistoryScopeWire,
) -> Result<BufferHistoryScope, ProtocolError> {
    match scope {
        fb::BufferHistoryScopeWire::Full => Ok(BufferHistoryScope::Full),
        fb::BufferHistoryScopeWire::Visible => Ok(BufferHistoryScope::Visible),
        _ => Err(ProtocolError::InvalidMessage(
            "unknown buffer history scope",
        )),
    }
}

fn encode_buffer_history_placement(
    placement: BufferHistoryPlacement,
) -> fb::BufferHistoryPlacementWire {
    match placement {
        BufferHistoryPlacement::Tab => fb::BufferHistoryPlacementWire::Tab,
        BufferHistoryPlacement::Floating => fb::BufferHistoryPlacementWire::Floating,
    }
}

fn decode_buffer_history_placement(
    placement: fb::BufferHistoryPlacementWire,
) -> Result<BufferHistoryPlacement, ProtocolError> {
    match placement {
        fb::BufferHistoryPlacementWire::Tab => Ok(BufferHistoryPlacement::Tab),
        fb::BufferHistoryPlacementWire::Floating => Ok(BufferHistoryPlacement::Floating),
        _ => Err(ProtocolError::InvalidMessage(
            "unknown buffer history placement",
        )),
    }
}

fn encode_node_break_destination(
    destination: NodeBreakDestination,
) -> fb::NodeBreakDestinationWire {
    match destination {
        NodeBreakDestination::Tab => fb::NodeBreakDestinationWire::Tab,
        NodeBreakDestination::Floating => fb::NodeBreakDestinationWire::Floating,
    }
}

fn decode_node_break_destination(
    destination: fb::NodeBreakDestinationWire,
) -> Result<NodeBreakDestination, ProtocolError> {
    match destination {
        fb::NodeBreakDestinationWire::Tab => Ok(NodeBreakDestination::Tab),
        fb::NodeBreakDestinationWire::Floating => Ok(NodeBreakDestination::Floating),
        _ => Err(ProtocolError::InvalidMessage(
            "unknown node break destination",
        )),
    }
}

fn encode_node_join_placement(placement: NodeJoinPlacement) -> fb::NodeJoinPlacementWire {
    match placement {
        NodeJoinPlacement::Left => fb::NodeJoinPlacementWire::Left,
        NodeJoinPlacement::Right => fb::NodeJoinPlacementWire::Right,
        NodeJoinPlacement::Up => fb::NodeJoinPlacementWire::Up,
        NodeJoinPlacement::Down => fb::NodeJoinPlacementWire::Down,
        NodeJoinPlacement::TabBefore => fb::NodeJoinPlacementWire::TabBefore,
        NodeJoinPlacement::TabAfter => fb::NodeJoinPlacementWire::TabAfter,
    }
}

fn decode_node_join_placement(
    placement: fb::NodeJoinPlacementWire,
) -> Result<NodeJoinPlacement, ProtocolError> {
    match placement {
        fb::NodeJoinPlacementWire::Left => Ok(NodeJoinPlacement::Left),
        fb::NodeJoinPlacementWire::Right => Ok(NodeJoinPlacement::Right),
        fb::NodeJoinPlacementWire::Up => Ok(NodeJoinPlacement::Up),
        fb::NodeJoinPlacementWire::Down => Ok(NodeJoinPlacement::Down),
        fb::NodeJoinPlacementWire::TabBefore => Ok(NodeJoinPlacement::TabBefore),
        fb::NodeJoinPlacementWire::TabAfter => Ok(NodeJoinPlacement::TabAfter),
        _ => Err(ProtocolError::InvalidMessage("unknown node join placement")),
    }
}

fn decode_cursor_state(cursor: fb::CursorState<'_>) -> Result<CursorState, ProtocolError> {
    let shape = match cursor.shape() {
        fb::CursorShapeWire::Block => CursorShape::Block,
        fb::CursorShapeWire::Underline => CursorShape::Underline,
        fb::CursorShapeWire::Beam => CursorShape::Beam,
        _ => return Err(ProtocolError::InvalidMessage("unknown cursor shape")),
    };
    Ok(CursorState {
        position: embers_core::CursorPosition {
            row: cursor.row(),
            col: cursor.col(),
        },
        shape,
    })
}

// ==================== ENCODING ====================

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>, ProtocolError> {
    let mut builder = FlatBufferBuilder::new();

    let envelope = match message {
        ClientMessage::Ping(req) => encode_ping_request(&mut builder, req),
        ClientMessage::Session(req) => encode_session_request(&mut builder, req),
        ClientMessage::Buffer(req) => encode_buffer_request(&mut builder, req),
        ClientMessage::Node(req) => encode_node_request(&mut builder, req),
        ClientMessage::Floating(req) => encode_floating_request(&mut builder, req),
        ClientMessage::Input(req) => encode_input_request(&mut builder, req),
        ClientMessage::Subscribe(req) => encode_subscribe_request(&mut builder, req),
        ClientMessage::Unsubscribe(req) => encode_unsubscribe_request(&mut builder, req),
        ClientMessage::Client(req) => encode_client_request(&mut builder, req),
    };

    builder.finish(envelope, Some("EMBR"));
    Ok(builder.finished_data().to_vec())
}

fn encode_ping_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &PingRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let payload = builder.create_string(&req.payload);
    let ping_request = fb::PingRequest::create(
        builder,
        &fb::PingRequestArgs {
            payload: Some(payload),
        },
    );
    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id.into(),
            kind: fb::MessageKind::PingRequest,
            ping_request: Some(ping_request),
            ..Default::default()
        },
    )
}

fn encode_client_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &ClientRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (op, client_id, session_id) = match req {
        ClientRequest::List { .. } => (fb::ClientOp::List, 0, 0),
        ClientRequest::Get { client_id, .. } => (
            fb::ClientOp::Get,
            client_id.map(|id| id.get()).unwrap_or(0),
            0,
        ),
        ClientRequest::Detach { client_id, .. } => (
            fb::ClientOp::Detach,
            client_id.map(|id| id.get()).unwrap_or(0),
            0,
        ),
        ClientRequest::Switch {
            client_id,
            session_id,
            ..
        } => (
            fb::ClientOp::Switch,
            client_id.map(|id| id.get()).unwrap_or(0),
            (*session_id).into(),
        ),
    };

    let client_req = fb::ClientRequest::create(
        builder,
        &fb::ClientRequestArgs {
            op,
            client_id,
            session_id,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::ClientRequest,
            client_request: Some(client_req),
            ..Default::default()
        },
    )
}

fn encode_session_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &SessionRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (op, session_id, buffer_id, child_node_id, name_str, title_str, force, index) = match req {
        SessionRequest::Create { name, .. } => (
            fb::SessionOp::Create,
            0,
            0,
            0,
            Some(name.as_str()),
            None,
            false,
            0,
        ),
        SessionRequest::List { .. } => (fb::SessionOp::List, 0, 0, 0, None, None, false, 0),
        SessionRequest::Get { session_id, .. } => (
            fb::SessionOp::Get,
            (*session_id).into(),
            0,
            0,
            None,
            None,
            false,
            0,
        ),
        SessionRequest::Close {
            session_id, force, ..
        } => (
            fb::SessionOp::Close,
            (*session_id).into(),
            0,
            0,
            None,
            None,
            *force,
            0,
        ),
        SessionRequest::Rename {
            session_id, name, ..
        } => (
            fb::SessionOp::Rename,
            (*session_id).into(),
            0,
            0,
            Some(name.as_str()),
            None,
            false,
            0,
        ),
        SessionRequest::AddRootTab {
            session_id,
            title,
            buffer_id,
            child_node_id,
            ..
        } => (
            fb::SessionOp::AddRootTab,
            (*session_id).into(),
            buffer_id.map_or(0, u64::from),
            child_node_id.map_or(0, u64::from),
            None,
            Some(title.as_str()),
            false,
            0,
        ),
        SessionRequest::SelectRootTab {
            session_id, index, ..
        } => (
            fb::SessionOp::SelectRootTab,
            (*session_id).into(),
            0,
            0,
            None,
            None,
            false,
            *index,
        ),
        SessionRequest::RenameRootTab {
            session_id,
            index,
            title,
            ..
        } => (
            fb::SessionOp::RenameRootTab,
            (*session_id).into(),
            0,
            0,
            None,
            Some(title.as_str()),
            false,
            *index,
        ),
        SessionRequest::CloseRootTab {
            session_id, index, ..
        } => (
            fb::SessionOp::CloseRootTab,
            (*session_id).into(),
            0,
            0,
            None,
            None,
            false,
            *index,
        ),
    };

    let name = name_str.map(|s| builder.create_string(s));
    let title = title_str.map(|s| builder.create_string(s));
    let session_req = fb::SessionRequest::create(
        builder,
        &fb::SessionRequestArgs {
            op,
            session_id,
            buffer_id,
            child_node_id,
            name,
            title,
            force,
            index,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::SessionRequest,
            session_request: Some(session_req),
            ..Default::default()
        },
    )
}

fn encode_buffer_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &BufferRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (
        op,
        buffer_id,
        session_id,
        client_id,
        attached_only,
        detached_only,
        force,
        start_line,
        line_count,
        history_scope,
        history_placement,
        title_str,
        command_vec,
        cwd_str,
        env_entries,
    ) = match req {
        BufferRequest::Create {
            title,
            command,
            cwd,
            env,
            ..
        } => (
            fb::BufferOp::Create,
            0,
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            title.as_deref(),
            Some(command),
            cwd.as_deref(),
            Some(env),
        ),
        BufferRequest::List {
            session_id,
            attached_only,
            detached_only,
            ..
        } => (
            fb::BufferOp::List,
            0,
            session_id.map(|s| s.into()).unwrap_or(0),
            0,
            *attached_only,
            *detached_only,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::Get { buffer_id, .. } => (
            fb::BufferOp::Get,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::Detach { buffer_id, .. } => (
            fb::BufferOp::Detach,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::Kill {
            buffer_id, force, ..
        } => (
            fb::BufferOp::Kill,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            *force,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::Capture { buffer_id, .. } => (
            fb::BufferOp::Capture,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::CaptureVisible { buffer_id, .. } => (
            fb::BufferOp::CaptureVisible,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::ScrollbackSlice {
            buffer_id,
            start_line,
            line_count,
            ..
        } => (
            fb::BufferOp::ScrollbackSlice,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            *start_line,
            *line_count,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::GetLocation { buffer_id, .. } => (
            fb::BufferOp::GetLocation,
            (*buffer_id).into(),
            0,
            0,
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::Reveal {
            buffer_id,
            client_id,
            ..
        } => (
            fb::BufferOp::Reveal,
            (*buffer_id).into(),
            0,
            client_id.unwrap_or(0),
            false,
            false,
            false,
            0,
            0,
            fb::BufferHistoryScopeWire::Full,
            fb::BufferHistoryPlacementWire::Tab,
            None,
            None,
            None,
            None,
        ),
        BufferRequest::OpenHistory {
            buffer_id,
            scope,
            placement,
            client_id,
            ..
        } => (
            fb::BufferOp::OpenHistory,
            (*buffer_id).into(),
            0,
            client_id.unwrap_or(0),
            false,
            false,
            false,
            0,
            0,
            encode_buffer_history_scope(*scope),
            encode_buffer_history_placement(*placement),
            None,
            None,
            None,
            None,
        ),
    };

    let title = title_str.map(|s| builder.create_string(s));
    let cwd = cwd_str.map(|s| builder.create_string(s));
    let command = command_vec.map(|cmd_vec| {
        let strings: Vec<_> = cmd_vec.iter().map(|s| builder.create_string(s)).collect();
        builder.create_vector(&strings)
    });
    let env_keys = env_entries.map(|env| {
        let keys = env.keys().cloned().collect::<Vec<_>>();
        create_string_vector(builder, &keys)
    });
    let env_values = env_entries.map(|env| {
        let values = env.values().cloned().collect::<Vec<_>>();
        create_string_vector(builder, &values)
    });

    let buffer_req = fb::BufferRequest::create(
        builder,
        &fb::BufferRequestArgs {
            op,
            buffer_id,
            session_id,
            client_id,
            attached_only,
            detached_only,
            force,
            start_line,
            line_count,
            history_scope,
            history_placement,
            title,
            command,
            cwd,
            env_keys,
            env_values,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::BufferRequest,
            buffer_request: Some(buffer_req),
            ..Default::default()
        },
    )
}

fn encode_node_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &NodeRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    type EncodedNodeRequest<'a> = (
        fb::NodeOp,
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        Option<&'a str>,
        u32,
        u32,
        fb::SplitDirectionWire,
        fb::NodeBreakDestinationWire,
        fb::NodeJoinPlacementWire,
        Option<&'a Vec<u16>>,
        Option<Vec<u64>>,
        Option<Vec<String>>,
        bool,
        u64,
        u64,
        u64,
    );

    let (
        op,
        session_id,
        node_id,
        leaf_node_id,
        tabs_node_id,
        child_node_id,
        target_leaf_node_id,
        buffer_id,
        new_buffer_id,
        title_str,
        index,
        active,
        direction,
        break_destination,
        join_placement,
        sizes_vec,
        child_node_ids_vec,
        titles_vec,
        insert_before,
        first_node_id,
        second_node_id,
        sibling_node_id,
    ): EncodedNodeRequest<'_> = match req {
        NodeRequest::GetTree { session_id, .. } => (
            fb::NodeOp::GetTree,
            (*session_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Split {
            leaf_node_id,
            direction,
            new_buffer_id,
            ..
        } => {
            let dir = match direction {
                SplitDirection::Horizontal => fb::SplitDirectionWire::Horizontal,
                SplitDirection::Vertical => fb::SplitDirectionWire::Vertical,
            };
            (
                fb::NodeOp::Split,
                0,
                0,
                (*leaf_node_id).into(),
                0,
                0,
                0,
                0,
                (*new_buffer_id).into(),
                None,
                0,
                0,
                dir,
                fb::NodeBreakDestinationWire::Tab,
                fb::NodeJoinPlacementWire::Left,
                None,
                None,
                None,
                false,
                0,
                0,
                0,
            )
        }
        NodeRequest::CreateSplit {
            session_id,
            direction,
            child_node_ids,
            sizes,
            ..
        } => {
            let dir = match direction {
                SplitDirection::Horizontal => fb::SplitDirectionWire::Horizontal,
                SplitDirection::Vertical => fb::SplitDirectionWire::Vertical,
            };
            (
                fb::NodeOp::CreateSplit,
                (*session_id).into(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                None,
                0,
                0,
                dir,
                fb::NodeBreakDestinationWire::Tab,
                fb::NodeJoinPlacementWire::Left,
                Some(sizes),
                Some(
                    child_node_ids
                        .iter()
                        .map(|node_id| u64::from(*node_id))
                        .collect::<Vec<_>>(),
                ),
                None,
                false,
                0,
                0,
                0,
            )
        }
        NodeRequest::CreateTabs {
            session_id,
            child_node_ids,
            titles,
            active,
            ..
        } => (
            fb::NodeOp::CreateTabs,
            (*session_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            *active,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            Some(
                child_node_ids
                    .iter()
                    .map(|node_id| u64::from(*node_id))
                    .collect::<Vec<_>>(),
            ),
            Some(titles.clone()),
            false,
            0,
            0,
            0,
        ),
        NodeRequest::ReplaceNode {
            node_id,
            child_node_id,
            ..
        } => (
            fb::NodeOp::ReplaceNode,
            0,
            (*node_id).into(),
            0,
            0,
            (*child_node_id).into(),
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::WrapInSplit {
            node_id,
            child_node_id,
            direction,
            insert_before,
            ..
        } => {
            let dir = match direction {
                SplitDirection::Horizontal => fb::SplitDirectionWire::Horizontal,
                SplitDirection::Vertical => fb::SplitDirectionWire::Vertical,
            };
            (
                fb::NodeOp::WrapInSplit,
                0,
                (*node_id).into(),
                0,
                0,
                (*child_node_id).into(),
                0,
                0,
                0,
                None,
                0,
                0,
                dir,
                fb::NodeBreakDestinationWire::Tab,
                fb::NodeJoinPlacementWire::Left,
                None,
                None,
                None,
                *insert_before,
                0,
                0,
                0,
            )
        }
        NodeRequest::WrapInTabs { node_id, title, .. } => (
            fb::NodeOp::WrapInTabs,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            Some(title.as_str()),
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::AddTab {
            tabs_node_id,
            title,
            buffer_id,
            child_node_id,
            index,
            ..
        } => (
            fb::NodeOp::AddTab,
            0,
            0,
            0,
            (*tabs_node_id).into(),
            child_node_id.map_or(0, u64::from),
            0,
            buffer_id.map_or(0, u64::from),
            0,
            Some(title.as_str()),
            *index,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::SelectTab {
            tabs_node_id,
            index,
            ..
        } => (
            fb::NodeOp::SelectTab,
            0,
            0,
            0,
            (*tabs_node_id).into(),
            0,
            0,
            0,
            0,
            None,
            *index,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Focus {
            session_id,
            node_id,
            ..
        } => (
            fb::NodeOp::Focus,
            (*session_id).into(),
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Close { node_id, .. } => (
            fb::NodeOp::Close,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::MoveBufferToNode {
            buffer_id,
            target_leaf_node_id,
            ..
        } => (
            fb::NodeOp::MoveBufferToNode,
            0,
            0,
            0,
            0,
            0,
            (*target_leaf_node_id).into(),
            (*buffer_id).into(),
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Resize { node_id, sizes, .. } => (
            fb::NodeOp::Resize,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            Some(sizes),
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Zoom { node_id, .. } => (
            fb::NodeOp::Zoom,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::Unzoom { session_id, .. } => (
            fb::NodeOp::Unzoom,
            (*session_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::ToggleZoom { node_id, .. } => (
            fb::NodeOp::ToggleZoom,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::SwapSiblings {
            first_node_id,
            second_node_id,
            ..
        } => (
            fb::NodeOp::SwapSiblings,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            (*first_node_id).into(),
            (*second_node_id).into(),
            0,
        ),
        NodeRequest::BreakNode {
            node_id,
            destination,
            ..
        } => (
            fb::NodeOp::BreakNode,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            encode_node_break_destination(*destination),
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::JoinBufferAtNode {
            node_id,
            buffer_id,
            placement,
            ..
        } => (
            fb::NodeOp::JoinBufferAtNode,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            (*buffer_id).into(),
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            encode_node_join_placement(*placement),
            None,
            None,
            None,
            false,
            0,
            0,
            0,
        ),
        NodeRequest::MoveNodeBefore {
            node_id,
            sibling_node_id,
            ..
        } => (
            fb::NodeOp::MoveNodeBefore,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            (*sibling_node_id).into(),
        ),
        NodeRequest::MoveNodeAfter {
            node_id,
            sibling_node_id,
            ..
        } => (
            fb::NodeOp::MoveNodeAfter,
            0,
            (*node_id).into(),
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            fb::SplitDirectionWire::Horizontal,
            fb::NodeBreakDestinationWire::Tab,
            fb::NodeJoinPlacementWire::Left,
            None,
            None,
            None,
            false,
            0,
            0,
            (*sibling_node_id).into(),
        ),
    };

    let title = title_str.map(|s| builder.create_string(s));
    let sizes = sizes_vec.map(|sizes| builder.create_vector(sizes));
    let child_node_ids = child_node_ids_vec.map(|ids| builder.create_vector(&ids));
    let titles = titles_vec.map(|values| create_string_vector(builder, &values));
    let node_req = fb::NodeRequest::create(
        builder,
        &fb::NodeRequestArgs {
            op,
            session_id,
            node_id,
            leaf_node_id,
            tabs_node_id,
            child_node_id,
            target_leaf_node_id,
            buffer_id,
            new_buffer_id,
            title,
            index,
            active,
            direction,
            break_destination,
            join_placement,
            sizes,
            child_node_ids,
            titles,
            insert_before,
            first_node_id,
            second_node_id,
            sibling_node_id,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::NodeRequest,
            node_request: Some(node_req),
            ..Default::default()
        },
    )
}

fn encode_floating_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &FloatingRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (
        op,
        floating_id,
        session_id,
        root_node_id,
        buffer_id,
        title_str,
        geom,
        focus,
        close_on_empty,
    ) = match req {
        FloatingRequest::Create {
            session_id,
            root_node_id,
            buffer_id,
            geometry,
            title,
            focus,
            close_on_empty,
            ..
        } => (
            fb::FloatingOp::Create,
            0,
            (*session_id).into(),
            root_node_id.map_or(0, u64::from),
            buffer_id.map_or(0, u64::from),
            title.as_deref(),
            Some(*geometry),
            *focus,
            *close_on_empty,
        ),
        FloatingRequest::Close { floating_id, .. } => (
            fb::FloatingOp::Close,
            (*floating_id).into(),
            0,
            0,
            0,
            None,
            None,
            true,
            true,
        ),
        FloatingRequest::Move {
            floating_id,
            geometry,
            ..
        } => (
            fb::FloatingOp::Move,
            (*floating_id).into(),
            0,
            0,
            0,
            None,
            Some(*geometry),
            true,
            true,
        ),
        FloatingRequest::Focus { floating_id, .. } => (
            fb::FloatingOp::Focus,
            (*floating_id).into(),
            0,
            0,
            0,
            None,
            None,
            true,
            true,
        ),
    };

    let title = title_str.map(|s| builder.create_string(s));
    let (x, y, width, height) = geom
        .map(|g| (g.x, g.y, g.width, g.height))
        .unwrap_or((0, 0, 0, 0));

    let floating_req = fb::FloatingRequest::create(
        builder,
        &fb::FloatingRequestArgs {
            op,
            floating_id,
            session_id,
            root_node_id,
            buffer_id,
            title,
            x,
            y,
            width,
            height,
            focus,
            close_on_empty,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::FloatingRequest,
            floating_request: Some(floating_req),
            ..Default::default()
        },
    )
}

fn encode_input_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &InputRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (op, buffer_id, bytes_vec, cols, rows) = match req {
        InputRequest::Send {
            buffer_id, bytes, ..
        } => (fb::InputOp::Send, (*buffer_id).into(), Some(bytes), 0, 0),
        InputRequest::Resize {
            buffer_id,
            cols,
            rows,
            ..
        } => (fb::InputOp::Resize, (*buffer_id).into(), None, *cols, *rows),
    };

    let bytes = bytes_vec.map(|b| builder.create_vector(b));
    let input_req = fb::InputRequest::create(
        builder,
        &fb::InputRequestArgs {
            op,
            buffer_id,
            bytes,
            cols,
            rows,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id().into(),
            kind: fb::MessageKind::InputRequest,
            input_request: Some(input_req),
            ..Default::default()
        },
    )
}

fn encode_subscribe_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &SubscribeRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let subscribe_req = fb::SubscribeRequest::create(
        builder,
        &fb::SubscribeRequestArgs {
            session_id: req.session_id.map(|s| s.into()).unwrap_or(0),
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id.into(),
            kind: fb::MessageKind::SubscribeRequest,
            subscribe_request: Some(subscribe_req),
            ..Default::default()
        },
    )
}

fn encode_unsubscribe_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &UnsubscribeRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let unsubscribe_req = fb::UnsubscribeRequest::create(
        builder,
        &fb::UnsubscribeRequestArgs {
            subscription_id: req.subscription_id,
        },
    );

    fb::Envelope::create(
        builder,
        &fb::EnvelopeArgs {
            request_id: req.request_id.into(),
            kind: fb::MessageKind::UnsubscribeRequest,
            unsubscribe_request: Some(unsubscribe_req),
            ..Default::default()
        },
    )
}

pub fn encode_server_envelope(envelope: &ServerEnvelope) -> Result<Vec<u8>, ProtocolError> {
    let mut builder = FlatBufferBuilder::new();

    let fb_envelope = match envelope {
        ServerEnvelope::Response(response) => encode_server_response(&mut builder, response),
        ServerEnvelope::Event(event) => encode_server_event(&mut builder, event),
    };

    builder.finish(fb_envelope, Some("EMBR"));
    Ok(builder.finished_data().to_vec())
}

fn encode_server_response<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    response: &ServerResponse,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    match response {
        ServerResponse::Pong(r) => {
            let payload = builder.create_string(&r.payload);
            let pong = fb::PingResponse::create(
                builder,
                &fb::PingResponseArgs {
                    payload: Some(payload),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::PingResponse,
                    ping_response: Some(pong),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Ok(r) => {
            let ok = fb::OkResponse::create(builder, &fb::OkResponseArgs {});
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::OkResponse,
                    ok_response: Some(ok),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Error(r) => {
            let msg = builder.create_string(&r.error.message);
            let err = fb::ErrorResponse::create(
                builder,
                &fb::ErrorResponseArgs {
                    code: encode_error_code(r.error.code),
                    message: Some(msg),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.map(|r| r.into()).unwrap_or(0),
                    kind: fb::MessageKind::ErrorResponse,
                    error_response: Some(err),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Sessions(r) => {
            let sessions_vec: Vec<_> = r
                .sessions
                .iter()
                .map(|s| encode_session_record(builder, s))
                .collect();
            let sessions = builder.create_vector(&sessions_vec);
            let sessions_resp = fb::SessionsResponse::create(
                builder,
                &fb::SessionsResponseArgs {
                    sessions: Some(sessions),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::SessionsResponse,
                    sessions_response: Some(sessions_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::SessionSnapshot(r) => {
            let snapshot = encode_session_snapshot(builder, &r.snapshot);
            let snapshot_resp = fb::SessionSnapshotResponse::create(
                builder,
                &fb::SessionSnapshotResponseArgs {
                    snapshot: Some(snapshot),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::SessionSnapshotResponse,
                    session_snapshot_response: Some(snapshot_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Buffers(r) => {
            let buffers_vec: Vec<_> = r
                .buffers
                .iter()
                .map(|b| encode_buffer_record(builder, b))
                .collect();
            let buffers = builder.create_vector(&buffers_vec);
            let buffers_resp = fb::BuffersResponse::create(
                builder,
                &fb::BuffersResponseArgs {
                    buffers: Some(buffers),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::BuffersResponse,
                    buffers_response: Some(buffers_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Buffer(r) => {
            let buffer = encode_buffer_record(builder, &r.buffer);
            let buffer_resp = fb::BufferResponse::create(
                builder,
                &fb::BufferResponseArgs {
                    buffer: Some(buffer),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::BufferResponse,
                    buffer_response: Some(buffer_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::FloatingList(r) => {
            let floating_vec: Vec<_> = r
                .floating
                .iter()
                .map(|f| encode_floating_record(builder, f))
                .collect();
            let floating = builder.create_vector(&floating_vec);
            let floating_resp = fb::FloatingListResponse::create(
                builder,
                &fb::FloatingListResponseArgs {
                    floating: Some(floating),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::FloatingListResponse,
                    floating_list_response: Some(floating_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Floating(r) => {
            let floating = encode_floating_record(builder, &r.floating);
            let floating_resp = fb::FloatingResponse::create(
                builder,
                &fb::FloatingResponseArgs {
                    floating: Some(floating),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::FloatingResponse,
                    floating_response: Some(floating_resp),
                    ..Default::default()
                },
            )
        }
        ServerResponse::SubscriptionAck(r) => {
            let ack = fb::SubscriptionAckResponse::create(
                builder,
                &fb::SubscriptionAckResponseArgs {
                    subscription_id: r.subscription_id,
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::SubscriptionAckResponse,
                    subscription_ack_response: Some(ack),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Clients(r) => {
            let clients_vec: Vec<_> = r
                .clients
                .iter()
                .map(|client| encode_client_record(builder, client))
                .collect();
            let clients = builder.create_vector(&clients_vec);
            let response = fb::ClientsResponse::create(
                builder,
                &fb::ClientsResponseArgs {
                    clients: Some(clients),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::ClientsResponse,
                    clients_response: Some(response),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Client(r) => {
            let client = encode_client_record(builder, &r.client);
            let response = fb::ClientResponse::create(
                builder,
                &fb::ClientResponseArgs {
                    client: Some(client),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::ClientResponse,
                    client_response: Some(response),
                    ..Default::default()
                },
            )
        }
        ServerResponse::BufferLocation(r) => {
            let location = fb::BufferLocation::create(
                builder,
                &fb::BufferLocationArgs {
                    buffer_id: r.location.buffer_id.into(),
                    session_id: r.location.session_id.map(|id| id.into()).unwrap_or(0),
                    node_id: r.location.node_id.map(|id| id.into()).unwrap_or(0),
                    floating_id: r.location.floating_id.map(|id| id.into()).unwrap_or(0),
                },
            );
            let response = fb::BufferLocationResponse::create(
                builder,
                &fb::BufferLocationResponseArgs {
                    location: Some(location),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::BufferLocationResponse,
                    buffer_location_response: Some(response),
                    ..Default::default()
                },
            )
        }
        ServerResponse::Snapshot(r) => {
            let title = r.title.as_ref().map(|t| builder.create_string(t));
            let cwd = r.cwd.as_ref().map(|c| builder.create_string(c));
            let lines_vec: Vec<_> = r.lines.iter().map(|l| builder.create_string(l)).collect();
            let lines = builder.create_vector(&lines_vec);
            let snapshot = fb::SnapshotResponse::create(
                builder,
                &fb::SnapshotResponseArgs {
                    buffer_id: r.buffer_id.into(),
                    sequence: r.sequence,
                    cols: r.size.cols,
                    rows: r.size.rows,
                    lines: Some(lines),
                    title,
                    cwd,
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::SnapshotResponse,
                    snapshot_response: Some(snapshot),
                    ..Default::default()
                },
            )
        }
        ServerResponse::VisibleSnapshot(r) => {
            let title = r.title.as_ref().map(|t| builder.create_string(t));
            let cwd = r.cwd.as_ref().map(|c| builder.create_string(c));
            let lines_vec: Vec<_> = r.lines.iter().map(|l| builder.create_string(l)).collect();
            let lines = builder.create_vector(&lines_vec);
            let cursor = r
                .cursor
                .as_ref()
                .map(|cursor| encode_cursor_state(builder, cursor));
            let snapshot = fb::VisibleSnapshotResponse::create(
                builder,
                &fb::VisibleSnapshotResponseArgs {
                    buffer_id: r.buffer_id.into(),
                    sequence: r.sequence,
                    cols: r.size.cols,
                    rows: r.size.rows,
                    lines: Some(lines),
                    title,
                    cwd,
                    viewport_top_line: r.viewport_top_line,
                    total_lines: r.total_lines,
                    alternate_screen: r.alternate_screen,
                    mouse_reporting: r.mouse_reporting,
                    focus_reporting: r.focus_reporting,
                    bracketed_paste: r.bracketed_paste,
                    cursor,
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::VisibleSnapshotResponse,
                    visible_snapshot_response: Some(snapshot),
                    ..Default::default()
                },
            )
        }
        ServerResponse::ScrollbackSlice(r) => {
            let lines_vec: Vec<_> = r.lines.iter().map(|l| builder.create_string(l)).collect();
            let lines = builder.create_vector(&lines_vec);
            let snapshot = fb::ScrollbackSliceResponse::create(
                builder,
                &fb::ScrollbackSliceResponseArgs {
                    buffer_id: r.buffer_id.into(),
                    start_line: r.start_line,
                    total_lines: r.total_lines,
                    lines: Some(lines),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: r.request_id.into(),
                    kind: fb::MessageKind::ScrollbackSliceResponse,
                    scrollback_slice_response: Some(snapshot),
                    ..Default::default()
                },
            )
        }
    }
}

fn encode_server_event<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    event: &ServerEvent,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    match event {
        ServerEvent::SessionCreated(e) => {
            let session = encode_session_record(builder, &e.session);
            let event = fb::SessionCreatedEvent::create(
                builder,
                &fb::SessionCreatedEventArgs {
                    session: Some(session),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::SessionCreatedEvent,
                    session_created_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::SessionClosed(e) => {
            let event = fb::SessionClosedEvent::create(
                builder,
                &fb::SessionClosedEventArgs {
                    session_id: e.session_id.into(),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::SessionClosedEvent,
                    session_closed_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::SessionRenamed(e) => {
            let name = builder.create_string(&e.name);
            let event = fb::SessionRenamedEvent::create(
                builder,
                &fb::SessionRenamedEventArgs {
                    session_id: e.session_id.into(),
                    name: Some(name),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::SessionRenamedEvent,
                    session_renamed_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::BufferCreated(e) => {
            let buffer = encode_buffer_record(builder, &e.buffer);
            let event = fb::BufferCreatedEvent::create(
                builder,
                &fb::BufferCreatedEventArgs {
                    buffer: Some(buffer),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::BufferCreatedEvent,
                    buffer_created_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::BufferDetached(e) => {
            let event = fb::BufferDetachedEvent::create(
                builder,
                &fb::BufferDetachedEventArgs {
                    buffer_id: e.buffer_id.into(),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::BufferDetachedEvent,
                    buffer_detached_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::NodeChanged(e) => {
            let event = fb::NodeChangedEvent::create(
                builder,
                &fb::NodeChangedEventArgs {
                    session_id: e.session_id.into(),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::NodeChangedEvent,
                    node_changed_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::FloatingChanged(e) => {
            let event = fb::FloatingChangedEvent::create(
                builder,
                &fb::FloatingChangedEventArgs {
                    session_id: e.session_id.into(),
                    floating_id: e.floating_id.map(|f| f.into()).unwrap_or(0),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::FloatingChangedEvent,
                    floating_changed_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::FocusChanged(e) => {
            let event = fb::FocusChangedEvent::create(
                builder,
                &fb::FocusChangedEventArgs {
                    session_id: e.session_id.into(),
                    focused_leaf_id: e.focused_leaf_id.map(|n| n.into()).unwrap_or(0),
                    focused_floating_id: e.focused_floating_id.map(|f| f.into()).unwrap_or(0),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::FocusChangedEvent,
                    focus_changed_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::RenderInvalidated(e) => {
            let event = fb::RenderInvalidatedEvent::create(
                builder,
                &fb::RenderInvalidatedEventArgs {
                    buffer_id: e.buffer_id.into(),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::RenderInvalidatedEvent,
                    render_invalidated_event: Some(event),
                    ..Default::default()
                },
            )
        }
        ServerEvent::ClientChanged(e) => {
            let client = encode_client_record(builder, &e.client);
            let event = fb::ClientChangedEvent::create(
                builder,
                &fb::ClientChangedEventArgs {
                    client: Some(client),
                    previous_session_id: e.previous_session_id.map(|id| id.into()).unwrap_or(0),
                },
            );
            fb::Envelope::create(
                builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::ClientChangedEvent,
                    client_changed_event: Some(event),
                    ..Default::default()
                },
            )
        }
    }
}

// ==================== RECORD ENCODING ====================

fn encode_session_record<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    record: &SessionRecord,
) -> flatbuffers::WIPOffset<fb::SessionRecord<'a>> {
    let name = builder.create_string(&record.name);
    let floating_ids: Vec<u64> = record.floating_ids.iter().map(|&f| f.into()).collect();
    let floating_ids_vec = builder.create_vector(&floating_ids);

    fb::SessionRecord::create(
        builder,
        &fb::SessionRecordArgs {
            id: record.id.into(),
            name: Some(name),
            root_node_id: record.root_node_id.into(),
            floating_ids: Some(floating_ids_vec),
            focused_leaf_id: record.focused_leaf_id.map(|n| n.into()).unwrap_or(0),
            focused_floating_id: record.focused_floating_id.map(|f| f.into()).unwrap_or(0),
            zoomed_node_id: record.zoomed_node_id.map(|n| n.into()).unwrap_or(0),
        },
    )
}

fn encode_buffer_record<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    record: &BufferRecord,
) -> flatbuffers::WIPOffset<fb::BufferRecord<'a>> {
    let title = builder.create_string(&record.title);
    let command_vec: Vec<_> = record
        .command
        .iter()
        .map(|c| builder.create_string(c))
        .collect();
    let command = builder.create_vector(&command_vec);
    let cwd = record.cwd.as_ref().map(|c| builder.create_string(c));
    let env_keys_vec = record.env.keys().cloned().collect::<Vec<_>>();
    let env_values_vec = record.env.values().cloned().collect::<Vec<_>>();
    let env_keys = create_string_vector(builder, &env_keys_vec);
    let env_values = create_string_vector(builder, &env_values_vec);

    let state = match record.state {
        BufferRecordState::Created => fb::BufferStateWire::Created,
        BufferRecordState::Running => fb::BufferStateWire::Running,
        BufferRecordState::Interrupted => fb::BufferStateWire::Interrupted,
        BufferRecordState::Exited => fb::BufferStateWire::Exited,
    };

    let activity = match record.activity {
        ActivityState::Idle => fb::ActivityStateWire::Idle,
        ActivityState::Activity => fb::ActivityStateWire::Activity,
        ActivityState::Bell => fb::ActivityStateWire::Bell,
    };
    let kind = match record.kind {
        BufferRecordKind::Pty => fb::BufferKindWire::Pty,
        BufferRecordKind::Helper => fb::BufferKindWire::Helper,
    };
    let helper_scope = record
        .helper_scope
        .map(encode_buffer_history_scope)
        .unwrap_or(fb::BufferHistoryScopeWire::Full);

    fb::BufferRecord::create(
        builder,
        &fb::BufferRecordArgs {
            id: record.id.into(),
            title: Some(title),
            command: Some(command),
            cwd,
            kind,
            state,
            pid: record.pid.unwrap_or(0),
            has_pid: record.pid.is_some(),
            attachment_node_id: record.attachment_node_id.map(|n| n.into()).unwrap_or(0),
            read_only: record.read_only,
            helper_source_buffer_id: record
                .helper_source_buffer_id
                .map(|id| id.into())
                .unwrap_or(0),
            helper_scope,
            has_helper_scope: record.helper_scope.is_some(),
            pty_cols: record.pty_size.cols,
            pty_rows: record.pty_size.rows,
            activity,
            last_snapshot_seq: record.last_snapshot_seq,
            exit_code: record.exit_code.unwrap_or(0),
            has_exit_code: record.exit_code.is_some(),
            env_keys: Some(env_keys),
            env_values: Some(env_values),
        },
    )
}

fn encode_node_record<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    record: &NodeRecord,
) -> flatbuffers::WIPOffset<fb::NodeRecord<'a>> {
    let kind = match record.kind {
        NodeRecordKind::BufferView => fb::NodeRecordKindWire::BufferView,
        NodeRecordKind::Split => fb::NodeRecordKindWire::Split,
        NodeRecordKind::Tabs => fb::NodeRecordKindWire::Tabs,
    };

    let buffer_view = record.buffer_view.as_ref().map(|bv| {
        fb::BufferViewRecord::create(
            builder,
            &fb::BufferViewRecordArgs {
                buffer_id: bv.buffer_id.into(),
                focused: bv.focused,
                zoomed: bv.zoomed,
                follow_output: bv.follow_output,
                last_render_cols: bv.last_render_size.cols,
                last_render_rows: bv.last_render_size.rows,
            },
        )
    });

    let split = record.split.as_ref().map(|split| {
        let dir = match split.direction {
            SplitDirection::Horizontal => fb::SplitDirectionWire::Horizontal,
            SplitDirection::Vertical => fb::SplitDirectionWire::Vertical,
        };
        let child_ids: Vec<u64> = split.child_ids.iter().map(|&c| c.into()).collect();
        let child_ids_vec = builder.create_vector(&child_ids);
        let sizes_vec = builder.create_vector(&split.sizes);

        fb::SplitRecord::create(
            builder,
            &fb::SplitRecordArgs {
                direction: dir,
                child_ids: Some(child_ids_vec),
                sizes: Some(sizes_vec),
            },
        )
    });

    let tabs = record.tabs.as_ref().map(|tabs| {
        let tabs_vec: Vec<_> = tabs
            .tabs
            .iter()
            .map(|tab| {
                let title = builder.create_string(&tab.title);
                fb::TabRecord::create(
                    builder,
                    &fb::TabRecordArgs {
                        title: Some(title),
                        child_id: tab.child_id.into(),
                    },
                )
            })
            .collect();
        let tabs_vector = builder.create_vector(&tabs_vec);

        fb::TabsRecord::create(
            builder,
            &fb::TabsRecordArgs {
                active: tabs.active,
                tabs: Some(tabs_vector),
            },
        )
    });

    fb::NodeRecord::create(
        builder,
        &fb::NodeRecordArgs {
            id: record.id.into(),
            session_id: record.session_id.into(),
            parent_id: record.parent_id.map(|p| p.into()).unwrap_or(0),
            kind,
            buffer_view,
            split,
            tabs,
        },
    )
}

fn encode_floating_record<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    record: &FloatingRecord,
) -> flatbuffers::WIPOffset<fb::FloatingRecord<'a>> {
    let title = record.title.as_ref().map(|t| builder.create_string(t));

    fb::FloatingRecord::create(
        builder,
        &fb::FloatingRecordArgs {
            id: record.id.into(),
            session_id: record.session_id.into(),
            root_node_id: record.root_node_id.into(),
            title,
            x: record.geometry.x,
            y: record.geometry.y,
            width: record.geometry.width,
            height: record.geometry.height,
            focused: record.focused,
            visible: record.visible,
            close_on_empty: record.close_on_empty,
        },
    )
}

fn encode_client_record<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    record: &ClientRecord,
) -> flatbuffers::WIPOffset<fb::ClientRecord<'a>> {
    let subscribed_session_ids: Vec<u64> = record
        .subscribed_session_ids
        .iter()
        .map(|session_id| (*session_id).into())
        .collect();
    let subscribed_session_ids = builder.create_vector(&subscribed_session_ids);

    fb::ClientRecord::create(
        builder,
        &fb::ClientRecordArgs {
            id: record.id,
            current_session_id: record.current_session_id.map(|id| id.into()).unwrap_or(0),
            subscribed_all_sessions: record.subscribed_all_sessions,
            subscribed_session_ids: Some(subscribed_session_ids),
        },
    )
}

fn encode_session_snapshot<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    snapshot: &SessionSnapshot,
) -> flatbuffers::WIPOffset<fb::SessionSnapshot<'a>> {
    let session = encode_session_record(builder, &snapshot.session);
    let nodes_vec: Vec<_> = snapshot
        .nodes
        .iter()
        .map(|n| encode_node_record(builder, n))
        .collect();
    let nodes = builder.create_vector(&nodes_vec);
    let buffers_vec: Vec<_> = snapshot
        .buffers
        .iter()
        .map(|b| encode_buffer_record(builder, b))
        .collect();
    let buffers = builder.create_vector(&buffers_vec);
    let floating_vec: Vec<_> = snapshot
        .floating
        .iter()
        .map(|f| encode_floating_record(builder, f))
        .collect();
    let floating = builder.create_vector(&floating_vec);

    fb::SessionSnapshot::create(
        builder,
        &fb::SessionSnapshotArgs {
            session: Some(session),
            nodes: Some(nodes),
            buffers: Some(buffers),
            floating: Some(floating),
        },
    )
}

// ==================== DECODING ====================

pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let envelope = fb::root_as_envelope(bytes)?;
    let request_id = RequestId(envelope.request_id());

    match envelope.kind() {
        fb::MessageKind::PingRequest => {
            let req = required(envelope.ping_request(), "ping_request")?;
            let payload = required(req.payload(), "ping_request.payload")?;
            Ok(ClientMessage::Ping(PingRequest {
                request_id,
                payload: payload.to_owned(),
            }))
        }
        fb::MessageKind::SessionRequest => {
            let req = required(envelope.session_request(), "session_request")?;
            let session_request = match req.op() {
                fb::SessionOp::Create => {
                    let name = required(req.name(), "session_request.name")?;
                    SessionRequest::Create {
                        request_id,
                        name: name.to_owned(),
                    }
                }
                fb::SessionOp::List => SessionRequest::List { request_id },
                fb::SessionOp::Get => SessionRequest::Get {
                    request_id,
                    session_id: SessionId(req.session_id()),
                },
                fb::SessionOp::Close => SessionRequest::Close {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    force: req.force(),
                },
                fb::SessionOp::Rename => SessionRequest::Rename {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    name: required(req.name(), "session_request.name")?.to_owned(),
                },
                fb::SessionOp::AddRootTab => SessionRequest::AddRootTab {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    title: required(req.title(), "session_request.title")?.to_owned(),
                    buffer_id: (req.buffer_id() != 0).then(|| BufferId(req.buffer_id())),
                    child_node_id: (req.child_node_id() != 0).then(|| NodeId(req.child_node_id())),
                },
                fb::SessionOp::SelectRootTab => SessionRequest::SelectRootTab {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    index: req.index(),
                },
                fb::SessionOp::RenameRootTab => SessionRequest::RenameRootTab {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    index: req.index(),
                    title: required(req.title(), "session_request.title")?.to_owned(),
                },
                fb::SessionOp::CloseRootTab => SessionRequest::CloseRootTab {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    index: req.index(),
                },
                _ => return Err(ProtocolError::InvalidMessage("unknown session op")),
            };
            Ok(ClientMessage::Session(session_request))
        }
        fb::MessageKind::BufferRequest => {
            let req = required(envelope.buffer_request(), "buffer_request")?;
            let buffer_request = match req.op() {
                fb::BufferOp::Create => {
                    let command = required(req.command(), "buffer_request.command")?;
                    let command_vec: Vec<String> = command.iter().map(|s| s.to_owned()).collect();
                    let env =
                        decode_string_map(req.env_keys(), req.env_values(), "buffer_request.env")?;
                    BufferRequest::Create {
                        request_id,
                        title: req.title().map(|t| t.to_owned()),
                        command: command_vec,
                        cwd: req.cwd().map(|c| c.to_owned()),
                        env,
                    }
                }
                fb::BufferOp::List => BufferRequest::List {
                    request_id,
                    session_id: if req.session_id() == 0 {
                        None
                    } else {
                        Some(SessionId(req.session_id()))
                    },
                    attached_only: req.attached_only(),
                    detached_only: req.detached_only(),
                },
                fb::BufferOp::Get => BufferRequest::Get {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                },
                fb::BufferOp::Detach => BufferRequest::Detach {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                },
                fb::BufferOp::Kill => BufferRequest::Kill {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    force: req.force(),
                },
                fb::BufferOp::Capture => BufferRequest::Capture {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                },
                fb::BufferOp::CaptureVisible => BufferRequest::CaptureVisible {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                },
                fb::BufferOp::ScrollbackSlice => BufferRequest::ScrollbackSlice {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    start_line: req.start_line(),
                    line_count: req.line_count(),
                },
                fb::BufferOp::GetLocation => BufferRequest::GetLocation {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                },
                fb::BufferOp::Reveal => BufferRequest::Reveal {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    client_id: (req.client_id() != 0).then_some(req.client_id()),
                },
                fb::BufferOp::OpenHistory => BufferRequest::OpenHistory {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    scope: decode_buffer_history_scope(req.history_scope())?,
                    placement: decode_buffer_history_placement(req.history_placement())?,
                    client_id: (req.client_id() != 0).then_some(req.client_id()),
                },
                _ => return Err(ProtocolError::InvalidMessage("unknown buffer op")),
            };
            Ok(ClientMessage::Buffer(buffer_request))
        }
        fb::MessageKind::NodeRequest => {
            let req = required(envelope.node_request(), "node_request")?;
            let node_request = match req.op() {
                fb::NodeOp::GetTree => NodeRequest::GetTree {
                    request_id,
                    session_id: SessionId(req.session_id()),
                },
                fb::NodeOp::Split => {
                    let direction = match req.direction() {
                        fb::SplitDirectionWire::Horizontal => SplitDirection::Horizontal,
                        fb::SplitDirectionWire::Vertical => SplitDirection::Vertical,
                        _ => return Err(ProtocolError::InvalidMessage("unknown split direction")),
                    };
                    NodeRequest::Split {
                        request_id,
                        leaf_node_id: NodeId(req.leaf_node_id()),
                        direction,
                        new_buffer_id: BufferId(req.new_buffer_id()),
                    }
                }
                fb::NodeOp::CreateSplit => {
                    let direction = match req.direction() {
                        fb::SplitDirectionWire::Horizontal => SplitDirection::Horizontal,
                        fb::SplitDirectionWire::Vertical => SplitDirection::Vertical,
                        _ => return Err(ProtocolError::InvalidMessage("unknown split direction")),
                    };
                    let child_node_ids =
                        required(req.child_node_ids(), "node_request.child_node_ids")?
                            .iter()
                            .map(NodeId)
                            .collect();
                    let sizes = req
                        .sizes()
                        .map(|sizes| sizes.iter().collect())
                        .unwrap_or_default();
                    NodeRequest::CreateSplit {
                        request_id,
                        session_id: SessionId(req.session_id()),
                        direction,
                        child_node_ids,
                        sizes,
                    }
                }
                fb::NodeOp::CreateTabs => {
                    let child_node_ids =
                        required(req.child_node_ids(), "node_request.child_node_ids")?
                            .iter()
                            .map(NodeId)
                            .collect();
                    let titles = required(req.titles(), "node_request.titles")?
                        .iter()
                        .map(|title| title.to_owned())
                        .collect();
                    NodeRequest::CreateTabs {
                        request_id,
                        session_id: SessionId(req.session_id()),
                        child_node_ids,
                        titles,
                        active: req.active(),
                    }
                }
                fb::NodeOp::ReplaceNode => NodeRequest::ReplaceNode {
                    request_id,
                    node_id: NodeId(req.node_id()),
                    child_node_id: NodeId(req.child_node_id()),
                },
                fb::NodeOp::WrapInSplit => {
                    let direction = match req.direction() {
                        fb::SplitDirectionWire::Horizontal => SplitDirection::Horizontal,
                        fb::SplitDirectionWire::Vertical => SplitDirection::Vertical,
                        _ => return Err(ProtocolError::InvalidMessage("unknown split direction")),
                    };
                    NodeRequest::WrapInSplit {
                        request_id,
                        node_id: NodeId(req.node_id()),
                        child_node_id: NodeId(req.child_node_id()),
                        direction,
                        insert_before: req.insert_before(),
                    }
                }
                fb::NodeOp::WrapInTabs => {
                    let title = required(req.title(), "node_request.title")?;
                    NodeRequest::WrapInTabs {
                        request_id,
                        node_id: NodeId(req.node_id()),
                        title: title.to_owned(),
                    }
                }
                fb::NodeOp::AddTab => {
                    let title = required(req.title(), "node_request.title")?;
                    NodeRequest::AddTab {
                        request_id,
                        tabs_node_id: NodeId(req.tabs_node_id()),
                        title: title.to_owned(),
                        buffer_id: (req.buffer_id() != 0).then(|| BufferId(req.buffer_id())),
                        child_node_id: (req.child_node_id() != 0)
                            .then(|| NodeId(req.child_node_id())),
                        index: req.index(),
                    }
                }
                fb::NodeOp::SelectTab => NodeRequest::SelectTab {
                    request_id,
                    tabs_node_id: NodeId(req.tabs_node_id()),
                    index: req.index(),
                },
                fb::NodeOp::Focus => NodeRequest::Focus {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    node_id: NodeId(req.node_id()),
                },
                fb::NodeOp::Close => NodeRequest::Close {
                    request_id,
                    node_id: NodeId(req.node_id()),
                },
                fb::NodeOp::MoveBufferToNode => NodeRequest::MoveBufferToNode {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    target_leaf_node_id: NodeId(req.target_leaf_node_id()),
                },
                fb::NodeOp::Resize => {
                    let sizes = required(req.sizes(), "node_request.sizes")?;
                    NodeRequest::Resize {
                        request_id,
                        node_id: NodeId(req.node_id()),
                        sizes: sizes.iter().collect(),
                    }
                }
                fb::NodeOp::Zoom => NodeRequest::Zoom {
                    request_id,
                    node_id: NodeId(req.node_id()),
                },
                fb::NodeOp::Unzoom => NodeRequest::Unzoom {
                    request_id,
                    session_id: SessionId(req.session_id()),
                },
                fb::NodeOp::ToggleZoom => NodeRequest::ToggleZoom {
                    request_id,
                    node_id: NodeId(req.node_id()),
                },
                fb::NodeOp::SwapSiblings => NodeRequest::SwapSiblings {
                    request_id,
                    first_node_id: NodeId(req.first_node_id()),
                    second_node_id: NodeId(req.second_node_id()),
                },
                fb::NodeOp::BreakNode => NodeRequest::BreakNode {
                    request_id,
                    node_id: NodeId(req.node_id()),
                    destination: decode_node_break_destination(req.break_destination())?,
                },
                fb::NodeOp::JoinBufferAtNode => NodeRequest::JoinBufferAtNode {
                    request_id,
                    node_id: NodeId(req.node_id()),
                    buffer_id: BufferId(req.buffer_id()),
                    placement: decode_node_join_placement(req.join_placement())?,
                },
                fb::NodeOp::MoveNodeBefore => NodeRequest::MoveNodeBefore {
                    request_id,
                    node_id: NodeId(req.node_id()),
                    sibling_node_id: NodeId(req.sibling_node_id()),
                },
                fb::NodeOp::MoveNodeAfter => NodeRequest::MoveNodeAfter {
                    request_id,
                    node_id: NodeId(req.node_id()),
                    sibling_node_id: NodeId(req.sibling_node_id()),
                },
                _ => return Err(ProtocolError::InvalidMessage("unknown node op")),
            };
            Ok(ClientMessage::Node(node_request))
        }
        fb::MessageKind::FloatingRequest => {
            let req = required(envelope.floating_request(), "floating_request")?;
            let floating_request = match req.op() {
                fb::FloatingOp::Create => FloatingRequest::Create {
                    request_id,
                    session_id: SessionId(req.session_id()),
                    root_node_id: (req.root_node_id() != 0).then(|| NodeId(req.root_node_id())),
                    buffer_id: (req.buffer_id() != 0).then(|| BufferId(req.buffer_id())),
                    geometry: FloatGeometry {
                        x: req.x(),
                        y: req.y(),
                        width: req.width(),
                        height: req.height(),
                    },
                    title: req.title().map(|t| t.to_owned()),
                    focus: req.focus(),
                    close_on_empty: req.close_on_empty(),
                },
                fb::FloatingOp::Close => FloatingRequest::Close {
                    request_id,
                    floating_id: FloatingId(req.floating_id()),
                },
                fb::FloatingOp::Move => FloatingRequest::Move {
                    request_id,
                    floating_id: FloatingId(req.floating_id()),
                    geometry: FloatGeometry {
                        x: req.x(),
                        y: req.y(),
                        width: req.width(),
                        height: req.height(),
                    },
                },
                fb::FloatingOp::Focus => FloatingRequest::Focus {
                    request_id,
                    floating_id: FloatingId(req.floating_id()),
                },
                _ => return Err(ProtocolError::InvalidMessage("unknown floating op")),
            };
            Ok(ClientMessage::Floating(floating_request))
        }
        fb::MessageKind::InputRequest => {
            let req = required(envelope.input_request(), "input_request")?;
            let input_request = match req.op() {
                fb::InputOp::Send => {
                    let bytes = required(req.bytes(), "input_request.bytes")?;
                    InputRequest::Send {
                        request_id,
                        buffer_id: BufferId(req.buffer_id()),
                        bytes: bytes.iter().collect(),
                    }
                }
                fb::InputOp::Resize => InputRequest::Resize {
                    request_id,
                    buffer_id: BufferId(req.buffer_id()),
                    cols: req.cols(),
                    rows: req.rows(),
                },
                _ => return Err(ProtocolError::InvalidMessage("unknown input op")),
            };
            Ok(ClientMessage::Input(input_request))
        }
        fb::MessageKind::SubscribeRequest => {
            let req = required(envelope.subscribe_request(), "subscribe_request")?;
            Ok(ClientMessage::Subscribe(SubscribeRequest {
                request_id,
                session_id: if req.session_id() == 0 {
                    None
                } else {
                    Some(SessionId(req.session_id()))
                },
            }))
        }
        fb::MessageKind::UnsubscribeRequest => {
            let req = required(envelope.unsubscribe_request(), "unsubscribe_request")?;
            Ok(ClientMessage::Unsubscribe(UnsubscribeRequest {
                request_id,
                subscription_id: req.subscription_id(),
            }))
        }
        fb::MessageKind::ClientRequest => {
            let req = required(envelope.client_request(), "client_request")?;
            let client_id = std::num::NonZeroU64::new(req.client_id());
            let request = match req.op() {
                fb::ClientOp::List => ClientRequest::List { request_id },
                fb::ClientOp::Get => ClientRequest::Get {
                    request_id,
                    client_id,
                },
                fb::ClientOp::Detach => ClientRequest::Detach {
                    request_id,
                    client_id,
                },
                fb::ClientOp::Switch => {
                    if req.session_id() == 0 {
                        return Err(ProtocolError::InvalidMessage(
                            "Switch request requires a non-zero session_id",
                        ));
                    }
                    ClientRequest::Switch {
                        request_id,
                        client_id,
                        session_id: SessionId(req.session_id()),
                    }
                }
                _ => return Err(ProtocolError::InvalidMessage("unknown client op")),
            };
            Ok(ClientMessage::Client(request))
        }
        other => Err(ProtocolError::InvalidMessageOwned(format!(
            "unexpected client message kind: {:?}",
            other
        ))),
    }
}

pub fn decode_server_envelope(bytes: &[u8]) -> Result<ServerEnvelope, ProtocolError> {
    let envelope = fb::root_as_envelope(bytes)?;

    match envelope.kind() {
        fb::MessageKind::PingResponse => {
            let resp = required(envelope.ping_response(), "ping_response")?;
            let payload = required(resp.payload(), "ping_response.payload")?;
            Ok(ServerEnvelope::Response(ServerResponse::Pong(
                PingResponse {
                    request_id: RequestId(envelope.request_id()),
                    payload: payload.to_owned(),
                },
            )))
        }
        fb::MessageKind::OkResponse => {
            Ok(ServerEnvelope::Response(ServerResponse::Ok(OkResponse {
                request_id: RequestId(envelope.request_id()),
            })))
        }
        fb::MessageKind::ErrorResponse => {
            let resp = required(envelope.error_response(), "error_response")?;
            let message = required(resp.message(), "error_response.message")?;
            Ok(ServerEnvelope::Response(ServerResponse::Error(
                ErrorResponse {
                    request_id: if envelope.request_id() == 0 {
                        None
                    } else {
                        Some(RequestId(envelope.request_id()))
                    },
                    error: WireError::new(decode_error_code(resp.code()), message),
                },
            )))
        }
        fb::MessageKind::SessionsResponse => {
            let resp = required(envelope.sessions_response(), "sessions_response")?;
            let sessions = required(resp.sessions(), "sessions_response.sessions")?;
            let sessions_vec: Result<Vec<_>, _> =
                sessions.iter().map(decode_session_record).collect();
            Ok(ServerEnvelope::Response(ServerResponse::Sessions(
                SessionsResponse {
                    request_id: RequestId(envelope.request_id()),
                    sessions: sessions_vec?,
                },
            )))
        }
        fb::MessageKind::SessionSnapshotResponse => {
            let resp = required(
                envelope.session_snapshot_response(),
                "session_snapshot_response",
            )?;
            let snapshot = required(resp.snapshot(), "session_snapshot_response.snapshot")?;
            Ok(ServerEnvelope::Response(ServerResponse::SessionSnapshot(
                SessionSnapshotResponse {
                    request_id: RequestId(envelope.request_id()),
                    snapshot: decode_session_snapshot(snapshot)?,
                },
            )))
        }
        fb::MessageKind::BuffersResponse => {
            let resp = required(envelope.buffers_response(), "buffers_response")?;
            let buffers = required(resp.buffers(), "buffers_response.buffers")?;
            let buffers_vec: Result<Vec<_>, _> = buffers.iter().map(decode_buffer_record).collect();
            Ok(ServerEnvelope::Response(ServerResponse::Buffers(
                BuffersResponse {
                    request_id: RequestId(envelope.request_id()),
                    buffers: buffers_vec?,
                },
            )))
        }
        fb::MessageKind::BufferResponse => {
            let resp = required(envelope.buffer_response(), "buffer_response")?;
            let buffer = required(resp.buffer(), "buffer_response.buffer")?;
            Ok(ServerEnvelope::Response(ServerResponse::Buffer(
                BufferResponse {
                    request_id: RequestId(envelope.request_id()),
                    buffer: decode_buffer_record(buffer)?,
                },
            )))
        }
        fb::MessageKind::FloatingListResponse => {
            let resp = required(envelope.floating_list_response(), "floating_list_response")?;
            let floating = required(resp.floating(), "floating_list_response.floating")?;
            let floating_vec: Result<Vec<_>, _> =
                floating.iter().map(decode_floating_record).collect();
            Ok(ServerEnvelope::Response(ServerResponse::FloatingList(
                FloatingListResponse {
                    request_id: RequestId(envelope.request_id()),
                    floating: floating_vec?,
                },
            )))
        }
        fb::MessageKind::FloatingResponse => {
            let resp = required(envelope.floating_response(), "floating_response")?;
            let floating = required(resp.floating(), "floating_response.floating")?;
            Ok(ServerEnvelope::Response(ServerResponse::Floating(
                FloatingResponse {
                    request_id: RequestId(envelope.request_id()),
                    floating: decode_floating_record(floating)?,
                },
            )))
        }
        fb::MessageKind::SubscriptionAckResponse => {
            let resp = required(
                envelope.subscription_ack_response(),
                "subscription_ack_response",
            )?;
            Ok(ServerEnvelope::Response(ServerResponse::SubscriptionAck(
                SubscriptionAckResponse {
                    request_id: RequestId(envelope.request_id()),
                    subscription_id: resp.subscription_id(),
                },
            )))
        }
        fb::MessageKind::ClientsResponse => {
            let resp = required(envelope.clients_response(), "clients_response")?;
            let clients = required(resp.clients(), "clients_response.clients")?;
            let clients_vec: Result<Vec<_>, _> = clients.iter().map(decode_client_record).collect();
            Ok(ServerEnvelope::Response(ServerResponse::Clients(
                ClientsResponse {
                    request_id: RequestId(envelope.request_id()),
                    clients: clients_vec?,
                },
            )))
        }
        fb::MessageKind::ClientResponse => {
            let resp = required(envelope.client_response(), "client_response")?;
            let client = required(resp.client(), "client_response.client")?;
            Ok(ServerEnvelope::Response(ServerResponse::Client(
                ClientResponse {
                    request_id: RequestId(envelope.request_id()),
                    client: decode_client_record(client)?,
                },
            )))
        }
        fb::MessageKind::BufferLocationResponse => {
            let resp = required(
                envelope.buffer_location_response(),
                "buffer_location_response",
            )?;
            let location = required(resp.location(), "buffer_location_response.location")?;
            Ok(ServerEnvelope::Response(ServerResponse::BufferLocation(
                BufferLocationResponse {
                    request_id: RequestId(envelope.request_id()),
                    location: BufferLocation {
                        buffer_id: BufferId(location.buffer_id()),
                        session_id: (location.session_id() != 0)
                            .then(|| SessionId(location.session_id())),
                        node_id: (location.node_id() != 0).then(|| NodeId(location.node_id())),
                        floating_id: (location.floating_id() != 0)
                            .then(|| FloatingId(location.floating_id())),
                    },
                },
            )))
        }
        fb::MessageKind::SnapshotResponse => {
            let resp = required(envelope.snapshot_response(), "snapshot_response")?;
            let lines = required(resp.lines(), "snapshot_response.lines")?;
            let lines_vec: Vec<String> = lines.iter().map(|l| l.to_owned()).collect();
            Ok(ServerEnvelope::Response(ServerResponse::Snapshot(
                SnapshotResponse {
                    request_id: RequestId(envelope.request_id()),
                    buffer_id: BufferId(resp.buffer_id()),
                    sequence: resp.sequence(),
                    size: PtySize {
                        cols: resp.cols(),
                        rows: resp.rows(),
                        pixel_width: 0,
                        pixel_height: 0,
                    },
                    lines: lines_vec,
                    title: resp.title().map(|t| t.to_owned()),
                    cwd: resp.cwd().map(|c| c.to_owned()),
                },
            )))
        }
        fb::MessageKind::VisibleSnapshotResponse => {
            let resp = required(
                envelope.visible_snapshot_response(),
                "visible_snapshot_response",
            )?;
            let lines = required(resp.lines(), "visible_snapshot_response.lines")?;
            let lines_vec: Vec<String> = lines.iter().map(|l| l.to_owned()).collect();
            let cursor = resp.cursor().map(decode_cursor_state).transpose()?;
            Ok(ServerEnvelope::Response(ServerResponse::VisibleSnapshot(
                VisibleSnapshotResponse {
                    request_id: RequestId(envelope.request_id()),
                    buffer_id: BufferId(resp.buffer_id()),
                    sequence: resp.sequence(),
                    size: PtySize {
                        cols: resp.cols(),
                        rows: resp.rows(),
                        pixel_width: 0,
                        pixel_height: 0,
                    },
                    lines: lines_vec,
                    title: resp.title().map(|t| t.to_owned()),
                    cwd: resp.cwd().map(|c| c.to_owned()),
                    viewport_top_line: resp.viewport_top_line(),
                    total_lines: resp.total_lines(),
                    alternate_screen: resp.alternate_screen(),
                    mouse_reporting: resp.mouse_reporting(),
                    focus_reporting: resp.focus_reporting(),
                    bracketed_paste: resp.bracketed_paste(),
                    cursor,
                },
            )))
        }
        fb::MessageKind::ScrollbackSliceResponse => {
            let resp = required(
                envelope.scrollback_slice_response(),
                "scrollback_slice_response",
            )?;
            let lines = required(resp.lines(), "scrollback_slice_response.lines")?;
            let lines_vec: Vec<String> = lines.iter().map(|l| l.to_owned()).collect();
            Ok(ServerEnvelope::Response(ServerResponse::ScrollbackSlice(
                ScrollbackSliceResponse {
                    request_id: RequestId(envelope.request_id()),
                    buffer_id: BufferId(resp.buffer_id()),
                    start_line: resp.start_line(),
                    total_lines: resp.total_lines(),
                    lines: lines_vec,
                },
            )))
        }
        fb::MessageKind::SessionCreatedEvent => {
            let event = required(envelope.session_created_event(), "session_created_event")?;
            let session = required(event.session(), "session_created_event.session")?;
            Ok(ServerEnvelope::Event(ServerEvent::SessionCreated(
                SessionCreatedEvent {
                    session: decode_session_record(session)?,
                },
            )))
        }
        fb::MessageKind::SessionClosedEvent => {
            let event = required(envelope.session_closed_event(), "session_closed_event")?;
            Ok(ServerEnvelope::Event(ServerEvent::SessionClosed(
                SessionClosedEvent {
                    session_id: SessionId(event.session_id()),
                },
            )))
        }
        fb::MessageKind::SessionRenamedEvent => {
            let event = required(envelope.session_renamed_event(), "session_renamed_event")?;
            let name = required(event.name(), "session_renamed_event.name")?;
            Ok(ServerEnvelope::Event(ServerEvent::SessionRenamed(
                SessionRenamedEvent {
                    session_id: SessionId(event.session_id()),
                    name: name.to_owned(),
                },
            )))
        }
        fb::MessageKind::BufferCreatedEvent => {
            let event = required(envelope.buffer_created_event(), "buffer_created_event")?;
            let buffer = required(event.buffer(), "buffer_created_event.buffer")?;
            Ok(ServerEnvelope::Event(ServerEvent::BufferCreated(
                BufferCreatedEvent {
                    buffer: decode_buffer_record(buffer)?,
                },
            )))
        }
        fb::MessageKind::BufferDetachedEvent => {
            let event = required(envelope.buffer_detached_event(), "buffer_detached_event")?;
            Ok(ServerEnvelope::Event(ServerEvent::BufferDetached(
                BufferDetachedEvent {
                    buffer_id: BufferId(event.buffer_id()),
                },
            )))
        }
        fb::MessageKind::NodeChangedEvent => {
            let event = required(envelope.node_changed_event(), "node_changed_event")?;
            Ok(ServerEnvelope::Event(ServerEvent::NodeChanged(
                NodeChangedEvent {
                    session_id: SessionId(event.session_id()),
                },
            )))
        }
        fb::MessageKind::FloatingChangedEvent => {
            let event = required(envelope.floating_changed_event(), "floating_changed_event")?;
            Ok(ServerEnvelope::Event(ServerEvent::FloatingChanged(
                FloatingChangedEvent {
                    session_id: SessionId(event.session_id()),
                    floating_id: if event.floating_id() == 0 {
                        None
                    } else {
                        Some(FloatingId(event.floating_id()))
                    },
                },
            )))
        }
        fb::MessageKind::FocusChangedEvent => {
            let event = required(envelope.focus_changed_event(), "focus_changed_event")?;
            Ok(ServerEnvelope::Event(ServerEvent::FocusChanged(
                FocusChangedEvent {
                    session_id: SessionId(event.session_id()),
                    focused_leaf_id: if event.focused_leaf_id() == 0 {
                        None
                    } else {
                        Some(NodeId(event.focused_leaf_id()))
                    },
                    focused_floating_id: if event.focused_floating_id() == 0 {
                        None
                    } else {
                        Some(FloatingId(event.focused_floating_id()))
                    },
                },
            )))
        }
        fb::MessageKind::RenderInvalidatedEvent => {
            let event = required(
                envelope.render_invalidated_event(),
                "render_invalidated_event",
            )?;
            Ok(ServerEnvelope::Event(ServerEvent::RenderInvalidated(
                RenderInvalidatedEvent {
                    buffer_id: BufferId(event.buffer_id()),
                },
            )))
        }
        fb::MessageKind::ClientChangedEvent => {
            let event = required(envelope.client_changed_event(), "client_changed_event")?;
            let client = required(event.client(), "client_changed_event.client")?;
            Ok(ServerEnvelope::Event(ServerEvent::ClientChanged(
                ClientChangedEvent {
                    client: decode_client_record(client)?,
                    previous_session_id: if event.previous_session_id() == 0 {
                        None
                    } else {
                        Some(SessionId(event.previous_session_id()))
                    },
                },
            )))
        }
        other => Err(ProtocolError::InvalidMessageOwned(format!(
            "unexpected server envelope kind: {:?}",
            other
        ))),
    }
}

// ==================== RECORD DECODING ====================

fn decode_session_record(record: fb::SessionRecord) -> Result<SessionRecord, ProtocolError> {
    let name = required(record.name(), "session_record.name")?;
    let floating_ids_fb = required(record.floating_ids(), "session_record.floating_ids")?;
    let floating_ids: Vec<FloatingId> = floating_ids_fb.iter().map(FloatingId).collect();

    Ok(SessionRecord {
        id: SessionId(record.id()),
        name: name.to_owned(),
        root_node_id: NodeId(record.root_node_id()),
        floating_ids,
        focused_leaf_id: if record.focused_leaf_id() == 0 {
            None
        } else {
            Some(NodeId(record.focused_leaf_id()))
        },
        focused_floating_id: if record.focused_floating_id() == 0 {
            None
        } else {
            Some(FloatingId(record.focused_floating_id()))
        },
        zoomed_node_id: if record.zoomed_node_id() == 0 {
            None
        } else {
            Some(NodeId(record.zoomed_node_id()))
        },
    })
}

fn decode_buffer_record(record: fb::BufferRecord) -> Result<BufferRecord, ProtocolError> {
    let title = required(record.title(), "buffer_record.title")?;
    let command_fb = required(record.command(), "buffer_record.command")?;
    let command: Vec<String> = command_fb.iter().map(|s| s.to_owned()).collect();

    let state = match record.state() {
        fb::BufferStateWire::Created => BufferRecordState::Created,
        fb::BufferStateWire::Running => BufferRecordState::Running,
        fb::BufferStateWire::Interrupted => BufferRecordState::Interrupted,
        fb::BufferStateWire::Exited => BufferRecordState::Exited,
        _ => return Err(ProtocolError::InvalidMessage("unknown buffer state")),
    };

    let activity = match record.activity() {
        fb::ActivityStateWire::Idle => ActivityState::Idle,
        fb::ActivityStateWire::Activity => ActivityState::Activity,
        fb::ActivityStateWire::Bell => ActivityState::Bell,
        _ => return Err(ProtocolError::InvalidMessage("unknown activity state")),
    };
    let env = decode_string_map(record.env_keys(), record.env_values(), "buffer_record.env")?;
    let kind = match record.kind() {
        fb::BufferKindWire::Pty => BufferRecordKind::Pty,
        fb::BufferKindWire::Helper => BufferRecordKind::Helper,
        _ => return Err(ProtocolError::InvalidMessage("unknown buffer kind")),
    };
    let helper_scope = if record.has_helper_scope() {
        Some(decode_buffer_history_scope(record.helper_scope())?)
    } else {
        None
    };

    Ok(BufferRecord {
        id: BufferId(record.id()),
        title: title.to_owned(),
        command,
        cwd: record.cwd().map(|c| c.to_owned()),
        kind,
        state,
        pid: record.has_pid().then(|| record.pid()),
        attachment_node_id: if record.attachment_node_id() == 0 {
            None
        } else {
            Some(NodeId(record.attachment_node_id()))
        },
        read_only: record.read_only(),
        helper_source_buffer_id: if record.helper_source_buffer_id() == 0 {
            None
        } else {
            Some(BufferId(record.helper_source_buffer_id()))
        },
        helper_scope,
        pty_size: PtySize {
            cols: record.pty_cols(),
            rows: record.pty_rows(),
            pixel_width: 0,
            pixel_height: 0,
        },
        activity,
        last_snapshot_seq: record.last_snapshot_seq(),
        exit_code: if record.has_exit_code() {
            Some(record.exit_code())
        } else {
            None
        },
        env,
    })
}

fn decode_node_record(record: fb::NodeRecord) -> Result<NodeRecord, ProtocolError> {
    let (kind, buffer_view, split, tabs) = match record.kind() {
        fb::NodeRecordKindWire::BufferView => {
            let buffer_view = required(record.buffer_view(), "node_record.buffer_view")?;
            (
                NodeRecordKind::BufferView,
                Some(BufferViewRecord {
                    buffer_id: BufferId(buffer_view.buffer_id()),
                    focused: buffer_view.focused(),
                    zoomed: buffer_view.zoomed(),
                    follow_output: buffer_view.follow_output(),
                    last_render_size: PtySize {
                        cols: buffer_view.last_render_cols(),
                        rows: buffer_view.last_render_rows(),
                        pixel_width: 0,
                        pixel_height: 0,
                    },
                }),
                None,
                None,
            )
        }
        fb::NodeRecordKindWire::Split => {
            let split = required(record.split(), "node_record.split")?;
            let child_ids = required(split.child_ids(), "node_record.split.child_ids")?
                .iter()
                .map(NodeId)
                .collect();
            let sizes = required(split.sizes(), "node_record.split.sizes")?
                .iter()
                .collect();
            let direction = match split.direction() {
                fb::SplitDirectionWire::Horizontal => SplitDirection::Horizontal,
                fb::SplitDirectionWire::Vertical => SplitDirection::Vertical,
                _ => return Err(ProtocolError::InvalidMessage("node_record.split.direction")),
            };
            (
                NodeRecordKind::Split,
                None,
                Some(SplitRecord {
                    direction,
                    child_ids,
                    sizes,
                }),
                None,
            )
        }
        fb::NodeRecordKindWire::Tabs => {
            let tabs = required(record.tabs(), "node_record.tabs")?;
            let tabs_fb = required(tabs.tabs(), "node_record.tabs.tabs")?;
            let tabs_vec = tabs_fb
                .iter()
                .map(|tab| {
                    Ok(TabRecord {
                        title: required(tab.title(), "node_record.tabs.title")?.to_owned(),
                        child_id: NodeId(tab.child_id()),
                    })
                })
                .collect::<Result<Vec<_>, ProtocolError>>()?;
            (
                NodeRecordKind::Tabs,
                None,
                None,
                Some(TabsRecord {
                    active: tabs.active(),
                    tabs: tabs_vec,
                }),
            )
        }
        _ => return Err(ProtocolError::InvalidMessage("unknown node kind")),
    };

    Ok(NodeRecord {
        id: NodeId(record.id()),
        session_id: SessionId(record.session_id()),
        parent_id: if record.parent_id() == 0 {
            None
        } else {
            Some(NodeId(record.parent_id()))
        },
        kind,
        buffer_view,
        split,
        tabs,
    })
}

fn decode_floating_record(record: fb::FloatingRecord) -> Result<FloatingRecord, ProtocolError> {
    Ok(FloatingRecord {
        id: FloatingId(record.id()),
        session_id: SessionId(record.session_id()),
        root_node_id: NodeId(record.root_node_id()),
        title: record.title().map(|t| t.to_owned()),
        geometry: FloatGeometry {
            x: record.x(),
            y: record.y(),
            width: record.width(),
            height: record.height(),
        },
        focused: record.focused(),
        visible: record.visible(),
        close_on_empty: record.close_on_empty(),
    })
}

fn decode_client_record(record: fb::ClientRecord) -> Result<ClientRecord, ProtocolError> {
    if record.id() == 0 {
        return Err(ProtocolError::InvalidMessageOwned(
            "client_record.id must be non-zero".to_owned(),
        ));
    }
    let subscribed_session_ids_fb = required(
        record.subscribed_session_ids(),
        "client_record.subscribed_session_ids",
    )?;
    let mut subscribed_session_ids = Vec::with_capacity(subscribed_session_ids_fb.len());
    for session_id in subscribed_session_ids_fb.iter() {
        if session_id == 0 {
            return Err(ProtocolError::InvalidMessageOwned(
                "client_record.subscribed_session_ids entries must be non-zero".to_owned(),
            ));
        }
        subscribed_session_ids.push(SessionId(session_id));
    }
    Ok(ClientRecord {
        id: record.id(),
        current_session_id: (record.current_session_id() != 0)
            .then(|| SessionId(record.current_session_id())),
        subscribed_all_sessions: record.subscribed_all_sessions(),
        subscribed_session_ids,
    })
}

fn decode_session_snapshot(
    snapshot: fb::SessionSnapshot,
) -> Result<SessionSnapshot, ProtocolError> {
    let session = required(snapshot.session(), "session_snapshot.session")?;
    let nodes = required(snapshot.nodes(), "session_snapshot.nodes")?;
    let buffers = required(snapshot.buffers(), "session_snapshot.buffers")?;
    let floating = required(snapshot.floating(), "session_snapshot.floating")?;

    let nodes_vec: Result<Vec<_>, _> = nodes.iter().map(decode_node_record).collect();
    let buffers_vec: Result<Vec<_>, _> = buffers.iter().map(decode_buffer_record).collect();
    let floating_vec: Result<Vec<_>, _> = floating.iter().map(decode_floating_record).collect();

    Ok(SessionSnapshot {
        session: decode_session_record(session)?,
        nodes: nodes_vec?,
        buffers: buffers_vec?,
        floating: floating_vec?,
    })
}

// ==================== HELPERS ====================

fn encode_error_code(code: ErrorCode) -> fb::ErrorCodeWire {
    match code {
        ErrorCode::Unknown => fb::ErrorCodeWire::Unknown,
        ErrorCode::InvalidRequest => fb::ErrorCodeWire::InvalidRequest,
        ErrorCode::ProtocolViolation => fb::ErrorCodeWire::ProtocolViolation,
        ErrorCode::Transport => fb::ErrorCodeWire::Transport,
        ErrorCode::NotFound => fb::ErrorCodeWire::NotFound,
        ErrorCode::Conflict => fb::ErrorCodeWire::Conflict,
        ErrorCode::Unsupported => fb::ErrorCodeWire::Unsupported,
        ErrorCode::Timeout => fb::ErrorCodeWire::Timeout,
        ErrorCode::Internal => fb::ErrorCodeWire::Internal,
    }
}

fn decode_error_code(code: fb::ErrorCodeWire) -> ErrorCode {
    match code {
        fb::ErrorCodeWire::Unknown => ErrorCode::Unknown,
        fb::ErrorCodeWire::InvalidRequest => ErrorCode::InvalidRequest,
        fb::ErrorCodeWire::ProtocolViolation => ErrorCode::ProtocolViolation,
        fb::ErrorCodeWire::Transport => ErrorCode::Transport,
        fb::ErrorCodeWire::NotFound => ErrorCode::NotFound,
        fb::ErrorCodeWire::Conflict => ErrorCode::Conflict,
        fb::ErrorCodeWire::Unsupported => ErrorCode::Unsupported,
        fb::ErrorCodeWire::Timeout => ErrorCode::Timeout,
        fb::ErrorCodeWire::Internal => ErrorCode::Internal,
        _ => ErrorCode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use flatbuffers::FlatBufferBuilder;

    use super::*;

    #[test]
    fn decode_node_record_rejects_missing_buffer_view_payload() {
        let mut builder = FlatBufferBuilder::new();
        let node = fb::NodeRecord::create(
            &mut builder,
            &fb::NodeRecordArgs {
                id: 1,
                session_id: 1,
                kind: fb::NodeRecordKindWire::BufferView,
                ..Default::default()
            },
        );
        builder.finish(node, None);

        let record =
            flatbuffers::root::<fb::NodeRecord>(builder.finished_data()).expect("node record root");
        let error = decode_node_record(record).expect_err("missing payload should be rejected");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessage("node_record.buffer_view")
        ));
    }

    #[test]
    fn decode_node_record_rejects_split_without_children() {
        let mut builder = FlatBufferBuilder::new();
        let split = fb::SplitRecord::create(
            &mut builder,
            &fb::SplitRecordArgs {
                direction: fb::SplitDirectionWire::Vertical,
                ..Default::default()
            },
        );
        let node = fb::NodeRecord::create(
            &mut builder,
            &fb::NodeRecordArgs {
                id: 2,
                session_id: 1,
                kind: fb::NodeRecordKindWire::Split,
                split: Some(split),
                ..Default::default()
            },
        );
        builder.finish(node, None);

        let record =
            flatbuffers::root::<fb::NodeRecord>(builder.finished_data()).expect("node record root");
        let error = decode_node_record(record).expect_err("missing child ids should be rejected");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessage("node_record.split.child_ids")
        ));
    }

    #[test]
    fn decode_node_record_rejects_tabs_without_tab_entries() {
        let mut builder = FlatBufferBuilder::new();
        let tabs = fb::TabsRecord::create(
            &mut builder,
            &fb::TabsRecordArgs {
                active: 0,
                ..Default::default()
            },
        );
        let node = fb::NodeRecord::create(
            &mut builder,
            &fb::NodeRecordArgs {
                id: 3,
                session_id: 1,
                kind: fb::NodeRecordKindWire::Tabs,
                tabs: Some(tabs),
                ..Default::default()
            },
        );
        builder.finish(node, None);

        let record =
            flatbuffers::root::<fb::NodeRecord>(builder.finished_data()).expect("node record root");
        let error = decode_node_record(record).expect_err("missing tabs should be rejected");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessage("node_record.tabs.tabs")
        ));
    }

    #[test]
    fn decode_client_record_rejects_zero_id() {
        let mut builder = FlatBufferBuilder::new();
        let subscribed_session_ids = builder.create_vector(&[1_u64]);
        let record = fb::ClientRecord::create(
            &mut builder,
            &fb::ClientRecordArgs {
                id: 0,
                current_session_id: 0,
                subscribed_all_sessions: false,
                subscribed_session_ids: Some(subscribed_session_ids),
            },
        );
        builder.finish(record, None);

        let record = flatbuffers::root::<fb::ClientRecord>(builder.finished_data())
            .expect("client record root");
        let error = decode_client_record(record).expect_err("zero client id should be rejected");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessageOwned(message)
                if message == "client_record.id must be non-zero"
        ));
    }

    #[test]
    fn decode_client_record_rejects_zero_subscribed_session_id() {
        let mut builder = FlatBufferBuilder::new();
        let subscribed_session_ids = builder.create_vector(&[1_u64, 0_u64, 3_u64]);
        let record = fb::ClientRecord::create(
            &mut builder,
            &fb::ClientRecordArgs {
                id: 44,
                current_session_id: 0,
                subscribed_all_sessions: false,
                subscribed_session_ids: Some(subscribed_session_ids),
            },
        );
        builder.finish(record, None);

        let record = flatbuffers::root::<fb::ClientRecord>(builder.finished_data())
            .expect("client record root");
        let error =
            decode_client_record(record).expect_err("zero subscribed session id should reject");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessageOwned(message)
                if message == "client_record.subscribed_session_ids entries must be non-zero"
        ));
    }

    #[test]
    fn decode_client_record_keeps_zero_current_session_id_optional() {
        let mut builder = FlatBufferBuilder::new();
        let subscribed_session_ids = builder.create_vector(&[5_u64]);
        let record = fb::ClientRecord::create(
            &mut builder,
            &fb::ClientRecordArgs {
                id: 44,
                current_session_id: 0,
                subscribed_all_sessions: true,
                subscribed_session_ids: Some(subscribed_session_ids),
            },
        );
        builder.finish(record, None);

        let record = flatbuffers::root::<fb::ClientRecord>(builder.finished_data())
            .expect("client record root");
        let decoded = decode_client_record(record).expect("zero current session id stays optional");

        assert_eq!(
            decoded,
            ClientRecord {
                id: 44,
                current_session_id: None,
                subscribed_all_sessions: true,
                subscribed_session_ids: vec![SessionId(5)],
            }
        );
    }

    #[test]
    fn encode_decode_session_renamed_event_roundtrip() {
        let original = ServerEnvelope::Event(ServerEvent::SessionRenamed(SessionRenamedEvent {
            session_id: SessionId(123),
            name: "my-session".to_string(),
        }));

        let encoded = encode_server_envelope(&original).expect("encode should succeed");

        let decoded = decode_server_envelope(&encoded).expect("decode should succeed");

        let ServerEnvelope::Event(ServerEvent::SessionRenamed(decoded_event)) = decoded else {
            panic!(
                "expected ServerEnvelope::Event(ServerEvent::SessionRenamed), got {:?}",
                decoded
            );
        };

        assert_eq!(decoded_event.session_id, SessionId(123));
        assert_eq!(decoded_event.name, "my-session");
    }

    #[test]
    fn encode_decode_session_rename_roundtrip() {
        let _builder = FlatBufferBuilder::new();

        // Create a SessionRequest::Rename
        let original = SessionRequest::Rename {
            request_id: RequestId(42),
            session_id: SessionId(123),
            name: "my-session".to_string(),
        };

        // Encode it
        let encoded = encode_client_message(&ClientMessage::Session(original.clone()))
            .expect("encode should succeed");

        // Decode it back
        let decoded = decode_client_message(&encoded).expect("decode should succeed");

        // Verify round-trip
        let ClientMessage::Session(SessionRequest::Rename {
            request_id: decoded_req_id,
            session_id: decoded_session_id,
            name: decoded_name,
        }) = &decoded
        else {
            panic!("expected SessionRequest::Rename, got {:?}", decoded);
        };

        // Also extract fields from original for comparison
        let SessionRequest::Rename {
            request_id: orig_req_id,
            session_id: orig_session_id,
            name: orig_name,
            ..
        } = &original
        else {
            panic!("expected SessionRequest::Rename, got {:?}", original);
        };

        assert_eq!(decoded_req_id, orig_req_id);
        assert_eq!(decoded_session_id, orig_session_id);
        assert_eq!(decoded_name, orig_name);
    }

    #[test]
    fn decode_session_rename_rejects_missing_name() {
        let mut builder = FlatBufferBuilder::new();

        // Construct the wire Rename payload WITHOUT a name field
        let session_req = fb::SessionRequest::create(
            &mut builder,
            &fb::SessionRequestArgs {
                op: fb::SessionOp::Rename,
                session_id: 123,
                buffer_id: 0,
                child_node_id: 0,
                name: None, // Missing name!
                title: None,
                force: false,
                index: 0,
            },
        );

        let envelope = fb::Envelope::create(
            &mut builder,
            &fb::EnvelopeArgs {
                request_id: 42,
                kind: fb::MessageKind::SessionRequest,
                session_request: Some(session_req),
                ..Default::default()
            },
        );

        builder.finish(envelope, Some("EMBR"));

        // Decode should fail with the expected error
        let error = decode_client_message(builder.finished_data())
            .expect_err("missing name should be rejected");

        assert!(matches!(
            error,
            ProtocolError::InvalidMessage("session_request.name")
        ));
    }

    #[test]
    fn encode_decode_client_get_none_roundtrip() {
        let original = ClientMessage::Client(ClientRequest::Get {
            request_id: RequestId(7),
            client_id: None,
        });

        let encoded = encode_client_message(&original).expect("encode should succeed");
        let decoded = decode_client_message(&encoded).expect("decode should succeed");

        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_client_switch_some_roundtrip() {
        let original = ClientMessage::Client(ClientRequest::Switch {
            request_id: RequestId(11),
            client_id: Some(NonZeroU64::new(42).expect("non-zero client id")),
            session_id: SessionId(99),
        });

        let encoded = encode_client_message(&original).expect("encode should succeed");
        let decoded = decode_client_message(&encoded).expect("decode should succeed");

        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_clients_response_roundtrip() {
        let original = ServerEnvelope::Response(ServerResponse::Clients(ClientsResponse {
            request_id: RequestId(19),
            clients: vec![
                ClientRecord {
                    id: 1,
                    current_session_id: None,
                    subscribed_all_sessions: false,
                    subscribed_session_ids: Vec::new(),
                },
                ClientRecord {
                    id: 2,
                    current_session_id: Some(SessionId(5)),
                    subscribed_all_sessions: true,
                    subscribed_session_ids: vec![SessionId(5), SessionId(9)],
                },
            ],
        }));

        let encoded = encode_server_envelope(&original).expect("encode should succeed");
        let decoded = decode_server_envelope(&encoded).expect("decode should succeed");

        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_decode_client_changed_event_roundtrip() {
        let original = ServerEnvelope::Event(ServerEvent::ClientChanged(ClientChangedEvent {
            client: ClientRecord {
                id: 44,
                current_session_id: None,
                subscribed_all_sessions: false,
                subscribed_session_ids: Vec::new(),
            },
            previous_session_id: None,
        }));

        let encoded = encode_server_envelope(&original).expect("encode should succeed");
        let decoded = decode_server_envelope(&encoded).expect("decode should succeed");

        assert_eq!(decoded, original);
    }
}
