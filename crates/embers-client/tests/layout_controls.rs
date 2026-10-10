//! Pane resizing, divider dragging and synchronize-panes through the
//! configured client.

use std::fs;

use embers_client::{
    ClientState, ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, KeyEvent,
    MouseButton, MouseEvent, MouseEventKind, MouseModifiers, MuxClient, PresentationModel,
    ScriptedTransport,
};
use embers_core::{BufferId, ErrorCode, RequestId, Size, WireError};
use embers_protocol::{
    ClientMessage, ErrorResponse, InputRequest, NodeRequest, OkResponse, ServerResponse,
    SessionRequest, SessionSnapshot, SessionSnapshotResponse, SynchronizePanesMode,
};
use tempfile::tempdir;

use crate::support::{
    FLOATING_TOP_LEAF_ID, FOCUSED_BUFFER_ID, FOCUSED_LEAF_ID, ROOT_SPLIT_ID, ROOT_TABS_ID,
    SESSION_ID, demo_state, floating_focused_state,
};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

fn manager_from_source(source: &str) -> (ConfigManager, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    fs::write(tempdir.path().join("config.rhai"), source).unwrap();
    (
        ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
        )
        .unwrap(),
        tempdir,
    )
}

fn session_snapshot(state: &ClientState) -> SessionSnapshot {
    SessionSnapshot {
        session: state.sessions.get(&SESSION_ID).unwrap().clone(),
        nodes: state
            .nodes
            .values()
            .filter(|node| node.session_id == SESSION_ID)
            .cloned()
            .collect(),
        buffers: state.buffers.values().cloned().collect(),
        floating: state.floating.values().cloned().collect(),
    }
}

fn configured<T: embers_client::Transport>(
    transport: T,
    state: ClientState,
    source: &str,
) -> (ConfiguredClient<T>, tempfile::TempDir) {
    let (config, tempdir) = manager_from_source(source);
    let mut configured = ConfiguredClient::new(MuxClient::new(transport), config);
    *configured.client_mut().state_mut() = state;
    (configured, tempdir)
}

fn synchronized(mut state: ClientState) -> ClientState {
    let root = state.nodes.get_mut(&ROOT_TABS_ID).unwrap();
    let tabs = root.tabs.as_mut().unwrap();
    let active = tabs.active as usize;
    tabs.tabs[active].synchronize_panes = true;
    state
}

fn mouse(column: u16, row: u16, kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        row,
        column,
        modifiers: MouseModifiers::default(),
        kind,
    }
}

/// The `(buffer, bytes)` of every input request, in order.
fn sent_input(transport: &FakeTransport) -> Vec<(BufferId, Vec<u8>)> {
    transport
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            ClientMessage::Input(InputRequest::Send {
                buffer_id, bytes, ..
            }) => Some((buffer_id, bytes)),
            _ => None,
        })
        .collect()
}

/// Queue the responses for an input fan-out to `buffers`: one ack per send,
/// one visible snapshot per buffer, then the session resync.
fn push_input_responses(transport: &FakeTransport, state: &ClientState, buffers: &[BufferId]) {
    for _ in buffers {
        transport.push_response(ServerResponse::Ok(OkResponse {
            request_id: RequestId(0),
        }));
    }
    for buffer_id in buffers {
        let mut snapshot = state.snapshots.get(buffer_id).unwrap().clone();
        snapshot.request_id = RequestId(0);
        transport.push_response(ServerResponse::VisibleSnapshot(snapshot));
    }
    transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(0),
        snapshot: session_snapshot(state),
    }));
}

fn push_resize_exchange(transport: &ScriptedTransport, state: &ClientState, sizes: Vec<u16>) {
    transport.push_exchange(
        ClientMessage::Node(NodeRequest::Resize {
            request_id: RequestId(1),
            node_id: ROOT_SPLIT_ID,
            sizes,
        }),
        ServerResponse::Ok(OkResponse {
            request_id: RequestId(1),
        }),
    );
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(2),
            session_id: SESSION_ID,
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(2),
            snapshot: session_snapshot(state),
        }),
    );
}

#[tokio::test]
async fn resize_actions_move_the_nearest_split_divider_in_cells() {
    let transport = ScriptedTransport::default();
    let state = demo_state();
    // The focused pane is the last child of the root split, so -L 3 moves the
    // divider before it three cells left.
    push_resize_exchange(&transport, &state, vec![23, 56]);
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        state,
        r#"bind("normal", "<M-h>", action.resize_left(3));"#,
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('h'))
        .await
        .unwrap();

    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn resize_actions_without_a_split_on_that_axis_send_nothing() {
    let transport = ScriptedTransport::default();
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        demo_state(),
        r#"bind("normal", "<M-k>", action.resize_up(3));"#,
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('k'))
        .await
        .unwrap();

    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn toggle_zoom_current_node_toggles_the_focused_pane() {
    let transport = FakeTransport::default();
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        demo_state(),
        r#"bind("normal", "<M-z>", action.toggle_zoom_current_node());"#,
    );

    // The follow-up resync fails against the empty fake transport; only the
    // zoom request matters here.
    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('z'))
        .await
        .unwrap();

    assert_eq!(
        transport.requests().first(),
        Some(&ClientMessage::Node(NodeRequest::ToggleZoom {
            request_id: RequestId(1),
            node_id: FOCUSED_LEAF_ID,
        }))
    );
}

#[tokio::test]
async fn dragging_a_divider_resizes_its_split() {
    let transport = ScriptedTransport::default();
    let state = demo_state();
    push_resize_exchange(&transport, &state, vec![40, 39]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    // Press on the root divider (column 26), drag it to column 40, release.
    for event in [
        mouse(26, 3, MouseEventKind::Press(MouseButton::Left)),
        mouse(40, 3, MouseEventKind::Drag(MouseButton::Left)),
        mouse(40, 3, MouseEventKind::Release(None)),
    ] {
        configured
            .handle_mouse(SESSION_ID, VIEWPORT, event)
            .await
            .unwrap();
    }

    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn divider_drags_keep_resizing_when_the_pointer_crosses_a_pane() {
    let transport = ScriptedTransport::default();
    let mut state = demo_state();
    // Even a pane that asked for the mouse does not get the drag.
    state
        .snapshots
        .get_mut(&FOCUSED_BUFFER_ID)
        .unwrap()
        .mouse_reporting = true;
    push_resize_exchange(&transport, &state, vec![50, 29]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    for event in [
        mouse(26, 3, MouseEventKind::Press(MouseButton::Left)),
        mouse(50, 3, MouseEventKind::Drag(MouseButton::Left)),
    ] {
        configured
            .handle_mouse(SESSION_ID, VIEWPORT, event)
            .await
            .unwrap();
    }

    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn drags_that_start_inside_a_pane_are_not_claimed() {
    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&FOCUSED_BUFFER_ID)
        .unwrap()
        .mouse_reporting = true;
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID]);
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    // Press inside the focused pane's content, then drag across the divider.
    for event in [
        mouse(40, 5, MouseEventKind::Press(MouseButton::Left)),
        mouse(26, 5, MouseEventKind::Drag(MouseButton::Left)),
    ] {
        configured
            .handle_mouse(SESSION_ID, VIEWPORT, event)
            .await
            .unwrap();
    }

    let requests = transport.requests();
    assert!(
        !requests
            .iter()
            .any(|request| matches!(request, ClientMessage::Node(NodeRequest::Resize { .. })))
    );
    let sent = sent_input(&transport);
    assert_eq!(
        sent.len(),
        1,
        "only the in-pane press is forwarded: {sent:?}"
    );
    assert_eq!(sent[0].0, FOCUSED_BUFFER_ID);
}

#[tokio::test]
async fn toggle_synchronize_panes_targets_the_active_window() {
    let transport = ScriptedTransport::default();
    let state = demo_state();
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::SetRootTabSynchronizePanes {
            request_id: RequestId(1),
            session_id: SESSION_ID,
            index: 1,
            mode: SynchronizePanesMode::Toggle,
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(1),
            snapshot: session_snapshot(&synchronized(state.clone())),
        }),
    );
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(2),
            session_id: SESSION_ID,
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(2),
            snapshot: session_snapshot(&synchronized(state.clone())),
        }),
    );
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        state,
        r#"bind("normal", "<M-s>", action.toggle_synchronize_panes());"#,
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('s'))
        .await
        .unwrap();

    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
    let presentation =
        PresentationModel::project(configured.client().state(), SESSION_ID, VIEWPORT).unwrap();
    assert!(presentation.root_tabs.unwrap().tabs[1].synchronize_panes);
}

#[tokio::test]
async fn synchronized_windows_send_typed_input_to_every_visible_pane() {
    let state = synchronized(demo_state());
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID, BufferId(2)]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Char('x'))
        .await
        .unwrap();

    // The focused pane first, then the window's other visible pane. Panes in
    // hidden tabs, other windows and popups are left alone.
    assert_eq!(
        sent_input(&transport),
        vec![
            (FOCUSED_BUFFER_ID, b"x".to_vec()),
            (BufferId(2), b"x".to_vec()),
        ]
    );
}

#[tokio::test]
async fn synchronized_input_is_encoded_for_each_pane() {
    let mut state = synchronized(demo_state());
    state
        .snapshots
        .get_mut(&FOCUSED_BUFFER_ID)
        .unwrap()
        .bracketed_paste = true;
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID, BufferId(2)]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_paste(SESSION_ID, VIEWPORT, b"hi".to_vec())
        .await
        .unwrap();

    assert_eq!(
        sent_input(&transport),
        vec![
            (FOCUSED_BUFFER_ID, b"\x1b[200~hi\x1b[201~".to_vec()),
            (BufferId(2), b"hi".to_vec()),
        ]
    );
}

#[tokio::test]
async fn synchronized_windows_include_scripted_input_to_the_current_pane() {
    let state = synchronized(demo_state());
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID, BufferId(2)]);
    push_input_responses(&transport, &state, &[BufferId(2)]);
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        state,
        r#"
            bind("normal", "<M-a>", action.send_keys_current("a"));
            bind("normal", "<M-b>", action.send_keys(2, "b"));
        "#,
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('a'))
        .await
        .unwrap();
    // Input aimed at a specific buffer is not synchronized.
    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('b'))
        .await
        .unwrap();

    assert_eq!(
        sent_input(&transport),
        vec![
            (FOCUSED_BUFFER_ID, b"a".to_vec()),
            (BufferId(2), b"a".to_vec()),
            (BufferId(2), b"b".to_vec()),
        ]
    );
}

#[tokio::test]
async fn popups_are_not_synchronized() {
    let state = synchronized(floating_focused_state());
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[BufferId(5)]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");
    assert_eq!(
        configured
            .client()
            .state()
            .sessions
            .get(&SESSION_ID)
            .and_then(|session| session.focused_leaf_id),
        Some(FLOATING_TOP_LEAF_ID)
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Char('x'))
        .await
        .unwrap();

    assert_eq!(sent_input(&transport), vec![(BufferId(5), b"x".to_vec())]);
}

#[tokio::test]
async fn unsynchronized_windows_only_send_to_the_focused_pane() {
    let state = demo_state();
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Char('x'))
        .await
        .unwrap();

    assert_eq!(
        sent_input(&transport),
        vec![(FOCUSED_BUFFER_ID, b"x".to_vec())]
    );
}

/// The error the server answers input to a pane whose program has exited.
fn dead_pane_error() -> ServerResponse {
    ServerResponse::Error(ErrorResponse {
        request_id: Some(RequestId(0)),
        error: WireError::new(ErrorCode::Conflict, "buffer runtime has already exited"),
    })
}

#[tokio::test]
async fn input_to_a_dead_pane_is_dropped_and_synchronized_copies_still_go_out() {
    let state = synchronized(demo_state());
    let transport = FakeTransport::default();
    // The focused pane is dead; the other synchronized pane takes its copy.
    transport.push_response(dead_pane_error());
    push_input_responses(&transport, &state, &[BufferId(2)]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Char('a'))
        .await
        .expect("typing into a dead pane does not fail");

    assert_eq!(
        sent_input(&transport),
        vec![
            (FOCUSED_BUFFER_ID, b"a".to_vec()),
            (BufferId(2), b"a".to_vec()),
        ]
    );
}

#[tokio::test]
async fn keys_pastes_and_focus_reports_to_a_dead_pane_are_dropped() {
    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&FOCUSED_BUFFER_ID)
        .unwrap()
        .focus_reporting = true;
    let transport = FakeTransport::default();
    for _ in 0..3 {
        transport.push_response(dead_pane_error());
        transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(0),
            snapshot: session_snapshot(&state),
        }));
    }
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Char('a'))
        .await
        .expect("a key to a dead pane is dropped");
    configured
        .handle_paste(SESSION_ID, VIEWPORT, b"pasted".to_vec())
        .await
        .expect("a paste to a dead pane is dropped");
    configured
        .handle_focus_event(SESSION_ID, VIEWPORT, true)
        .await
        .expect("a focus report to a dead pane is dropped");

    assert_eq!(
        sent_input(&transport),
        vec![
            (FOCUSED_BUFFER_ID, b"a".to_vec()),
            (FOCUSED_BUFFER_ID, b"pasted".to_vec()),
            (FOCUSED_BUFFER_ID, b"\x1b[I".to_vec()),
        ]
    );
}

/// A kitty graphics reply, as the host sends it to a pane's query.
const KITTY_GRAPHICS_REPLY: &[u8] = b"\x1b_Gi=31;OK\x1b\\";

#[tokio::test]
async fn host_replies_are_not_synchronized() {
    let state = synchronized(demo_state());
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID]);
    let (mut configured, _tempdir) = configured(transport.clone(), state, "");

    configured
        .handle_host_reply(SESSION_ID, VIEWPORT, KITTY_GRAPHICS_REPLY.to_vec())
        .await
        .unwrap();

    assert_eq!(
        sent_input(&transport),
        vec![(FOCUSED_BUFFER_ID, KITTY_GRAPHICS_REPLY.to_vec())]
    );
}

#[tokio::test]
async fn host_replies_skip_the_command_prompt_and_bindings() {
    let state = demo_state();
    let transport = FakeTransport::default();
    push_input_responses(&transport, &state, &[FOCUSED_BUFFER_ID]);
    let (mut configured, _tempdir) = configured(
        transport.clone(),
        state,
        r#"bind("normal", "<M-:>", action.command_prompt());"#,
    );
    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt(':'))
        .await
        .unwrap();
    assert_eq!(configured.current_mode(), embers_client::PROMPT_MODE);

    configured
        .handle_host_reply(SESSION_ID, VIEWPORT, KITTY_GRAPHICS_REPLY.to_vec())
        .await
        .unwrap();

    // The prompt line is untouched and still open; the reply went to the pane.
    assert_eq!(configured.current_mode(), embers_client::PROMPT_MODE);
    assert_eq!(configured.command_prompt().unwrap().display(), ":");
    assert_eq!(
        sent_input(&transport),
        vec![(FOCUSED_BUFFER_ID, KITTY_GRAPHICS_REPLY.to_vec())]
    );
}
