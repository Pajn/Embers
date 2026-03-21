use mux_core::{FloatGeometry, SessionId};
use mux_server::{BufferAttachment, Node, ServerState, TabEntry};

fn root_leaf(state: &ServerState, session_id: SessionId) -> mux_core::NodeId {
    state
        .session(session_id)
        .expect("session exists")
        .focused_leaf
        .expect("session has focused leaf")
}

#[test]
fn buffer_backed_floating_tracks_focus_geometry_and_detach_on_close() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let root_buffer = state.create_buffer("root", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "root", root_buffer)
        .expect("add root tab");
    let root_leaf = root_leaf(&state, session_id);

    let popup_buffer = state.create_buffer("popup", vec!["/bin/sh".to_owned()], None);
    let floating_id = state
        .create_floating_from_buffer(
            session_id,
            popup_buffer,
            FloatGeometry::new(4, 3, 40, 12),
            Some("popup".to_owned()),
        )
        .expect("create floating from buffer");

    let floating_root = state
        .floating_window(floating_id)
        .expect("floating exists")
        .root_node;
    assert!(matches!(
        &state.buffer(popup_buffer).expect("buffer exists").attachment,
        BufferAttachment::Attached(node_id) if *node_id == floating_root
    ));

    state
        .focus_floating(floating_id)
        .expect("focus floating window");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_floating,
        Some(floating_id)
    );

    state
        .focus_leaf(session_id, root_leaf)
        .expect("focus back to tiled root");
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_floating,
        None
    );
    assert!(
        !state
            .floating_window(floating_id)
            .expect("floating exists")
            .focused
    );

    let new_geometry = FloatGeometry::new(10, 6, 60, 18);
    state
        .move_floating(floating_id, new_geometry)
        .expect("move floating window");
    assert_eq!(
        state
            .floating_window(floating_id)
            .expect("floating exists")
            .geometry,
        new_geometry
    );

    state
        .close_floating(floating_id)
        .expect("close floating window");
    assert!(matches!(
        &state
            .buffer(popup_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(root_leaf)
    );
    assert!(state.floating_window(floating_id).is_err());

    state.validate().expect("floating lifecycle remains valid");
}

#[test]
fn floating_tabs_close_on_empty_and_restore_root_focus() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");

    let root_buffer = state.create_buffer("root", vec!["/bin/sh".to_owned()], None);
    state
        .add_root_tab_from_buffer(session_id, "root", root_buffer)
        .expect("add root tab");
    let root_leaf = root_leaf(&state, session_id);

    let popup_buffer = state.create_buffer("popup", vec!["/bin/sh".to_owned()], None);
    let popup_leaf = state
        .create_buffer_view(session_id, popup_buffer)
        .expect("create detached popup leaf");
    let popup_tabs = state
        .create_tabs_node(session_id, vec![TabEntry::new("popup", popup_leaf)], 0)
        .expect("create popup tabs root");
    let floating_id = state
        .create_floating_window(
            session_id,
            popup_tabs,
            FloatGeometry::new(2, 2, 30, 10),
            Some("popup".to_owned()),
        )
        .expect("create floating tabs");

    state
        .focus_floating(floating_id)
        .expect("focus floating tabs");
    state.close_node(popup_leaf).expect("close only popup tab");

    assert!(state.floating_window(floating_id).is_err());
    assert_eq!(
        state
            .session(session_id)
            .expect("session exists")
            .focused_leaf,
        Some(root_leaf)
    );
    assert!(matches!(
        &state
            .buffer(popup_buffer)
            .expect("buffer exists")
            .attachment,
        BufferAttachment::Detached
    ));

    match state
        .node(state.root_tabs(session_id).expect("root tabs"))
        .expect("root exists")
    {
        Node::Tabs(_) => {}
        other => panic!("expected root tabs, got {other:?}"),
    }

    state.validate().expect("close-on-empty keeps state valid");
}
