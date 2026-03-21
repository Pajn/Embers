use embers_core::SessionId;
use embers_server::{BufferAttachment, Node, ServerState, TabsNode};

fn root_tabs(state: &ServerState, session_id: SessionId) -> TabsNode {
    let root_id = state.root_tabs(session_id).expect("session has root tabs");
    match state.node(root_id).expect("root node exists") {
        Node::Tabs(tabs) => tabs.clone(),
        other => panic!("expected root tabs node, got {other:?}"),
    }
}

#[test]
fn moving_buffer_between_leaves_replaces_target_and_closes_source_view() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add root tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("root tab focuses first leaf");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .split_leaf_with_new_buffer(
            first_leaf,
            embers_core::SplitDirection::Horizontal,
            second_buffer,
        )
        .expect("split root leaf");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("split focuses new leaf");

    state
        .move_buffer_to_leaf(first_buffer, second_leaf)
        .expect("move buffer into target leaf");

    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.tabs[0].child, second_leaf);
    match state.node(second_leaf).expect("target leaf exists") {
        Node::BufferView(view) => assert_eq!(view.buffer_id, first_buffer),
        other => panic!("expected target buffer view, got {other:?}"),
    }
    assert!(matches!(
        &state.buffer(first_buffer).expect("buffer exists").attachment,
        BufferAttachment::Attached(node_id) if *node_id == second_leaf
    ));
    assert!(matches!(
        &state
            .buffer(second_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(second_leaf)
    );

    state.validate().expect("move keeps state valid");
}

#[test]
fn detached_buffer_can_reattach_to_existing_leaf() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add root tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("root tab focuses first leaf");
    state.close_node(first_leaf).expect("close source view");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "two", second_buffer)
        .expect("add replacement root tab");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("replacement root tab focuses second leaf");

    state
        .move_buffer_to_leaf(first_buffer, second_leaf)
        .expect("reattach detached buffer");

    match state.node(second_leaf).expect("target leaf exists") {
        Node::BufferView(view) => assert_eq!(view.buffer_id, first_buffer),
        other => panic!("expected target buffer view, got {other:?}"),
    }
    assert!(matches!(
        &state.buffer(first_buffer).expect("buffer exists").attachment,
        BufferAttachment::Attached(node_id) if *node_id == second_leaf
    ));
    assert!(matches!(
        &state
            .buffer(second_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));

    state.validate().expect("reattach keeps state valid");
}

#[test]
fn attached_buffers_must_detach_before_cross_session_move() {
    let mut state = ServerState::new();
    let source_session = state.create_session("source");
    let target_session = state.create_session("target");

    let source_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(source_session, "one", source_buffer)
        .expect("add source tab");
    let source_leaf = state
        .session(source_session)
        .expect("source session exists")
        .focused_leaf
        .expect("source tab focuses source leaf");

    let target_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(target_session, "two", target_buffer)
        .expect("add target tab");
    let target_leaf = state
        .session(target_session)
        .expect("target session exists")
        .focused_leaf
        .expect("target tab focuses target leaf");

    let error = state
        .move_buffer_to_leaf(source_buffer, target_leaf)
        .expect_err("attached cross-session move should be rejected");
    assert!(
        error
            .to_string()
            .contains("detached before moving across sessions")
    );

    state.close_node(source_leaf).expect("detach source view");
    state
        .move_buffer_to_leaf(source_buffer, target_leaf)
        .expect("detached buffer can move across sessions");
    match state.node(target_leaf).expect("target leaf exists") {
        Node::BufferView(view) => assert_eq!(view.buffer_id, source_buffer),
        other => panic!("expected target buffer view, got {other:?}"),
    }

    state
        .validate()
        .expect("cross-session reattach keeps state valid");
}
