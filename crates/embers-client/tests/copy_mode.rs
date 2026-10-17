//! Copy-mode behaviour driven through `ConfiguredClient`: vi motions, counts,
//! selection, search direction, config bindings, and mouse selection.

use std::fs;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, KeyEvent, MouseButton,
    MouseEvent, MouseEventKind, MouseModifiers, MuxClient, PresentationModel, SelectionKind,
    SelectionPoint,
};
use embers_core::{
    BufferId, CursorPosition, CursorShape, CursorState, NodeId, RequestId, Size, SnapshotLine,
};
use embers_protocol::{
    BufferRequest, ClientMessage, InputRequest, NodeRequest, OkResponse, ScrollbackSliceResponse,
    ServerResponse, SessionSnapshot, SessionSnapshotResponse, SnapshotResponse,
};
use tempfile::{TempDir, tempdir};

use crate::support::{
    ROOT_BUFFER_LEAF_ID, ROOT_SPLIT_LEFT_LEAF_ID, ROOT_SPLIT_RIGHT_LEAF_ID, SESSION_ID,
    root_buffer_state, root_split_state,
};

const BUFFER: BufferId = BufferId(7);
const NODE: NodeId = ROOT_BUFFER_LEAF_ID;
const ROWS: u16 = 4;
const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

/// Ten lines of history; the last four (6..=9) are on screen.
const LINES: [&str; 10] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "foo.bar baz_qux",
    "  indented line",
    "",
    "$ last",
];

const ENTER_COPY: &str = r#"bind("normal", "<C-x>", action.enter_copy_mode());"#;

fn manager_from_source(source: &str) -> (ConfigManager, TempDir) {
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

/// A single-pane session showing the bottom `ROWS` lines of `LINES`, with the
/// program cursor at the start of the first visible row.
fn copy_state() -> embers_client::ClientState {
    let mut state = root_buffer_state();
    let total = LINES.len() as u64;
    let top = total - u64::from(ROWS);
    let visible = LINES[top as usize..]
        .iter()
        .map(|line| SnapshotLine::plain(*line))
        .collect::<Vec<_>>();
    let snapshot = state.snapshots.get_mut(&BUFFER).unwrap();
    snapshot.total_lines = total;
    snapshot.viewport_top_line = top;
    snapshot.lines = visible.clone();
    snapshot.cursor = Some(CursorState {
        position: CursorPosition { row: 0, col: 0 },
        shape: CursorShape::Block,
    });
    let view = state.view_state_mut(NODE).unwrap();
    view.visible_line_count = ROWS;
    view.total_line_count = total;
    view.scroll_top_line = top;
    view.follow_output = true;
    view.visible_lines = visible;
    state
}

fn copy_client(
    transport: &FakeTransport,
    source: &str,
) -> (ConfiguredClient<FakeTransport>, TempDir) {
    let (config, tempdir) = manager_from_source(source);
    let mut configured = ConfiguredClient::new(MuxClient::new(transport.clone()), config);
    *configured.client_mut().state_mut() = copy_state();
    (configured, tempdir)
}

fn capture_response(lines: &[&str]) -> ServerResponse {
    ServerResponse::Snapshot(SnapshotResponse {
        request_id: RequestId(0),
        buffer_id: BUFFER,
        sequence: 1,
        size: embers_core::PtySize::new(80, 24),
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        title: None,
        cwd: None,
    })
}

fn push_capture(transport: &FakeTransport) {
    transport.push_response(capture_response(&LINES));
}

fn push_slice(transport: &FakeTransport, start: u64) {
    let start_index = start as usize;
    transport.push_response(ServerResponse::ScrollbackSlice(ScrollbackSliceResponse {
        request_id: RequestId(0),
        buffer_id: BUFFER,
        start_line: start,
        total_lines: LINES.len() as u64,
        lines: LINES[start_index..start_index + usize::from(ROWS)]
            .iter()
            .map(|line| SnapshotLine::plain(*line))
            .collect(),
    }));
}

fn chars(text: &str) -> Vec<KeyEvent> {
    text.chars().map(KeyEvent::Char).collect()
}

/// Send keys, failing on any error. Action errors only surface as
/// notifications, which also catches a request the test did not queue a
/// response for.
async fn press(client: &mut ConfiguredClient<FakeTransport>, keys: Vec<KeyEvent>) {
    for key in keys {
        client.handle_key(SESSION_ID, VIEWPORT, key).await.unwrap();
        assert!(
            client.notifications().is_empty(),
            "{:?}",
            client.notifications()
        );
    }
}

async fn enter_copy(client: &mut ConfiguredClient<FakeTransport>) {
    press(client, vec![KeyEvent::Ctrl('x')]).await;
    assert_eq!(client.current_mode(), "copy");
}

fn cursor(client: &ConfiguredClient<FakeTransport>) -> (u64, u16) {
    let point = client
        .client()
        .state()
        .view_state(NODE)
        .and_then(|view| view.copy_cursor)
        .expect("copy cursor");
    (point.line, point.column)
}

fn top(client: &ConfiguredClient<FakeTransport>) -> u64 {
    client
        .client()
        .state()
        .view_state(NODE)
        .unwrap()
        .scroll_top_line
}

fn clipboard(client: &mut ConfiguredClient<FakeTransport>) -> String {
    use base64::Engine as _;

    let output = client.drain_terminal_output();
    assert_eq!(output.len(), 1, "expected one OSC 52 write");
    let text = String::from_utf8(output[0].clone()).unwrap();
    let payload = text
        .strip_prefix("\x1b]52;c;")
        .and_then(|rest| rest.strip_suffix('\x07'))
        .expect("OSC 52 sequence");
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn copy_mode_starts_at_the_program_cursor_and_moves_by_words() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);

    enter_copy(&mut client).await;
    assert_eq!(cursor(&client), (6, 0));

    // "foo.bar baz_qux": punctuation forms its own word, `_` does not split.
    let expected = [
        (6, 3),  // .
        (6, 4),  // bar
        (6, 8),  // baz_qux
        (7, 2),  // indented (crosses the line break, skips indentation)
        (7, 11), // line
        (8, 0),  // the empty line is a stop, as in vim
        (9, 0),  // $
        (9, 2),  // last
    ];
    for position in expected {
        press(&mut client, chars("w")).await;
        assert_eq!(cursor(&client), position);
    }

    press(&mut client, chars("b")).await;
    assert_eq!(cursor(&client), (9, 0));
    press(&mut client, chars("bb")).await;
    assert_eq!(cursor(&client), (7, 11));
    press(&mut client, chars("e")).await;
    assert_eq!(cursor(&client), (7, 14));

    // The buffer text is captured once and reused while it is unchanged.
    let captures = transport
        .requests()
        .iter()
        .filter(|request| {
            matches!(
                request,
                ClientMessage::Buffer(BufferRequest::Capture { .. })
            )
        })
        .count();
    assert_eq!(captures, 1);
    // The view is frozen while in copy mode.
    assert!(
        !client
            .client()
            .state()
            .view_state(NODE)
            .unwrap()
            .follow_output
    );
}

#[tokio::test]
async fn space_separated_word_motions_ignore_punctuation() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    press(&mut client, chars("W")).await;
    assert_eq!(cursor(&client), (6, 8));
    press(&mut client, chars("B")).await;
    assert_eq!(cursor(&client), (6, 0));
    press(&mut client, chars("E")).await;
    assert_eq!(cursor(&client), (6, 6));
    press(&mut client, chars("e")).await;
    assert_eq!(cursor(&client), (6, 14));
}

#[tokio::test]
async fn line_motions_jump_to_start_indentation_and_end() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    press(&mut client, chars("j$")).await;
    assert_eq!(cursor(&client), (7, 14));
    press(&mut client, chars("^")).await;
    assert_eq!(cursor(&client), (7, 2));
    press(&mut client, chars("0")).await;
    assert_eq!(cursor(&client), (7, 0));
}

#[tokio::test]
async fn count_prefixes_repeat_motions() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    press(&mut client, chars("3w")).await;
    assert_eq!(cursor(&client), (6, 8));

    // Moving five lines up crosses into scrollback and fetches it.
    push_slice(&transport, 1);
    press(&mut client, chars("5k")).await;
    assert_eq!(cursor(&client), (1, 8));
    assert_eq!(top(&client), 1);

    // `0` continues a count rather than jumping to the line start.
    push_slice(&transport, 6);
    press(&mut client, chars("10j")).await;
    assert_eq!(cursor(&client), (9, 8));
    assert_eq!(top(&client), 6);

    // Without a pending count `0` is the start-of-line motion.
    press(&mut client, chars("0")).await;
    assert_eq!(cursor(&client), (9, 0));
}

#[tokio::test]
async fn history_screen_and_page_motions_cross_the_scrollback_boundary() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    push_slice(&transport, 0);
    press(&mut client, chars("gg")).await;
    assert_eq!((cursor(&client), top(&client)), ((0, 0), 0));

    push_slice(&transport, 6);
    press(&mut client, chars("G")).await;
    assert_eq!((cursor(&client), top(&client)), ((9, 0), 6));

    press(&mut client, chars("H")).await;
    assert_eq!(cursor(&client), (6, 0));
    press(&mut client, chars("M")).await;
    assert_eq!(cursor(&client), (7, 0));
    press(&mut client, chars("L")).await;
    assert_eq!(cursor(&client), (9, 0));

    push_slice(&transport, 4);
    press(&mut client, vec![KeyEvent::Ctrl('u')]).await;
    assert_eq!((cursor(&client), top(&client)), ((7, 0), 4));

    push_slice(&transport, 6);
    press(&mut client, vec![KeyEvent::Ctrl('d')]).await;
    assert_eq!((cursor(&client), top(&client)), ((9, 0), 6));

    push_slice(&transport, 2);
    press(&mut client, vec![KeyEvent::Ctrl('b')]).await;
    assert_eq!((cursor(&client), top(&client)), ((5, 0), 2));

    push_slice(&transport, 6);
    press(&mut client, vec![KeyEvent::Ctrl('f')]).await;
    assert_eq!((cursor(&client), top(&client)), ((9, 0), 6));
}

#[tokio::test]
async fn character_jumps_repeat_and_reverse() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    // "foo.bar baz_qux"
    press(&mut client, chars("fa")).await;
    assert_eq!(cursor(&client), (6, 5));
    press(&mut client, chars(";")).await;
    assert_eq!(cursor(&client), (6, 9));
    press(&mut client, chars(",")).await;
    assert_eq!(cursor(&client), (6, 5));

    press(&mut client, chars("0tz")).await;
    assert_eq!(cursor(&client), (6, 9));
    // Repeating `t` does not get stuck just before the same character.
    press(&mut client, chars(";")).await;
    assert_eq!(cursor(&client), (6, 9));

    press(&mut client, chars("Fo")).await;
    assert_eq!(cursor(&client), (6, 2));
    press(&mut client, chars("$To")).await;
    assert_eq!(cursor(&client), (6, 3));

    press(&mut client, chars("02fa")).await;
    assert_eq!(cursor(&client), (6, 9));

    // A non-character key cancels a pending jump without moving.
    press(&mut client, vec![KeyEvent::Char('f'), KeyEvent::Escape]).await;
    assert_eq!(cursor(&client), (6, 9));
    assert_eq!(client.current_mode(), "copy");
}

#[tokio::test]
async fn selections_extend_with_motions_swap_ends_and_yank() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    press(&mut client, chars("2wv")).await;
    assert_eq!(client.current_mode(), "select");
    press(&mut client, chars("e")).await;
    let selection = client
        .client()
        .state()
        .view_state(NODE)
        .and_then(|view| view.selection_state.clone())
        .unwrap();
    assert_eq!(selection.kind, SelectionKind::Character);
    assert_eq!(selection.anchor, SelectionPoint { line: 6, column: 4 });
    assert_eq!(selection.cursor, SelectionPoint { line: 6, column: 6 });

    press(&mut client, chars("o")).await;
    assert_eq!(cursor(&client), (6, 4));
    press(&mut client, chars("b")).await;
    assert_eq!(cursor(&client), (6, 3));

    push_capture(&transport);
    press(&mut client, chars("y")).await;
    assert_eq!(clipboard(&mut client), ".bar");
    assert_eq!(client.current_mode(), "normal");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.selection_state.is_none());
    assert!(view.copy_cursor.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn escape_clears_the_selection_but_stays_in_copy_mode() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    press(&mut client, chars("vw")).await;
    press(&mut client, vec![KeyEvent::Escape]).await;
    assert_eq!(client.current_mode(), "copy");
    assert_eq!(cursor(&client), (6, 3));
    assert!(
        client
            .client()
            .state()
            .view_state(NODE)
            .unwrap()
            .selection_state
            .is_none()
    );

    // `V` then `v` switches the selection kind instead of clearing it.
    press(&mut client, chars("Vv")).await;
    assert_eq!(
        client
            .client()
            .state()
            .view_state(NODE)
            .and_then(|view| view.selection_state.as_ref())
            .map(|selection| selection.kind),
        Some(SelectionKind::Character)
    );
    press(&mut client, chars("q")).await;
    assert_eq!(client.current_mode(), "normal");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.copy_cursor.is_none());
    assert!(view.selection_state.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn generic_mode_actions_enter_and_leave_copy_mode() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(
        &transport,
        r#"
            bind("normal", "<C-x>", action.enter_mode("copy"));
            bind("copy", "x", action.leave_mode());
        "#,
    );

    enter_copy(&mut client).await;
    assert_eq!(cursor(&client), (6, 0));
    press(&mut client, chars("x")).await;
    assert_eq!(client.current_mode(), "normal");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.copy_cursor.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn backward_search_moves_the_cursor_and_reverses_n() {
    let transport = FakeTransport::default();
    push_capture(&transport);
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;
    press(&mut client, chars("G")).await;
    assert_eq!(cursor(&client), (9, 0));

    // Backspace and C-h both delete from the search prompt.
    let mut keys = chars("?oxy");
    keys.extend([KeyEvent::Backspace, KeyEvent::Ctrl('h'), KeyEvent::Enter]);
    press(&mut client, keys).await;
    assert_eq!(client.current_mode(), "copy");
    assert_eq!(cursor(&client), (6, 2));

    press(&mut client, chars("n")).await;
    assert_eq!(cursor(&client), (6, 1));
    press(&mut client, chars("N")).await;
    assert_eq!(cursor(&client), (6, 2));

    // Forward search starts after the cursor.
    let mut keys = chars("/o");
    keys.push(KeyEvent::Enter);
    push_capture(&transport);
    push_slice(&transport, 0);
    press(&mut client, keys).await;
    assert_eq!(cursor(&client), (0, 3));
}

#[tokio::test]
async fn copy_mode_bindings_from_config_can_move_between_panes() {
    let transport = FakeTransport::default();
    let (config, _tempdir) = manager_from_source(
        r#"
            bind("normal", "<C-x>", action.enter_copy_mode());
            bind("copy", "<C-h>", action.focus_left());
            bind("select", "<C-l>", action.focus_right());
        "#,
    );
    let mut client = ConfiguredClient::new(MuxClient::new(transport.clone()), config);
    let state = root_split_state();
    let mut left_focused = state.clone();
    left_focused
        .sessions
        .get_mut(&SESSION_ID)
        .unwrap()
        .focused_leaf_id = Some(ROOT_SPLIT_LEFT_LEAF_ID);
    *client.client_mut().state_mut() = state.clone();

    enter_copy(&mut client).await;
    transport.push_response(ServerResponse::Ok(OkResponse {
        request_id: RequestId(1),
    }));
    transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(2),
        snapshot: session_snapshot(&left_focused),
    }));
    press(&mut client, vec![KeyEvent::Ctrl('h')]).await;

    assert!(
        client.notifications().is_empty(),
        "{:?}",
        client.notifications()
    );
    assert_eq!(
        transport.requests()[0],
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(1),
            session_id: SESSION_ID,
            node_id: ROOT_SPLIT_LEFT_LEAF_ID,
        })
    );
    assert_eq!(client.current_mode(), "copy");

    // The select-mode table is just as configurable.
    press(&mut client, chars("v")).await;
    assert_eq!(client.current_mode(), "select");
    transport.push_response(ServerResponse::Ok(OkResponse {
        request_id: RequestId(3),
    }));
    transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(4),
        snapshot: session_snapshot(&state),
    }));
    press(&mut client, vec![KeyEvent::Ctrl('l')]).await;
    assert!(
        client.notifications().is_empty(),
        "{:?}",
        client.notifications()
    );
    assert_eq!(
        transport.requests()[2],
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(3),
            session_id: SESSION_ID,
            node_id: ROOT_SPLIT_RIGHT_LEAF_ID,
        })
    );
}

fn session_snapshot(state: &embers_client::ClientState) -> SessionSnapshot {
    SessionSnapshot {
        session: state.sessions.get(&SESSION_ID).unwrap().clone(),
        nodes: state.nodes.values().cloned().collect(),
        buffers: state.buffers.values().cloned().collect(),
        floating: Vec::new(),
    }
}

fn content_origin(client: &ConfiguredClient<FakeTransport>) -> (u16, u16) {
    let presentation =
        PresentationModel::project(client.client().state(), SESSION_ID, VIEWPORT).unwrap();
    let leaf = presentation.focused_leaf().unwrap();
    let origin = leaf.content_rect.origin;
    (origin.x as u16, origin.y as u16)
}

fn mouse(column: u16, row: u16, kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        row,
        column,
        modifiers: MouseModifiers::default(),
        kind,
    }
}

async fn send_mouse(client: &mut ConfiguredClient<FakeTransport>, event: MouseEvent) {
    client
        .handle_mouse(SESSION_ID, VIEWPORT, event)
        .await
        .unwrap();
    assert!(
        client.notifications().is_empty(),
        "{:?}",
        client.notifications()
    );
}

#[tokio::test]
async fn mouse_drag_selects_and_copies_on_release() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    let (x, y) = content_origin(&client);

    send_mouse(
        &mut client,
        mouse(x + 4, y, MouseEventKind::Press(MouseButton::Left)),
    )
    .await;
    assert_eq!(client.current_mode(), "normal");
    send_mouse(
        &mut client,
        mouse(x + 6, y, MouseEventKind::Drag(MouseButton::Left)),
    )
    .await;
    assert_eq!(client.current_mode(), "select");
    send_mouse(
        &mut client,
        mouse(x + 9, y + 1, MouseEventKind::Drag(MouseButton::Left)),
    )
    .await;
    let selection = client
        .client()
        .state()
        .view_state(NODE)
        .and_then(|view| view.selection_state.clone())
        .unwrap();
    assert_eq!(selection.anchor, SelectionPoint { line: 6, column: 4 });
    assert_eq!(selection.cursor, SelectionPoint { line: 7, column: 9 });

    push_capture(&transport);
    send_mouse(
        &mut client,
        mouse(
            x + 9,
            y + 1,
            MouseEventKind::Release(Some(MouseButton::Left)),
        ),
    )
    .await;
    assert_eq!(clipboard(&mut client), "bar baz_qux\n  indented");
    assert_eq!(client.current_mode(), "normal");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.selection_state.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn wheel_scrolling_in_copy_mode_keeps_the_cursor_on_screen() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    let (x, y) = content_origin(&client);
    enter_copy(&mut client).await;
    press(&mut client, chars("G")).await;

    push_slice(&transport, 3);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelUp)).await;
    assert_eq!((cursor(&client), top(&client)), ((6, 0), 3));

    // Scrolling back to the bottom does not resume following output.
    push_slice(&transport, 6);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelDown)).await;
    assert_eq!((cursor(&client), top(&client)), ((6, 0), 6));
    assert!(
        !client
            .client()
            .state()
            .view_state(NODE)
            .unwrap()
            .follow_output
    );
    assert_eq!(client.current_mode(), "copy");
}

#[tokio::test]
async fn a_plain_click_does_not_enter_copy_mode() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    let (x, y) = content_origin(&client);

    send_mouse(
        &mut client,
        mouse(x + 4, y, MouseEventKind::Press(MouseButton::Left)),
    )
    .await;
    send_mouse(
        &mut client,
        mouse(x + 4, y, MouseEventKind::Release(Some(MouseButton::Left))),
    )
    .await;
    assert_eq!(client.current_mode(), "normal");
    assert!(transport.requests().is_empty());
    assert!(client.drain_terminal_output().is_empty());
}

#[tokio::test]
async fn double_click_copies_a_word_and_triple_click_a_line() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    let (x, y) = content_origin(&client);
    let press = mouse(x + 9, y, MouseEventKind::Press(MouseButton::Left));
    let release = mouse(x + 9, y, MouseEventKind::Release(Some(MouseButton::Left)));

    send_mouse(&mut client, press).await;
    send_mouse(&mut client, release).await;
    push_capture(&transport);
    send_mouse(&mut client, press).await;
    assert_eq!(clipboard(&mut client), "baz_qux");
    assert_eq!(client.current_mode(), "normal");
    send_mouse(&mut client, release).await;

    push_capture(&transport);
    send_mouse(&mut client, press).await;
    assert_eq!(clipboard(&mut client), "foo.bar baz_qux");
    assert_eq!(client.current_mode(), "normal");
    send_mouse(&mut client, release).await;
}

#[tokio::test]
async fn drags_go_to_programs_that_requested_the_mouse() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    client
        .client_mut()
        .state_mut()
        .snapshots
        .get_mut(&BUFFER)
        .unwrap()
        .mouse_reporting = true;
    let state = client.client().state().clone();
    let (x, y) = content_origin(&client);

    for request_id in [1, 4] {
        transport.push_response(ServerResponse::Ok(OkResponse {
            request_id: RequestId(request_id),
        }));
        let mut snapshot = state.snapshots.get(&BUFFER).unwrap().clone();
        snapshot.request_id = RequestId(request_id + 1);
        transport.push_response(ServerResponse::VisibleSnapshot(snapshot));
        transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(request_id + 2),
            snapshot: session_snapshot(&state),
        }));
    }

    send_mouse(
        &mut client,
        mouse(x + 4, y, MouseEventKind::Press(MouseButton::Left)),
    )
    .await;
    send_mouse(
        &mut client,
        mouse(x + 6, y, MouseEventKind::Drag(MouseButton::Left)),
    )
    .await;

    let sends = transport
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            ClientMessage::Input(InputRequest::Send { bytes, .. }) => Some(bytes),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sends,
        vec![b"\x1b[<0;5;1M".to_vec(), b"\x1b[<32;7;1M".to_vec()]
    );
    assert_eq!(client.current_mode(), "normal");
    assert!(
        client
            .client()
            .state()
            .view_state(NODE)
            .unwrap()
            .selection_state
            .is_none()
    );
}

#[tokio::test]
async fn scrolling_back_enters_copy_mode_and_scrolling_to_the_bottom_leaves_it() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    let (x, y) = content_origin(&client);

    // The cursor starts at the program's cursor and stays on screen.
    push_slice(&transport, 3);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelUp)).await;
    assert_eq!(client.current_mode(), "copy");
    assert_eq!((cursor(&client), top(&client)), ((6, 0), 3));
    press(&mut client, chars("k")).await;
    assert_eq!(cursor(&client), (5, 0));

    push_slice(&transport, 6);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelDown)).await;
    assert_eq!(client.current_mode(), "normal");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.copy_cursor.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn scrolling_to_the_bottom_keeps_a_selection() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");
    let (x, y) = content_origin(&client);

    push_slice(&transport, 3);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelUp)).await;
    press(&mut client, chars("v")).await;
    push_slice(&transport, 6);
    send_mouse(&mut client, mouse(x, y, MouseEventKind::WheelDown)).await;
    assert_eq!(client.current_mode(), "select");
}

#[tokio::test]
async fn page_up_enters_copy_mode_with_the_cursor_on_screen() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, "");

    push_slice(&transport, 2);
    press(&mut client, vec![KeyEvent::PageUp]).await;
    assert_eq!(client.current_mode(), "copy");
    assert_eq!((cursor(&client), top(&client)), ((5, 0), 2));
}

#[tokio::test]
async fn paragraph_motions_move_between_blank_lines() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(&transport, ENTER_COPY);
    enter_copy(&mut client).await;

    push_capture(&transport);
    press(&mut client, chars("}")).await;
    assert_eq!(cursor(&client), (8, 0));
    press(&mut client, chars("}")).await;
    assert_eq!(cursor(&client), (9, 5));
    press(&mut client, chars("{")).await;
    assert_eq!(cursor(&client), (8, 0));

    push_slice(&transport, 0);
    press(&mut client, chars("{")).await;
    assert_eq!((cursor(&client), top(&client)), ((0, 0), 0));
}

#[tokio::test]
async fn leader_bindings_leave_copy_mode_and_run() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(
        &transport,
        r#"
            set_leader("<C-a>");
            bind("normal", "<C-x>", action.enter_copy_mode());
            bind("normal", "<leader>:", action.command_prompt());
        "#,
    );
    enter_copy(&mut client).await;
    press(&mut client, chars("v")).await;

    press(&mut client, vec![KeyEvent::Ctrl('a'), KeyEvent::Char(':')]).await;
    assert_eq!(client.current_mode(), "prompt");
    let view = client.client().state().view_state(NODE).unwrap();
    assert!(view.copy_cursor.is_none());
    assert!(view.selection_state.is_none());
    assert!(view.follow_output);
}

#[tokio::test]
async fn a_copy_mode_binding_of_the_leader_key_shadows_leader_bindings() {
    let transport = FakeTransport::default();
    let (mut client, _tempdir) = copy_client(
        &transport,
        r#"
            set_leader("<C-a>");
            bind("normal", "<C-x>", action.enter_copy_mode());
            bind("normal", "<leader>:", action.command_prompt());
            bind("copy", "<C-a>", action.copy_cursor_up());
        "#,
    );
    enter_copy(&mut client).await;

    push_slice(&transport, 5);
    press(&mut client, vec![KeyEvent::Ctrl('a'), KeyEvent::Char(':')]).await;
    assert_eq!(client.current_mode(), "copy");
    assert_eq!(cursor(&client), (5, 0));
}

#[tokio::test]
async fn the_wheel_over_another_pane_leaves_copy_mode_alone() {
    let transport = FakeTransport::default();
    let (config, _tempdir) = manager_from_source("");
    let mut client = ConfiguredClient::new(MuxClient::new(transport.clone()), config);
    // The left pane is focused and shows the bottom of `LINES`; the right
    // pane follows its output.
    let mut state = root_split_state();
    state.sessions.get_mut(&SESSION_ID).unwrap().focused_leaf_id = Some(ROOT_SPLIT_LEFT_LEAF_ID);
    let single = copy_state();
    *state.snapshots.get_mut(&BUFFER).unwrap() = single.snapshots[&BUFFER].clone();
    *state.view_state_mut(ROOT_SPLIT_LEFT_LEAF_ID).unwrap() =
        single.view_state(NODE).unwrap().clone();
    *client.client_mut().state_mut() = state;
    let presentation =
        PresentationModel::project(client.client().state(), SESSION_ID, VIEWPORT).unwrap();
    let origin = |node_id| {
        let leaf = presentation
            .leaves
            .iter()
            .find(|leaf| leaf.node_id == node_id)
            .unwrap();
        (
            leaf.content_rect.origin.x as u16,
            leaf.content_rect.origin.y as u16,
        )
    };
    let (left_x, left_y) = origin(ROOT_SPLIT_LEFT_LEAF_ID);
    let (right_x, right_y) = origin(ROOT_SPLIT_RIGHT_LEAF_ID);

    push_slice(&transport, 3);
    send_mouse(&mut client, mouse(left_x, left_y, MouseEventKind::WheelUp)).await;
    assert_eq!(client.current_mode(), "copy");

    send_mouse(
        &mut client,
        mouse(right_x, right_y, MouseEventKind::WheelDown),
    )
    .await;
    assert_eq!(client.current_mode(), "copy");
    let left = client
        .client()
        .state()
        .view_state(ROOT_SPLIT_LEFT_LEAF_ID)
        .unwrap();
    assert!(left.copy_cursor.is_some());
    assert_eq!(left.scroll_top_line, 3);
}
