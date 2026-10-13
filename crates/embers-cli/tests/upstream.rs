//! A server's connection to another server through a command, here
//! `embers proxy` run directly where SSH would run it on another machine.

use embers_core::{MuxError, RequestId};
use embers_protocol::{
    ClientMessage, ServerEvent, ServerResponse, SessionRequest, SubscribeRequest,
};
use embers_server::Upstream;
use embers_test_support::{TestServer, cargo_bin_path};
use tokio::time::{Duration, timeout};

fn proxy_command(server: &TestServer) -> Vec<String> {
    vec![
        cargo_bin_path("embers").display().to_string(),
        "--socket".to_owned(),
        server.socket_path().display().to_string(),
        "proxy".to_owned(),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstream_requests_and_events_pass_through_the_proxy() {
    let server = TestServer::start().await.expect("start server");
    let mut upstream = Upstream::connect(&proxy_command(&server))
        .await
        .expect("connect through the proxy");

    let subscribe = ClientMessage::Subscribe(SubscribeRequest {
        request_id: RequestId(1),
        session_id: None,
    });
    match upstream.request(&subscribe).await.expect("subscribe") {
        ServerResponse::SubscriptionAck(_) => {}
        other => panic!("expected subscription ack, got {other:?}"),
    }

    let create = ClientMessage::Session(SessionRequest::Create {
        request_id: RequestId(2),
        name: "alpha".to_owned(),
    });
    let session_id = match upstream.request(&create).await.expect("create session") {
        ServerResponse::SessionSnapshot(response) => response.snapshot.session.id,
        other => panic!("expected session snapshot, got {other:?}"),
    };

    // The event may arrive before the response; either way it is kept.
    loop {
        let event = timeout(Duration::from_secs(10), upstream.next_event())
            .await
            .expect("event in time")
            .expect("read event")
            .expect("connection stays open");
        if let ServerEvent::SessionCreated(event) = event {
            assert_eq!(event.session.id, session_id);
            break;
        }
    }

    drop(upstream);
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn upstream_reports_what_a_failed_command_said() {
    let command = [
        "sh".to_owned(),
        "-c".to_owned(),
        "echo 'devbox: connection refused' >&2; exit 255".to_owned(),
    ];
    let error = match Upstream::connect(&command).await {
        Ok(_) => panic!("a command that exits at once cannot connect"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(matches!(error, MuxError::Transport(_)), "{message}");
    assert!(message.contains("devbox: connection refused"), "{message}");
    assert!(message.contains("255"), "{message}");
}

#[tokio::test]
async fn upstream_refuses_an_empty_command() {
    match Upstream::connect(&[]).await {
        Err(MuxError::InvalidInput(_)) => {}
        Err(other) => panic!("expected invalid input, got {other}"),
        Ok(_) => panic!("an empty command cannot connect"),
    }
}
