use embers_core::{NodeId, new_request_id};
use embers_protocol::{ClientMessage, InputRequest, SessionSnapshot};
use embers_test_support::{TestConnection, TestServer, acquire_test_lock};

use crate::support::{cli_command, run_cli, session_snapshot_by_name, stdout};

/// Start a session with one window split into two side-by-side panes and
/// return `(left, right)` pane ids.
fn side_by_side_panes(server: &TestServer) -> (u64, u64) {
    run_cli(server, ["new-session", "alpha"]);
    run_cli(server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);
    let split = run_cli(server, ["split-window", "--", "/bin/sh"]);
    let right = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns pane id");
    let listed = run_cli(server, ["list-panes"]);
    let left = stdout(&listed)
        .lines()
        .filter_map(|line| line.split('\t').next()?.parse::<u64>().ok())
        .find(|pane| *pane != right)
        .expect("other pane exists");
    (left, right)
}

fn split_sizes(snapshot: &SessionSnapshot, pane: u64) -> Vec<u16> {
    snapshot
        .nodes
        .iter()
        .find_map(|node| {
            node.split
                .as_ref()
                .filter(|split| split.child_ids.contains(&NodeId(pane)))
        })
        .expect("pane sits in a split")
        .sizes
        .clone()
}

/// Size each pane's PTY the way an attached client drawing the window at
/// 80x24 would: a tab bar row, then 39 and 40 columns either side of the
/// divider, each pane losing a row to its title.
async fn draw_at_80x24(connection: &mut TestConnection, snapshot: &SessionSnapshot) {
    let split = snapshot
        .nodes
        .iter()
        .find_map(|node| node.split.as_ref())
        .expect("window has a split");
    for (child, cols) in split.child_ids.iter().zip([39, 40]) {
        let buffer_id = snapshot
            .nodes
            .iter()
            .find(|node| node.id == *child)
            .and_then(|node| node.buffer_view.as_ref())
            .expect("split child is a pane")
            .buffer_id;
        connection
            .request(&ClientMessage::Input(InputRequest::Resize {
                request_id: new_request_id(),
                buffer_id,
                cols,
                rows: 22,
            }))
            .await
            .expect("resize pane pty");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_pane_moves_borders_in_cells_like_tmux() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    let (left, right) = side_by_side_panes(&server);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    draw_at_80x24(&mut connection, &snapshot).await;

    // The right pane is the split's last child: -L moves its left border left.
    run_cli(
        &server,
        ["resize-pane", "-t", &right.to_string(), "-L", "3"],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(split_sizes(&snapshot, right), vec![36, 43]);

    // -R with no adjustment moves one cell; the left pane's right border moves.
    run_cli(&server, ["resize-pane", "-t", &left.to_string(), "-R"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(split_sizes(&snapshot, right), vec![37, 42]);

    // There is no top/bottom split, so -U changes nothing.
    run_cli(&server, ["resize-pane", "-t", &left.to_string(), "-U", "5"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(split_sizes(&snapshot, right), vec![37, 42]);

    // Without -t the focused pane (the new right pane) is resized.
    run_cli(&server, ["resize-pane", "-R", "2"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(split_sizes(&snapshot, right), vec![39, 40]);

    run_cli(&server, ["resize-pane", "-t", &left.to_string(), "-Z"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(snapshot.session.zoomed_node_id, Some(NodeId(left)));
    run_cli(&server, ["resize-pane", "-t", &left.to_string(), "-Z"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(snapshot.session.zoomed_node_id, None);

    cli_command(&server)
        .args(["resize-pane", "-t", &left.to_string(), "-L", "-R"])
        .assert()
        .failure();
    cli_command(&server)
        .args(["resize-pane", "-t", &left.to_string()])
        .assert()
        .failure();

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronize_panes_toggles_per_window() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");
    side_by_side_panes(&server);
    run_cli(&server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");

    let synchronized = |snapshot: &SessionSnapshot| {
        let root = snapshot
            .nodes
            .iter()
            .find(|node| node.id == snapshot.session.root_node_id)
            .expect("root node");
        root.tabs
            .as_ref()
            .expect("root tabs")
            .tabs
            .iter()
            .map(|tab| tab.synchronize_panes)
            .collect::<Vec<_>>()
    };

    let toggled = run_cli(&server, ["synchronize-panes", "-t", "alpha:0"]);
    assert_eq!(stdout(&toggled).trim(), "on");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(synchronized(&snapshot), vec![true, false]);

    let on = run_cli(&server, ["synchronize-panes", "-t", "alpha:0", "on"]);
    assert_eq!(stdout(&on).trim(), "on");
    // Without -t the active (second) window is used.
    run_cli(&server, ["synchronize-panes", "on"]);
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(synchronized(&snapshot), vec![true, true]);

    let off = run_cli(&server, ["synchronize-panes", "-t", "alpha:0", "off"]);
    assert_eq!(stdout(&off).trim(), "off");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(synchronized(&snapshot), vec![false, true]);

    server.shutdown().await.expect("shutdown server");
}
