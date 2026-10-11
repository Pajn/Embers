use embers_core::{ActivityState, MuxError, PtySize};
use embers_server::{BufferAttachment, BufferState, ServerState};

#[test]
fn running_buffers_transition_to_exited_and_track_metadata() {
    let mut state = ServerState::new();
    let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);

    state
        .mark_buffer_running(buffer_id, Some(42))
        .expect("mark running");
    let sequence = state.note_buffer_output(buffer_id).expect("note output");
    assert_eq!(sequence, 1);
    state
        .set_buffer_size(buffer_id, PtySize::new(120, 40))
        .expect("resize buffer");
    state
        .mark_buffer_exited(buffer_id, Some(0))
        .expect("mark exited");

    let buffer = state.buffer(buffer_id).expect("buffer exists");
    assert_eq!(buffer.pty_size, PtySize::new(120, 40));
    assert_eq!(buffer.activity, ActivityState::Activity);
    assert_eq!(buffer.last_snapshot_seq, 1);
    assert!(matches!(
        buffer.state,
        BufferState::Exited(ref exited) if exited.exit_code == Some(0)
    ));
}

#[test]
fn single_attachment_requires_detach_before_reattach() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let first_buffer = state.create_buffer("first", vec!["first".to_owned()], None);
    let second_buffer = state.create_buffer("second", vec!["second".to_owned()], None);
    let first_view = state
        .create_buffer_view(session_id, first_buffer)
        .expect("create first view");
    let second_view = state
        .create_buffer_view(session_id, second_buffer)
        .expect("create second view");
    state
        .add_root_tab(session_id, "first", first_view)
        .expect("attach first view to root");
    state
        .add_root_tab(session_id, "second", second_view)
        .expect("attach second view to root");

    let error = state
        .attach_buffer(first_buffer, second_view)
        .expect_err("reattach without detach should fail");
    assert!(matches!(error, MuxError::Conflict(_)));

    state.close_node(first_view).expect("close original view");
    state
        .attach_buffer(first_buffer, second_view)
        .expect("reattach detached buffer");

    assert!(matches!(
        state.buffer(first_buffer).expect("first buffer").attachment,
        BufferAttachment::Attached(node_id) if node_id == second_view
    ));
    assert!(matches!(
        state
            .buffer(second_buffer)
            .expect("second buffer")
            .attachment,
        BufferAttachment::Detached
    ));
    assert!(matches!(state.node(first_view), Err(MuxError::NotFound(_))));
    assert_eq!(
        state
            .node(second_view)
            .expect("second view still exists")
            .as_buffer_view()
            .expect("second node is a buffer view")
            .buffer_id,
        first_buffer
    );
    state.validate().expect("state stays valid after reattach");
}

#[test]
fn closing_a_view_detaches_but_preserves_running_buffer() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
    state
        .mark_buffer_running(buffer_id, Some(7))
        .expect("mark running");
    let view_id = state
        .create_buffer_view(session_id, buffer_id)
        .expect("create buffer view");
    state
        .add_root_tab(session_id, "shell", view_id)
        .expect("attach view to root tabs");

    state.close_node(view_id).expect("close buffer view");

    let buffer = state.buffer(buffer_id).expect("buffer still exists");
    assert!(matches!(buffer.attachment, BufferAttachment::Detached));
    assert!(matches!(
        buffer.state,
        BufferState::Running(ref running) if running.pid == Some(7)
    ));
}

#[test]
fn focusing_a_leaf_clears_recorded_activity() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let first_buffer = state.create_buffer("first", vec!["/bin/sh".to_owned()], None);
    let first_view = state
        .create_buffer_view(session_id, first_buffer)
        .expect("create first view");
    state
        .add_root_tab(session_id, "first", first_view)
        .expect("attach first view");

    let second_buffer = state.create_buffer("second", vec!["/bin/sh".to_owned()], None);
    let second_view = state
        .create_buffer_view(session_id, second_buffer)
        .expect("create second view");
    state
        .add_root_tab(session_id, "second", second_view)
        .expect("attach second view");
    state
        .focus_leaf(session_id, second_view)
        .expect("focus second leaf");

    state
        .set_buffer_activity(first_buffer, ActivityState::Bell)
        .expect("mark first buffer active");

    state
        .focus_leaf(session_id, first_view)
        .expect("focus hidden first leaf");

    assert_eq!(
        state
            .buffer(first_buffer)
            .expect("first buffer exists")
            .activity,
        ActivityState::Idle
    );
}

#[test]
fn resize_updates_buffer_size_for_attached_and_detached_buffers() {
    let mut state = ServerState::new();
    let session_id = state.create_session("main");
    let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
    let view_id = state
        .create_buffer_view(session_id, buffer_id)
        .expect("create buffer view");
    state
        .add_root_tab(session_id, "shell", view_id)
        .expect("attach view to root tabs");

    state
        .set_buffer_size(buffer_id, PtySize::new(100, 30))
        .expect("resize attached buffer");
    assert_eq!(
        state.buffer(buffer_id).expect("buffer exists").pty_size,
        PtySize::new(100, 30)
    );

    state.close_node(view_id).expect("close buffer view");
    state
        .set_buffer_size(buffer_id, PtySize::new(90, 20))
        .expect("resize detached buffer");
    assert_eq!(
        state.buffer(buffer_id).expect("buffer exists").pty_size,
        PtySize::new(90, 20)
    );
}

#[test]
fn exited_detached_buffers_can_be_removed_cleanly() {
    let mut state = ServerState::new();
    let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);

    state
        .mark_buffer_running(buffer_id, Some(88))
        .expect("mark running");
    state
        .mark_buffer_exited(buffer_id, Some(0))
        .expect("mark exited");

    let removed = state
        .remove_buffer(buffer_id)
        .expect("remove detached exited buffer");
    assert!(matches!(
        removed.state,
        BufferState::Exited(ref exited) if exited.exit_code == Some(0)
    ));
    assert!(matches!(
        state.buffer(buffer_id),
        Err(MuxError::NotFound(_))
    ));
}

mod exited_buffer_disposal {
    use embers_core::{BufferId, NodeId, SessionId, SplitDirection};
    use embers_server::{BufferAttachment, REMAIN_ON_EXIT_OPTION, ServerState};

    /// A session showing two running shells side by side; returns the session,
    /// the buffer shown in the split-off pane and that pane's view.
    fn session_with_split() -> (ServerState, SessionId, BufferId, NodeId) {
        let mut state = ServerState::new();
        let session_id = state.create_session("main");
        let first = state.create_buffer("first", vec!["/bin/sh".to_owned()], None);
        let second = state.create_buffer("second", vec!["/bin/sh".to_owned()], None);
        for buffer_id in [first, second] {
            state
                .mark_buffer_running(buffer_id, None)
                .expect("mark running");
        }
        let first_view = state
            .create_buffer_view(session_id, first)
            .expect("create first view");
        state
            .add_root_tab_from_subtree(session_id, "main", first_view)
            .expect("add root tab");
        state
            .split_leaf_with_new_buffer(first_view, SplitDirection::Vertical, second)
            .expect("split");
        let BufferAttachment::Attached(second_view) =
            state.buffer(second).expect("second buffer").attachment
        else {
            panic!("second buffer is not attached");
        };
        (state, session_id, second, second_view)
    }

    #[test]
    fn exited_buffer_closes_its_view_and_is_removed() {
        let (mut state, session_id, buffer_id, view_id) = session_with_split();
        state
            .mark_buffer_exited(buffer_id, Some(0))
            .expect("mark exited");

        let disposed = state.dispose_exited_buffers();

        assert_eq!(disposed.removed_buffers, vec![buffer_id]);
        assert!(disposed.changed_sessions.contains(&session_id));
        assert!(state.buffer(buffer_id).is_err());
        assert!(state.node(view_id).is_err());
        assert_eq!(state.buffers.len(), 1, "the other pane's buffer stays");
        state.validate().expect("state stays valid");
    }

    #[test]
    fn running_buffers_are_left_alone() {
        let (mut state, _, buffer_id, view_id) = session_with_split();

        assert!(state.dispose_exited_buffers().is_empty());
        assert!(state.buffer(buffer_id).is_ok());
        assert!(state.node(view_id).is_ok());
    }

    #[test]
    fn remain_on_exit_keeps_the_view_until_it_is_closed() {
        let (mut state, _, buffer_id, view_id) = session_with_split();
        state
            .buffers
            .get_mut(&buffer_id)
            .expect("buffer")
            .user_options
            .insert(REMAIN_ON_EXIT_OPTION.to_owned(), "on".to_owned());
        state
            .mark_buffer_exited(buffer_id, Some(0))
            .expect("mark exited");

        assert!(state.dispose_exited_buffers().is_empty());
        assert_eq!(
            state.buffer(buffer_id).expect("buffer kept").attachment,
            BufferAttachment::Attached(view_id)
        );

        state.close_node(view_id).expect("close dead pane");
        assert_eq!(
            state.dispose_exited_buffers().removed_buffers,
            vec![buffer_id]
        );
        assert!(state.buffer(buffer_id).is_err());
    }

    #[test]
    fn remain_on_exit_failed_only_keeps_failed_commands() {
        for (exit_code, kept) in [(Some(0), false), (Some(1), true), (None, true)] {
            let (mut state, _, buffer_id, _) = session_with_split();
            state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer")
                .user_options
                .insert(REMAIN_ON_EXIT_OPTION.to_owned(), "failed".to_owned());
            state
                .mark_buffer_exited(buffer_id, exit_code)
                .expect("mark exited");
            state.dispose_exited_buffers();
            assert_eq!(
                state.buffer(buffer_id).is_ok(),
                kept,
                "exit code {exit_code:?}"
            );
        }
    }

    #[test]
    fn detached_buffer_that_exits_is_removed() {
        let (mut state, session_id, buffer_id, _) = session_with_split();
        state.detach_buffer(buffer_id).expect("detach");
        state
            .mark_buffer_exited(buffer_id, Some(0))
            .expect("mark exited");

        let disposed = state.dispose_exited_buffers();
        assert_eq!(disposed.removed_buffers, vec![buffer_id]);
        assert!(!disposed.changed_sessions.contains(&session_id));
        assert!(state.buffer(buffer_id).is_err());
    }

    #[test]
    fn buffer_that_exits_before_it_is_first_shown_waits_for_its_view() {
        let mut state = ServerState::new();
        let session_id = state.create_session("main");
        let buffer_id = state.create_buffer("quick", vec!["true".to_owned()], None);
        state
            .mark_buffer_exited(buffer_id, Some(0))
            .expect("mark exited");

        // Created detached and not shown yet: keep it for the request that
        // attaches it, and for captures meanwhile.
        assert!(state.dispose_exited_buffers().is_empty());
        assert!(state.buffer(buffer_id).is_ok());

        // Once shown, it closes like any pane whose command exited.
        let view_id = state
            .create_buffer_view(session_id, buffer_id)
            .expect("create view");
        state
            .add_root_tab_from_subtree(session_id, "quick", view_id)
            .expect("add root tab");
        let disposed = state.dispose_exited_buffers();
        assert_eq!(disposed.removed_buffers, vec![buffer_id]);
        assert!(state.node(view_id).is_err());
        state.validate().expect("state stays valid");
    }
}
