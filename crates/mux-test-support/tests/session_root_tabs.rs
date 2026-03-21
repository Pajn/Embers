use mux_core::{ErrorCode, new_request_id};
use mux_protocol::{
    BufferRecord, BufferRequest, ClientMessage, ServerResponse, SessionRequest,
    SessionSnapshotResponse, SessionsResponse, TabsRecord,
};
use mux_test_support::{TestConnection, TestServer};

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
    session_id: mux_core::SessionId,
) -> ServerResponse {
    connection
        .request(&ClientMessage::Session(SessionRequest::Get {
            request_id: new_request_id(),
            session_id,
        }))
        .await
        .expect("get session request succeeds")
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

async fn get_buffer(
    connection: &mut TestConnection,
    buffer_id: mux_core::BufferId,
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

fn root_tabs(snapshot: &mux_protocol::SessionSnapshot) -> TabsRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .and_then(|node| node.tabs.clone())
        .expect("session root snapshot includes tabs record")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_list_get_and_close_sessions_via_socket() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let alpha = create_session(&mut connection, "alpha").await;
    let beta = create_session(&mut connection, "beta").await;

    let list = connection
        .request(&ClientMessage::Session(SessionRequest::List {
            request_id: new_request_id(),
        }))
        .await
        .expect("list sessions request succeeds");
    let sessions = match list {
        ServerResponse::Sessions(SessionsResponse { sessions, .. }) => sessions,
        other => panic!("expected sessions response, got {other:?}"),
    };
    assert_eq!(sessions.len(), 2);
    assert!(
        sessions
            .iter()
            .any(|session| session.id == alpha.snapshot.session.id)
    );
    assert!(
        sessions
            .iter()
            .any(|session| session.id == beta.snapshot.session.id)
    );

    let fetched = get_session(&mut connection, alpha.snapshot.session.id).await;
    let fetched = match fetched {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(fetched.snapshot.session.name, "alpha");
    assert!(root_tabs(&fetched.snapshot).tabs.is_empty());

    let close = connection
        .request(&ClientMessage::Session(SessionRequest::Close {
            request_id: new_request_id(),
            session_id: alpha.snapshot.session.id,
            force: false,
        }))
        .await
        .expect("close session request succeeds");
    assert!(matches!(close, ServerResponse::Ok(_)));

    let missing = get_session(&mut connection, alpha.snapshot.session.id).await;
    match missing {
        ServerResponse::Error(error) => assert_eq!(error.error.code, ErrorCode::NotFound),
        other => panic!("expected not found error, got {other:?}"),
    }

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_select_rename_and_close_root_tabs_via_socket() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let session = create_session(&mut connection, "main").await;
    let session_id = session.snapshot.session.id;
    let root_node_id = session.snapshot.session.root_node_id;

    let first_buffer = create_buffer(&mut connection, "shell").await;
    let first_added = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id,
            title: "shell".to_owned(),
            buffer_id: Some(first_buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add first root tab request succeeds");
    let first_added = match first_added {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let first_tabs = root_tabs(&first_added.snapshot);
    let first_leaf = first_tabs.tabs[0].child_id;
    assert_eq!(first_added.snapshot.session.root_node_id, root_node_id);
    assert_eq!(first_tabs.active, 0);
    assert_eq!(first_tabs.tabs.len(), 1);

    let second_buffer = create_buffer(&mut connection, "logs").await;
    let second_added = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id,
            title: "logs".to_owned(),
            buffer_id: Some(second_buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add second root tab request succeeds");
    let second_added = match second_added {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let second_tabs = root_tabs(&second_added.snapshot);
    assert_eq!(second_added.snapshot.session.root_node_id, root_node_id);
    assert_eq!(second_tabs.active, 1);
    assert_eq!(second_tabs.tabs.len(), 2);

    let selected = connection
        .request(&ClientMessage::Session(SessionRequest::SelectRootTab {
            request_id: new_request_id(),
            session_id,
            index: 0,
        }))
        .await
        .expect("select root tab request succeeds");
    let selected = match selected {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(root_tabs(&selected.snapshot).active, 0);
    assert_eq!(selected.snapshot.session.focused_leaf_id, Some(first_leaf));

    let renamed = connection
        .request(&ClientMessage::Session(SessionRequest::RenameRootTab {
            request_id: new_request_id(),
            session_id,
            index: 0,
            title: "editor".to_owned(),
        }))
        .await
        .expect("rename root tab request succeeds");
    let renamed = match renamed {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(root_tabs(&renamed.snapshot).tabs[0].title, "editor");

    let closed_second = connection
        .request(&ClientMessage::Session(SessionRequest::CloseRootTab {
            request_id: new_request_id(),
            session_id,
            index: 1,
        }))
        .await
        .expect("close second root tab request succeeds");
    let closed_second = match closed_second {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    assert_eq!(root_tabs(&closed_second.snapshot).tabs.len(), 1);
    assert_eq!(
        get_buffer(&mut connection, second_buffer.id)
            .await
            .attachment_node_id,
        None
    );

    let closed_last = connection
        .request(&ClientMessage::Session(SessionRequest::CloseRootTab {
            request_id: new_request_id(),
            session_id,
            index: 0,
        }))
        .await
        .expect("close last root tab request succeeds");
    let closed_last = match closed_last {
        ServerResponse::SessionSnapshot(snapshot) => snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let final_tabs = root_tabs(&closed_last.snapshot);
    assert!(final_tabs.tabs.is_empty());
    assert_eq!(closed_last.snapshot.session.focused_leaf_id, None);
    assert_eq!(
        get_buffer(&mut connection, first_buffer.id)
            .await
            .attachment_node_id,
        None
    );

    server.shutdown().await.expect("shutdown server");
}
