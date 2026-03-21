use std::collections::BTreeSet;

use mux_core::{BufferId, FloatGeometry, NodeId, SessionId, SplitDirection};
use mux_server::{BufferAttachment, Node, ServerState};
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
    let root = state.root_tabs(session_id).expect("root tabs");
    let tab_count = match state.node(root).expect("root node") {
        Node::Tabs(tabs) => tabs.tabs.len(),
        _ => 0,
    };

    match selector % 5 {
        0 => {
            let (_, leaf_id) = new_leaf(state, session_id, &format!("tab-{arg}"));
            let _ = state.add_root_tab(session_id, format!("tab-{arg}"), leaf_id);
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
            if tab_count > 0 {
                let _ = state.switch_tab(root, usize::from(arg) % tab_count);
            }
        }
        4 => {
            if tab_count > 0 {
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
    assert_eq!(root_tab_child(&state, session_id, 0), leaf_id);
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
    assert_eq!(root_tab_child(&state, session_id, 0), leaf_id);
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
        prop_assert_eq!(root_tab_child(&state, session_id, 0), leaf_id);
    }
}
