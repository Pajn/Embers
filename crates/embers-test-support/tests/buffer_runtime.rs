use std::time::{Duration, Instant};

use embers_core::{PtySize, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferRequest, ClientMessage, InputRequest, OkResponse,
    ServerResponse, SnapshotResponse,
};
use embers_test_support::{TestConnection, TestServer, acquire_test_lock};
use tokio::time::sleep;

async fn create_buffer(connection: &mut TestConnection, command: &[&str]) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some("buffer".to_owned()),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd: None,
            env: Default::default(),
        }))
        .await
        .expect("create buffer request succeeds");

    match response {
        ServerResponse::Buffer(buffer) => buffer.buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn get_buffer(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("get buffer request succeeds");

    match response {
        ServerResponse::Buffer(buffer) => buffer.buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn list_detached_buffers(connection: &mut TestConnection) -> Vec<BufferRecord> {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: new_request_id(),
            session_id: None,
            attached_only: false,
            detached_only: true,
        }))
        .await
        .expect("list buffers request succeeds");

    match response {
        ServerResponse::Buffers(buffers) => buffers.buffers,
        other => panic!("expected buffers response, got {other:?}"),
    }
}

async fn capture_buffer(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
) -> SnapshotResponse {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Capture {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("capture request succeeds");

    match response {
        ServerResponse::Snapshot(snapshot) => snapshot,
        other => panic!("expected snapshot response, got {other:?}"),
    }
}

async fn send_input(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
    input: &str,
) {
    let response = connection
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: new_request_id(),
            buffer_id,
            bytes: input.as_bytes().to_vec(),
        }))
        .await
        .expect("send input request succeeds");

    assert!(matches!(response, ServerResponse::Ok(OkResponse { .. })));
}

async fn resize_buffer(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
    cols: u16,
    rows: u16,
) {
    let response = connection
        .request(&ClientMessage::Input(InputRequest::Resize {
            request_id: new_request_id(),
            buffer_id,
            cols,
            rows,
        }))
        .await
        .expect("resize request succeeds");

    assert!(matches!(response, ServerResponse::Ok(OkResponse { .. })));
}

async fn detach_buffer(connection: &mut TestConnection, buffer_id: embers_core::BufferId) {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Detach {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("detach request succeeds");

    assert!(matches!(response, ServerResponse::Ok(OkResponse { .. })));
}

async fn kill_buffer(connection: &mut TestConnection, buffer_id: embers_core::BufferId) {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Kill {
            request_id: new_request_id(),
            buffer_id,
            force: true,
        }))
        .await
        .expect("kill request succeeds");

    assert!(matches!(response, ServerResponse::Ok(OkResponse { .. })));
}

async fn wait_for_capture_contains(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
    needle: &str,
) -> SnapshotResponse {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = capture_buffer(connection, buffer_id).await;
        let text = snapshot.lines.join("\n");
        if text.contains(needle) {
            return snapshot;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for capture containing {needle:?}; got {text:?}");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_exit(
    connection: &mut TestConnection,
    buffer_id: embers_core::BufferId,
) -> BufferRecord {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let buffer = get_buffer(connection, buffer_id).await;
        if matches!(buffer.state, BufferRecordState::Exited) {
            return buffer;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for buffer {buffer_id} to exit");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_buffers_accept_input_and_keep_running_after_detach_requests() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "printf 'ready\\n'; while IFS= read -r line; do printf 'seen:%s\\n' \"$line\"; done",
        ],
    )
    .await;
    assert_eq!(buffer.state, BufferRecordState::Running);
    assert_eq!(buffer.attachment_node_id, None);

    let detached = list_detached_buffers(&mut connection).await;
    assert!(detached.iter().any(|candidate| candidate.id == buffer.id));

    wait_for_capture_contains(&mut connection, buffer.id, "ready").await;
    send_input(&mut connection, buffer.id, "hello\n").await;
    wait_for_capture_contains(&mut connection, buffer.id, "seen:hello").await;

    detach_buffer(&mut connection, buffer.id).await;
    let detached_buffer = get_buffer(&mut connection, buffer.id).await;
    assert_eq!(detached_buffer.attachment_node_id, None);
    assert_eq!(detached_buffer.state, BufferRecordState::Running);

    send_input(&mut connection, buffer.id, "again\n").await;
    wait_for_capture_contains(&mut connection, buffer.id, "seen:again").await;

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_and_kill_requests_update_buffer_state_and_preserve_capture() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &["/bin/sh", "-lc", "printf 'alive\\n'; cat"],
    )
    .await;
    wait_for_capture_contains(&mut connection, buffer.id, "alive").await;

    resize_buffer(&mut connection, buffer.id, 100, 30).await;
    let resized = get_buffer(&mut connection, buffer.id).await;
    assert_eq!(resized.pty_size, PtySize::new(100, 30));
    let resized_snapshot = capture_buffer(&mut connection, buffer.id).await;
    assert_eq!(resized_snapshot.size, PtySize::new(100, 30));

    kill_buffer(&mut connection, buffer.id).await;
    let exited = wait_for_exit(&mut connection, buffer.id).await;
    assert_eq!(exited.state, BufferRecordState::Exited);

    let captured = capture_buffer(&mut connection, buffer.id).await;
    assert!(captured.lines.join("\n").contains("alive"));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_preserves_scrollback_for_long_output() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "i=1; while [ $i -le 40 ]; do printf 'line-%02d\\n' \"$i\"; i=$((i+1)); done",
        ],
    )
    .await;

    let snapshot = wait_for_capture_contains(&mut connection, buffer.id, "line-40").await;
    let text = snapshot.lines.join("\n");
    assert!(text.contains("line-01"));
    assert!(text.contains("line-40"));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn visible_snapshot_surfaces_terminal_modes_and_cursor_metadata() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "printf '\\033]0;embers\\007\\033[?1049h\\033[?1000h\\033[?1004h\\033[?2004hhello'",
        ],
    )
    .await;
    wait_for_capture_contains(&mut connection, buffer.id, "hello").await;

    let snapshot = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("visible capture succeeds");
    let text = snapshot.lines.join("\n");
    assert!(text.contains("hello"));
    assert_eq!(snapshot.title.as_deref(), Some("embers"));
    assert!(snapshot.alternate_screen);
    assert!(snapshot.mouse_reporting);
    assert!(snapshot.focus_reporting);
    assert!(snapshot.bracketed_paste);
    assert!(snapshot.total_lines >= u64::from(snapshot.size.rows));
    assert!(snapshot.cursor.is_some());

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrollback_slice_returns_history_while_full_capture_stays_available() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "i=1; while [ $i -le 40 ]; do printf 'line-%02d\\n' \"$i\"; i=$((i+1)); done",
        ],
    )
    .await;

    let captured = wait_for_capture_contains(&mut connection, buffer.id, "line-40").await;
    wait_for_exit(&mut connection, buffer.id).await;
    let visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("visible capture succeeds");
    let slice = connection
        .capture_scrollback_slice(buffer.id, 0, 3)
        .await
        .expect("scrollback slice succeeds");
    let expected_prefix = captured.lines.iter().take(3).cloned().collect::<Vec<_>>();

    assert!(captured.lines.join("\n").contains("line-01"));
    assert!(captured.lines.join("\n").contains("line-40"));
    assert!(visible.total_lines >= 40);
    assert!(visible.viewport_top_line > 0);
    assert_eq!(slice.lines, expected_prefix);
    assert_eq!(slice.start_line, 0);
    assert_eq!(slice.total_lines, visible.total_lines);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_snapshot_reads_stay_stable_without_new_output() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "i=1; while [ $i -le 8 ]; do printf 'line-%02d\\n' \"$i\"; i=$((i+1)); done",
        ],
    )
    .await;

    wait_for_capture_contains(&mut connection, buffer.id, "line-08").await;
    wait_for_exit(&mut connection, buffer.id).await;

    let first_full = capture_buffer(&mut connection, buffer.id).await;
    let second_full = capture_buffer(&mut connection, buffer.id).await;
    assert_eq!(first_full.sequence, second_full.sequence);
    assert_eq!(first_full.size, second_full.size);
    assert_eq!(first_full.lines, second_full.lines);
    assert_eq!(first_full.title, second_full.title);
    assert_eq!(first_full.cwd, second_full.cwd);

    let first_visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("first visible capture succeeds");
    let second_visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("second visible capture succeeds");
    assert_eq!(first_visible.sequence, second_visible.sequence);
    assert_eq!(first_visible.size, second_visible.size);
    assert_eq!(first_visible.lines, second_visible.lines);
    assert_eq!(first_visible.title, second_visible.title);
    assert_eq!(first_visible.cwd, second_visible.cwd);
    assert_eq!(
        first_visible.viewport_top_line,
        second_visible.viewport_top_line
    );
    assert_eq!(first_visible.total_lines, second_visible.total_lines);
    assert_eq!(
        first_visible.alternate_screen,
        second_visible.alternate_screen
    );
    assert_eq!(
        first_visible.mouse_reporting,
        second_visible.mouse_reporting
    );
    assert_eq!(
        first_visible.focus_reporting,
        second_visible.focus_reporting
    );
    assert_eq!(
        first_visible.bracketed_paste,
        second_visible.bracketed_paste
    );
    assert_eq!(first_visible.cursor, second_visible.cursor);

    let first_slice = connection
        .capture_scrollback_slice(buffer.id, 2, 3)
        .await
        .expect("first scrollback slice succeeds");
    let second_slice = connection
        .capture_scrollback_slice(buffer.id, 2, 3)
        .await
        .expect("second scrollback slice succeeds");
    assert_eq!(first_slice.start_line, second_slice.start_line);
    assert_eq!(first_slice.total_lines, second_slice.total_lines);
    assert_eq!(first_slice.lines, second_slice.lines);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_visible_capture_tracks_latest_size_and_output() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let buffer = create_buffer(
        &mut connection,
        &[
            "/bin/sh",
            "-lc",
            "printf '\\033]0;detached-preview\\007ready\\n'; while IFS= read -r line; do printf 'seen:%s\\n' \"$line\"; done",
        ],
    )
    .await;
    assert_eq!(buffer.attachment_node_id, None);

    wait_for_capture_contains(&mut connection, buffer.id, "ready").await;
    let initial_visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("initial visible capture succeeds");
    assert_eq!(initial_visible.size, PtySize::new(80, 24));
    assert_eq!(initial_visible.title.as_deref(), Some("detached-preview"));
    assert!(initial_visible.lines.join("\n").contains("ready"));

    resize_buffer(&mut connection, buffer.id, 96, 18).await;
    let resized_visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("resized visible capture succeeds");
    assert_eq!(resized_visible.size, PtySize::new(96, 18));
    assert!(resized_visible.total_lines >= 1);

    send_input(&mut connection, buffer.id, "after-resize\n").await;
    wait_for_capture_contains(&mut connection, buffer.id, "seen:after-resize").await;

    let visible = connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("final visible capture succeeds");
    assert_eq!(visible.size, PtySize::new(96, 18));
    assert_eq!(visible.title.as_deref(), Some("detached-preview"));
    assert!(visible.lines.join("\n").contains("seen:after-resize"));

    let captured = capture_buffer(&mut connection, buffer.id).await;
    assert_eq!(captured.size, PtySize::new(96, 18));
    assert!(captured.lines.join("\n").contains("ready"));
    assert!(captured.lines.join("\n").contains("seen:after-resize"));

    let slice = connection
        .capture_scrollback_slice(buffer.id, 0, 4)
        .await
        .expect("detached scrollback slice succeeds");
    assert!(slice.total_lines >= 2);
    assert!(slice.lines.join("\n").contains("ready"));

    server.shutdown().await.expect("shutdown server");
}
