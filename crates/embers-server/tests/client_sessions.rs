use std::num::NonZeroU64;
use std::time::Instant;

use embers_core::{BufferId, RequestId, init_test_tracing};
use embers_protocol::{
    BufferRequest, BufferResponse, ClientMessage, ClientRequest, ClientResponse, ClientsResponse,
    InputRequest, ProtocolClient, ServerEnvelope, ServerEvent, ServerResponse, SessionRequest,
    SessionSnapshotResponse, SnapshotResponse, SubscribeRequest,
};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, timeout};

async fn request_session_snapshot(
    client: &mut ProtocolClient,
    request: SessionRequest,
) -> SessionSnapshotResponse {
    match client
        .request(&ClientMessage::Session(request))
        .await
        .expect("session request succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response,
        other => panic!("expected session snapshot response, got {other:?}"),
    }
}

async fn request_client(client: &mut ProtocolClient, request: ClientRequest) -> ClientResponse {
    match client
        .request(&ClientMessage::Client(request))
        .await
        .expect("client request succeeds")
    {
        ServerResponse::Client(response) => response,
        other => panic!("expected client response, got {other:?}"),
    }
}

async fn request_clients(client: &mut ProtocolClient, request: ClientRequest) -> ClientsResponse {
    match client
        .request(&ClientMessage::Client(request))
        .await
        .expect("client list request succeeds")
    {
        ServerResponse::Clients(response) => response,
        other => panic!("expected clients response, got {other:?}"),
    }
}

async fn request_buffer(client: &mut ProtocolClient, request: BufferRequest) -> BufferResponse {
    match client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer request succeeds")
    {
        ServerResponse::Buffer(response) => response,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn capture_buffer(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
) -> SnapshotResponse {
    match client
        .request(&ClientMessage::Buffer(BufferRequest::Capture {
            request_id,
            buffer_id,
        }))
        .await
        .expect("capture request succeeds")
    {
        ServerResponse::Snapshot(response) => response,
        other => panic!("expected snapshot response, got {other:?}"),
    }
}

async fn wait_for_snapshot_line(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    expected: &str,
) -> SnapshotResponse {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = capture_buffer(client, request_id, buffer_id).await;
        if snapshot.lines.iter().any(|line| line.contains(expected)) {
            return snapshot;
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!(
        "capture for buffer {buffer_id} did not contain expected line '{expected}' before timeout"
    );
}

async fn recv_event(client: &mut ProtocolClient) -> ServerEvent {
    let envelope = timeout(Duration::from_secs(2), client.recv())
        .await
        .expect("event arrives before timeout")
        .expect("receive succeeds")
        .expect("server kept connection open");
    match envelope {
        ServerEnvelope::Event(event) => event,
        other => panic!("expected event envelope, got {other:?}"),
    }
}

#[tokio::test]
async fn closing_session_clears_client_binding_and_retires_session_subscriptions() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");

    let mut client_a = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client A");
    let mut client_b = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client B");

    let main_id = request_session_snapshot(
        &mut client_a,
        SessionRequest::Create {
            request_id: RequestId(1),
            name: "main".to_owned(),
        },
    )
    .await
    .snapshot
    .session
    .id;
    let _ops_id = request_session_snapshot(
        &mut client_a,
        SessionRequest::Create {
            request_id: RequestId(2),
            name: "ops".to_owned(),
        },
    )
    .await
    .snapshot
    .session
    .id;

    let client_a_id = request_client(
        &mut client_a,
        ClientRequest::Get {
            request_id: RequestId(3),
            client_id: None,
        },
    )
    .await
    .client
    .id;
    let client_b_id = request_client(
        &mut client_b,
        ClientRequest::Get {
            request_id: RequestId(4),
            client_id: None,
        },
    )
    .await
    .client
    .id;

    let switched = request_client(
        &mut client_a,
        ClientRequest::Switch {
            request_id: RequestId(5),
            client_id: None,
            session_id: main_id,
        },
    )
    .await
    .client;
    assert_eq!(switched.id, client_a_id);
    assert_eq!(switched.current_session_id, Some(main_id));

    let subscribed = client_b
        .request(&ClientMessage::Subscribe(SubscribeRequest {
            request_id: RequestId(6),
            session_id: Some(main_id),
        }))
        .await
        .expect("subscribe request succeeds");
    assert!(matches!(subscribed, ServerResponse::SubscriptionAck(_)));

    let close = client_b
        .request(&ClientMessage::Session(SessionRequest::Close {
            request_id: RequestId(7),
            session_id: main_id,
            force: false,
        }))
        .await
        .expect("close session request succeeds");
    assert!(matches!(close, ServerResponse::Ok(_)));

    let first_event = recv_event(&mut client_b).await;
    let second_event = recv_event(&mut client_b).await;
    assert!(matches!(
        first_event,
        ServerEvent::SessionClosed(event) if event.session_id == main_id
    ));
    assert!(matches!(
        second_event,
        ServerEvent::ClientChanged(event)
            if event.client.id == client_a_id
                && event.client.current_session_id.is_none()
                && event.previous_session_id == Some(main_id)
    ));

    let maybe_extra = timeout(Duration::from_millis(200), client_b.recv()).await;
    assert!(maybe_extra.is_err(), "unexpected extra event after close");

    let refreshed_a = request_client(
        &mut client_b,
        ClientRequest::Get {
            request_id: RequestId(8),
            client_id: Some(NonZeroU64::new(client_a_id).expect("non-zero client id")),
        },
    )
    .await
    .client;
    assert_eq!(refreshed_a.current_session_id, None);

    let refreshed_b = request_client(
        &mut client_b,
        ClientRequest::Get {
            request_id: RequestId(9),
            client_id: None,
        },
    )
    .await
    .client;
    assert_eq!(refreshed_b.id, client_b_id);
    assert_eq!(refreshed_b.subscribed_session_ids, Vec::new());

    let clients = request_clients(
        &mut client_b,
        ClientRequest::List {
            request_id: RequestId(10),
        },
    )
    .await
    .clients;
    for client in clients {
        assert_ne!(client.current_session_id, Some(main_id));
        assert!(
            !client.subscribed_session_ids.contains(&main_id),
            "stale session subscription leaked into list-clients for client {}",
            client.id
        );
    }

    handle.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn concurrent_input_from_multiple_clients_reaches_shared_buffer() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");

    let mut client_a = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client A");
    let mut client_b = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client B");

    let buffer = request_buffer(
        &mut client_a,
        BufferRequest::Create {
            request_id: RequestId(21),
            title: Some("shared".to_owned()),
            command: vec![
                "/bin/sh".to_owned(),
                "-lc".to_owned(),
                "printf 'ready\\n'; while IFS= read -r line; do printf 'seen:%s\\n' \"$line\"; done"
                    .to_owned(),
            ],
            cwd: None,
            env: Default::default(),
        },
    )
    .await
    .buffer;
    let buffer_id = buffer.id;

    let _ = wait_for_snapshot_line(&mut client_a, RequestId(22), buffer_id, "ready").await;

    let send_a_message = ClientMessage::Input(InputRequest::Send {
        request_id: RequestId(23),
        buffer_id,
        bytes: b"from-a\n".to_vec(),
    });
    let send_b_message = ClientMessage::Input(InputRequest::Send {
        request_id: RequestId(24),
        buffer_id,
        bytes: b"from-b\n".to_vec(),
    });
    let send_a = client_a.request(&send_a_message);
    let send_b = client_b.request(&send_b_message);
    let (response_a, response_b) = tokio::join!(send_a, send_b);
    assert!(matches!(
        response_a.expect("client A input succeeds"),
        ServerResponse::Ok(_)
    ));
    assert!(matches!(
        response_b.expect("client B input succeeds"),
        ServerResponse::Ok(_)
    ));

    let capture =
        wait_for_snapshot_line(&mut client_a, RequestId(25), buffer_id, "seen:from-a").await;
    let capture_text = capture.lines.join("\n");
    if capture_text.contains("seen:from-b") {
        handle.shutdown().await.expect("shutdown server");
        return;
    }

    let capture =
        wait_for_snapshot_line(&mut client_a, RequestId(26), buffer_id, "seen:from-b").await;
    let capture_text = capture.lines.join("\n");
    assert!(
        capture_text.contains("seen:from-a"),
        "capture should retain both client inputs, got {capture_text:?}"
    );

    handle.shutdown().await.expect("shutdown server");
}
