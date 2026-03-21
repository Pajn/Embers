use mux_core::SessionId;
use mux_server::{BufferAttachment, Node, ServerState, TabsNode};

fn root_tabs(state: &ServerState, session_id: SessionId) -> TabsNode {
    let root_id = state.root_tabs(session_id).expect("session has root tabs");
    match state.node(root_id).expect("root node exists") {
        Node::Tabs(tabs) => tabs.clone(),
        other => panic!("expected root tabs node, got {other:?}"),
    }
}

#[test]
fn session_creation_starts_with_empty_root_tabs() {
    let mut state = ServerState::new();

    let session_id = state.create_session("main");
    let session = state.session(session_id).expect("session exists");
    let tabs = root_tabs(&state, session_id);

    assert_eq!(session.root_node, tabs.id);
    assert!(tabs.tabs.is_empty());
    assert_eq!(tabs.active, 0);
    assert_eq!(session.focused_leaf, None);

    state.validate().expect("new session remains valid");
}

#[test]
fn root_tabs_can_be_added_from_buffer_and_subtree_and_renamed() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
    let first_index = state
        .add_root_tab_from_buffer(session_id, "shell", first_buffer)
        .expect("add root tab from buffer");
    assert_eq!(first_index, 0);

    let second_buffer = state.create_buffer("logs", vec!["/bin/sh".to_owned()], None);
    let second_leaf = state
        .create_buffer_view(session_id, second_buffer)
        .expect("create detached subtree leaf");
    let second_index = state
        .add_root_tab_from_subtree(session_id, "logs", second_leaf)
        .expect("add root tab from subtree");
    assert_eq!(second_index, 1);

    state
        .rename_root_tab(session_id, 0, "primary")
        .expect("rename root tab");

    let tabs = root_tabs(&state, session_id);
    let first_view = match &state
        .buffer(first_buffer)
        .expect("buffer exists")
        .attachment
    {
        BufferAttachment::Attached(node_id) => *node_id,
        BufferAttachment::Detached => panic!("buffer should be attached"),
    };
    assert_eq!(tabs.active, 1);
    assert_eq!(tabs.tabs.len(), 2);
    assert_eq!(tabs.tabs[0].title, "primary");
    assert_eq!(tabs.tabs[0].child, first_view);
    assert_eq!(tabs.tabs[1].title, "logs");
    assert_eq!(tabs.tabs[1].child, second_leaf);

    state.validate().expect("root tab insertion stays valid");
}

#[test]
fn switching_root_tabs_restores_previous_focus() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add first tab");
    let first_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("first tab focuses first leaf");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "two", second_buffer)
        .expect("add second tab");
    let second_leaf = state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("second tab focuses second leaf");

    state
        .select_root_tab(session_id, 0)
        .expect("switch back to first tab");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(first_leaf)
    );

    state
        .select_root_tab(session_id, 1)
        .expect("switch back to second tab");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(second_leaf)
    );

    state.validate().expect("focus switching stays valid");
}

#[test]
fn closing_root_tabs_detaches_buffers_and_preserves_empty_root() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let first_buffer = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first_buffer)
        .expect("add first tab");

    let second_buffer = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "two", second_buffer)
        .expect("add second tab");

    state
        .close_root_tab(session_id, 1)
        .expect("close active second tab");
    assert!(matches!(
        &state
            .buffer(second_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(root_tabs(&state, session_id).tabs.len(), 1);

    state
        .close_root_tab(session_id, 0)
        .expect("close final root tab");
    let tabs = root_tabs(&state, session_id);
    assert!(tabs.tabs.is_empty());
    assert_eq!(tabs.active, 0);
    assert!(matches!(
        &state
            .buffer(first_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        None
    );

    state.validate().expect("empty root tabs remain valid");
}
