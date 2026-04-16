use embers_client::PresentationModel;
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, PtySize, SessionId, Size, SplitDirection,
};
use embers_protocol::{
    BufferRecord, BufferRecordKind, BufferRecordState, BufferViewRecord, NodeRecord, NodeRecordKind,
};

use crate::support::{
    FLOATING_BOTTOM_LEAF_ID, FLOATING_ID, FLOATING_SPLIT_ID, FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID,
    LEFT_LEAF_ID, NESTED_TABS_ID, ROOT_BUFFER_LEAF_ID, ROOT_ONLY_SPLIT_ID, ROOT_SPLIT_LEFT_LEAF_ID,
    ROOT_SPLIT_RIGHT_LEAF_ID, ROOT_TABS_ID, SESSION_ID, demo_state, floating_focused_state,
    root_buffer_state, root_split_state,
};

#[test]
fn projects_nested_tabs_in_split_and_tracks_focus_path() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let root_tabs = presentation
        .root_tabs
        .as_ref()
        .expect("root tabs are visible");
    assert_eq!(root_tabs.node_id, ROOT_TABS_ID);
    assert_eq!(root_tabs.tabs.len(), 2);
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").tabs_path,
        vec![ROOT_TABS_ID, NESTED_TABS_ID]
    );
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").node_id,
        FOCUSED_LEAF_ID
    );

    let left_leaf = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == LEFT_LEAF_ID)
        .expect("left leaf is visible");
    let right_leaf = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == FOCUSED_LEAF_ID)
        .expect("right leaf is visible");

    assert_eq!(left_leaf.rect.origin.x, 0);
    assert!(right_leaf.rect.origin.x > left_leaf.rect.origin.x);
}

#[test]
fn projects_split_in_floating_window() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let floating = presentation
        .floating
        .iter()
        .find(|window| window.floating_id == FLOATING_ID)
        .expect("floating window exists");
    assert_eq!(floating.rect.size.width, 20);
    assert_eq!(floating.rect.size.height, 7);

    let floating_leaves = presentation
        .leaves
        .iter()
        .filter(|leaf| leaf.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    assert_eq!(floating_leaves.len(), 2);
    assert!(
        floating_leaves
            .iter()
            .any(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
    );
    assert!(
        floating_leaves
            .iter()
            .any(|leaf| leaf.node_id == FLOATING_BOTTOM_LEAF_ID)
    );

    let floating_divider = presentation
        .dividers
        .iter()
        .find(|divider| divider.floating_id == Some(FLOATING_ID))
        .expect("floating split divider exists");
    assert_eq!(floating_divider.direction, SplitDirection::Horizontal);
}

#[test]
fn floating_windows_can_start_on_the_top_row() {
    let mut state = demo_state();
    state
        .floating
        .get_mut(&FLOATING_ID)
        .expect("floating window exists")
        .geometry = FloatGeometry::new(0, 0, 20, 7);

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let floating = presentation
        .floating
        .iter()
        .find(|window| window.floating_id == FLOATING_ID)
        .expect("floating window exists");
    assert_eq!(floating.rect.origin.y, 0);
    assert_eq!(floating.rect.size.height, 7);
}

#[test]
fn zoomed_floating_windows_keep_floating_context() {
    let mut state = floating_focused_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FLOATING_SPLIT_ID);

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let floating = presentation
        .floating
        .iter()
        .find(|window| window.floating_id == FLOATING_ID)
        .expect("zoomed floating frame exists");
    let floating_leaves = presentation
        .leaves
        .iter()
        .filter(|leaf| leaf.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    let floating_dividers = presentation
        .dividers
        .iter()
        .filter(|divider| divider.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    let floating_leaf_ids = floating_leaves
        .iter()
        .map(|leaf| leaf.node_id)
        .collect::<Vec<_>>();
    assert_eq!(floating.rect.size.width, 20);
    assert_eq!(floating.rect.size.height, 7);
    assert_eq!(floating_leaf_ids.len(), 2);
    assert!(floating_leaf_ids.contains(&FLOATING_TOP_LEAF_ID));
    assert!(floating_leaf_ids.contains(&FLOATING_BOTTOM_LEAF_ID));
    assert_eq!(presentation.leaves.len(), 2);
    assert_eq!(floating_dividers.len(), 1);
    assert_eq!(presentation.dividers.len(), 1);
    assert_eq!(floating_dividers[0].direction, SplitDirection::Horizontal);
}

#[test]
fn zoomed_floating_descendants_keep_floating_context() {
    let mut state = floating_focused_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FLOATING_TOP_LEAF_ID);

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(presentation.focused_floating_id(), Some(FLOATING_ID));
    assert_eq!(presentation.floating.len(), 1);
    assert_eq!(presentation.floating[0].floating_id, FLOATING_ID);
    assert_eq!(presentation.leaves.len(), 1);
    assert_eq!(presentation.leaves[0].node_id, FLOATING_TOP_LEAF_ID);
    assert_eq!(presentation.leaves[0].floating_id, Some(FLOATING_ID));
}

#[test]
fn stale_zoomed_floating_nodes_outside_session_are_ignored() {
    let mut state = demo_state();
    let session = state.sessions.get_mut(&SESSION_ID).expect("session exists");
    session.zoomed_node_id = Some(FLOATING_TOP_LEAF_ID);
    session.floating_ids.clear();

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(
        presentation.root_tabs.as_ref().map(|tabs| tabs.node_id),
        Some(ROOT_TABS_ID)
    );
    assert_eq!(
        presentation.focused_leaf().map(|leaf| leaf.node_id),
        Some(FOCUSED_LEAF_ID)
    );
    assert!(presentation.floating.is_empty());
    assert!(
        presentation
            .leaves
            .iter()
            .all(|leaf| leaf.floating_id.is_none())
    );
}

#[test]
fn foreign_session_zoom_targets_are_ignored() {
    const FOREIGN_SESSION_ID: SessionId = SessionId(99);
    const FOREIGN_NODE_ID: NodeId = NodeId(999);
    const FOREIGN_BUFFER_ID: BufferId = BufferId(999);

    let mut state = demo_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FOREIGN_NODE_ID);
    state.nodes.insert(
        FOREIGN_NODE_ID,
        NodeRecord {
            id: FOREIGN_NODE_ID,
            session_id: FOREIGN_SESSION_ID,
            // Intentional: point the foreign node at the local root tabs so session checks win.
            parent_id: Some(ROOT_TABS_ID),
            kind: NodeRecordKind::BufferView,
            buffer_view: Some(BufferViewRecord {
                buffer_id: FOREIGN_BUFFER_ID,
                focused: false,
                zoomed: false,
                follow_output: true,
                last_render_size: PtySize::new(80, 24),
            }),
            split: None,
            tabs: None,
        },
    );
    state.buffers.insert(
        FOREIGN_BUFFER_ID,
        BufferRecord {
            id: FOREIGN_BUFFER_ID,
            title: "foreign".to_owned(),
            command: vec!["/bin/sh".to_owned()],
            cwd: Some("/tmp".to_owned()),
            kind: BufferRecordKind::Pty,
            pid: None,
            env: Default::default(),
            state: BufferRecordState::Running,
            attachment_node_id: Some(FOREIGN_NODE_ID),
            read_only: false,
            helper_source_buffer_id: None,
            helper_scope: None,
            pty_size: PtySize::new(80, 24),
            activity: ActivityState::Idle,
            last_snapshot_seq: 1,
            exit_code: None,
        },
    );

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(
        presentation.root_tabs.as_ref().map(|tabs| tabs.node_id),
        Some(ROOT_TABS_ID)
    );
    assert_eq!(
        presentation.focused_leaf().map(|leaf| leaf.node_id),
        Some(FOCUSED_LEAF_ID)
    );
    assert!(
        presentation
            .leaves
            .iter()
            .all(|leaf| leaf.node_id != FOREIGN_NODE_ID)
    );
}

#[test]
fn projects_root_buffer_without_tabs_frame() {
    let state = root_buffer_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert!(presentation.root_tabs.is_none());
    assert!(presentation.tab_bars.is_empty());
    assert_eq!(presentation.leaves.len(), 1);
    assert_eq!(presentation.leaves[0].node_id, ROOT_BUFFER_LEAF_ID);
    assert!(presentation.leaves[0].tabs_path.is_empty());
}

#[test]
fn projects_root_split_without_tabs_frame() {
    let state = root_split_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert!(presentation.root_tabs.is_none());
    assert!(presentation.tab_bars.is_empty());
    assert_eq!(presentation.leaves.len(), 2);
    assert_eq!(presentation.dividers.len(), 1);
    assert_eq!(presentation.dividers[0].direction, SplitDirection::Vertical);
    assert_eq!(presentation.session_id, SESSION_ID);
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").node_id,
        ROOT_SPLIT_RIGHT_LEAF_ID
    );
    assert!(
        presentation
            .leaves
            .iter()
            .any(|leaf| leaf.node_id == ROOT_SPLIT_LEFT_LEAF_ID)
    );
    assert_eq!(
        presentation.focus_target(embers_client::NavigationDirection::Left),
        Some(ROOT_SPLIT_LEFT_LEAF_ID)
    );
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").tabs_path,
        Vec::<embers_core::NodeId>::new()
    );
    assert_eq!(presentation.dividers[0].floating_id, None);
    assert_ne!(ROOT_ONLY_SPLIT_ID, ROOT_SPLIT_LEFT_LEAF_ID);
}
