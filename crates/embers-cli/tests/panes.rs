use std::time::Duration;

use embers_core::RequestId;
use embers_protocol::{BufferRequest, ClientMessage, InputRequest, ServerResponse};
use embers_test_support::{TestConnection, TestServer, acquire_test_lock};
use tokio::time::sleep;

use crate::support::{run_cli, session_snapshot_by_name, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pane_commands_round_trip_through_cli() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
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
    let _guard = acquire_test_lock().await.expect("acquire test lock");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffer_show_and_history_open_helper_buffers() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
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

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let leaf = snapshot
        .session
        .focused_leaf_id
        .expect("focused pane exists");
    let buffer_id = snapshot
        .nodes
        .iter()
        .find(|node| node.id == leaf)
        .and_then(|node| node.buffer_view.as_ref())
        .map(|view| view.buffer_id)
        .expect("focused pane buffer exists");

    run_cli(
        &server,
        [
            "send-keys",
            "-t",
            &leaf.to_string(),
            "--enter",
            "printf",
            "history-helper\\n",
        ],
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let captured = run_cli(&server, ["capture-pane", "-t", &leaf.to_string()]);
        if stdout(&captured).contains("history-helper") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for pane output"
        );
        sleep(Duration::from_millis(50)).await;
    }

    let shown = run_cli(&server, ["buffer", "show", &buffer_id.to_string()]);
    let shown_stdout = stdout(&shown);
    assert!(shown_stdout.contains(&format!("id\t{buffer_id}")));
    assert!(shown_stdout.contains("kind\tpty"));
    assert!(shown_stdout.contains(&format!("location\tsession:1\tnode:{leaf}")));

    let opened = run_cli(
        &server,
        [
            "buffer",
            "history",
            &buffer_id.to_string(),
            "--scope",
            "visible",
        ],
    );
    let opened_stdout = stdout(&opened).trim().to_owned();
    let helper_buffer_id = opened_stdout
        .split('\t')
        .next()
        .expect("helper buffer id column")
        .parse::<u64>()
        .expect("helper buffer id parses");

    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let helper = snapshot
        .buffers
        .iter()
        .find(|buffer| buffer.id.0 == helper_buffer_id)
        .expect("helper buffer exists in session");
    assert_eq!(helper.kind, embers_protocol::BufferRecordKind::Helper);
    assert!(helper.read_only);
    assert_eq!(helper.helper_source_buffer_id, Some(buffer_id));
    assert_eq!(
        helper.helper_scope,
        Some(embers_protocol::BufferHistoryScope::Visible)
    );

    let helper_capture = connection
        .request(&ClientMessage::Buffer(BufferRequest::Capture {
            request_id: RequestId(3),
            buffer_id: helper.id,
        }))
        .await
        .expect("capture helper succeeds");
    let helper_text = match helper_capture {
        ServerResponse::Snapshot(response) => response.lines.join("\n"),
        other => panic!("expected helper snapshot response, got {other:?}"),
    };
    assert!(helper_text.contains("history-helper"));

    let send = connection
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(4),
            buffer_id: helper.id,
            bytes: b"nope".to_vec(),
        }))
        .await
        .expect("helper send request returns a response");
    assert!(
        matches!(send, ServerResponse::Error(_)),
        "helper buffers reject input, got {send:?}"
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_commands_cover_zoom_swap_break_join_and_reorder() {
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
    let second_pane_id = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns pane id");

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let first_pane_id = snapshot
        .nodes
        .iter()
        .find(|node| node.buffer_view.as_ref().is_some() && node.id.0 != second_pane_id)
        .map(|node| node.id.0)
        .expect("first pane id exists");

    run_cli(&server, ["node", "zoom", &first_pane_id.to_string()]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(
        snapshot.session.zoomed_node_id,
        Some(embers_core::NodeId(first_pane_id))
    );

    run_cli(&server, ["node", "unzoom", "-t", "alpha"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(snapshot.session.zoomed_node_id, None);

    let parent_split_id = snapshot
        .nodes
        .iter()
        .find(|node| {
            node.split.as_ref().is_some_and(|split| {
                split
                    .child_ids
                    .contains(&embers_core::NodeId(first_pane_id))
                    && split
                        .child_ids
                        .contains(&embers_core::NodeId(second_pane_id))
            })
        })
        .map(|node| node.id)
        .expect("parent split exists");

    run_cli(
        &server,
        [
            "node",
            "swap",
            &first_pane_id.to_string(),
            &second_pane_id.to_string(),
        ],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let split = snapshot
        .nodes
        .iter()
        .find(|node| node.id == parent_split_id)
        .and_then(|node| node.split.as_ref())
        .expect("split still exists");
    assert_eq!(split.child_ids[0], embers_core::NodeId(second_pane_id));

    run_cli(
        &server,
        [
            "node",
            "move-before",
            &first_pane_id.to_string(),
            &second_pane_id.to_string(),
        ],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let split = snapshot
        .nodes
        .iter()
        .find(|node| node.id == parent_split_id)
        .and_then(|node| node.split.as_ref())
        .expect("split still exists after reorder");
    assert_eq!(split.child_ids[0], embers_core::NodeId(first_pane_id));

    run_cli(
        &server,
        [
            "node",
            "break",
            &second_pane_id.to_string(),
            "--to",
            "floating",
        ],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(snapshot.floating.len(), 1);

    let detached = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: RequestId(10),
            title: Some("notes".to_owned()),
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

    run_cli(
        &server,
        [
            "node",
            "join-buffer",
            &first_pane_id.to_string(),
            &detached_buffer_id.to_string(),
            "--as",
            "tab-after",
        ],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|node| node.tabs.as_ref().is_some_and(|tabs| tabs.tabs.len() >= 2)),
        "join-buffer created or reused a tabs container"
    );

    server.shutdown().await.expect("shutdown server");
}
