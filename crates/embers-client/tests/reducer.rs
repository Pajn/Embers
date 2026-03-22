use embers_client::{ClientState, MuxClient, ScriptedTransport};
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, PtySize, RequestId, SessionId, SplitDirection,
};
use embers_protocol::{
    BufferDetachedEvent, BufferRecord, BufferRecordState, BufferViewRecord, BuffersResponse,
    ClientMessage, FloatingChangedEvent, FloatingRecord, FocusChangedEvent, NodeChangedEvent,
    NodeRecord, NodeRecordKind, RenderInvalidatedEvent, ServerEvent, ServerResponse, SessionRecord,
    SessionRequest, SessionSnapshot, SessionSnapshotResponse, SplitRecord, TabRecord, TabsRecord,
    VisibleSnapshotResponse,
};

fn buffer(id: u64, attachment_node_id: Option<u64>, title: &str) -> BufferRecord {
    BufferRecord {
        id: BufferId(id),
        title: title.to_owned(),
        command: vec!["/bin/sh".to_owned()],
        cwd: Some("/tmp".to_owned()),
        pid: None,
        env: Default::default(),
        state: BufferRecordState::Running,
        attachment_node_id: attachment_node_id.map(NodeId),
        pty_size: PtySize::new(80, 24),
        activity: ActivityState::Idle,
        last_snapshot_seq: 0,
        exit_code: None,
    }
}

fn buffer_view_node(
    id: u64,
    session_id: u64,
    parent_id: Option<u64>,
    buffer_id: u64,
) -> NodeRecord {
    NodeRecord {
        id: NodeId(id),
        session_id: SessionId(session_id),
        parent_id: parent_id.map(NodeId),
        kind: NodeRecordKind::BufferView,
        buffer_view: Some(BufferViewRecord {
            buffer_id: BufferId(buffer_id),
            focused: false,
            zoomed: false,
            follow_output: true,
            last_render_size: PtySize::new(80, 24),
        }),
        split: None,
        tabs: None,
    }
}

fn session_snapshot(root_active: u32, nested_active: u32) -> SessionSnapshot {
    let session_id = SessionId(1);
    SessionSnapshot {
        session: SessionRecord {
            id: session_id,
            name: "main".to_owned(),
            root_node_id: NodeId(10),
            floating_ids: vec![embers_core::FloatingId(90)],
            focused_leaf_id: Some(NodeId(11)),
            focused_floating_id: None,
        },
        nodes: vec![
            NodeRecord {
                id: NodeId(10),
                session_id,
                parent_id: None,
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: root_active,
                    tabs: vec![
                        TabRecord {
                            title: "shell".to_owned(),
                            child_id: NodeId(11),
                        },
                        TabRecord {
                            title: "hidden".to_owned(),
                            child_id: NodeId(20),
                        },
                    ],
                }),
            },
            buffer_view_node(11, 1, Some(10), 1),
            NodeRecord {
                id: NodeId(20),
                session_id,
                parent_id: Some(NodeId(10)),
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: nested_active,
                    tabs: vec![
                        TabRecord {
                            title: "build".to_owned(),
                            child_id: NodeId(21),
                        },
                        TabRecord {
                            title: "logs".to_owned(),
                            child_id: NodeId(22),
                        },
                    ],
                }),
            },
            buffer_view_node(21, 1, Some(20), 2),
            NodeRecord {
                id: NodeId(22),
                session_id,
                parent_id: Some(NodeId(20)),
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: SplitDirection::Vertical,
                    child_ids: vec![NodeId(23), NodeId(24)],
                    sizes: vec![2, 1],
                }),
                tabs: None,
            },
            buffer_view_node(23, 1, Some(22), 3),
            buffer_view_node(24, 1, Some(22), 4),
            buffer_view_node(30, 1, None, 5),
        ],
        buffers: vec![
            buffer(1, Some(11), "shell"),
            buffer(2, Some(21), "build"),
            buffer(3, Some(23), "logs-a"),
            buffer(4, Some(24), "logs-b"),
            buffer(5, Some(30), "popup"),
        ],
        floating: vec![FloatingRecord {
            id: embers_core::FloatingId(90),
            session_id,
            root_node_id: NodeId(30),
            title: Some("popup".to_owned()),
            geometry: FloatGeometry::new(4, 3, 30, 12),
            focused: false,
            visible: true,
            close_on_empty: true,
        }],
    }
}

fn visible_snapshot(
    buffer_id: u64,
    total_lines: u64,
    viewport_top_line: u64,
    alternate_screen: bool,
) -> VisibleSnapshotResponse {
    VisibleSnapshotResponse {
        request_id: RequestId(0),
        buffer_id: BufferId(buffer_id),
        sequence: 1,
        size: PtySize::new(80, 24),
        lines: vec!["line-a".to_owned(), "line-b".to_owned()],
        title: None,
        cwd: None,
        viewport_top_line,
        total_lines,
        alternate_screen,
        mouse_reporting: false,
        focus_reporting: false,
        bracketed_paste: false,
        cursor: None,
    }
}

#[test]
fn initial_session_snapshot_apply_populates_cache() {
    let snapshot = session_snapshot(0, 0);
    let mut state = ClientState::default();

    state.apply_session_snapshot(snapshot);

    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.nodes.len(), 8);
    assert_eq!(state.buffers.len(), 5);
    assert_eq!(state.floating.len(), 1);
    assert_eq!(
        state
            .nodes
            .get(&NodeId(20))
            .and_then(|node| node.tabs.as_ref())
            .map(|tabs| tabs.active),
        Some(0)
    );
    assert_eq!(
        state
            .buffers
            .get(&BufferId(5))
            .and_then(|buffer| buffer.attachment_node_id),
        Some(NodeId(30))
    );
}

#[test]
fn buffer_detach_focus_and_invalidation_events_update_cache() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));

    state.apply_event(&ServerEvent::BufferDetached(BufferDetachedEvent {
        buffer_id: BufferId(1),
    }));
    state.apply_event(&ServerEvent::FocusChanged(FocusChangedEvent {
        session_id: SessionId(1),
        focused_leaf_id: Some(NodeId(24)),
        focused_floating_id: Some(embers_core::FloatingId(90)),
    }));
    state.apply_event(&ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
        buffer_id: BufferId(5),
    }));

    assert_eq!(
        state
            .buffers
            .get(&BufferId(1))
            .and_then(|buffer| buffer.attachment_node_id),
        None
    );
    assert_eq!(
        state
            .sessions
            .get(&SessionId(1))
            .and_then(|session| session.focused_leaf_id),
        Some(NodeId(24))
    );
    assert_eq!(
        state
            .floating
            .get(&embers_core::FloatingId(90))
            .map(|floating| floating.focused),
        Some(true)
    );
    assert!(state.invalidated_buffers.contains(&BufferId(5)));
}

#[test]
fn node_and_floating_events_mark_session_dirty() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));

    state.apply_event(&ServerEvent::NodeChanged(NodeChangedEvent {
        session_id: SessionId(1),
    }));
    state.apply_event(&ServerEvent::FloatingChanged(FloatingChangedEvent {
        session_id: SessionId(1),
        floating_id: Some(embers_core::FloatingId(90)),
    }));

    assert_eq!(
        state.dirty_sessions.iter().copied().collect::<Vec<_>>(),
        vec![SessionId(1)]
    );
}

#[test]
fn hidden_nested_subtree_state_updates_on_snapshot_refresh() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));
    state.apply_session_snapshot(session_snapshot(0, 1));

    assert_eq!(
        state
            .nodes
            .get(&NodeId(20))
            .and_then(|node| node.tabs.as_ref())
            .map(|tabs| tabs.active),
        Some(1)
    );
}

#[test]
fn session_snapshot_initializes_view_state_for_each_buffer_view() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));

    assert_eq!(state.view_state.len(), 5);
    let root = state.view_state(NodeId(11)).expect("root leaf view state");
    assert_eq!(root.buffer_id, BufferId(1));
    assert!(root.follow_output);
    assert_eq!(root.scroll_top_line, 0);
    assert_eq!(root.visible_line_count, 24);
    assert_eq!(root.total_line_count, 24);
}

#[test]
fn visible_snapshot_updates_following_views_to_live_bottom() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));

    state.apply_buffer_snapshot(visible_snapshot(1, 40, 16, false));

    let root = state.view_state(NodeId(11)).expect("root leaf view state");
    assert_eq!(root.total_line_count, 40);
    assert_eq!(root.scroll_top_line, 16);
    assert_eq!(root.visible_line_count, 24);
    assert!(!root.alternate_screen);
}

#[test]
fn scrolled_view_preserves_position_and_alternate_screen_keeps_state() {
    let mut state = ClientState::default();
    state.apply_session_snapshot(session_snapshot(0, 0));
    let view = state
        .view_state
        .get_mut(&NodeId(11))
        .expect("root leaf view state");
    view.follow_output = false;
    view.scroll_top_line = 5;

    state.apply_buffer_snapshot(visible_snapshot(1, 50, 26, true));
    let root = state.view_state(NodeId(11)).expect("root leaf view state");
    assert_eq!(root.scroll_top_line, 5);
    assert!(root.alternate_screen);
    assert_eq!(root.total_line_count, 50);

    state.apply_buffer_snapshot(visible_snapshot(1, 60, 36, false));
    let root = state.view_state(NodeId(11)).expect("root leaf view state");
    assert_eq!(root.scroll_top_line, 5);
    assert!(!root.alternate_screen);
    assert_eq!(root.total_line_count, 60);
}

#[tokio::test]
async fn process_next_event_resyncs_session_after_mutation() {
    let transport = ScriptedTransport::default();
    transport.push_event(ServerEvent::NodeChanged(NodeChangedEvent {
        session_id: SessionId(1),
    }));
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(1),
            session_id: SessionId(1),
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(1),
            snapshot: session_snapshot(1, 1),
        }),
    );

    let mut client = MuxClient::new(transport.clone());
    let event = client
        .process_next_event()
        .await
        .expect("event is processed");

    assert_eq!(
        event,
        ServerEvent::NodeChanged(NodeChangedEvent {
            session_id: SessionId(1),
        })
    );
    assert_eq!(
        client
            .state()
            .nodes
            .get(&NodeId(10))
            .and_then(|node| node.tabs.as_ref())
            .map(|tabs| tabs.active),
        Some(1)
    );
    transport.assert_exhausted().expect("all requests consumed");
}

#[tokio::test]
async fn reconnect_resync_rebuilds_sessions_and_detached_buffers() {
    let transport = ScriptedTransport::default();
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::List {
            request_id: RequestId(1),
        }),
        ServerResponse::Sessions(embers_protocol::SessionsResponse {
            request_id: RequestId(1),
            sessions: vec![SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(10),
                floating_ids: vec![],
                focused_leaf_id: Some(NodeId(11)),
                focused_floating_id: None,
            }],
        }),
    );
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(2),
            session_id: SessionId(1),
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(2),
            snapshot: session_snapshot(0, 0),
        }),
    );
    transport.push_exchange(
        ClientMessage::Buffer(embers_protocol::BufferRequest::List {
            request_id: RequestId(3),
            session_id: None,
            attached_only: false,
            detached_only: true,
        }),
        ServerResponse::Buffers(BuffersResponse {
            request_id: RequestId(3),
            buffers: vec![buffer(9, None, "detached")],
        }),
    );

    let mut client = MuxClient::new(transport.clone());
    client.state_mut().sessions.insert(
        SessionId(99),
        SessionRecord {
            id: SessionId(99),
            name: "stale".to_owned(),
            root_node_id: NodeId(999),
            floating_ids: vec![],
            focused_leaf_id: None,
            focused_floating_id: None,
        },
    );
    client
        .state_mut()
        .buffers
        .insert(BufferId(7), buffer(7, None, "old-detached"));

    client
        .resync_all_sessions()
        .await
        .expect("full resync succeeds");

    assert!(client.state().sessions.contains_key(&SessionId(1)));
    assert!(!client.state().sessions.contains_key(&SessionId(99)));
    assert!(client.state().buffers.contains_key(&BufferId(9)));
    assert!(!client.state().buffers.contains_key(&BufferId(7)));
    transport.assert_exhausted().expect("all requests consumed");
}
