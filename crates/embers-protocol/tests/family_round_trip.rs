use embers_core::{
    ActivityState, BufferId, CursorPosition, CursorShape, CursorState, ErrorCode, FloatGeometry,
    FloatingId, NodeId, PtySize, RequestId, SessionId, SplitDirection, WireError,
};
use embers_protocol::*;

#[test]
fn client_message_families_round_trip() {
    let messages = vec![
        ClientMessage::Ping(PingRequest {
            request_id: RequestId(1),
            payload: "hello".to_owned(),
        }),
        ClientMessage::Session(SessionRequest::Create {
            request_id: RequestId(2),
            name: "main".to_owned(),
        }),
        ClientMessage::Session(SessionRequest::List {
            request_id: RequestId(3),
        }),
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(4),
            session_id: SessionId(10),
        }),
        ClientMessage::Session(SessionRequest::Close {
            request_id: RequestId(5),
            session_id: SessionId(10),
            force: true,
        }),
        ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: RequestId(6),
            session_id: SessionId(10),
            title: "editor".to_owned(),
            buffer_id: Some(BufferId(20)),
            child_node_id: None,
        }),
        ClientMessage::Session(SessionRequest::SelectRootTab {
            request_id: RequestId(7),
            session_id: SessionId(10),
            index: 1,
        }),
        ClientMessage::Session(SessionRequest::RenameRootTab {
            request_id: RequestId(8),
            session_id: SessionId(10),
            index: 0,
            title: "shell".to_owned(),
        }),
        ClientMessage::Session(SessionRequest::CloseRootTab {
            request_id: RequestId(9),
            session_id: SessionId(10),
            index: 2,
        }),
        ClientMessage::Buffer(BufferRequest::Create {
            request_id: RequestId(10),
            title: Some("shell".to_owned()),
            command: vec!["bash".to_owned(), "-lc".to_owned(), "pwd".to_owned()],
            cwd: Some("/tmp".to_owned()),
            env: std::collections::BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]),
        }),
        ClientMessage::Buffer(BufferRequest::List {
            request_id: RequestId(11),
            session_id: Some(SessionId(10)),
            attached_only: true,
            detached_only: false,
        }),
        ClientMessage::Buffer(BufferRequest::Get {
            request_id: RequestId(12),
            buffer_id: BufferId(20),
        }),
        ClientMessage::Buffer(BufferRequest::Detach {
            request_id: RequestId(13),
            buffer_id: BufferId(20),
        }),
        ClientMessage::Buffer(BufferRequest::Kill {
            request_id: RequestId(14),
            buffer_id: BufferId(20),
            force: true,
        }),
        ClientMessage::Buffer(BufferRequest::Capture {
            request_id: RequestId(15),
            buffer_id: BufferId(20),
        }),
        ClientMessage::Buffer(BufferRequest::CaptureVisible {
            request_id: RequestId(151),
            buffer_id: BufferId(20),
        }),
        ClientMessage::Buffer(BufferRequest::ScrollbackSlice {
            request_id: RequestId(152),
            buffer_id: BufferId(20),
            start_line: 4,
            line_count: 8,
        }),
        ClientMessage::Buffer(BufferRequest::GetLocation {
            request_id: RequestId(153),
            buffer_id: BufferId(20),
        }),
        ClientMessage::Buffer(BufferRequest::Reveal {
            request_id: RequestId(154),
            buffer_id: BufferId(20),
            client_id: Some(7),
        }),
        ClientMessage::Buffer(BufferRequest::OpenHistory {
            request_id: RequestId(155),
            buffer_id: BufferId(20),
            scope: BufferHistoryScope::Visible,
            placement: BufferHistoryPlacement::Floating,
            client_id: None,
        }),
        ClientMessage::Node(NodeRequest::GetTree {
            request_id: RequestId(16),
            session_id: SessionId(10),
        }),
        ClientMessage::Node(NodeRequest::Split {
            request_id: RequestId(17),
            leaf_node_id: NodeId(30),
            direction: SplitDirection::Vertical,
            new_buffer_id: BufferId(21),
        }),
        ClientMessage::Node(NodeRequest::CreateSplit {
            request_id: RequestId(170),
            session_id: SessionId(10),
            direction: SplitDirection::Horizontal,
            child_node_ids: vec![NodeId(50), NodeId(51)],
            sizes: vec![2, 1],
        }),
        ClientMessage::Node(NodeRequest::CreateTabs {
            request_id: RequestId(171),
            session_id: SessionId(10),
            child_node_ids: vec![NodeId(52), NodeId(53)],
            titles: vec!["one".to_owned(), "two".to_owned()],
            active: 1,
        }),
        ClientMessage::Node(NodeRequest::ReplaceNode {
            request_id: RequestId(172),
            node_id: NodeId(54),
            child_node_id: NodeId(55),
        }),
        ClientMessage::Node(NodeRequest::WrapInSplit {
            request_id: RequestId(173),
            node_id: NodeId(56),
            child_node_id: NodeId(57),
            direction: SplitDirection::Vertical,
            insert_before: true,
        }),
        ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: RequestId(18),
            node_id: NodeId(31),
            title: "editor".to_owned(),
        }),
        ClientMessage::Node(NodeRequest::AddTab {
            request_id: RequestId(19),
            tabs_node_id: NodeId(32),
            title: "logs".to_owned(),
            buffer_id: Some(BufferId(23)),
            child_node_id: None,
            index: 2,
        }),
        ClientMessage::Node(NodeRequest::SelectTab {
            request_id: RequestId(20),
            tabs_node_id: NodeId(32),
            index: 1,
        }),
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(21),
            session_id: SessionId(10),
            node_id: NodeId(30),
        }),
        ClientMessage::Node(NodeRequest::Close {
            request_id: RequestId(22),
            node_id: NodeId(33),
        }),
        ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: RequestId(23),
            buffer_id: BufferId(22),
            target_leaf_node_id: NodeId(34),
        }),
        ClientMessage::Node(NodeRequest::Resize {
            request_id: RequestId(24),
            node_id: NodeId(35),
            sizes: vec![3, 2, 1],
        }),
        ClientMessage::Node(NodeRequest::Zoom {
            request_id: RequestId(241),
            node_id: NodeId(35),
        }),
        ClientMessage::Node(NodeRequest::Unzoom {
            request_id: RequestId(242),
            session_id: SessionId(10),
        }),
        ClientMessage::Node(NodeRequest::ToggleZoom {
            request_id: RequestId(243),
            node_id: NodeId(35),
        }),
        ClientMessage::Node(NodeRequest::SwapSiblings {
            request_id: RequestId(244),
            first_node_id: NodeId(50),
            second_node_id: NodeId(51),
        }),
        ClientMessage::Node(NodeRequest::BreakNode {
            request_id: RequestId(245),
            node_id: NodeId(35),
            destination: NodeBreakDestination::Floating,
        }),
        ClientMessage::Node(NodeRequest::JoinBufferAtNode {
            request_id: RequestId(246),
            node_id: NodeId(35),
            buffer_id: BufferId(22),
            placement: NodeJoinPlacement::TabAfter,
        }),
        ClientMessage::Node(NodeRequest::MoveNodeBefore {
            request_id: RequestId(247),
            node_id: NodeId(35),
            sibling_node_id: NodeId(36),
        }),
        ClientMessage::Node(NodeRequest::MoveNodeAfter {
            request_id: RequestId(248),
            node_id: NodeId(35),
            sibling_node_id: NodeId(36),
        }),
        ClientMessage::Floating(FloatingRequest::Create {
            request_id: RequestId(25),
            session_id: SessionId(10),
            root_node_id: Some(NodeId(35)),
            buffer_id: None,
            geometry: FloatGeometry::new(4, 2, 60, 18),
            title: Some("inspector".to_owned()),
            focus: false,
            close_on_empty: false,
        }),
        ClientMessage::Floating(FloatingRequest::Close {
            request_id: RequestId(26),
            floating_id: FloatingId(40),
        }),
        ClientMessage::Floating(FloatingRequest::Move {
            request_id: RequestId(27),
            floating_id: FloatingId(40),
            geometry: FloatGeometry::new(8, 6, 50, 14),
        }),
        ClientMessage::Floating(FloatingRequest::Focus {
            request_id: RequestId(28),
            floating_id: FloatingId(40),
        }),
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(29),
            buffer_id: BufferId(22),
            bytes: vec![0x1b, b'[', b'A'],
        }),
        ClientMessage::Input(InputRequest::Resize {
            request_id: RequestId(30),
            buffer_id: BufferId(22),
            cols: 132,
            rows: 42,
        }),
        ClientMessage::Subscribe(SubscribeRequest {
            request_id: RequestId(31),
            session_id: Some(SessionId(10)),
        }),
        ClientMessage::Unsubscribe(UnsubscribeRequest {
            request_id: RequestId(32),
            subscription_id: 99,
        }),
    ];

    for message in messages {
        let encoded = encode_client_message(&message).expect("encode client message");
        let decoded = decode_client_message(&encoded).expect("decode client message");
        assert_eq!(decoded, message);
    }
}

#[test]
fn server_envelope_families_round_trip() {
    let snapshot = sample_snapshot();
    let session = snapshot.session.clone();
    let buffers = snapshot.buffers.clone();
    let floating = snapshot.floating.clone();

    let envelopes = vec![
        ServerEnvelope::Response(ServerResponse::Pong(PingResponse {
            request_id: RequestId(30),
            payload: "pong".to_owned(),
        })),
        ServerEnvelope::Response(ServerResponse::Ok(OkResponse {
            request_id: RequestId(31),
        })),
        ServerEnvelope::Response(ServerResponse::Error(ErrorResponse {
            request_id: None,
            error: WireError::new(ErrorCode::ProtocolViolation, "bad frame"),
        })),
        ServerEnvelope::Response(ServerResponse::Error(ErrorResponse {
            request_id: Some(RequestId(32)),
            error: WireError::new(ErrorCode::NotFound, "missing"),
        })),
        ServerEnvelope::Response(ServerResponse::Sessions(SessionsResponse {
            request_id: RequestId(33),
            sessions: vec![session.clone()],
        })),
        ServerEnvelope::Response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(34),
            snapshot: snapshot.clone(),
        })),
        ServerEnvelope::Response(ServerResponse::Buffers(BuffersResponse {
            request_id: RequestId(35),
            buffers: buffers.clone(),
        })),
        ServerEnvelope::Response(ServerResponse::Buffer(BufferResponse {
            request_id: RequestId(36),
            buffer: buffers[0].clone(),
        })),
        ServerEnvelope::Response(ServerResponse::BufferLocation(BufferLocationResponse {
            request_id: RequestId(361),
            location: BufferLocation {
                buffer_id: BufferId(11),
                session_id: Some(SessionId(10)),
                node_id: Some(NodeId(21)),
                floating_id: None,
            },
        })),
        ServerEnvelope::Response(ServerResponse::FloatingList(FloatingListResponse {
            request_id: RequestId(37),
            floating: floating.clone(),
        })),
        ServerEnvelope::Response(ServerResponse::Floating(FloatingResponse {
            request_id: RequestId(38),
            floating: floating[0].clone(),
        })),
        ServerEnvelope::Response(ServerResponse::SubscriptionAck(SubscriptionAckResponse {
            request_id: RequestId(39),
            subscription_id: 700,
        })),
        ServerEnvelope::Response(ServerResponse::Snapshot(SnapshotResponse {
            request_id: RequestId(40),
            buffer_id: BufferId(11),
            sequence: 9,
            size: PtySize::new(120, 40),
            lines: vec!["alpha".to_owned(), "beta".to_owned()],
            title: Some("shell".to_owned()),
            cwd: Some("/tmp".to_owned()),
        })),
        ServerEnvelope::Response(ServerResponse::VisibleSnapshot(VisibleSnapshotResponse {
            request_id: RequestId(401),
            buffer_id: BufferId(11),
            sequence: 10,
            size: PtySize::new(120, 40),
            lines: vec!["alpha".to_owned(), "beta".to_owned(), "".to_owned()],
            title: Some("shell".to_owned()),
            cwd: Some("/tmp".to_owned()),
            viewport_top_line: 17,
            total_lines: 43,
            alternate_screen: true,
            mouse_reporting: true,
            focus_reporting: true,
            bracketed_paste: true,
            cursor: Some(CursorState {
                position: CursorPosition { row: 1, col: 2 },
                shape: CursorShape::Beam,
            }),
        })),
        ServerEnvelope::Response(ServerResponse::ScrollbackSlice(ScrollbackSliceResponse {
            request_id: RequestId(402),
            buffer_id: BufferId(11),
            start_line: 12,
            total_lines: 43,
            lines: vec!["gamma".to_owned(), "delta".to_owned()],
        })),
        ServerEnvelope::Event(ServerEvent::SessionCreated(SessionCreatedEvent {
            session: session.clone(),
        })),
        ServerEnvelope::Event(ServerEvent::SessionClosed(SessionClosedEvent {
            session_id: SessionId(10),
        })),
        ServerEnvelope::Event(ServerEvent::BufferCreated(BufferCreatedEvent {
            buffer: buffers[0].clone(),
        })),
        ServerEnvelope::Event(ServerEvent::BufferDetached(BufferDetachedEvent {
            buffer_id: BufferId(11),
        })),
        ServerEnvelope::Event(ServerEvent::NodeChanged(NodeChangedEvent {
            session_id: SessionId(10),
        })),
        ServerEnvelope::Event(ServerEvent::FloatingChanged(FloatingChangedEvent {
            session_id: SessionId(10),
            floating_id: Some(FloatingId(30)),
        })),
        ServerEnvelope::Event(ServerEvent::FocusChanged(FocusChangedEvent {
            session_id: SessionId(10),
            focused_leaf_id: Some(NodeId(21)),
            focused_floating_id: Some(FloatingId(30)),
        })),
        ServerEnvelope::Event(ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
            buffer_id: BufferId(11),
        })),
    ];

    for envelope in envelopes {
        let encoded = encode_server_envelope(&envelope).expect("encode server envelope");
        let decoded = decode_server_envelope(&encoded).expect("decode server envelope");
        assert_eq!(decoded, envelope);
    }
}

fn sample_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        session: SessionRecord {
            id: SessionId(10),
            name: "main".to_owned(),
            root_node_id: NodeId(20),
            floating_ids: vec![FloatingId(30)],
            focused_leaf_id: Some(NodeId(21)),
            focused_floating_id: Some(FloatingId(30)),
            zoomed_node_id: Some(NodeId(24)),
        },
        nodes: vec![
            NodeRecord {
                id: NodeId(20),
                session_id: SessionId(10),
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: SplitDirection::Horizontal,
                    child_ids: vec![NodeId(21), NodeId(22)],
                    sizes: vec![70, 50],
                }),
                tabs: None,
            },
            NodeRecord {
                id: NodeId(21),
                session_id: SessionId(10),
                parent_id: Some(NodeId(20)),
                kind: NodeRecordKind::BufferView,
                buffer_view: Some(BufferViewRecord {
                    buffer_id: BufferId(11),
                    focused: true,
                    zoomed: false,
                    follow_output: true,
                    last_render_size: PtySize::new(120, 40),
                }),
                split: None,
                tabs: None,
            },
            NodeRecord {
                id: NodeId(22),
                session_id: SessionId(10),
                parent_id: Some(NodeId(20)),
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: 1,
                    tabs: vec![
                        TabRecord {
                            title: "logs".to_owned(),
                            child_id: NodeId(23),
                        },
                        TabRecord {
                            title: "shell".to_owned(),
                            child_id: NodeId(24),
                        },
                    ],
                }),
            },
            NodeRecord {
                id: NodeId(23),
                session_id: SessionId(10),
                parent_id: Some(NodeId(22)),
                kind: NodeRecordKind::BufferView,
                buffer_view: Some(BufferViewRecord {
                    buffer_id: BufferId(12),
                    focused: false,
                    zoomed: false,
                    follow_output: false,
                    last_render_size: PtySize::new(80, 24),
                }),
                split: None,
                tabs: None,
            },
            NodeRecord {
                id: NodeId(24),
                session_id: SessionId(10),
                parent_id: Some(NodeId(22)),
                kind: NodeRecordKind::BufferView,
                buffer_view: Some(BufferViewRecord {
                    buffer_id: BufferId(13),
                    focused: false,
                    zoomed: true,
                    follow_output: true,
                    last_render_size: PtySize::new(100, 30),
                }),
                split: None,
                tabs: None,
            },
        ],
        buffers: vec![
            sample_buffer_record(
                BufferId(11),
                Some(NodeId(21)),
                BufferRecordState::Running,
                ActivityState::Activity,
                None,
            ),
            sample_buffer_record(
                BufferId(12),
                Some(NodeId(23)),
                BufferRecordState::Exited,
                ActivityState::Bell,
                Some(0),
            ),
            sample_buffer_record(
                BufferId(13),
                Some(NodeId(24)),
                BufferRecordState::Created,
                ActivityState::Idle,
                None,
            ),
        ],
        floating: vec![FloatingRecord {
            id: FloatingId(30),
            session_id: SessionId(10),
            root_node_id: NodeId(24),
            title: Some("inspector".to_owned()),
            geometry: FloatGeometry::new(4, 2, 60, 18),
            focused: true,
            visible: true,
            close_on_empty: false,
        }],
    }
}

fn sample_buffer_record(
    id: BufferId,
    attachment_node_id: Option<NodeId>,
    state: BufferRecordState,
    activity: ActivityState,
    exit_code: Option<i32>,
) -> BufferRecord {
    BufferRecord {
        id,
        title: format!("buffer-{id}"),
        command: vec!["bash".to_owned(), "-lc".to_owned(), "echo mux".to_owned()],
        cwd: Some("/tmp".to_owned()),
        kind: BufferRecordKind::Pty,
        state,
        pid: Some(4242),
        attachment_node_id,
        read_only: false,
        helper_source_buffer_id: None,
        helper_scope: None,
        pty_size: PtySize::new(120, 40),
        activity,
        last_snapshot_seq: 9,
        exit_code,
        env: std::collections::BTreeMap::from([("TERM".to_owned(), "xterm-256color".to_owned())]),
    }
}
