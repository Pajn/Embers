use std::time::Duration;

use embers_core::RequestId;
use embers_protocol::{BufferRequest, ClientMessage, ServerResponse};
use embers_test_support::{TestConnection, TestServer};
use tokio::time::sleep;

use crate::support::{run_cli, session_snapshot_by_name, stdout};

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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let captured = loop {
        let captured = run_cli(&server, ["capture-pane", "-t", &other_pane_id.to_string()]);
        if stdout(&captured).contains("cli-pane") {
            break captured;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for pane {} to render cli-pane",
            other_pane_id
        );
        sleep(Duration::from_millis(50)).await;
    };
    assert!(stdout(&captured).contains("cli-pane"));

    run_cli(&server, ["kill-pane", "-t", &other_pane_id.to_string()]);
    let listed = run_cli(&server, ["list-panes"]);
    assert_eq!(stdout(&listed).trim().lines().count(), 1);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_buffers_can_be_listed_and_attached_via_cli() {
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
    let target_pane_id = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns pane id");

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let detached = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: RequestId(1),
            title: Some("detached-tools".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
        }))
        .await
        .expect("buffer create succeeds");
    let detached_buffer_id = match detached {
        ServerResponse::Buffer(response) => response.buffer.id,
        other => panic!("expected buffer response, got {other:?}"),
    };

    let listed = run_cli(&server, ["list-buffers", "--detached"]);
    assert!(
        stdout(&listed)
            .lines()
            .filter(|line| line.starts_with(&format!("{detached_buffer_id}\t")))
            .any(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                fields
                    .get(1)
                    .is_some_and(|status| status == &"created" || status == &"running")
            })
    );

    run_cli(
        &server,
        [
            "attach-buffer",
            &detached_buffer_id.to_string(),
            "-t",
            &target_pane_id.to_string(),
        ],
    );

    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let moved_leaf = snapshot
        .nodes
        .iter()
        .find(|node| node.id == embers_core::NodeId(target_pane_id))
        .expect("target pane exists");
    assert_eq!(
        moved_leaf
            .buffer_view
            .as_ref()
            .expect("target is buffer view")
            .buffer_id,
        detached_buffer_id
    );

    let listed = run_cli(&server, ["list-buffers", "--attached"]);
    assert!(
        stdout(&listed)
            .lines()
            .any(|line| line.starts_with(&format!(
                "{detached_buffer_id}\trunning\tattached:{target_pane_id}\t"
            )))
    );

    let buffers = connection
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: RequestId(2),
            session_id: None,
            attached_only: false,
            detached_only: false,
        }))
        .await
        .expect("buffer list succeeds");
    let previous_buffer_id = match buffers {
        ServerResponse::Buffers(response) => response
            .buffers
            .into_iter()
            .find(|buffer| buffer.id != detached_buffer_id && buffer.attachment_node_id.is_none())
            .map(|buffer| buffer.id)
            .expect("previous pane buffer became detached"),
        other => panic!("expected buffers response, got {other:?}"),
    };

    let detached_again = run_cli(&server, ["list-buffers", "--detached"]);
    assert!(
        stdout(&detached_again)
            .lines()
            .any(|line| line.starts_with(&format!("{previous_buffer_id}\trunning\tdetached\t")))
    );

    server.shutdown().await.expect("shutdown server");
}
