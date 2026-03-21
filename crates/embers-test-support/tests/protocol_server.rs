use std::time::Duration;

use embers_core::{ErrorCode, MuxError, RequestId, SessionId, new_request_id};
use embers_protocol::{
    ClientMessage, FrameType, NodeRequest, PingRequest, RawFrame, ServerEnvelope, ServerEvent,
    ServerResponse, SessionRequest, decode_server_envelope, encode_client_message, read_frame,
    write_frame,
};
use embers_test_support::{TestConnection, TestServer};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::time::sleep;

async fn create_session(
    connection: &mut TestConnection,
    name: &str,
) -> embers_protocol::SessionSnapshotResponse {
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

fn expect_error(
    response: ServerResponse,
    request_id: Option<RequestId>,
    code: ErrorCode,
) -> String {
    match response {
        ServerResponse::Error(error) => {
            assert_eq!(error.request_id, request_id);
            assert_eq!(error.error.code, code);
            error.error.message
        }
        other => panic!("expected error response, got {other:?}"),
    }
}

fn encode_frame_bytes(frame: &RawFrame) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(13 + frame.payload.len());
    bytes.extend_from_slice(&(frame.payload.len() as u32).to_le_bytes());
    bytes.push(frame.frame_type as u8);
    bytes.extend_from_slice(&u64::from(frame.request_id).to_le_bytes());
    bytes.extend_from_slice(&frame.payload);
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscriptions_fan_out_to_multiple_clients_with_session_filters() {
    let server = TestServer::start().await.expect("start server");
    let mut actor = TestConnection::connect(server.socket_path())
        .await
        .expect("connect actor");
    let mut global = TestConnection::connect(server.socket_path())
        .await
        .expect("connect global subscriber");
    let mut scoped = TestConnection::connect(server.socket_path())
        .await
        .expect("connect scoped subscriber");
    let mut other_scope = TestConnection::connect(server.socket_path())
        .await
        .expect("connect other scoped subscriber");

    let main_session = create_session(&mut actor, "main").await;
    let other_session = create_session(&mut actor, "other").await;
    let main_session_id = main_session.snapshot.session.id;
    let other_session_id = other_session.snapshot.session.id;

    global.subscribe(None).await.expect("subscribe globally");
    scoped
        .subscribe(Some(main_session_id))
        .await
        .expect("subscribe to main session");
    other_scope
        .subscribe(Some(other_session_id))
        .await
        .expect("subscribe to other session");

    let close_request_id = RequestId(41);
    let close_response = actor
        .request(&ClientMessage::Session(SessionRequest::Close {
            request_id: close_request_id,
            session_id: main_session_id,
            force: false,
        }))
        .await
        .expect("close session request succeeds");
    assert!(matches!(close_response, ServerResponse::Ok(_)));

    let global_event = global
        .wait_for_event(Duration::from_secs(1), |event| {
            matches!(
                event,
                ServerEvent::SessionClosed(closed) if closed.session_id == main_session_id
            )
        })
        .await
        .expect("global subscriber receives session close");
    assert!(matches!(
        global_event,
        ServerEvent::SessionClosed(closed) if closed.session_id == main_session_id
    ));

    let scoped_event = scoped
        .wait_for_event(Duration::from_secs(1), |event| {
            matches!(
                event,
                ServerEvent::SessionClosed(closed) if closed.session_id == main_session_id
            )
        })
        .await
        .expect("scoped subscriber receives matching session close");
    assert!(matches!(
        scoped_event,
        ServerEvent::SessionClosed(closed) if closed.session_id == main_session_id
    ));

    let other_scope_error = other_scope
        .wait_for_event(Duration::from_millis(200), |event| {
            matches!(
                event,
                ServerEvent::SessionClosed(closed) if closed.session_id == main_session_id
            )
        })
        .await
        .expect_err("non-matching scoped subscriber should not receive the event");
    assert!(matches!(other_scope_error, MuxError::Timeout(_)));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn fragmented_request_frames_round_trip_and_preserve_correlation_id() {
    let server = TestServer::start().await.expect("start server");
    let mut stream = UnixStream::connect(server.socket_path())
        .await
        .expect("connect raw client");

    let request_id = RequestId(52);
    let payload = encode_client_message(&ClientMessage::Ping(PingRequest {
        request_id,
        payload: "fragmented".to_owned(),
    }))
    .expect("encode ping request");
    let frame = RawFrame::new(FrameType::Request, request_id, payload);

    for chunk in encode_frame_bytes(&frame).chunks(3) {
        stream.write_all(chunk).await.expect("write request chunk");
        tokio::task::yield_now().await;
    }

    let response_frame = read_frame(&mut stream)
        .await
        .expect("read response frame")
        .expect("response frame");
    assert_eq!(response_frame.frame_type, FrameType::Response);
    assert_eq!(response_frame.request_id, request_id);

    match decode_server_envelope(&response_frame.payload).expect("decode response payload") {
        ServerEnvelope::Response(ServerResponse::Pong(pong)) => {
            assert_eq!(pong.request_id, request_id);
            assert_eq!(pong.payload, "fragmented");
        }
        other => panic!("expected pong response, got {other:?}"),
    }

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn malformed_payloads_return_protocol_violation_errors() {
    let server = TestServer::start().await.expect("start server");
    let mut stream = UnixStream::connect(server.socket_path())
        .await
        .expect("connect raw client");

    let request_id = RequestId(61);
    let malformed = RawFrame::new(FrameType::Request, request_id, vec![0, 1, 2, 3, 4]);
    write_frame(&mut stream, &malformed)
        .await
        .expect("write malformed request");

    let response_frame = read_frame(&mut stream)
        .await
        .expect("read response frame")
        .expect("response frame");
    assert_eq!(response_frame.frame_type, FrameType::Response);
    assert_eq!(response_frame.request_id, request_id);

    match decode_server_envelope(&response_frame.payload).expect("decode response payload") {
        ServerEnvelope::Response(ServerResponse::Error(error)) => {
            assert_eq!(error.request_id, Some(request_id));
            assert_eq!(error.error.code, ErrorCode::ProtocolViolation);
        }
        other => panic!("expected protocol violation response, got {other:?}"),
    }

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn typed_errors_cover_invalid_ids_and_impossible_mutations() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect client");

    let missing_request_id = RequestId(71);
    let missing_response = connection
        .request(&ClientMessage::Session(SessionRequest::Get {
            request_id: missing_request_id,
            session_id: SessionId(999),
        }))
        .await
        .expect("missing session request returns response");
    expect_error(
        missing_response,
        Some(missing_request_id),
        ErrorCode::NotFound,
    );

    let session = create_session(&mut connection, "empty").await;
    let invalid_focus_request_id = RequestId(72);
    let invalid_focus_response = connection
        .request(&ClientMessage::Node(NodeRequest::Focus {
            request_id: invalid_focus_request_id,
            session_id: session.snapshot.session.id,
            node_id: session.snapshot.session.root_node_id,
        }))
        .await
        .expect("invalid focus request returns response");
    let message = expect_error(
        invalid_focus_response,
        Some(invalid_focus_request_id),
        ErrorCode::InvalidRequest,
    );
    assert!(message.contains("no focusable leaf"));

    let invalid_move_request_id = RequestId(73);
    let invalid_move_response = connection
        .request(&ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: invalid_move_request_id,
            buffer_id: embers_core::BufferId(1),
            target_leaf_node_id: session.snapshot.session.root_node_id,
        }))
        .await
        .expect("invalid move request returns response");
    expect_error(
        invalid_move_response,
        Some(invalid_move_request_id),
        ErrorCode::InvalidRequest,
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_subscribers_are_cleaned_up_without_breaking_remaining_clients() {
    let server = TestServer::start().await.expect("start server");
    let mut actor = TestConnection::connect(server.socket_path())
        .await
        .expect("connect actor");
    let mut surviving_subscriber = TestConnection::connect(server.socket_path())
        .await
        .expect("connect surviving subscriber");
    let mut disconnected_subscriber = TestConnection::connect(server.socket_path())
        .await
        .expect("connect subscriber to disconnect");

    let session = create_session(&mut actor, "cleanup").await;
    let session_id = session.snapshot.session.id;

    surviving_subscriber
        .subscribe(Some(session_id))
        .await
        .expect("subscribe surviving client");
    disconnected_subscriber
        .subscribe(Some(session_id))
        .await
        .expect("subscribe client to disconnect");

    drop(disconnected_subscriber);
    sleep(Duration::from_millis(50)).await;

    let close_response = actor
        .request(&ClientMessage::Session(SessionRequest::Close {
            request_id: RequestId(81),
            session_id,
            force: false,
        }))
        .await
        .expect("close session succeeds");
    assert!(matches!(close_response, ServerResponse::Ok(_)));

    let event = surviving_subscriber
        .wait_for_event(Duration::from_secs(1), |server_event| {
            matches!(
                server_event,
                ServerEvent::SessionClosed(closed) if closed.session_id == session_id
            )
        })
        .await
        .expect("surviving subscriber receives session close");
    assert!(matches!(
        event,
        ServerEvent::SessionClosed(closed) if closed.session_id == session_id
    ));

    let ping = actor
        .ping("still-alive")
        .await
        .expect("server stays usable");
    assert_eq!(ping, "still-alive");

    server.shutdown().await.expect("shutdown server");
}
