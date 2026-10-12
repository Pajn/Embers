use embers_core::{BufferId, NodeId, SessionId, SplitDirection};
use embers_protocol::NodeJoinPlacement;
use embers_server::{BufferAttachment, Node, ServerState, TabsNode};

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
fn closing_root_tabs_keeps_root_and_closing_last_window_resets_to_empty_tabs() {
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
    let root = state.session(session_id).expect("session exists").root_node;
    let first_attachment = match &state
        .buffer(first_buffer)
        .expect("buffer exists")
        .attachment
    {
        BufferAttachment::Attached(node_id) => *node_id,
        BufferAttachment::Detached => panic!("buffer should remain attached"),
    };
    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.id, root);
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.tabs[0].child, first_attachment);
    assert_eq!(
        state.node_parent(first_attachment).expect("window parent"),
        Some(root)
    );

    state
        .close_node(first_attachment)
        .expect("close last window's pane");
    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.id, root);
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

#[test]
fn root_tabs_track_synchronize_panes_per_window() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    for title in ["one", "two"] {
        let buffer = state.create_buffer(title, vec!["/bin/sh".to_owned()], None);
        state
            .add_root_tab_from_buffer(session_id, title, buffer)
            .expect("add root tab");
    }
    assert!(
        root_tabs(&state, session_id)
            .tabs
            .iter()
            .all(|tab| !tab.synchronize_panes)
    );

    assert!(
        state
            .set_root_tab_synchronize_panes(session_id, 1, None)
            .expect("toggle on")
    );
    let tabs = root_tabs(&state, session_id);
    assert!(!tabs.tabs[0].synchronize_panes);
    assert!(tabs.tabs[1].synchronize_panes);

    assert!(
        !state
            .set_root_tab_synchronize_panes(session_id, 1, None)
            .expect("toggle off")
    );
    assert!(
        state
            .set_root_tab_synchronize_panes(session_id, 0, Some(true))
            .expect("set on")
    );
    assert!(
        state
            .set_root_tab_synchronize_panes(session_id, 0, Some(true))
            .expect("setting on again keeps it on")
    );
    assert!(root_tabs(&state, session_id).tabs[0].synchronize_panes);

    state
        .set_root_tab_synchronize_panes(session_id, 2, Some(true))
        .expect_err("out of range window index is rejected");

    state
        .validate()
        .expect("synchronize-panes keeps state valid");
}

fn attached_view(state: &ServerState, buffer_id: BufferId) -> NodeId {
    match state.buffer(buffer_id).expect("buffer exists").attachment {
        BufferAttachment::Attached(node_id) => node_id,
        BufferAttachment::Detached => panic!("buffer {buffer_id} is detached"),
    }
}

/// A session with windows "one" and "two", each a single pane, with "two"
/// active. Returns the buffers shown in the two windows.
fn two_window_session(state: &mut ServerState) -> (SessionId, BufferId, BufferId) {
    let session_id = state.create_session("main");
    let first = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first)
        .expect("add first window");
    let second = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "two", second)
        .expect("add second window");
    (session_id, first, second)
}

/// The session's root is still tabs, listing `window` as its only window.
fn assert_single_root_window(state: &ServerState, session_id: SessionId, window: NodeId) {
    let tabs = root_tabs(state, session_id);
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.active, 0);
    assert_eq!(tabs.tabs[0].child, window);
    assert_eq!(
        state.node_parent(window).expect("window parent"),
        Some(tabs.id)
    );
    state.validate().expect("one-window session stays valid");
}

#[test]
fn closing_a_window_keeps_the_last_one_in_root_tabs() {
    let mut state = ServerState::new();
    let (session_id, first, second) = two_window_session(&mut state);
    state
        .rename_root_tab(session_id, 0, "renamed")
        .expect("rename first window");
    state
        .set_root_tab_synchronize_panes(session_id, 0, Some(true))
        .expect("synchronize first window");

    state
        .close_root_tab(session_id, 1)
        .expect("close second window");

    let first_view = attached_view(&state, first);
    assert_single_root_window(&state, session_id, first_view);
    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.tabs[0].title, "renamed");
    assert!(tabs.tabs[0].synchronize_panes);
    assert!(matches!(
        state.buffer(second).expect("buffer exists").attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state.session(session_id).expect("session").focused_leaf,
        Some(first_view)
    );
}

#[test]
fn killing_the_only_pane_of_a_window_keeps_root_tabs() {
    let mut state = ServerState::new();
    let (session_id, first, second) = two_window_session(&mut state);

    state
        .close_node(attached_view(&state, second))
        .expect("kill the second window's pane");

    assert_single_root_window(&state, session_id, attached_view(&state, first));
    assert_eq!(root_tabs(&state, session_id).tabs[0].title, "one");
}

#[test]
fn joining_a_window_into_another_keeps_root_tabs() {
    let mut state = ServerState::new();
    let (session_id, first, second) = two_window_session(&mut state);
    let first_view = attached_view(&state, first);

    state
        .join_buffer_at_node(first_view, second, NodeJoinPlacement::Right)
        .expect("join the second window's pane into the first");

    let window = state
        .node_parent(first_view)
        .expect("pane parent")
        .expect("joined pane sits in a split");
    assert!(matches!(
        state.node(window).expect("window"),
        Node::Split(_)
    ));
    assert_single_root_window(&state, session_id, window);
    assert_eq!(root_tabs(&state, session_id).tabs[0].title, "one");
}

#[test]
fn moving_a_buffer_out_of_its_window_keeps_root_tabs() {
    let mut state = ServerState::new();
    let (session_id, first, second) = two_window_session(&mut state);
    let first_view = attached_view(&state, first);
    let second_view = attached_view(&state, second);

    state
        .move_buffer_to_leaf(second, first_view)
        .expect("show the second buffer in the first window");

    assert!(!state.nodes.contains_key(&second_view));
    assert_single_root_window(&state, session_id, first_view);
    assert_eq!(root_tabs(&state, session_id).tabs[0].title, "one");
}

#[test]
fn breaking_a_window_into_a_popup_keeps_root_tabs() {
    let mut state = ServerState::new();
    let (session_id, first, second) = two_window_session(&mut state);

    state
        .break_node(attached_view(&state, second), true)
        .expect("break the second window into a popup");

    assert_single_root_window(&state, session_id, attached_view(&state, first));
    assert_eq!(
        state.session(session_id).expect("session").floating.len(),
        1
    );
}

#[test]
fn breaking_a_pane_out_and_closing_it_again_keeps_root_tabs() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let first = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first)
        .expect("add first window");
    let first_view = attached_view(&state, first);
    let second = state.create_buffer("two", vec!["/bin/sh".to_owned()], None);
    state
        .split_leaf_with_new_buffer(first_view, SplitDirection::Vertical, second)
        .expect("split the window");

    state
        .break_node(attached_view(&state, second), false)
        .expect("break the new pane into its own window");

    let tabs = root_tabs(&state, session_id);
    assert_eq!(tabs.tabs.len(), 2);
    assert_eq!(tabs.tabs[0].child, first_view);
    assert_eq!(tabs.tabs[0].title, "one");
    assert_eq!(tabs.tabs[1].child, attached_view(&state, second));
    state.validate().expect("broken-out window stays valid");

    state
        .close_root_tab(session_id, 1)
        .expect("close the new window");
    assert_single_root_window(&state, session_id, first_view);
}

#[test]
fn nested_tabs_still_collapse_inside_a_one_window_session() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let first = state.create_buffer("one", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "one", first)
        .expect("add first window");
    let first_view = attached_view(&state, first);
    let nested = state
        .wrap_node_in_tabs(first_view, "inner")
        .expect("wrap the pane in nested tabs");

    state
        .normalize_upwards(nested)
        .expect("normalize nested tabs");

    assert!(!state.nodes.contains_key(&nested));
    assert_single_root_window(&state, session_id, first_view);
}

#[test]
fn session_root_tabs_cannot_be_split_or_replaced() {
    let mut state = ServerState::new();
    let (session_id, first, _) = two_window_session(&mut state);
    let root = state.root_tabs(session_id).expect("root tabs");
    let other = state.create_buffer("other", vec!["/bin/sh".to_owned()], None);
    let other_view = state
        .create_buffer_view(session_id, other)
        .expect("create detached view");

    state
        .wrap_node_in_split(root, SplitDirection::Vertical, other_view, false)
        .expect_err("the root is not wrapped in a split");
    state
        .join_buffer_at_node(root, first, NodeJoinPlacement::Right)
        .expect_err("nothing is joined beside the root");
    state
        .replace_node(root, other_view)
        .expect_err("the root is not replaced");

    assert_eq!(state.root_tabs(session_id).expect("root stays tabs"), root);
    assert_eq!(root_tabs(&state, session_id).tabs.len(), 2);
    state
        .add_root_tab_from_subtree(session_id, "other", other_view)
        .expect("the view can still become a window");
    state
        .validate()
        .expect("refused changes leave the state valid");
}
