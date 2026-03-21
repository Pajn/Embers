use embers_core::{SessionId, SplitDirection};
use embers_server::{BufferAttachment, Node, ServerState, SplitNode, TabsNode};

fn root_tabs(state: &ServerState, session_id: SessionId) -> TabsNode {
    let root_id = state.root_tabs(session_id).expect("session has root tabs");
    match state.node(root_id).expect("root node exists") {
        Node::Tabs(tabs) => tabs.clone(),
        other => panic!("expected root tabs node, got {other:?}"),
    }
}

fn split_node(state: &ServerState, node_id: embers_core::NodeId) -> SplitNode {
    match state.node(node_id).expect("split node exists") {
        Node::Split(split) => split.clone(),
        other => panic!("expected split node, got {other:?}"),
    }
}

#[test]
fn repeated_nested_splits_preserve_leaf_order_and_focus() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add initial tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("initial tab focuses first leaf");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    let outer_split = state
        .split_leaf_with_new_buffer(first_leaf, SplitDirection::Vertical, second_buffer)
        .expect("split first leaf");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses new leaf");

    let third_buffer = state.create_buffer("three", vec!["/bin/sh".to_owned()], None);
    let inner_split = state
        .split_leaf_with_new_buffer(second_leaf, SplitDirection::Horizontal, third_buffer)
        .expect("split nested leaf");
    let third_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("nested split focuses newest leaf");

    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.tabs[0].child, outer_split);

    let outer = split_node(&state, outer_split);
    assert_eq!(outer.direction, SplitDirection::Vertical);
    assert_eq!(outer.children, vec![first_leaf, inner_split]);
    assert_eq!(outer.sizes, vec![1, 1]);

    let inner = split_node(&state, inner_split);
    assert_eq!(inner.direction, SplitDirection::Horizontal);
    assert_eq!(inner.children, vec![second_leaf, third_leaf]);
    assert_eq!(inner.sizes, vec![1, 1]);

    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf, second_leaf, third_leaf]
    );

    state.validate().expect("nested splits remain valid");
}

#[test]
fn resize_updates_split_weights_and_rejects_invalid_sizes() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add initial tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("initial tab focuses first leaf");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    let split_id = state
        .split_leaf_with_new_buffer(first_leaf, SplitDirection::Vertical, second_buffer)
        .expect("split leaf");

    state
        .resize_split_children(split_id, vec![3, 2])
        .expect("resize split");
    assert_eq!(split_node(&state, split_id).sizes, vec![3, 2]);

    assert!(state.resize_split_children(split_id, vec![5]).is_err());
    assert!(state.resize_split_children(split_id, vec![5, 0]).is_err());

    state.validate().expect("resized split remains valid");
}

#[test]
fn closing_leaf_normalizes_split_and_detaches_buffer() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add initial tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("initial tab focuses first leaf");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .split_leaf_with_new_buffer(first_leaf, SplitDirection::Horizontal, second_buffer)
        .expect("split leaf");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses new leaf");

    state.close_node(second_leaf).expect("close focused leaf");

    assert_eq!(
        state.session(session_id).expect("session exists").root_node,
        first_leaf
    );
    assert_eq!(state.node_parent(first_leaf).expect("first leaf parent"), None);
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(first_leaf)
    );
    assert!(matches!(
        &state
            .buffer(second_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf]
    );

    state.validate().expect("close normalizes split");
}
