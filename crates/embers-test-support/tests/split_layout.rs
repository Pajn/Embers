use embers_core::{SplitDirection, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRequest, ClientMessage, NodeRequest, ServerResponse, SessionRequest,
    SessionSnapshot, SessionSnapshotResponse, SplitRecord, TabsRecord,
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

async fn create_buffer(connection: &mut TestConnection, title: &str) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some(title.to_owned()),
            command: vec!["/bin/sh".to_owned(), "-lc".to_owned(), "cat".to_owned()],
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

fn root_tabs(snapshot: &SessionSnapshot) -> TabsRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .and_then(|node| node.tabs.clone())
        .expect("session root snapshot includes tabs record")
}

fn root_node(snapshot: &SessionSnapshot) -> &embers_protocol::NodeRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .expect("session root snapshot includes root node")
}

fn split_record(snapshot: &SessionSnapshot, node_id: embers_core::NodeId) -> SplitRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .and_then(|node| node.split.clone())
        .expect("snapshot includes split node")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_and_resize_requests_build_nested_layouts_via_socket() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let first_buffer = create_buffer(&mut connection, "one").await;
    let first_snapshot = add_root_tab(&mut connection, session_id, "one", first_buffer.id).await;
    let first_leaf = root_tabs(&first_snapshot).tabs[0].child_id;

    let second_buffer = create_buffer(&mut connection, "two").await;
    let split_snapshot = connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: first_leaf,
            direction: SplitDirection::Vertical,
            new_buffer_id: second_buffer.id,
        }))
        .await
        .expect("split request succeeds");
    let split_snapshot = match split_snapshot {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let outer_split_id = root_tabs(&split_snapshot).tabs[0].child_id;
    let outer_split = split_record(&split_snapshot, outer_split_id);
    let second_leaf = split_snapshot
        .session
        .focused_leaf_id
        .expect("split focuses new leaf");
    assert_eq!(outer_split.direction, SplitDirection::Vertical);
    assert_eq!(outer_split.child_ids, vec![first_leaf, second_leaf]);
    assert_eq!(outer_split.sizes, vec![1, 1]);

    let third_buffer = create_buffer(&mut connection, "three").await;
    let nested_snapshot = connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: second_leaf,
            direction: SplitDirection::Horizontal,
            new_buffer_id: third_buffer.id,
        }))
        .await
        .expect("nested split request succeeds");
    let nested_snapshot = match nested_snapshot {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let updated_outer = split_record(&nested_snapshot, outer_split_id);
    let inner_split_id = updated_outer.child_ids[1];
    let inner_split = split_record(&nested_snapshot, inner_split_id);
    let third_leaf = nested_snapshot
        .session
        .focused_leaf_id
        .expect("nested split focuses newest leaf");
    assert_eq!(updated_outer.child_ids[0], first_leaf);
    assert_eq!(inner_split.direction, SplitDirection::Horizontal);
    assert_eq!(inner_split.child_ids, vec![second_leaf, third_leaf]);
    assert_eq!(inner_split.sizes, vec![1, 1]);

    let resized = connection
        .request(&ClientMessage::Node(NodeRequest::Resize {
            request_id: new_request_id(),
            node_id: outer_split_id,
            sizes: vec![4, 1],
        }))
        .await
        .expect("resize request succeeds");
    let resized = match resized {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(split_record(&resized, outer_split_id).sizes, vec![4, 1]);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focus_and_close_requests_normalize_layout_and_detach_buffers() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;

    let first_buffer = create_buffer(&mut connection, "one").await;
    let first_snapshot = add_root_tab(&mut connection, session_id, "one", first_buffer.id).await;
    let first_leaf = root_tabs(&first_snapshot).tabs[0].child_id;

    let second_buffer = create_buffer(&mut connection, "two").await;
    let split_snapshot = connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: first_leaf,
            direction: SplitDirection::Horizontal,
            new_buffer_id: second_buffer.id,
        }))
        .await
        .expect("split request succeeds");
    let split_snapshot = match split_snapshot {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let second_leaf = split_snapshot
        .session
        .focused_leaf_id
        .expect("split focuses new leaf");

    let focused = connection
        .request(&ClientMessage::Node(NodeRequest::Focus {
            request_id: new_request_id(),
            session_id,
            node_id: first_leaf,
        }))
        .await
        .expect("focus request succeeds");
    let focused = match focused {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(focused.session.focused_leaf_id, Some(first_leaf));

    let closed = connection
        .request(&ClientMessage::Node(NodeRequest::Close {
            request_id: new_request_id(),
            node_id: first_leaf,
        }))
        .await
        .expect("close request succeeds");
    let closed = match closed {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let root = root_node(&closed);
    let root_buffer = root
        .buffer_view
        .as_ref()
        .expect("single remaining pane collapses to the root buffer view");
    assert_eq!(root.id, second_leaf);
    assert_eq!(root_buffer.buffer_id, second_buffer.id);
    assert_eq!(closed.session.focused_leaf_id, Some(second_leaf));
    assert_eq!(
        get_buffer(&mut connection, first_buffer.id)
            .await
            .attachment_node_id,
        None
    );

    server.shutdown().await.expect("shutdown server");
}
