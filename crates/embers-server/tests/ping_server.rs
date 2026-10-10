use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{
    ClientMessage, KillServerRequest, PingRequest, ProtocolClient, ServerEnvelope, ServerEvent,
    ServerResponse, SubscribeRequest,
};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn server_replies_to_ping() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");

    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");
    let response = client
        .request(&ClientMessage::Ping(PingRequest {
            request_id: RequestId(9),
            payload: "hello".to_owned(),
        }))
        .await
        .expect("request succeeds");

    match response {
        ServerResponse::Pong(pong) => assert_eq!(pong.payload, "hello"),
        other => panic!("expected pong, got {other:?}"),
    }

    handle.shutdown().await.expect("shutdown server");
}

/// Read what the server sends `client` until it closes the connection.
async fn read_until_closed(client: &mut ProtocolClient) -> Vec<ServerEnvelope> {
    let mut received = Vec::new();
    loop {
        match timeout(Duration::from_secs(10), client.recv())
            .await
            .expect("server closes the connection")
        {
            Ok(Some(envelope)) => received.push(envelope),
            Ok(None) | Err(_) => return received,
        }
    }
}

#[tokio::test]
async fn connections_are_told_when_the_server_stops_on_purpose() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");

    // An attached client subscribes; a bare connection does not.
    let mut subscribed = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect subscribed client");
    subscribed
        .request(&ClientMessage::Subscribe(SubscribeRequest {
            request_id: RequestId(1),
            session_id: None,
        }))
        .await
        .expect("subscribe");
    let mut bare = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect bare client");
    bare.request(&ClientMessage::Ping(PingRequest {
        request_id: RequestId(2),
        payload: String::new(),
    }))
    .await
    .expect("ping");

    let mut killer = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect killer");
    killer
        .request(&ClientMessage::KillServer(KillServerRequest {
            request_id: RequestId(3),
        }))
        .await
        .expect("kill-server succeeds");

    for (name, client) in [("subscribed", &mut subscribed), ("bare", &mut bare)] {
        let received = read_until_closed(client).await;
        assert_eq!(
            received.last(),
            Some(&ServerEnvelope::Event(ServerEvent::ServerShutdown)),
            "{name} client was not told the server stopped: {received:?}"
        );
    }
    handle.shutdown().await.expect("server finishes stopping");
}
