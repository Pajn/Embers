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

fn tabs_record(snapshot: &SessionSnapshot, node_id: embers_core::NodeId) -> TabsRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .and_then(|node| node.tabs.clone())
        .expect("snapshot includes tabs node")
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
async fn nested_tab_mutations_round_trip_through_socket() {
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
    let outer_split_id = root_tabs(&split_snapshot).tabs[0].child_id;
    let second_leaf = split_snapshot
        .session
        .focused_leaf_id
        .expect("split focuses second leaf");

    let wrapped = connection
        .request(&ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: new_request_id(),
            node_id: second_leaf,
            title: "nested".to_owned(),
        }))
        .await
        .expect("wrap request succeeds");
    let wrapped = match wrapped {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let tabs_id = split_record(&wrapped, outer_split_id).child_ids[1];

    let fourth_buffer = create_buffer(&mut connection, "four").await;
    let split_inner = connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: second_leaf,
            direction: SplitDirection::Vertical,
            new_buffer_id: fourth_buffer.id,
        }))
        .await
        .expect("inner split request succeeds");
    let split_inner = match split_inner {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let inner_split_id = tabs_record(&split_inner, tabs_id).tabs[0].child_id;
    let fourth_leaf = split_inner
        .session
        .focused_leaf_id
        .expect("inner split focuses newest leaf");

    let third_buffer = create_buffer(&mut connection, "three").await;
    let added = connection
        .request(&ClientMessage::Node(NodeRequest::AddTab {
            request_id: new_request_id(),
            tabs_node_id: tabs_id,
            title: "other".to_owned(),
            buffer_id: Some(third_buffer.id),
            child_node_id: None,
            index: 1,
        }))
        .await
        .expect("add nested tab request succeeds");
    let added = match added {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let third_leaf = added
        .session
        .focused_leaf_id
        .expect("new nested tab focuses new leaf");

    let selected_first = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: tabs_id,
            index: 0,
        }))
        .await
        .expect("select first nested tab request succeeds");
    let selected_first = match selected_first {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(selected_first.session.focused_leaf_id, Some(fourth_leaf));

    let selected_second = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: tabs_id,
            index: 1,
        }))
        .await
        .expect("select second nested tab request succeeds");
    let selected_second = match selected_second {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(selected_second.session.focused_leaf_id, Some(third_leaf));

    let closed = connection
        .request(&ClientMessage::Node(NodeRequest::Close {
            request_id: new_request_id(),
            node_id: third_leaf,
        }))
        .await
        .expect("close nested tab node request succeeds");
    let closed = match closed {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(closed.session.focused_leaf_id, Some(fourth_leaf));
    assert_eq!(
        split_record(&closed, outer_split_id).child_ids[1],
        inner_split_id
    );
    assert_eq!(
        get_buffer(&mut connection, third_buffer.id)
            .await
            .attachment_node_id,
        None
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_tree_returns_nested_tab_structure() {
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
    let second_leaf = split_snapshot
        .session
        .focused_leaf_id
        .expect("split focuses second leaf");

    let wrapped = connection
        .request(&ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: new_request_id(),
            node_id: second_leaf,
            title: "nested".to_owned(),
        }))
        .await
        .expect("wrap request succeeds");
    let wrapped = match wrapped {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let tabs_id = split_record(&wrapped, outer_split_id).child_ids[1];

    let third_buffer = create_buffer(&mut connection, "three").await;
    connection
        .request(&ClientMessage::Node(NodeRequest::AddTab {
            request_id: new_request_id(),
            tabs_node_id: tabs_id,
            title: "other".to_owned(),
            buffer_id: Some(third_buffer.id),
            child_node_id: None,
            index: 1,
        }))
        .await
        .expect("add nested tab request succeeds");

    let tree = connection
        .request(&ClientMessage::Node(NodeRequest::GetTree {
            request_id: new_request_id(),
            session_id,
        }))
        .await
        .expect("get tree request succeeds");
    let tree = match tree {
        ServerResponse::SessionSnapshot(snapshot) => snapshot.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };

    let outer_split = split_record(&tree, outer_split_id);
    assert_eq!(outer_split.child_ids[1], tabs_id);
    let nested_tabs = tabs_record(&tree, tabs_id);
    assert_eq!(nested_tabs.tabs.len(), 2);
    assert_eq!(nested_tabs.tabs[0].title, "nested");
    assert_eq!(nested_tabs.tabs[1].title, "other");

    server.shutdown().await.expect("shutdown server");
}
