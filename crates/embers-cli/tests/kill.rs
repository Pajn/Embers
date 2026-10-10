//! `kill-pane`, `kill-window`, `kill-session` and `kill-popup` end the programs
//! they show, like tmux; `--detach` closes the view and keeps them running.

use embers_core::{BufferId, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferRequest, ClientMessage, ServerResponse,
};
use embers_test_support::{TestConnection, TestServer};
use tokio::time::{Duration, Instant, sleep};

use crate::support::{run_cli, session_snapshot_by_name, stdout};

const LONG_RUNNING: &str = "sleep 60";

/// Returns whether `pid` names a live process (zombies count as gone).
fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

async fn get_buffer(connection: &mut TestConnection, buffer_id: BufferId) -> Option<BufferRecord> {
    match connection
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("get buffer round-trips")
    {
        ServerResponse::Buffer(response) => Some(response.buffer),
        ServerResponse::Error(_) => None,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

/// The running buffers shown in session `name`, with their pids.
async fn session_processes(connection: &mut TestConnection, name: &str) -> Vec<(BufferId, u32)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session_snapshot_by_name(connection, name).await;
        let processes = snapshot
            .buffers
            .iter()
            .filter_map(|buffer| buffer.pid.map(|pid| (buffer.id, pid)))
            .collect::<Vec<_>>();
        if processes.len() == snapshot.buffers.len() {
            return processes;
        }
        assert!(Instant::now() < deadline, "buffers did not report pids");
        sleep(Duration::from_millis(25)).await;
    }
}

async fn assert_killed(connection: &mut TestConnection, processes: &[(BufferId, u32)]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    for &(buffer_id, pid) in processes {
        while process_alive(pid) || get_buffer(connection, buffer_id).await.is_some() {
            assert!(
                Instant::now() < deadline,
                "buffer {buffer_id} (pid {pid}) survived the kill"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }
}

fn new_window(server: &TestServer, title: &str) {
    run_cli(
        server,
        [
            "new-window",
            "-t",
            "alpha",
            "--title",
            title,
            "--",
            "/bin/sh",
            "-c",
            LONG_RUNNING,
        ],
    );
}

/// Splits the focused pane of the only session; returns the new pane's id.
fn split(server: &TestServer) -> String {
    let split = run_cli(
        server,
        ["split-window", "--", "/bin/sh", "-c", LONG_RUNNING],
    );
    stdout(&split).trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_pane_ends_the_pane_program() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "work");
    let before = session_processes(&mut connection, "alpha").await;
    let pane = split(&server);
    let killed = session_processes(&mut connection, "alpha")
        .await
        .into_iter()
        .filter(|process| !before.contains(process))
        .collect::<Vec<_>>();
    assert_eq!(killed.len(), 1);

    run_cli(&server, ["kill-pane", "-t", &pane]);

    assert_killed(&mut connection, &killed).await;
    let (_, survivor) = before[0];
    assert!(process_alive(survivor), "the other pane keeps running");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_pane_detach_keeps_the_program_running_detached() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "work");
    let before = session_processes(&mut connection, "alpha").await;
    let pane = split(&server);
    let (buffer_id, pid) = session_processes(&mut connection, "alpha")
        .await
        .into_iter()
        .find(|process| !before.contains(process))
        .expect("split pane has a process");

    run_cli(&server, ["kill-pane", "--detach", "-t", &pane]);

    sleep(Duration::from_millis(200)).await;
    let record = get_buffer(&mut connection, buffer_id)
        .await
        .expect("detached buffer is kept");
    assert_eq!(record.attachment_node_id, None);
    assert_eq!(record.state, BufferRecordState::Running);
    assert!(process_alive(pid));
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_window_ends_every_program_in_the_window() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "keep");
    let kept = session_processes(&mut connection, "alpha").await;
    new_window(&server, "work");
    split(&server);
    let killed = session_processes(&mut connection, "alpha")
        .await
        .into_iter()
        .filter(|process| !kept.contains(process))
        .collect::<Vec<_>>();
    assert_eq!(killed.len(), 2);

    run_cli(&server, ["kill-window", "-t", "alpha:work"]);

    assert_killed(&mut connection, &killed).await;
    assert!(process_alive(kept[0].1), "the other window keeps running");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_session_ends_every_program_in_the_session() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "one");
    split(&server);
    new_window(&server, "two");
    run_cli(
        &server,
        [
            "display-popup",
            "-t",
            "alpha",
            "--",
            "/bin/sh",
            "-c",
            LONG_RUNNING,
        ],
    );
    let killed = session_processes(&mut connection, "alpha").await;
    assert_eq!(killed.len(), 4);

    run_cli(&server, ["kill-session", "-t", "alpha"]);

    assert_killed(&mut connection, &killed).await;
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_popup_ends_the_popup_program() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "work");
    let before = session_processes(&mut connection, "alpha").await;
    let created = run_cli(
        &server,
        [
            "display-popup",
            "-t",
            "alpha",
            "--",
            "/bin/sh",
            "-c",
            LONG_RUNNING,
        ],
    );
    let popup_id = stdout(&created).trim().to_owned();
    let killed = session_processes(&mut connection, "alpha")
        .await
        .into_iter()
        .filter(|process| !before.contains(process))
        .collect::<Vec<_>>();
    assert_eq!(killed.len(), 1);

    run_cli(&server, ["kill-popup", "-t", &popup_id]);

    assert_killed(&mut connection, &killed).await;
    assert!(process_alive(before[0].1));
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_server_stops_the_server_and_every_program() {
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, "work");
    split(&server);
    let killed = session_processes(&mut connection, "alpha").await;
    assert_eq!(killed.len(), 2);
    drop(connection);

    run_cli(&server, ["kill-server"]);

    // The command returns once the server has stopped listening.
    assert!(
        TestConnection::connect(server.socket_path()).await.is_err(),
        "server still accepts connections after kill-server"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    for &(_, pid) in &killed {
        while process_alive(pid) {
            assert!(Instant::now() < deadline, "pid {pid} survived kill-server");
            sleep(Duration::from_millis(25)).await;
        }
    }
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_server_ends_a_detached_server_process() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket = tempdir.path().join("mux.sock");
    let cli = |args: &[&str]| {
        let output = embers_test_support::cargo_bin("embers")
            .arg("--socket")
            .arg(&socket)
            .args(args)
            .output()
            .expect("cli runs");
        assert!(
            output.status.success(),
            "embers {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    // Starts a detached `__serve` process for the socket.
    cli(&["new-session", "alpha"]);
    let server_pid = std::fs::read_to_string(socket.with_extension("pid"))
        .expect("server writes its pid file")
        .trim()
        .parse::<u32>()
        .expect("pid file holds a pid");

    cli(&["kill-server"]);

    let deadline = Instant::now() + Duration::from_secs(10);
    while process_alive(server_pid) {
        if Instant::now() >= deadline {
            let _ = std::process::Command::new("kill")
                .arg(server_pid.to_string())
                .status();
            panic!("server process {server_pid} survived kill-server");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

#[test]
fn kill_server_without_a_server_fails_without_starting_one() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket = tempdir.path().join("mux.sock");
    let output = embers_test_support::cargo_bin("embers")
        .arg("--socket")
        .arg(&socket)
        .arg("kill-server")
        .output()
        .expect("cli runs");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no embers server"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!socket.exists(), "kill-server must not start a server");
}
