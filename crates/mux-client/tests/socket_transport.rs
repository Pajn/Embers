use mux_client::{SocketTransport, Transport};
use mux_core::RequestId;
use mux_protocol::{
    ClientMessage, PingRequest, ServerEvent, ServerResponse, SessionRequest, SubscribeRequest,
};
use mux_test_support::{TestConnection, TestServer};

#[tokio::test]
async fn socket_transport_sends_requests_and_receives_events() {
    let server = TestServer::start().await.expect("server starts");
    let transport = SocketTransport::connect(server.socket_path())
        .await
        .expect("transport connects");

    let pong = transport
        .request(ClientMessage::Ping(PingRequest {
            request_id: RequestId(1),
            payload: "phase10".to_owned(),
        }))
        .await
        .expect("ping succeeds");
    assert_eq!(
        pong,
        ServerResponse::Pong(mux_protocol::PingResponse {
            request_id: RequestId(1),
            payload: "phase10".to_owned(),
        })
    );

    let subscribe = transport
        .request(ClientMessage::Subscribe(SubscribeRequest {
            request_id: RequestId(2),
            session_id: None,
        }))
        .await
        .expect("subscribe succeeds");
    assert!(matches!(subscribe, ServerResponse::SubscriptionAck(_)));

    let mut actor = TestConnection::connect(server.socket_path())
        .await
        .expect("actor connects");
    let created = actor
        .request(&ClientMessage::Session(SessionRequest::Create {
            request_id: RequestId(3),
            name: "alpha".to_owned(),
        }))
        .await
        .expect("session creation succeeds");
    assert!(matches!(created, ServerResponse::SessionSnapshot(_)));

    let event = transport.next_event().await.expect("event arrives");
    assert!(matches!(event, ServerEvent::SessionCreated(_)));

    server.shutdown().await.expect("server shuts down");
}
