use embers_test_support::{TestConnection, TestServer};

use crate::support::{run_cli, session_snapshot_by_name, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn window_commands_round_trip_through_cli() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "alpha"]);
    let first = run_cli(
        &server,
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
    assert_eq!(stdout(&first).trim(), "0\teditor");

    let second = run_cli(
        &server,
        [
            "new-window",
            "-t",
            "alpha",
            "--title",
            "logs",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(stdout(&second).trim(), "1\tlogs");

    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t0\teditor\n1\t1\tlogs");

    run_cli(&server, ["select-window", "-t", "alpha:editor"]);
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t1\teditor\n1\t0\tlogs");

    run_cli(&server, ["rename-window", "-t", "alpha:editor", "ops"]);
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t1\tops\n1\t0\tlogs");

    run_cli(&server, ["rename-window", "-t", "alpha:logs", "tail"]);
    run_cli(&server, ["kill-window", "-t", "alpha:ops"]);

    // The window left keeps its name and its place in the window list.
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t1\ttail");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let root = snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .expect("root node exists");
    let tabs = root.tabs.as_ref().expect("one window keeps the root tabs");
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.tabs[0].title, "tail");

    // It is still addressed like any window.
    run_cli(&server, ["select-window", "-t", "alpha:tail"]);
    run_cli(&server, ["rename-window", "-t", "alpha:0", "last"]);
    let synchronized = run_cli(&server, ["synchronize-panes", "-t", "alpha:last", "on"]);
    assert_eq!(stdout(&synchronized).trim(), "on");
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t1\tlast");

    server.shutdown().await.expect("shutdown server");
}
