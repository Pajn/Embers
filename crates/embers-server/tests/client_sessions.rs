use std::num::NonZeroU64;

use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{
    ClientMessage, ClientRequest, ClientResponse, ClientsResponse, ProtocolClient, ServerEnvelope,
    ServerEvent, ServerResponse, SessionRequest, SessionSnapshotResponse, SubscribeRequest,
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
