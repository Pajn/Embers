use std::time::{Duration, Instant};

use crate::support::integration_test_lock;
use embers_core::{SplitDirection, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRequest, BuffersResponse, ClientMessage, InputRequest, NodeRequest,
    ServerResponse, SessionRequest, SessionSnapshot, SessionSnapshotResponse, SnapshotResponse,
};
use embers_test_support::{TestConnection, TestServer};
use tokio::time::sleep;

async fn create_session(connection: &mut TestConnection, name: &str) -> SessionSnapshotResponse {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::Create {
            request_id: new_request_id(),
            name: name.to_owned(),
        }))
        .await
        .expect("create session request succeeds");

    match response {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    }
}

async fn get_session(
    connection: &mut TestConnection,
    session_id: embers_core::SessionId,
) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::Get {
            request_id: new_request_id(),
            session_id,
        }))
        .await
        .expect("get session request succeeds");

    match response {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    }
}

async fn create_echo_buffer(connection: &mut TestConnection, title: &str) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some(title.to_owned()),
            command: vec![
                "/bin/sh".to_owned(),
                "-lc".to_owned(),
                "printf 'ready\\n'; while IFS= read -r line; do printf 'seen:%s\\n' \"$line\"; done"
                    .to_owned(),
            ],
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

async fn add_root_tab(
    connection: &mut TestConnection,
    session_id: embers_core::SessionId,
    title: &str,
    buffer_id: embers_core::BufferId,
) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id,
            title: title.to_owned(),
            buffer_id: Some(buffer_id),
            child_node_id: None,
        }))
        .await
        .expect("add root tab request succeeds");

    match response {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
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

    assert!(matches!(response, ServerResponse::Ok(_)));
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

    assert!(matches!(response, ServerResponse::Ok(_)));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detach_list_capture_and_reattach_buffer_via_socket() {
    let _guard = integration_test_lock().lock().await;
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let primary = create_echo_buffer(&mut connection, "primary").await;
    add_root_tab(&mut connection, session_id, "primary", primary.id).await;
    wait_for_capture_contains(&mut connection, primary.id, "ready").await;

    let detach = connection
        .request(&ClientMessage::Buffer(BufferRequest::Detach {
            request_id: new_request_id(),
            buffer_id: primary.id,
        }))
        .await
        .expect("detach request succeeds");
    assert!(matches!(detach, ServerResponse::Ok(_)));

    let detached = connection
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: new_request_id(),
            session_id: None,
            attached_only: false,
            detached_only: true,
        }))
        .await
        .expect("list detached buffers request succeeds");
    let detached = match detached {
        ServerResponse::Buffers(BuffersResponse { buffers, .. }) => buffers,
        other => panic!("expected buffers response, got {other:?}"),
    };
    assert!(detached.iter().any(|buffer| buffer.id == primary.id));

    send_input(&mut connection, primary.id, "detached\n").await;
    wait_for_capture_contains(&mut connection, primary.id, "seen:detached").await;

    let replacement = create_echo_buffer(&mut connection, "replacement").await;
    let replacement_snapshot =
        add_root_tab(&mut connection, session_id, "replacement", replacement.id).await;
    let target_leaf = replacement_snapshot
        .session
        .focused_leaf_id
        .expect("replacement tab focuses target leaf");

    let moved = connection
        .request(&ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: new_request_id(),
            buffer_id: primary.id,
            target_leaf_node_id: target_leaf,
        }))
        .await
        .expect("move request succeeds");
    let moved = match moved {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(moved.session.focused_leaf_id, Some(target_leaf));
    let target_view = moved
        .nodes
        .iter()
        .find(|node| node.id == target_leaf)
        .and_then(|node| node.buffer_view.clone())
        .expect("target leaf remains a buffer view");
    assert_eq!(target_view.buffer_id, primary.id);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_request_replaces_target_leaf_without_killing_buffer() {
    let _guard = integration_test_lock().lock().await;
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let first = create_echo_buffer(&mut connection, "first").await;
    let root_snapshot = add_root_tab(&mut connection, session_id, "first", first.id).await;
    let first_leaf = root_snapshot
        .session
        .focused_leaf_id
        .expect("root tab focuses first leaf");
    wait_for_capture_contains(&mut connection, first.id, "ready").await;

    let second = create_echo_buffer(&mut connection, "second").await;
    let split = connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: first_leaf,
            direction: SplitDirection::Horizontal,
            new_buffer_id: second.id,
        }))
        .await
        .expect("split request succeeds");
    let split = match split {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let second_leaf = split
        .session
        .focused_leaf_id
        .expect("split focuses second leaf");

    let moved = connection
        .request(&ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: new_request_id(),
            buffer_id: first.id,
            target_leaf_node_id: second_leaf,
        }))
        .await
        .expect("move request succeeds");
    let moved = match moved {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(moved.session.focused_leaf_id, Some(second_leaf));
    assert_eq!(
        get_buffer(&mut connection, second.id)
            .await
            .attachment_node_id,
        None
    );

    send_input(&mut connection, first.id, "moved\n").await;
    wait_for_capture_contains(&mut connection, first.id, "seen:moved").await;
    assert_eq!(
        get_session(&mut connection, session_id)
            .await
            .session
            .focused_leaf_id,
        Some(second_leaf)
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detach_and_reattach_preserve_runtime_identity_size_and_capture() {
    let _guard = integration_test_lock().lock().await;
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let primary = create_echo_buffer(&mut connection, "primary").await;
    let attached = add_root_tab(&mut connection, session_id, "primary", primary.id).await;
    let original_leaf = attached
        .session
        .focused_leaf_id
        .expect("primary tab focuses leaf");
    wait_for_capture_contains(&mut connection, primary.id, "ready").await;

    let before_detach = get_buffer(&mut connection, primary.id).await;
    resize_buffer(&mut connection, primary.id, 120, 33).await;
    let resized = get_buffer(&mut connection, primary.id).await;
    assert_eq!(resized.pty_size, embers_core::PtySize::new(120, 33));

    let detach = connection
        .request(&ClientMessage::Buffer(BufferRequest::Detach {
            request_id: new_request_id(),
            buffer_id: primary.id,
        }))
        .await
        .expect("detach request succeeds");
    assert!(matches!(detach, ServerResponse::Ok(_)));

    let detached = get_buffer(&mut connection, primary.id).await;
    assert_eq!(detached.attachment_node_id, None);
    assert_eq!(detached.pid, before_detach.pid);
    assert_eq!(detached.pty_size, embers_core::PtySize::new(120, 33));
    assert_eq!(detached.state, before_detach.state);

    send_input(&mut connection, primary.id, "detached-still-live\n").await;
    let detached_capture =
        wait_for_capture_contains(&mut connection, primary.id, "seen:detached-still-live").await;
    assert!(detached_capture.lines.join("\n").contains("ready"));

    let replacement = create_echo_buffer(&mut connection, "replacement").await;
    let replacement_snapshot =
        add_root_tab(&mut connection, session_id, "replacement", replacement.id).await;
    let target_leaf = replacement_snapshot
        .session
        .focused_leaf_id
        .expect("replacement tab focuses target leaf");
    assert_ne!(target_leaf, original_leaf);

    let moved = connection
        .request(&ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: new_request_id(),
            buffer_id: primary.id,
            target_leaf_node_id: target_leaf,
        }))
        .await
        .expect("move request succeeds");
    assert!(matches!(moved, ServerResponse::SessionSnapshot(_)));

    let reattached = get_buffer(&mut connection, primary.id).await;
    assert_eq!(reattached.attachment_node_id, Some(target_leaf));
    assert_eq!(reattached.pid, before_detach.pid);
    assert_eq!(reattached.pty_size, embers_core::PtySize::new(120, 33));

    send_input(&mut connection, primary.id, "reattached\n").await;
    wait_for_capture_contains(&mut connection, primary.id, "seen:reattached").await;

    server.shutdown().await.expect("shutdown server");
}
