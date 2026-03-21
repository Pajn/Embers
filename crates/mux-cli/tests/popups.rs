mod support;

use mux_test_support::{TestConnection, TestServer};

use support::{run_cli, session_snapshot_by_name, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn popup_commands_round_trip_through_cli() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "alpha"]);
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
