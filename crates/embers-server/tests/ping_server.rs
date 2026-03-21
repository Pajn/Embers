use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{ClientMessage, PingRequest, ProtocolClient, ServerResponse};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;

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
