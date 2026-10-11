use embers_core::{Size, new_request_id};
use embers_protocol::{ClientMessage, SessionRequest};
use embers_test_support::{TestConnection, TestServer};

use crate::support::{run_cli, session_snapshot_by_name, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn popup_commands_round_trip_through_cli() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "alpha"]);
    // A window keeps the session alive once the popup, its only other pane,
    // is gone.
    run_cli(&server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);
    let created = run_cli(
        &server,
        [
            "display-popup",
            "-t",
            "alpha",
            "--title",
            "scratch",
            "--x",
            "2",
            "--y",
            "1",
            "--width",
            "20",
            "--height",
            "6",
            "--",
            "/bin/sh",
        ],
    );
    let popup_id = stdout(&created)
        .trim()
        .parse::<u64>()
        .expect("display-popup returns popup id");

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(snapshot.floating.len(), 1);
    assert_eq!(u64::from(snapshot.floating[0].id), popup_id);

    run_cli(&server, ["kill-popup", "-t", &popup_id.to_string()]);

    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert!(snapshot.floating.is_empty());

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_popup_without_a_border_gives_the_command_the_whole_popup() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    // As an attached client would, so panes start at their drawn size.
    let session_id = session_snapshot_by_name(&mut connection, "alpha")
        .await
        .session
        .id;
    connection
        .request(&ClientMessage::Session(SessionRequest::SetViewport {
            request_id: new_request_id(),
            session_id,
            viewport: Size {
                width: 80,
                height: 24,
            },
        }))
        .await
        .expect("set the session viewport");
    let geometry = ["--x", "2", "--y", "1", "--width", "20", "--height", "6"];

    for (flag, border, pty_size) in [
        (None, true, (18, 3)),
        (Some("-B"), false, (20, 5)),
        (Some("--no-border"), false, (20, 5)),
    ] {
        let mut args = vec!["display-popup", "-t", "alpha"];
        args.extend(flag);
        args.extend(geometry);
        args.extend(["--", "/bin/sh"]);
        let popup_id = stdout(&run_cli(&server, args))
            .trim()
            .parse::<u64>()
            .expect("display-popup returns popup id");

        let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
        let popup = snapshot
            .floating
            .iter()
            .find(|popup| u64::from(popup.id) == popup_id)
            .expect("popup exists");
        assert_eq!(popup.border, border, "{flag:?}");
        // The command starts at the size it is drawn at: inside the frame,
        // or the whole popup, below the pane's title row.
        let root = snapshot
            .nodes
            .iter()
            .find(|node| node.id == popup.root_node_id)
            .and_then(|node| node.buffer_view.as_ref())
            .expect("popup shows a pane");
        let buffer = snapshot
            .buffers
            .iter()
            .find(|buffer| buffer.id == root.buffer_id)
            .expect("pane buffer exists");
        assert_eq!(
            (buffer.pty_size.cols, buffer.pty_size.rows),
            pty_size,
            "{flag:?}"
        );

        run_cli(&server, ["kill-popup", "-t", &popup_id.to_string()]);
    }

    server.shutdown().await.expect("shutdown server");
}
