//! `set-environment` / `show-environment`: session and global environments
//! applied to panes created afterwards.

use std::time::Duration;

use embers_test_support::TestServer;
use tokio::time::sleep;

use crate::support::{cli_command, run_cli, stdout};

/// Wait until the first pane of `session`'s first window shows `needle`.
async fn wait_for_capture(server: &TestServer, session: &str, needle: &str) -> String {
    let panes = stdout(&run_cli(
        server,
        ["list-panes", "-t", &format!("{session}:0")],
    ));
    let pane = panes.split('\t').next().expect("a pane").to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let captured = stdout(&run_cli(server, ["capture-pane", "-t", &pane]));
        if captured.contains(needle) {
            return captured;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {needle:?}; pane shows:\n{captured}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_and_show_environment_round_trip() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);

    run_cli(
        &server,
        ["set-environment", "-t", "alpha", "EDITOR", "nvim"],
    );
    run_cli(&server, ["set-environment", "-t", "alpha", "EMPTY", ""]);
    run_cli(&server, ["set-environment", "-g", "PAGER", "less -R"]);

    let session = run_cli(&server, ["show-environment", "-t", "alpha"]);
    assert_eq!(stdout(&session), "EDITOR=nvim\nEMPTY=\n");
    let global = run_cli(&server, ["show-environment", "-g"]);
    assert_eq!(stdout(&global), "PAGER=less -R\n");
    let one = run_cli(&server, ["show-environment", "-t", "alpha", "EDITOR"]);
    assert_eq!(stdout(&one), "EDITOR=nvim\n");

    run_cli(&server, ["set-environment", "-u", "-t", "alpha", "EDITOR"]);
    let session = run_cli(&server, ["show-environment", "-t", "alpha"]);
    assert_eq!(stdout(&session), "EMPTY=\n");

    let missing = cli_command(&server)
        .args(["show-environment", "-t", "alpha", "EDITOR"])
        .output()
        .expect("cli runs");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("unknown variable"));

    let invalid = cli_command(&server)
        .args(["set-environment", "-g", "A=B", "c"])
        .output()
        .expect("cli runs");
    assert!(!invalid.status.success());

    let no_value = cli_command(&server)
        .args(["set-environment", "-g", "LONELY"])
        .output()
        .expect("cli runs");
    assert!(!no_value.status.success());

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_panes_get_the_global_then_session_environment() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-session", "beta"]);

    run_cli(
        &server,
        ["set-environment", "-g", "EMBERS_TEST_SHARED", "global"],
    );
    run_cli(
        &server,
        ["set-environment", "-g", "EMBERS_TEST_ONLY_GLOBAL", "g"],
    );
    run_cli(
        &server,
        [
            "set-environment",
            "-t",
            "alpha",
            "EMBERS_TEST_SHARED",
            "session",
        ],
    );

    run_cli(
        &server,
        [
            "new-window",
            "-t",
            "alpha",
            "--",
            "/bin/sh",
            "-c",
            "echo \"env:$EMBERS_TEST_SHARED:$EMBERS_TEST_ONLY_GLOBAL\"; sleep 30",
        ],
    );
    wait_for_capture(&server, "alpha", "env:session:g").await;

    // Another session sees only the global value.
    run_cli(
        &server,
        [
            "new-window",
            "-t",
            "beta",
            "--",
            "/bin/sh",
            "-c",
            "echo \"env:$EMBERS_TEST_SHARED\"; sleep 30",
        ],
    );
    wait_for_capture(&server, "beta", "env:global").await;

    server.shutdown().await.expect("shutdown server");
}
