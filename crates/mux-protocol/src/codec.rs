use flatbuffers::FlatBufferBuilder;
use mux_core::{
    ActivityState, BufferId, ErrorCode, FloatGeometry, FloatingId, NodeId, PtySize, RequestId,
    SessionId, SplitDirection, WireError,
};
use thiserror::Error;

use crate::framing::FrameType;
use crate::generated::mux::protocol as fb;
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

fn encode_session_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    req: &SessionRequest,
) -> flatbuffers::WIPOffset<fb::Envelope<'a>> {
    let (op, session_id, name_str, force) = match req {
        SessionRequest::Create { name, .. } => {
            (fb::SessionOp::Create, 0, Some(name.as_str()), false)
        }
        SessionRequest::List { .. } => (fb::SessionOp::List, 0, None, false),
        SessionRequest::Get { session_id, .. } => {
            (fb::SessionOp::Get, (*session_id).into(), None, false)
        }
        SessionRequest::Close {
            session_id, force, ..
        } => (fb::SessionOp::Close, (*session_id).into(), None, *force),
    };

    let name = name_str.map(|s| builder.create_string(s));
    let session_req = fb::SessionRequest::create(
        builder,
        &fb::SessionRequestArgs {
            op,
            session_id,
            name,
            force,
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
        attached_only,
        detached_only,
        force,
        title_str,
        command_vec,
        cwd_str,
    ) = match req {
        BufferRequest::Create {
            title,
            command,
            cwd,
            ..
        } => (
            fb::BufferOp::Create,
            0,
            0,
            false,
            false,
            false,
            title.as_deref(),
            Some(command),
            cwd.as_deref(),
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
            *attached_only,
            *detached_only,
            false,
            None,
            None,
            None,
        ),
        BufferRequest::Get { buffer_id, .. } => (
            fb::BufferOp::Get,
            (*buffer_id).into(),
            0,
            false,
            false,
            false,
            None,
            None,
            None,
        ),
        BufferRequest::Detach { buffer_id, .. } => (
            fb::BufferOp::Detach,
            (*buffer_id).into(),
            0,
            false,
            false,
            false,
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
            false,
            false,
            *force,
            None,
            None,
            None,
        ),
        BufferRequest::Capture { buffer_id, .. } => (
            fb::BufferOp::Capture,
            (*buffer_id).into(),
            0,
            false,
            false,
            false,
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

    let buffer_req = fb::BufferRequest::create(
        builder,
        &fb::BufferRequestArgs {
            op,
            buffer_id,
            session_id,
            attached_only,
            detached_only,
            force,
            title,
            command,
            cwd,
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
        direction,
    ) = match req {
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
            fb::SplitDirectionWire::Horizontal,
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
                dir,
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
            fb::SplitDirectionWire::Horizontal,
        ),
        NodeRequest::AddTab {
            tabs_node_id,
            title,
            child_node_id,
            ..
        } => (
            fb::NodeOp::AddTab,
            0,
            0,
            0,
            (*tabs_node_id).into(),
            (*child_node_id).into(),
            0,
            0,
            0,
            Some(title.as_str()),
            0,
            fb::SplitDirectionWire::Horizontal,
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
            *index as u32,
            fb::SplitDirectionWire::Horizontal,
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
            fb::SplitDirectionWire::Horizontal,
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
            fb::SplitDirectionWire::Horizontal,
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
            fb::SplitDirectionWire::Horizontal,
        ),
    };

    let title = title_str.map(|s| builder.create_string(s));
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
            direction,
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
    let (op, floating_id, session_id, root_node_id, title_str, geom) = match req {
        FloatingRequest::Create {
            session_id,
            root_node_id,
            geometry,
            title,
            ..
        } => (
            fb::FloatingOp::Create,
            0,
            (*session_id).into(),
            (*root_node_id).into(),
            title.as_deref(),
            Some(*geometry),
        ),
        FloatingRequest::Close { floating_id, .. } => (
            fb::FloatingOp::Close,
            (*floating_id).into(),
            0,
            0,
            None,
            None,
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
            None,
            Some(*geometry),
        ),
        FloatingRequest::Focus { floating_id, .. } => (
            fb::FloatingOp::Focus,
            (*floating_id).into(),
            0,
            0,
            None,
            None,
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
            title,
            x,
            y,
            width,
            height,
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

    let state = match record.state {
        BufferRecordState::Created => fb::BufferStateWire::Created,
        BufferRecordState::Running => fb::BufferStateWire::Running,
        BufferRecordState::Exited => fb::BufferStateWire::Exited,
    };

    let activity = match record.activity {
        ActivityState::Idle => fb::ActivityStateWire::Idle,
        ActivityState::Activity => fb::ActivityStateWire::Activity,
        ActivityState::Bell => fb::ActivityStateWire::Bell,
    };

    fb::BufferRecord::create(
        builder,
        &fb::BufferRecordArgs {
            id: record.id.into(),
            title: Some(title),
            command: Some(command),
            cwd,
            state,
            attachment_node_id: record.attachment_node_id.map(|n| n.into()).unwrap_or(0),
            pty_cols: record.pty_size.cols,
            pty_rows: record.pty_size.rows,
            activity,
            last_snapshot_seq: record.last_snapshot_seq,
            exit_code: record.exit_code.unwrap_or(0),
            has_exit_code: record.exit_code.is_some(),
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
                active: tabs.active as u32,
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
                    BufferRequest::Create {
                        request_id,
                        title: req.title().map(|t| t.to_owned()),
                        command: command_vec,
                        cwd: req.cwd().map(|c| c.to_owned()),
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
                        child_node_id: NodeId(req.child_node_id()),
                    }
                }
                fb::NodeOp::SelectTab => NodeRequest::SelectTab {
                    request_id,
                    tabs_node_id: NodeId(req.tabs_node_id()),
                    index: req.index() as usize,
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
                    root_node_id: NodeId(req.root_node_id()),
                    geometry: FloatGeometry {
                        x: req.x(),
                        y: req.y(),
                        width: req.width(),
                        height: req.height(),
                    },
                    title: req.title().map(|t| t.to_owned()),
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
    })
}

fn decode_buffer_record(record: fb::BufferRecord) -> Result<BufferRecord, ProtocolError> {
    let title = required(record.title(), "buffer_record.title")?;
    let command_fb = required(record.command(), "buffer_record.command")?;
    let command: Vec<String> = command_fb.iter().map(|s| s.to_owned()).collect();

    let state = match record.state() {
        fb::BufferStateWire::Created => BufferRecordState::Created,
        fb::BufferStateWire::Running => BufferRecordState::Running,
        fb::BufferStateWire::Exited => BufferRecordState::Exited,
        _ => return Err(ProtocolError::InvalidMessage("unknown buffer state")),
    };

    let activity = match record.activity() {
        fb::ActivityStateWire::Idle => ActivityState::Idle,
        fb::ActivityStateWire::Activity => ActivityState::Activity,
        fb::ActivityStateWire::Bell => ActivityState::Bell,
        _ => return Err(ProtocolError::InvalidMessage("unknown activity state")),
    };

    Ok(BufferRecord {
        id: BufferId(record.id()),
        title: title.to_owned(),
        command,
        cwd: record.cwd().map(|c| c.to_owned()),
        state,
        attachment_node_id: if record.attachment_node_id() == 0 {
            None
        } else {
            Some(NodeId(record.attachment_node_id()))
        },
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
    })
}

fn decode_node_record(record: fb::NodeRecord) -> Result<NodeRecord, ProtocolError> {
    let kind = match record.kind() {
        fb::NodeRecordKindWire::BufferView => NodeRecordKind::BufferView,
        fb::NodeRecordKindWire::Split => NodeRecordKind::Split,
        fb::NodeRecordKindWire::Tabs => NodeRecordKind::Tabs,
        _ => return Err(ProtocolError::InvalidMessage("unknown node kind")),
    };

    let buffer_view = record.buffer_view().map(|bv| BufferViewRecord {
        buffer_id: BufferId(bv.buffer_id()),
        focused: bv.focused(),
        zoomed: bv.zoomed(),
        follow_output: bv.follow_output(),
        last_render_size: PtySize {
            cols: bv.last_render_cols(),
            rows: bv.last_render_rows(),
            pixel_width: 0,
            pixel_height: 0,
        },
    });

    let split = record.split().and_then(|split| {
        let child_ids_fb = split.child_ids()?;
        let child_ids: Vec<NodeId> = child_ids_fb.iter().map(NodeId).collect();
        let sizes_fb = split.sizes()?;
        let sizes: Vec<u16> = sizes_fb.iter().collect();
        let direction = match split.direction() {
            fb::SplitDirectionWire::Horizontal => SplitDirection::Horizontal,
            fb::SplitDirectionWire::Vertical => SplitDirection::Vertical,
            _ => return None,
        };
        Some(SplitRecord {
            direction,
            child_ids,
            sizes,
        })
    });

    let tabs = record.tabs().and_then(|tabs| {
        let tabs_fb = tabs.tabs()?;
        let tabs_vec: Vec<TabRecord> = tabs_fb
            .iter()
            .filter_map(|tab| {
                let title = tab.title()?.to_owned();
                Some(TabRecord {
                    title,
                    child_id: NodeId(tab.child_id()),
                })
            })
            .collect();
        Some(TabsRecord {
            active: tabs.active() as usize,
            tabs: tabs_vec,
        })
    });

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
