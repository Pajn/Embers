#![allow(dead_code)]

use embers_client::ClientState;
use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, SessionId, SplitDirection,
};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferViewRecord, FloatingRecord, NodeRecord, NodeRecordKind,
    SessionRecord, SessionSnapshot, SnapshotResponse, SplitRecord, TabRecord, TabsRecord,
};

pub const SESSION_ID: SessionId = SessionId(1);
pub const ROOT_TABS_ID: NodeId = NodeId(10);
pub const HIDDEN_ROOT_LEAF_ID: NodeId = NodeId(11);
pub const ROOT_SPLIT_ID: NodeId = NodeId(20);
pub const LEFT_LEAF_ID: NodeId = NodeId(21);
pub const NESTED_TABS_ID: NodeId = NodeId(30);
pub const HIDDEN_NESTED_LEAF_ID: NodeId = NodeId(31);
pub const FOCUSED_LEAF_ID: NodeId = NodeId(32);
pub const FLOATING_ID: FloatingId = FloatingId(90);
pub const FLOATING_SPLIT_ID: NodeId = NodeId(40);
pub const FLOATING_TOP_LEAF_ID: NodeId = NodeId(41);
pub const FLOATING_BOTTOM_LEAF_ID: NodeId = NodeId(42);
pub const ROOT_BUFFER_LEAF_ID: NodeId = NodeId(50);
pub const ROOT_ONLY_SPLIT_ID: NodeId = NodeId(60);
pub const ROOT_SPLIT_LEFT_LEAF_ID: NodeId = NodeId(61);
pub const ROOT_SPLIT_RIGHT_LEAF_ID: NodeId = NodeId(62);

pub fn demo_state() -> ClientState {
    let mut state = ClientState::default();
    state.apply_session_snapshot(demo_snapshot(None));
    for snapshot in demo_snapshots() {
        state.apply_buffer_snapshot(snapshot);
    }
    state
}

pub fn floating_focused_state() -> ClientState {
    let mut state = ClientState::default();
    state.apply_session_snapshot(demo_snapshot(Some((FLOATING_ID, FLOATING_TOP_LEAF_ID))));
    for snapshot in demo_snapshots() {
        state.apply_buffer_snapshot(snapshot);
    }
    state
}

pub fn root_focus_state() -> ClientState {
    let mut state = ClientState::default();
    state.apply_session_snapshot(demo_snapshot(None));
    for snapshot in demo_snapshots() {
        state.apply_buffer_snapshot(snapshot);
    }
    if let Some(session) = state.sessions.get_mut(&SESSION_ID) {
        session.focused_floating_id = None;
        session.focused_leaf_id = Some(LEFT_LEAF_ID);
    }
    state
}

pub fn root_buffer_state() -> ClientState {
    let mut state = ClientState::default();
    state.apply_session_snapshot(root_buffer_snapshot());
    state.apply_buffer_snapshot(snapshot(7, ["root buffer", "extra line"]));
    state
}

pub fn root_split_state() -> ClientState {
    let mut state = ClientState::default();
    state.apply_session_snapshot(root_split_snapshot());
    state.apply_buffer_snapshot(snapshot(7, ["left root pane"]));
    state.apply_buffer_snapshot(snapshot(8, ["right root pane"]));
    state
}

fn demo_snapshot(focused_floating: Option<(FloatingId, NodeId)>) -> SessionSnapshot {
    let (focused_floating_id, focused_leaf_id) = focused_floating
        .map(|(floating_id, leaf_id)| {
            if floating_id.0 == 0 {
                (None, Some(leaf_id))
            } else {
                (Some(floating_id), Some(leaf_id))
            }
        })
        .unwrap_or((None, Some(FOCUSED_LEAF_ID)));

    SessionSnapshot {
        session: SessionRecord {
            id: SESSION_ID,
            name: "demo".to_owned(),
            root_node_id: ROOT_TABS_ID,
            floating_ids: vec![FLOATING_ID],
            focused_leaf_id,
            focused_floating_id,
        },
        nodes: vec![
            NodeRecord {
                id: ROOT_TABS_ID,
                session_id: SESSION_ID,
                parent_id: None,
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: 1,
                    tabs: vec![
                        TabRecord {
                            title: "shell".to_owned(),
                            child_id: HIDDEN_ROOT_LEAF_ID,
                        },
                        TabRecord {
                            title: "workspace".to_owned(),
                            child_id: ROOT_SPLIT_ID,
                        },
                    ],
                }),
            },
            buffer_view_node(HIDDEN_ROOT_LEAF_ID, Some(ROOT_TABS_ID), BufferId(1)),
            NodeRecord {
                id: ROOT_SPLIT_ID,
                session_id: SESSION_ID,
                parent_id: Some(ROOT_TABS_ID),
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: SplitDirection::Vertical,
                    child_ids: vec![LEFT_LEAF_ID, NESTED_TABS_ID],
                    sizes: vec![1, 2],
                }),
                tabs: None,
            },
            buffer_view_node(LEFT_LEAF_ID, Some(ROOT_SPLIT_ID), BufferId(2)),
            NodeRecord {
                id: NESTED_TABS_ID,
                session_id: SESSION_ID,
                parent_id: Some(ROOT_SPLIT_ID),
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: 1,
                    tabs: vec![
                        TabRecord {
                            title: "build".to_owned(),
                            child_id: HIDDEN_NESTED_LEAF_ID,
                        },
                        TabRecord {
                            title: "logs-long-title".to_owned(),
                            child_id: FOCUSED_LEAF_ID,
                        },
                    ],
                }),
            },
            buffer_view_node(HIDDEN_NESTED_LEAF_ID, Some(NESTED_TABS_ID), BufferId(3)),
            buffer_view_node(FOCUSED_LEAF_ID, Some(NESTED_TABS_ID), BufferId(4)),
            NodeRecord {
                id: FLOATING_SPLIT_ID,
                session_id: SESSION_ID,
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: SplitDirection::Horizontal,
                    child_ids: vec![FLOATING_TOP_LEAF_ID, FLOATING_BOTTOM_LEAF_ID],
                    sizes: vec![1, 1],
                }),
                tabs: None,
            },
            buffer_view_node(FLOATING_TOP_LEAF_ID, Some(FLOATING_SPLIT_ID), BufferId(5)),
            buffer_view_node(
                FLOATING_BOTTOM_LEAF_ID,
                Some(FLOATING_SPLIT_ID),
                BufferId(6),
            ),
        ],
        buffers: vec![
            buffer(1, Some(HIDDEN_ROOT_LEAF_ID), "shell", ActivityState::Idle),
            buffer(2, Some(LEFT_LEAF_ID), "editor", ActivityState::Activity),
            buffer(3, Some(HIDDEN_NESTED_LEAF_ID), "build", ActivityState::Bell),
            buffer(
                4,
                Some(FOCUSED_LEAF_ID),
                "logs-long-title",
                ActivityState::Idle,
            ),
            buffer(
                5,
                Some(FLOATING_TOP_LEAF_ID),
                "popup-top",
                ActivityState::Idle,
            ),
            buffer(
                6,
                Some(FLOATING_BOTTOM_LEAF_ID),
                "popup-bottom",
                ActivityState::Idle,
            ),
        ],
        floating: vec![FloatingRecord {
            id: FLOATING_ID,
            session_id: SESSION_ID,
            root_node_id: FLOATING_SPLIT_ID,
            title: Some("popup".to_owned()),
            geometry: FloatGeometry::new(14, 5, 20, 7),
            focused: focused_floating_id == Some(FLOATING_ID),
            visible: true,
            close_on_empty: true,
        }],
    }
}

fn root_buffer_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        session: SessionRecord {
            id: SESSION_ID,
            name: "root-buffer".to_owned(),
            root_node_id: ROOT_BUFFER_LEAF_ID,
            floating_ids: Vec::new(),
            focused_leaf_id: Some(ROOT_BUFFER_LEAF_ID),
            focused_floating_id: None,
        },
        nodes: vec![buffer_view_node(ROOT_BUFFER_LEAF_ID, None, BufferId(7))],
        buffers: vec![buffer(
            7,
            Some(ROOT_BUFFER_LEAF_ID),
            "root-buffer",
            ActivityState::Idle,
        )],
        floating: Vec::new(),
    }
}

fn root_split_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        session: SessionRecord {
            id: SESSION_ID,
            name: "root-split".to_owned(),
            root_node_id: ROOT_ONLY_SPLIT_ID,
            floating_ids: Vec::new(),
            focused_leaf_id: Some(ROOT_SPLIT_RIGHT_LEAF_ID),
            focused_floating_id: None,
        },
        nodes: vec![
            NodeRecord {
                id: ROOT_ONLY_SPLIT_ID,
                session_id: SESSION_ID,
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: SplitDirection::Vertical,
                    child_ids: vec![ROOT_SPLIT_LEFT_LEAF_ID, ROOT_SPLIT_RIGHT_LEAF_ID],
                    sizes: vec![1, 1],
                }),
                tabs: None,
            },
            buffer_view_node(ROOT_SPLIT_LEFT_LEAF_ID, Some(ROOT_ONLY_SPLIT_ID), BufferId(7)),
            buffer_view_node(
                ROOT_SPLIT_RIGHT_LEAF_ID,
                Some(ROOT_ONLY_SPLIT_ID),
                BufferId(8),
            ),
        ],
        buffers: vec![
            buffer(
                7,
                Some(ROOT_SPLIT_LEFT_LEAF_ID),
                "root-left",
                ActivityState::Idle,
            ),
            buffer(
                8,
                Some(ROOT_SPLIT_RIGHT_LEAF_ID),
                "root-right",
                ActivityState::Activity,
            ),
        ],
        floating: Vec::new(),
    }
}

fn buffer_view_node(id: NodeId, parent_id: Option<NodeId>, buffer_id: BufferId) -> NodeRecord {
    NodeRecord {
        id,
        session_id: SESSION_ID,
        parent_id,
        kind: NodeRecordKind::BufferView,
        buffer_view: Some(BufferViewRecord {
            buffer_id,
            focused: false,
            zoomed: false,
            follow_output: true,
            last_render_size: PtySize::new(80, 24),
        }),
        split: None,
        tabs: None,
    }
}

fn buffer(
    id: u64,
    attachment_node_id: Option<NodeId>,
    title: &str,
    activity: ActivityState,
) -> BufferRecord {
    BufferRecord {
        id: BufferId(id),
        title: title.to_owned(),
        command: vec!["/bin/sh".to_owned()],
        cwd: Some("/tmp".to_owned()),
        pid: None,
        env: Default::default(),
        state: BufferRecordState::Running,
        attachment_node_id,
        pty_size: PtySize::new(80, 24),
        activity,
        last_snapshot_seq: 0,
        exit_code: None,
    }
}

fn demo_snapshots() -> Vec<SnapshotResponse> {
    vec![
        snapshot(2, ["left pane", "line two", "line three"]),
        snapshot(4, ["logs visible", "second row", "third row"]),
        snapshot(5, ["popup top"]),
        snapshot(6, ["popup bottom"]),
    ]
}

fn snapshot<const N: usize>(buffer_id: u64, lines: [&str; N]) -> SnapshotResponse {
    SnapshotResponse {
        request_id: embers_core::RequestId(0),
        buffer_id: BufferId(buffer_id),
        sequence: 1,
        size: PtySize::new(80, 24),
        lines: lines.into_iter().map(str::to_owned).collect(),
        title: None,
        cwd: None,
    }
}
