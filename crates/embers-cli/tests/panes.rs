mod support;

use std::time::Duration;

use embers_test_support::{TestConnection, TestServer};
use tokio::time::sleep;

use support::{run_cli, session_snapshot_by_name, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pane_commands_round_trip_through_cli() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "alpha"]);
    run_cli(
        &server,
        [
            "new-window",
            "-t",
            "alpha",
            "--title",
            "work",
            "--",
            "/bin/sh",
        ],
    );

    let split = run_cli(&server, ["split-window", "--", "/bin/sh"]);
    let new_pane_id = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns pane id");

    let listed = run_cli(&server, ["list-panes"]);
    let lines = stdout(&listed)
        .trim()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);

    let pane_ids = lines
        .iter()
        .map(|line| {
            line.split('\t')
                .next()
                .expect("pane id column")
                .parse::<u64>()
                .expect("pane id parses")
        })
        .collect::<Vec<_>>();
    assert!(pane_ids.contains(&new_pane_id));
    let other_pane_id = pane_ids
        .into_iter()
        .find(|pane_id| *pane_id != new_pane_id)
        .expect("other pane exists");

    run_cli(&server, ["select-pane", "-t", &other_pane_id.to_string()]);
    let listed = run_cli(&server, ["list-panes"]);
    assert!(
        stdout(&listed)
            .lines()
            .any(|line| line.starts_with(&format!("{other_pane_id}\t")) && line.contains("\t1\t"))
    );

    run_cli(
        &server,
        [
            "resize-pane",
            "-t",
            &other_pane_id.to_string(),
            "--sizes",
            "3,1",
        ],
    );

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let parent_split = snapshot
        .nodes
        .iter()
        .find(|node| {
            node.split.as_ref().is_some_and(|split| {
                split
                    .child_ids
                    .contains(&embers_core::NodeId(other_pane_id))
            })
        })
        .expect("parent split exists");
    assert_eq!(
        parent_split.split.as_ref().expect("split payload").sizes,
        vec![3, 1]
    );

    run_cli(
        &server,
        [
            "send-keys",
            "-t",
            &other_pane_id.to_string(),
            "--enter",
            "printf",
            "cli-pane\\n",
        ],
    );
    sleep(Duration::from_millis(100)).await;

    let captured = run_cli(&server, ["capture-pane", "-t", &other_pane_id.to_string()]);
    assert!(stdout(&captured).contains("cli-pane"));

    run_cli(&server, ["kill-pane", "-t", &other_pane_id.to_string()]);
    let listed = run_cli(&server, ["list-panes"]);
    assert_eq!(stdout(&listed).trim().lines().count(), 1);

    server.shutdown().await.expect("shutdown server");
}
