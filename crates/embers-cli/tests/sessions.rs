use std::fs;
use std::path::Path;
use std::time::Duration;

use embers_test_support::cargo_bin;
use predicates::prelude::*;

use embers_test_support::TestServer;
use tempfile::tempdir;

use crate::support::{cli_command, run_cli, stdout};

async fn shutdown_spawned_server(socket_path: &Path) {
    let pid_path = socket_path.with_extension("pid");
    let pid = wait_for_pid(&pid_path)
        .await
        .trim()
        .parse::<i32>()
        .expect("pid parses");
    assert!(pid > 0, "invalid pid: {pid}");

    // SAFETY: pid comes from our own pid file and SIGTERM targets that specific process.
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(result, 0, "failed to signal spawned server");

    for _ in 0..50 {
        if !socket_path.exists() && !pid_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "timed out waiting for spawned server shutdown (socket: {}, pid file: {})",
        socket_path.display(),
        pid_path.display()
    );
}

async fn wait_for_socket(socket_path: &Path) {
    for _ in 0..50 {
        if socket_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("timed out waiting for socket {}", socket_path.display());
}

async fn wait_for_pid(pid_path: &Path) -> String {
    for _ in 0..50 {
        if let Ok(pid) = fs::read_to_string(pid_path) {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("timed out waiting for pid file {}", pid_path.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_commands_round_trip_through_cli() {
    let server = TestServer::start().await.expect("start server");

    let created = run_cli(&server, ["new-session", "alpha"]);
    assert_eq!(stdout(&created).trim(), "1\talpha");

    let listed = run_cli(&server, ["list-sessions"]);
    assert_eq!(stdout(&listed).trim(), "1\talpha");

    cli_command(&server)
        .arg("has-session")
        .arg("-t")
        .arg("alpha")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    cli_command(&server)
        .arg("kill-session")
        .arg("-t")
        .arg("alpha")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let listed = run_cli(&server, ["list-sessions"]);
    assert!(stdout(&listed).trim().is_empty());

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn has_session_reports_missing_names_precisely() {
    let server = TestServer::start().await.expect("start server");

    cli_command(&server)
        .arg("has-session")
        .arg("-t")
        .arg("missing")
        .assert()
        .failure()
        .stderr(predicate::str::contains("session 'missing' was not found"));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_bootstraps_server_on_first_run() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");

    let output = cargo_bin("embers")
        .arg("--socket")
        .arg(&socket_path)
        .arg("list-sessions")
        .output()
        .expect("cli command runs");
    assert!(
        output.status.success(),
        "list-sessions failed without a pre-existing server:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());

    wait_for_socket(&socket_path).await;
    shutdown_spawned_server(&socket_path).await;
}
