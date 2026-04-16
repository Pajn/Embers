use std::collections::BTreeSet;

use embers_core::{BufferId, FloatGeometry, NodeId, SessionId, SplitDirection};
use embers_protocol::NodeJoinPlacement;
use embers_server::{BufferAttachment, Node, ServerState};
use proptest::prelude::*;

fn seed_single_leaf_session(state: &mut ServerState, name: &str) -> (SessionId, BufferId, NodeId) {
    let session_id = state.create_session(name);
    let buffer_id = state.create_buffer("shell", vec!["sh".to_owned()], None);
    let leaf_id = state
        .create_buffer_view(session_id, buffer_id)
        .expect("create root leaf");
    state
        .add_root_tab(session_id, "main", leaf_id)
        .expect("insert root tab");
    (session_id, buffer_id, leaf_id)
}

fn new_leaf(state: &mut ServerState, session_id: SessionId, label: &str) -> (BufferId, NodeId) {
    let buffer_id = state.create_buffer(label, vec!["sh".to_owned()], None);
    let leaf_id = state
        .create_buffer_view(session_id, buffer_id)
        .expect("create detached leaf");
    (buffer_id, leaf_id)
}

fn new_buffer(state: &mut ServerState, label: &str) -> BufferId {
    state.create_buffer(label, vec!["sh".to_owned()], None)
}

fn attached_view(state: &ServerState, buffer_id: BufferId) -> NodeId {
    match state.buffer(buffer_id).expect("buffer exists").attachment {
        BufferAttachment::Attached(node_id) => node_id,
        BufferAttachment::Detached => panic!("buffer {buffer_id} is detached"),
    }
}

fn session_root(state: &ServerState, session_id: SessionId) -> NodeId {
    state.session(session_id).expect("session exists").root_node
}

fn root_tab_child(state: &ServerState, session_id: SessionId, index: usize) -> NodeId {
    let root = state.root_tabs(session_id).expect("root tabs");
    match state.node(root).expect("root node") {
        Node::Tabs(tabs) => tabs.tabs[index].child,
        other => panic!("expected root tabs, got {other:?}"),
    }
}

fn reachable_nodes(state: &ServerState, session_id: SessionId) -> BTreeSet<NodeId> {
    fn visit(state: &ServerState, node_id: NodeId, seen: &mut BTreeSet<NodeId>) {
        if !seen.insert(node_id) {
            return;
        }
        for child in state.node(node_id).expect("node exists").child_ids() {
            visit(state, child, seen);
        }
    }

    let mut seen = BTreeSet::new();
    let session = state.session(session_id).expect("session exists");
    visit(state, session.root_node, &mut seen);
    for floating_id in &session.floating {
        let floating = state
            .floating_window(*floating_id)
            .expect("floating exists");
        visit(state, floating.root_node, &mut seen);
    }
    seen
}

fn apply_random_op(state: &mut ServerState, session_id: SessionId, selector: u8, arg: u8) {
    let root_tabs = state.root_tabs(session_id).ok();
    let tab_count = root_tabs
        .and_then(|root| match state.node(root).expect("root node") {
            Node::Tabs(tabs) => Some(tabs.tabs.len()),
            _ => None,
        })
        .unwrap_or(0);

    match selector % 5 {
        0 => {
            if state.root_tabs(session_id).is_ok() {
                let (_, leaf_id) = new_leaf(state, session_id, &format!("tab-{arg}"));
                let _ = state.add_root_tab(session_id, format!("tab-{arg}"), leaf_id);
            }
        }
        1 => {
            if let Some(focused_leaf) = state.session(session_id).expect("session").focused_leaf {
                let buffer_id = new_buffer(state, &format!("split-{arg}"));
                let direction = if arg.is_multiple_of(2) {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                };
                let _ = state.split_leaf_with_new_buffer(focused_leaf, direction, buffer_id);
            }
        }
        2 => {
            if let Some(focused_leaf) = state.session(session_id).expect("session").focused_leaf {
                let _ = state.close_node(focused_leaf);
            }
        }
        3 => {
            if let Some(root) = root_tabs
                && tab_count > 0
            {
                let _ = state.switch_tab(root, usize::from(arg) % tab_count);
            }
        }
        4 => {
            if let Some(root) = root_tabs
                && tab_count > 0
            {
                let _ = state.close_tab(root, usize::from(arg) % tab_count);
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn node_creation_and_parent_ownership_are_consistent() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let buffer_id = new_buffer(&mut state, "beta");

    let split_id = state
        .split_leaf_with_new_buffer(leaf_id, SplitDirection::Horizontal, buffer_id)
        .expect("split root leaf");
    let new_leaf = attached_view(&state, buffer_id);

    assert_eq!(
        state.node_parent(leaf_id).expect("old leaf parent"),
        Some(split_id)
    );
    assert_eq!(
        state.node_parent(new_leaf).expect("new leaf parent"),
        Some(split_id)
    );
    assert_eq!(root_tab_child(&state, session_id, 0), split_id);
    state.validate().expect("state should validate");
}

#[test]
fn split_normalization_collapses_single_child_split() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let buffer_id = new_buffer(&mut state, "beta");
    let split_id = state
        .split_leaf_with_new_buffer(leaf_id, SplitDirection::Vertical, buffer_id)
        .expect("split root leaf");
    let new_leaf = attached_view(&state, buffer_id);

    state.close_node(new_leaf).expect("close new leaf");

    assert!(!state.nodes.contains_key(&split_id));
    assert_eq!(session_root(&state, session_id), leaf_id);
    assert_eq!(state.node_parent(leaf_id).expect("leaf parent"), None);
    state.validate().expect("state should validate");
}

#[test]
fn tabs_normalization_collapses_nested_singleton_tabs() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");

    let wrapped = state
        .wrap_node_in_tabs(leaf_id, "inner")
        .expect("wrap leaf in nested tabs");
    state
        .normalize_upwards(wrapped)
        .expect("normalize wrapped tabs");

    assert!(!state.nodes.contains_key(&wrapped));
    assert_eq!(session_root(&state, session_id), leaf_id);
    assert_eq!(state.node_parent(leaf_id).expect("leaf parent"), None);
    state.validate().expect("state should validate");
}

#[test]
fn focus_heals_deterministically_after_close() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let buffer_id = new_buffer(&mut state, "beta");
    state
        .split_leaf_with_new_buffer(leaf_id, SplitDirection::Horizontal, buffer_id)
        .expect("split root leaf");
    let new_leaf = attached_view(&state, buffer_id);

    state.close_node(new_leaf).expect("close focused leaf");

    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(leaf_id)
    );
    let leaf = state.node(leaf_id).expect("leaf exists");
    assert!(matches!(leaf, Node::BufferView(view) if view.view.focused));
    state.validate().expect("state should validate");
}

#[test]
fn create_split_rejects_duplicate_children_without_mutating_state() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let (_, leaf_id) = new_leaf(&mut state, session_id, "dup");
    state
        .create_floating_window(
            session_id,
            leaf_id,
            FloatGeometry::new(1, 1, 10, 6),
            Some("popup".to_owned()),
        )
        .expect("create floating window");

    let error = state
        .create_split_node(
            session_id,
            SplitDirection::Horizontal,
            vec![leaf_id, leaf_id],
        )
        .expect_err("duplicate split children should fail");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    assert_eq!(state.node_parent(leaf_id).expect("leaf parent"), None);
    state.validate().expect("state remains valid");
}

#[test]
fn create_tabs_rejects_children_with_existing_parents() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");

    let error = state
        .create_tabs_node(
            session_id,
            vec![embers_server::TabEntry::new("main", leaf_id)],
            0,
        )
        .expect_err("attached child should be rejected");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    state.validate().expect("state remains valid");
}

#[test]
fn floating_root_cannot_be_reused() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let (_, floating_leaf) = new_leaf(&mut state, session_id, "popup");

    state
        .create_floating_window(
            session_id,
            floating_leaf,
            FloatGeometry::new(2, 2, 15, 8),
            Some("popup".to_owned()),
        )
        .expect("create floating window");

    let error = state
        .create_floating_window(
            session_id,
            floating_leaf,
            FloatGeometry::new(4, 4, 20, 10),
            Some("duplicate".to_owned()),
        )
        .expect_err("floating root reuse should fail");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    state.validate().expect("state remains valid");
}

#[test]
fn add_tab_sibling_rejects_self_parenting() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let wrapped = state
        .wrap_node_in_tabs(leaf_id, "nested")
        .expect("wrap node in tabs");

    let error = state
        .add_tab_sibling(wrapped, "self", wrapped)
        .expect_err("tabs should not be able to contain themselves");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    state.validate().expect("state remains valid");
    assert_eq!(
        state.node_parent(wrapped).expect("wrapped parent"),
        Some(state.root_tabs(session_id).expect("root tabs"))
    );
}

#[test]
fn public_detach_buffer_closes_live_views() {
    let mut state = ServerState::new();
    let (session_id, buffer_id, leaf_id) = seed_single_leaf_session(&mut state, "alpha");

    state.detach_buffer(buffer_id).expect("detach buffer");

    assert!(matches!(
        state.node(leaf_id),
        Err(embers_core::MuxError::NotFound(_))
    ));
    assert!(matches!(
        state.buffer(buffer_id).expect("buffer exists").attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        None
    );
    state.validate().expect("state remains valid");
}

#[test]
fn zoom_toggle_tracks_session_zoomed_node_and_clears_on_close() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");

    state.toggle_zoom_node(leaf_id).expect("zoom leaf");
    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        Some(leaf_id)
    );

    state.close_node(leaf_id).expect("close zoomed leaf");
    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        None
    );
    state.validate().expect("state remains valid");
}

#[test]
fn zooming_moves_focus_into_the_zoomed_subtree() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let detached = new_buffer(&mut state, "logs");

    state
        .join_buffer_at_node(leaf_id, detached, NodeJoinPlacement::Right)
        .expect("join right");
    let detached_view = attached_view(&state, detached);
    state
        .focus_leaf(session_id, detached_view)
        .expect("focus detached view");

    state.zoom_node(leaf_id).expect("zoom leaf");

    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        Some(leaf_id)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(leaf_id)
    );
    assert!(matches!(
        state.node(leaf_id).expect("leaf"),
        Node::BufferView(view) if view.view.focused
    ));
    assert!(matches!(
        state.node(detached_view).expect("detached leaf"),
        Node::BufferView(view) if !view.view.focused
    ));
    state.validate().expect("state remains valid");
}

#[test]
fn closing_zoomed_tab_clears_session_zoom() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let (_, second_leaf) = new_leaf(&mut state, session_id, "second");
    state
        .add_root_tab(session_id, "second", second_leaf)
        .expect("add second root tab");
    let root_tabs = state.root_tabs(session_id).expect("root tabs");
    state.switch_tab(root_tabs, 0).expect("focus first tab");

    state.toggle_zoom_node(leaf_id).expect("zoom leaf");
    state.close_tab(root_tabs, 0).expect("close zoomed tab");

    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        None
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(second_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_floating,
        None
    );
    state.validate().expect("state remains valid");
}

#[test]
fn zooming_inactive_tab_is_rejected() {
    let mut state = ServerState::new();
    let (session_id, _, first_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, second_leaf) = new_leaf(&mut state, session_id, "second");
    let root_tabs = state.root_tabs(session_id).expect("root tabs");
    state
        .add_root_tab(session_id, "second", second_leaf)
        .expect("add second root tab");

    assert!(state.zoom_node(first_leaf).is_err());
    assert!(state.toggle_zoom_node(first_leaf).is_err());
    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        None
    );
    state.validate().expect("state remains valid");

    state.switch_tab(root_tabs, 0).expect("focus first tab");
    state.zoom_node(first_leaf).expect("zoom visible tab");
    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        Some(first_leaf)
    );
}

#[test]
fn closing_zoomed_floating_clears_session_zoom() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let popup_buffer = new_buffer(&mut state, "popup");
    let floating_id = state
        .create_floating_from_buffer(
            session_id,
            popup_buffer,
            FloatGeometry::new(5, 3, 40, 12),
            Some("popup".to_owned()),
        )
        .expect("create floating");
    let floating_root = state
        .floating_window(floating_id)
        .expect("floating")
        .root_node;

    state
        .toggle_zoom_node(floating_root)
        .expect("zoom floating");
    state
        .close_floating(floating_id)
        .expect("close zoomed floating");

    assert_eq!(
        state.session(session_id).expect("session").zoomed_node,
        None
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_floating,
        None
    );
    state.validate().expect("state remains valid");
}

#[test]
fn swap_and_reorder_operate_only_on_siblings() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let second = new_buffer(&mut state, "second");
    let third = new_buffer(&mut state, "third");
    let split_id = state
        .split_leaf_with_new_buffer(leaf_id, SplitDirection::Vertical, second)
        .expect("split root");
    let _second_leaf = attached_view(&state, second);
    let third_leaf = state
        .create_buffer_view(session_id, third)
        .expect("third leaf");
    state
        .wrap_node_in_split(split_id, SplitDirection::Vertical, third_leaf, false)
        .expect("wrap split with third leaf");

    let root = root_tab_child(&state, session_id, 0);
    let split = match state.node(root).expect("root split") {
        Node::Split(split) => split.clone(),
        other => panic!("expected split, got {other:?}"),
    };
    let first_child = split.children[0];
    let second_child = split.children[1];
    let non_sibling = match state.node(first_child).expect("nested split") {
        Node::Split(split) => split.children[0],
        other => panic!("expected nested split, got {other:?}"),
    };

    state
        .swap_sibling_nodes(first_child, second_child)
        .expect("swap siblings");
    let swapped_children = match state.node(root).expect("root split") {
        Node::Split(split) => split.children.clone(),
        other => panic!("expected split after swap, got {other:?}"),
    };
    assert_eq!(swapped_children[0], second_child);
    assert_eq!(swapped_children[1], first_child);
    assert!(
        state.swap_sibling_nodes(non_sibling, second_child).is_err(),
        "non-siblings should not swap"
    );
    assert!(
        state.move_node_before(non_sibling, second_child).is_err(),
        "non-siblings should not reorder"
    );
    state
        .move_node_before(first_child, second_child)
        .expect("move sibling before");
    let restored_children = match state.node(root).expect("root split") {
        Node::Split(split) => split.children.clone(),
        other => panic!("expected split after reorder, got {other:?}"),
    };
    assert_eq!(restored_children[0], first_child);
    assert_eq!(restored_children[1], second_child);
    state.validate().expect("state remains valid");
}

#[test]
fn break_node_to_floating_preserves_subtree_and_focuses_popup() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let buffer_id = new_buffer(&mut state, "beta");
    state
        .split_leaf_with_new_buffer(leaf_id, SplitDirection::Horizontal, buffer_id)
        .expect("split root");
    let new_leaf = attached_view(&state, buffer_id);

    state.break_node(new_leaf, true).expect("break to floating");

    let session = state.session(session_id).expect("session");
    assert_eq!(session.floating.len(), 1);
    let floating = state
        .floating_window(session.floating[0])
        .expect("floating exists");
    assert_eq!(floating.root_node, new_leaf);
    assert_eq!(session.focused_floating, Some(floating.id));
    assert_eq!(session.focused_leaf, Some(new_leaf));
    match state.node(new_leaf).expect("new floating leaf exists") {
        Node::BufferView(leaf) => assert!(leaf.view.focused),
        other => panic!("expected floating root leaf, got {other:?}"),
    }
    state.validate().expect("state remains valid");
}

#[test]
fn breaking_existing_floating_to_floating_preserves_geometry_and_title() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let (_, popup_leaf) = new_leaf(&mut state, session_id, "popup");
    let geometry = FloatGeometry::new(7, 8, 30, 12);
    let floating_id = state
        .create_floating_window_with_options(
            session_id,
            popup_leaf,
            geometry,
            Some("popup".to_owned()),
            false,
            true,
        )
        .expect("create floating window");

    state
        .break_node(popup_leaf, true)
        .expect("breaking an existing floating root to floating is a no-op");

    let session = state.session(session_id).expect("session");
    assert_eq!(session.floating, vec![floating_id]);
    let floating = state.floating_window(floating_id).expect("floating exists");
    assert_eq!(floating.root_node, popup_leaf);
    assert_eq!(floating.geometry, geometry);
    assert_eq!(floating.title.as_deref(), Some("popup"));
    state.validate().expect("state remains valid");
}

#[test]
fn breaking_existing_tab_to_tab_preserves_order() {
    let mut state = ServerState::new();
    let (session_id, _, first_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, second_leaf) = new_leaf(&mut state, session_id, "beta");
    state
        .add_root_tab(session_id, "beta", second_leaf)
        .expect("add second root tab");
    let root_tabs = state.root_tabs(session_id).expect("root tabs");
    let before = match state.node(root_tabs).expect("root tabs") {
        Node::Tabs(tabs) => tabs.tabs.iter().map(|tab| tab.child).collect::<Vec<_>>(),
        other => panic!("expected root tabs, got {other:?}"),
    };

    state
        .break_node(first_leaf, false)
        .expect("breaking an existing tab to a tab is a no-op");

    let after = match state.node(root_tabs).expect("root tabs") {
        Node::Tabs(tabs) => tabs.tabs.iter().map(|tab| tab.child).collect::<Vec<_>>(),
        other => panic!("expected root tabs, got {other:?}"),
    };
    assert_eq!(after, before);
    state.validate().expect("state remains valid");
}

#[test]
fn join_buffer_at_node_can_insert_tabs_and_splits() {
    let mut state = ServerState::new();
    let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let detached = new_buffer(&mut state, "tools");

    state
        .join_buffer_at_node(leaf_id, detached, NodeJoinPlacement::Right)
        .expect("join right");
    let root = root_tab_child(&state, session_id, 0);
    let detached_view = attached_view(&state, detached);
    match state.node(root).expect("root") {
        Node::Split(split) => assert_eq!(split.children, vec![leaf_id, detached_view]),
        other => panic!("expected split root, got {other:?}"),
    }
    let target_root = root;

    let detached_again = state.create_buffer("logs", vec!["sh".to_owned()], None);
    state
        .join_buffer_at_node(root, detached_again, NodeJoinPlacement::TabAfter)
        .expect("join after as tab");
    let root = session_root(&state, session_id);
    let detached_again_view = attached_view(&state, detached_again);
    let root_tabs = match state.node(root).expect("root") {
        Node::Tabs(tabs) => tabs,
        other => panic!("expected root tabs, got {other:?}"),
    };
    let children = root_tabs
        .tabs
        .iter()
        .map(|tab| tab.child)
        .collect::<Vec<_>>();
    assert_eq!(children, vec![target_root, detached_again_view]);
    state.validate().expect("state remains valid");
}

#[test]
fn join_buffer_at_node_rejects_buffers_already_contained_by_target() {
    let mut state = ServerState::new();
    let (session_id, buffer_id, leaf_id) = seed_single_leaf_session(&mut state, "alpha");

    let error = state
        .join_buffer_at_node(leaf_id, buffer_id, NodeJoinPlacement::Right)
        .expect_err("joining a buffer into its own view should fail");

    assert!(matches!(error, embers_core::MuxError::Conflict(_)));
    assert_eq!(attached_view(&state, buffer_id), leaf_id);
    assert_eq!(root_tab_child(&state, session_id, 0), leaf_id);
    state.validate().expect("state remains valid");
}

#[test]
fn join_buffer_at_node_rejects_attached_buffers_from_other_sessions() {
    let mut state = ServerState::new();
    let (target_session_id, _, target_leaf_id) = seed_single_leaf_session(&mut state, "alpha");
    let (source_session_id, source_buffer_id, source_leaf_id) =
        seed_single_leaf_session(&mut state, "beta");
    let target_before = reachable_nodes(&state, target_session_id);
    let source_before = reachable_nodes(&state, source_session_id);

    let error = state
        .join_buffer_at_node(target_leaf_id, source_buffer_id, NodeJoinPlacement::Right)
        .expect_err("cross-session rehoming should fail");

    assert!(matches!(error, embers_core::MuxError::Conflict(_)));
    assert_eq!(attached_view(&state, source_buffer_id), source_leaf_id);
    assert_eq!(reachable_nodes(&state, target_session_id), target_before);
    assert_eq!(reachable_nodes(&state, source_session_id), source_before);
    state.validate().expect("state remains valid");
}

#[test]
fn focused_floating_transfers_back_to_root_when_closed() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, floating_leaf) = new_leaf(&mut state, session_id, "popup");

    state
        .create_floating_window(
            session_id,
            floating_leaf,
            FloatGeometry::new(5, 5, 20, 10),
            Some("popup".to_owned()),
        )
        .expect("create floating window");
    state
        .focus_leaf(session_id, floating_leaf)
        .expect("focus popup");
    state.close_node(floating_leaf).expect("close popup root");

    let session = state.session(session_id).expect("session exists");
    assert_eq!(session.focused_leaf, Some(root_leaf));
    assert_eq!(session.focused_floating, None);
    state.validate().expect("state should validate");
}

#[test]
fn subtree_ownership_validation_detects_cross_session_child() {
    let mut state = ServerState::new();
    let (_session_a, _, leaf_a) = seed_single_leaf_session(&mut state, "alpha");
    let (_session_b, _, leaf_b) = seed_single_leaf_session(&mut state, "beta");
    let buffer_id = new_buffer(&mut state, "gamma");
    let split_id = state
        .split_leaf_with_new_buffer(leaf_a, SplitDirection::Horizontal, buffer_id)
        .expect("split root leaf");

    if let Node::Split(split) = state.nodes.get_mut(&split_id).expect("split exists") {
        split.children[1] = leaf_b;
    }
    if let Node::BufferView(leaf) = state.nodes.get_mut(&leaf_b).expect("leaf exists") {
        leaf.parent = Some(split_id);
    }

    let error = state.validate().expect_err("validation should fail");
    assert!(error.to_string().contains("must belong to session"));
    assert!(error.to_string().contains(&leaf_b.to_string()));
}

#[test]
fn floating_ownership_validation_detects_parented_root() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let (_, floating_leaf) = new_leaf(&mut state, session_id, "popup");
    let floating_id = state
        .create_floating_window(
            session_id,
            floating_leaf,
            FloatGeometry::new(2, 2, 15, 8),
            Some("popup".to_owned()),
        )
        .expect("create floating window");
    let root_tabs = state.root_tabs(session_id).expect("root tabs");

    if let Node::BufferView(leaf) = state.nodes.get_mut(&floating_leaf).expect("leaf exists") {
        leaf.parent = Some(root_tabs);
    }

    let error = state.validate().expect_err("validation should fail");
    assert!(error.to_string().contains(&floating_id.to_string()));
}

#[test]
fn create_buffer_view_rolls_back_when_attach_fails() {
    let mut state = ServerState::new();
    let (session_id, buffer_id, _) = seed_single_leaf_session(&mut state, "alpha");
    let node_count = state.nodes.len();

    let error = state
        .create_buffer_view(session_id, buffer_id)
        .expect_err("attached buffers cannot create a second view");

    assert!(matches!(error, embers_core::MuxError::Conflict(_)));
    assert_eq!(state.nodes.len(), node_count);
    assert_eq!(
        attached_view(&state, buffer_id),
        root_tab_child(&state, session_id, 0)
    );
    state.validate().expect("state remains valid");
}

#[test]
fn focus_leaf_rejects_detached_leaves_without_mutating_focus() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, detached_leaf) = new_leaf(&mut state, session_id, "detached");

    let error = state
        .focus_leaf(session_id, detached_leaf)
        .expect_err("detached leaf should not be focusable");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
}

#[test]
fn focus_leaf_rejects_hidden_floating_leaves_without_mutating_focus() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, floating_leaf) = new_leaf(&mut state, session_id, "popup");
    let floating_id = state
        .create_floating_window_with_options(
            session_id,
            floating_leaf,
            FloatGeometry::new(4, 4, 20, 10),
            Some("popup".to_owned()),
            false,
            true,
        )
        .expect("create floating window");
    state
        .floating
        .get_mut(&floating_id)
        .expect("floating exists")
        .visible = false;

    let error = state
        .focus_leaf(session_id, floating_leaf)
        .expect_err("hidden floating leaf should not be focusable");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_floating,
        None
    );
    state.validate().expect("state remains valid");
}

#[test]
fn create_floating_window_rolls_back_when_focus_fails() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let empty_tabs = state
        .create_tabs_node(session_id, Vec::new(), 0)
        .expect("create empty tabs root");

    let error = state
        .create_floating_window_with_options(
            session_id,
            empty_tabs,
            FloatGeometry::new(4, 4, 20, 10),
            Some("popup".to_owned()),
            true,
            true,
        )
        .expect_err("empty floating root should not be focusable");

    assert!(matches!(error, embers_core::MuxError::NotFound(_)));
    assert_eq!(
        state.session(session_id).expect("session").floating,
        Vec::new()
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_floating,
        None
    );
    assert!(
        matches!(state.node(empty_tabs), Ok(Node::Tabs(_))),
        "empty tabs node should remain a tabs container"
    );
    assert_eq!(
        state
            .node_parent(empty_tabs)
            .expect("empty tabs parent lookup"),
        None
    );
    assert_eq!(
        state
            .floating_id_for_node(empty_tabs)
            .expect("floating lookup"),
        None
    );
    state.validate().expect("state should be valid");
}

#[test]
fn break_node_rolls_back_when_breaking_to_floating_cannot_focus() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let empty_tabs = state
        .create_tabs_node(session_id, Vec::new(), 0)
        .expect("create empty tabs root");
    state
        .add_root_tab(session_id, "scratch", empty_tabs)
        .expect("attach empty tabs to root tabs");
    let focused_leaf_before = state.session(session_id).expect("session").focused_leaf;

    let error = state
        .break_node(empty_tabs, true)
        .expect_err("empty tabs cannot become a focused floating window");

    assert!(matches!(error, embers_core::MuxError::NotFound(_)));
    assert_eq!(
        state.session(session_id).expect("session").floating,
        Vec::new()
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        focused_leaf_before
    );
    assert_eq!(root_tab_child(&state, session_id, 1), empty_tabs);
    assert_eq!(
        state
            .node_parent(empty_tabs)
            .expect("empty tabs parent lookup"),
        state.root_tabs(session_id).ok()
    );
    assert_eq!(
        state
            .floating_id_for_node(empty_tabs)
            .expect("floating lookup"),
        None
    );
    state.validate().expect("state should be valid");
}

#[test]
fn add_tab_from_buffer_rolls_back_when_hidden_floating_cannot_focus() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (popup_buffer, popup_leaf) = new_leaf(&mut state, session_id, "popup");
    let floating_id = state
        .create_floating_window_with_options(
            session_id,
            popup_leaf,
            FloatGeometry::new(4, 4, 20, 10),
            Some("popup".to_owned()),
            false,
            true,
        )
        .expect("create floating window");
    let tabs_id = state
        .wrap_node_in_tabs(popup_leaf, "popup")
        .expect("wrap popup leaf in tabs");
    state
        .floating
        .get_mut(&floating_id)
        .expect("floating exists")
        .visible = false;

    let added_buffer = new_buffer(&mut state, "extra");
    let node_count = state.nodes.len();

    let error = state
        .add_tab_from_buffer(tabs_id, "extra", added_buffer)
        .expect_err("hidden floating tabs should reject focus");

    assert!(matches!(error, embers_core::MuxError::InvalidInput(_)));
    assert_eq!(state.nodes.len(), node_count);
    assert_eq!(
        state
            .buffer(added_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    );
    assert_eq!(
        state
            .buffer(popup_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Attached(popup_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    state.validate().expect("state remains valid");
}

#[test]
fn break_node_into_hidden_tabs_branch_preserves_focus() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (_, split_left) = new_leaf(&mut state, session_id, "split-left");
    let (_, split_right) = new_leaf(&mut state, session_id, "split-right");
    let split_root = state
        .create_split_node(
            session_id,
            SplitDirection::Vertical,
            vec![split_left, split_right],
        )
        .expect("create hidden split");
    let (_, other_leaf) = new_leaf(&mut state, session_id, "other");
    let hidden_tabs = state
        .create_tabs_node(
            session_id,
            vec![
                embers_server::TabEntry::new("split", split_root),
                embers_server::TabEntry::new("other", other_leaf),
            ],
            0,
        )
        .expect("create hidden tabs");
    let floating_id = state
        .create_floating_window_with_options(
            session_id,
            hidden_tabs,
            FloatGeometry::new(4, 4, 20, 10),
            Some("popup".to_owned()),
            false,
            true,
        )
        .expect("create floating window");
    state
        .floating
        .get_mut(&floating_id)
        .expect("floating exists")
        .visible = false;
    state
        .focus_leaf(session_id, root_leaf)
        .expect("refocus root");

    state
        .break_node(split_right, false)
        .expect("breaking a hidden node into a hidden tabs branch should succeed");

    let hidden_children = match state.node(hidden_tabs).expect("hidden tabs") {
        Node::Tabs(tabs) => tabs.tabs.iter().map(|tab| tab.child).collect::<Vec<_>>(),
        other => panic!("expected hidden tabs, got {other:?}"),
    };
    assert!(hidden_children.contains(&split_right));
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    assert_eq!(
        state
            .floating_id_for_node(split_right)
            .expect("floating lookup"),
        Some(floating_id)
    );
    state.validate().expect("state remains valid");
}

#[test]
fn join_buffer_at_node_into_hidden_floating_preserves_focus() {
    let mut state = ServerState::new();
    let (session_id, _, root_leaf) = seed_single_leaf_session(&mut state, "alpha");
    let (popup_buffer, popup_leaf) = new_leaf(&mut state, session_id, "popup");
    let floating_id = state
        .create_floating_window_with_options(
            session_id,
            popup_leaf,
            FloatGeometry::new(4, 4, 20, 10),
            Some("popup".to_owned()),
            false,
            true,
        )
        .expect("create floating window");
    state
        .floating
        .get_mut(&floating_id)
        .expect("floating exists")
        .visible = false;

    let detached_buffer = new_buffer(&mut state, "detached");

    state
        .join_buffer_at_node(popup_leaf, detached_buffer, NodeJoinPlacement::Right)
        .expect("hidden floating leaf should allow joining without stealing focus");

    let detached_view = attached_view(&state, detached_buffer);
    let floating_root = state
        .floating_window(floating_id)
        .expect("floating")
        .root_node;
    match state.node(floating_root).expect("floating root") {
        Node::Split(split) => assert_eq!(split.children, vec![popup_leaf, detached_view]),
        other => panic!("expected floating split root, got {other:?}"),
    }
    assert_eq!(
        state.buffer(popup_buffer).expect("popup buffer").attachment,
        BufferAttachment::Attached(popup_leaf)
    );
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(root_leaf)
    );
    assert_eq!(
        state
            .floating_id_for_node(detached_view)
            .expect("floating lookup"),
        Some(floating_id)
    );
    state.validate().expect("state remains valid");
}

#[test]
fn root_tabs_are_preserved_when_last_tab_closes() {
    let mut state = ServerState::new();
    let (session_id, _, _) = seed_single_leaf_session(&mut state, "alpha");
    let root_tabs = state.root_tabs(session_id).expect("root tabs");

    state.close_tab(root_tabs, 0).expect("close only root tab");

    match state.node(root_tabs).expect("root tabs still exist") {
        Node::Tabs(tabs) => assert!(tabs.tabs.is_empty()),
        other => panic!("expected root tabs, got {other:?}"),
    }
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        None
    );
    state.validate().expect("state should validate");
}

proptest! {
    #[test]
    fn random_mutation_sequences_preserve_invariants(
        ops in prop::collection::vec((0u8..5, any::<u8>()), 1..48)
    ) {
        let mut state = ServerState::new();
        let (session_id, _, _) = seed_single_leaf_session(&mut state, "prop");

        for (selector, arg) in ops {
            apply_random_op(&mut state, session_id, selector, arg);
            let validation = state.validate();
            prop_assert!(validation.is_ok(), "validation failed: {:?}", validation.err());
        }
    }

    #[test]
    fn no_orphaned_nodes_after_random_mutations(
        ops in prop::collection::vec((0u8..5, any::<u8>()), 1..48)
    ) {
        let mut state = ServerState::new();
        let (session_id, _, _) = seed_single_leaf_session(&mut state, "prop");

        for (selector, arg) in ops {
            apply_random_op(&mut state, session_id, selector, arg);
            let reachable = reachable_nodes(&state, session_id);
            prop_assert_eq!(reachable.len(), state.nodes.len());
        }
    }

    #[test]
    fn nested_singleton_tabs_normalize_to_a_valid_tree(depth in 1usize..6) {
        let mut state = ServerState::new();
        let (session_id, _, leaf_id) = seed_single_leaf_session(&mut state, "prop");
        let mut current = leaf_id;

        for index in 0..depth {
            current = state
                .wrap_node_in_tabs(current, format!("nested-{index}"))
                .expect("wrap node");
        }

        state.normalize_upwards(current).expect("normalize wrappers");
        prop_assert!(state.validate().is_ok());
        prop_assert_eq!(session_root(&state, session_id), leaf_id);
    }
}
