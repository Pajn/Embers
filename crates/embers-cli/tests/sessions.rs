mod support;

use predicates::prelude::*;

use embers_test_support::TestServer;

use support::{cli_command, run_cli, stdout};

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
