//! `display-message`: format expansion with `-p`, and delivery to the
//! clients showing a session.

use std::time::Duration;

use embers_core::new_request_id;
use embers_protocol::{ClientMessage, ClientRequest, ServerEvent, ServerResponse};
use embers_test_support::{TestConnection, TestServer};

use crate::support::{cli_command, run_cli, session_snapshot_by_name, stdout};

async fn alpha_with_window(server: &TestServer) -> String {
    run_cli(server, ["new-session", "alpha"]);
    run_cli(
        server,
        [
            "new-window",
            "-t",
            "alpha",
            "--title",
            "editor",
            "--",
            "/bin/sh",
        ],
    );
    let panes = stdout(&run_cli(server, ["list-panes", "-t", "alpha:0"]));
    panes.split('\t').next().expect("a pane").to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_message_print_expands_formats() {
    let server = TestServer::start().await.expect("start server");
    let pane = alpha_with_window(&server).await;

    let printed = run_cli(
        &server,
        [
            "display-message",
            "-p",
            "-t",
            &pane,
            "[#S] #I:#W pane #{node_id} ## #{pane_current_command}",
        ],
    );
    assert_eq!(
        stdout(&printed).trim_end(),
        format!("[alpha] 0:editor pane {pane} # sh")
    );

    let numbered = run_cli(
        &server,
        [
            "--base-index",
            "1",
            "display-message",
            "-p",
            "-t",
            &pane,
            "#I",
        ],
    );
    assert_eq!(stdout(&numbered).trim_end(), "1");

    // Words are joined like tmux joins a quoted message.
    let joined = run_cli(&server, ["display-message", "-p", "hello", "#S"]);
    assert_eq!(stdout(&joined).trim_end(), "hello alpha");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_message_reaches_clients_showing_the_session() {
    let server = TestServer::start().await.expect("start server");
    let pane = alpha_with_window(&server).await;

    // Without an attached client there is nowhere to show the message.
    let nobody = cli_command(&server)
        .args(["display-message", "-t", &pane, "hi"])
        .output()
        .expect("cli runs");
    assert!(!nobody.status.success());
    let stderr = String::from_utf8_lossy(&nobody.stderr);
    assert!(stderr.contains("no client"), "stderr: {stderr}");

    let mut client = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    client.subscribe(None).await.expect("subscribe");
    let session_id = session_snapshot_by_name(&mut client, "alpha")
        .await
        .session
        .id;
    let response = client
        .request(&ClientMessage::Client(ClientRequest::Switch {
            request_id: new_request_id(),
            client_id: None,
            session_id,
            node_id: None,
        }))
        .await
        .expect("switch");
    let client_id = match response {
        ServerResponse::Client(response) => response.client.id,
        other => panic!("unexpected response {other:?}"),
    };

    run_cli(&server, ["display-message", "-d", "1500", "#S says hi"]);
    let event = client
        .wait_for_event(Duration::from_secs(5), |event| {
            matches!(event, ServerEvent::DisplayMessage(_))
        })
        .await
        .expect("display message event");
    let ServerEvent::DisplayMessage(event) = event else {
        unreachable!()
    };
    assert_eq!(event.message, "alpha says hi");
    assert_eq!(event.duration_ms, 1500);

    run_cli(
        &server,
        ["display-message", "-c", &client_id.to_string(), "direct"],
    );
    let event = client
        .wait_for_event(Duration::from_secs(5), |event| {
            matches!(event, ServerEvent::DisplayMessage(_))
        })
        .await
        .expect("display message event");
    let ServerEvent::DisplayMessage(event) = event else {
        unreachable!()
    };
    assert_eq!(event.message, "direct");
    assert_eq!(event.duration_ms, 0);

    server.shutdown().await.expect("shutdown server");
}
