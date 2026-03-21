use embers_core::{FloatGeometry, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRequest, ClientMessage, FloatingRequest, NodeRequest, ServerResponse,
    SessionRequest, SessionSnapshot, SessionSnapshotResponse,
};
use embers_test_support::{TestConnection, TestServer};

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

async fn create_buffer(connection: &mut TestConnection, title: &str) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some(title.to_owned()),
            command: vec!["/bin/sh".to_owned(), "-lc".to_owned(), "cat".to_owned()],
            cwd: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_focus_move_and_close_floating_window_via_socket() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let root_buffer = create_buffer(&mut connection, "root").await;
    let root_snapshot = add_root_tab(&mut connection, session_id, "root", root_buffer.id).await;
    let root_leaf = root_snapshot
        .session
        .focused_leaf_id
        .expect("root tab focuses root leaf");

    let popup_buffer = create_buffer(&mut connection, "popup").await;
    let created = connection
        .request(&ClientMessage::Floating(FloatingRequest::Create {
            request_id: new_request_id(),
            session_id,
            root_node_id: None,
            buffer_id: Some(popup_buffer.id),
            geometry: FloatGeometry::new(4, 2, 40, 12),
            title: Some("popup".to_owned()),
        }))
        .await
        .expect("create floating request succeeds");
    let floating = match created {
        ServerResponse::Floating(response) => response.floating,
        other => panic!("expected floating response, got {other:?}"),
    };
    assert!(floating.focused);
    assert_eq!(floating.geometry, FloatGeometry::new(4, 2, 40, 12));

    connection
        .request(&ClientMessage::Node(NodeRequest::Focus {
            request_id: new_request_id(),
            session_id,
            node_id: root_leaf,
        }))
        .await
        .expect("focus root leaf request succeeds");

    let focused = connection
        .request(&ClientMessage::Floating(FloatingRequest::Focus {
            request_id: new_request_id(),
            floating_id: floating.id,
        }))
        .await
        .expect("focus floating request succeeds");
    let focused = match focused {
        ServerResponse::Floating(response) => response.floating,
        other => panic!("expected floating response, got {other:?}"),
    };
    assert!(focused.focused);

    let moved = connection
        .request(&ClientMessage::Floating(FloatingRequest::Move {
            request_id: new_request_id(),
            floating_id: floating.id,
            geometry: FloatGeometry::new(10, 6, 60, 18),
        }))
        .await
        .expect("move floating request succeeds");
    let moved = match moved {
        ServerResponse::Floating(response) => response.floating,
        other => panic!("expected floating response, got {other:?}"),
    };
    assert_eq!(moved.geometry, FloatGeometry::new(10, 6, 60, 18));

    let session_snapshot = get_session(&mut connection, session_id).await;
    assert_eq!(
        session_snapshot.session.focused_floating_id,
        Some(floating.id)
    );

    let closed = connection
        .request(&ClientMessage::Floating(FloatingRequest::Close {
            request_id: new_request_id(),
            floating_id: floating.id,
        }))
        .await
        .expect("close floating request succeeds");
    assert!(matches!(closed, ServerResponse::Ok(_)));
    assert_eq!(
        get_buffer(&mut connection, popup_buffer.id)
            .await
            .attachment_node_id,
        None
    );
    assert!(
        get_session(&mut connection, session_id)
            .await
            .floating
            .is_empty()
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_last_tab_in_floating_tabs_removes_popup() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let root_buffer = create_buffer(&mut connection, "root").await;
    let root_snapshot = add_root_tab(&mut connection, session_id, "root", root_buffer.id).await;
    let root_leaf = root_snapshot
        .session
        .focused_leaf_id
        .expect("root tab focuses root leaf");

    let popup_buffer = create_buffer(&mut connection, "popup").await;
    let created = connection
        .request(&ClientMessage::Floating(FloatingRequest::Create {
            request_id: new_request_id(),
            session_id,
            root_node_id: None,
            buffer_id: Some(popup_buffer.id),
            geometry: FloatGeometry::new(2, 2, 30, 10),
            title: Some("popup".to_owned()),
        }))
        .await
        .expect("create floating request succeeds");
    let floating = match created {
        ServerResponse::Floating(response) => response.floating,
        other => panic!("expected floating response, got {other:?}"),
    };
    let popup_leaf = floating.root_node_id;

    let wrapped = connection
        .request(&ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: new_request_id(),
            node_id: popup_leaf,
            title: "popup".to_owned(),
        }))
        .await
        .expect("wrap floating root request succeeds");
    let wrapped = match wrapped {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(wrapped.floating.len(), 1);

    let closed = connection
        .request(&ClientMessage::Node(NodeRequest::Close {
            request_id: new_request_id(),
            node_id: popup_leaf,
        }))
        .await
        .expect("close popup leaf request succeeds");
    let closed = match closed {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert!(closed.floating.is_empty());
    assert_eq!(closed.session.focused_leaf_id, Some(root_leaf));
    assert_eq!(
        get_buffer(&mut connection, popup_buffer.id)
            .await
            .attachment_node_id,
        None
    );

    server.shutdown().await.expect("shutdown server");
}
