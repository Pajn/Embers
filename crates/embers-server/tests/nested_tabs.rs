use embers_core::{SessionId, SplitDirection};
use embers_server::{BufferAttachment, Node, ServerState, SplitNode, TabsNode};

fn root_tabs(state: &ServerState, session_id: SessionId) -> TabsNode {
    let root_id = state.root_tabs(session_id).expect("session has root tabs");
    match state.node(root_id).expect("root node exists") {
        Node::Tabs(tabs) => tabs.clone(),
        other => panic!("expected root tabs node, got {other:?}"),
    }
}

fn tabs_node(state: &ServerState, node_id: embers_core::NodeId) -> TabsNode {
    match state.node(node_id).expect("tabs node exists") {
        Node::Tabs(tabs) => tabs.clone(),
        other => panic!("expected tabs node, got {other:?}"),
    }
}

fn split_node(state: &ServerState, node_id: embers_core::NodeId) -> SplitNode {
    match state.node(node_id).expect("split node exists") {
        Node::Split(split) => split.clone(),
        other => panic!("expected split node, got {other:?}"),
    }
}

#[test]
fn wrap_node_in_tabs_reparents_subtree_and_preserves_focus() {
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
        .expect("split leaf");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses second leaf");

    let tabs_id = state
        .wrap_node_in_tabs(second_leaf, "nested")
        .expect("wrap leaf in tabs");

    let root_child = root_tabs(&state, session_id).tabs[0].child;
    assert_eq!(root_child, outer_split);

    let outer = split_node(&state, outer_split);
    assert_eq!(outer.children, vec![first_leaf, tabs_id]);

    let wrapped = tabs_node(&state, tabs_id);
    assert_eq!(wrapped.active, 0);
    assert_eq!(wrapped.tabs.len(), 1);
    assert_eq!(wrapped.tabs[0].title, "nested");
    assert_eq!(wrapped.tabs[0].child, second_leaf);
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(second_leaf)
    );
    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf, second_leaf]
    );

    state.validate().expect("wrapped tabs remain valid");
}

#[test]
fn nested_tabs_restore_focus_and_collapse_when_last_sibling_closes() {
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
        .split_leaf_with_new_buffer(first_leaf, SplitDirection::Horizontal, second_buffer)
        .expect("split leaf");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses second leaf");

    let tabs_id = state
        .wrap_node_in_tabs(second_leaf, "base")
        .expect("wrap leaf in tabs");

    let fourth_buffer = state.create_buffer("four", vec!["/bin/sh".to_owned()], None);
    let inner_split = state
        .split_leaf_with_new_buffer(second_leaf, SplitDirection::Vertical, fourth_buffer)
        .expect("split first nested tab leaf");
    let fourth_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses fourth leaf");

    let third_buffer = state.create_buffer("three", vec!["/bin/sh".to_owned()], None);
    let nested_index = state
        .add_tab_from_buffer(tabs_id, "other", third_buffer)
        .expect("add second nested tab");
    assert_eq!(nested_index, 1);
    let third_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("new nested tab focuses new leaf");

    state
        .switch_tab(tabs_id, 0)
        .expect("switch back to first nested tab");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(fourth_leaf)
    );
    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf, second_leaf, fourth_leaf]
    );

    state
        .switch_tab(tabs_id, 1)
        .expect("switch to second nested tab");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(third_leaf)
    );
    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf, third_leaf]
    );

    state
        .close_node(third_leaf)
        .expect("close second nested tab");

    let outer = split_node(&state, outer_split);
    assert_eq!(outer.children[1], inner_split);
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(fourth_leaf)
    );
    assert!(matches!(
        &state
            .buffer(third_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state
            .visible_session_leaves(session_id)
            .expect("visible leaves"),
        vec![first_leaf, second_leaf, fourth_leaf]
    );

    state
        .validate()
        .expect("nested tab close normalizes correctly");
}
