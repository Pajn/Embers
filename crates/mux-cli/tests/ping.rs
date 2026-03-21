use mux_test_support::{TestServer, cargo_bin};
use predicates::prelude::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_command_reaches_server() {
    let server = TestServer::start().await.expect("start server");
    let mut command = cargo_bin("mux-cli");
    command
        .arg("ping")
        .arg("--socket")
        .arg(server.socket_path())
        .arg("workspace");

    command
        .assert()
        .success()
        .stdout(predicate::str::contains("pong workspace"));

    server.shutdown().await.expect("shutdown server");
}
