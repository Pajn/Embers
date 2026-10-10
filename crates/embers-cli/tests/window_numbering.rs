//! Window numbering: `--base-index` / `EMBERS_BASE_INDEX` (tmux `base-index`)
//! and windows renumbering when one closes (tmux `renumber-windows on`).

use embers_test_support::TestServer;

use crate::support::{cli_command, run_cli, stdout};

fn new_window(server: &TestServer, args: &[&str], title: &str) -> String {
    let mut command = vec!["new-window", "-t", "alpha", "--title", title];
    command.splice(0..0, args.iter().copied());
    command.extend(["--", "/bin/sh"]);
    stdout(&run_cli(server, command)).trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base_index_flag_numbers_windows_from_the_base() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);

    assert_eq!(
        new_window(&server, &["--base-index", "1"], "editor"),
        "1\teditor"
    );
    assert_eq!(
        new_window(&server, &["--base-index", "1"], "logs"),
        "2\tlogs"
    );

    let listed = run_cli(
        &server,
        ["--base-index", "1", "list-windows", "-t", "alpha"],
    );
    assert_eq!(stdout(&listed).trim(), "1\t0\teditor\n2\t1\tlogs");

    // `1` is the first window, not ambiguous between 0- and 1-based numbering.
    run_cli(
        &server,
        ["--base-index", "1", "select-window", "-t", "alpha:1"],
    );
    let listed = run_cli(
        &server,
        ["--base-index", "1", "list-windows", "-t", "alpha"],
    );
    assert_eq!(stdout(&listed).trim(), "1\t1\teditor\n2\t0\tlogs");

    // No window is numbered 0.
    let missing = cli_command(&server)
        .args(["--base-index", "1", "select-window", "-t", "alpha:0"])
        .output()
        .expect("cli runs");
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("out of range"),
        "stderr: {}",
        String::from_utf8_lossy(&missing.stderr)
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base_index_env_var_applies_when_no_flag_is_given() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, &[], "editor");
    new_window(&server, &[], "logs");

    let selected = cli_command(&server)
        .env(embers_client::BASE_INDEX_ENV_VAR, "1")
        .args(["select-window", "-t", "alpha:1"])
        .output()
        .expect("cli runs");
    assert!(selected.status.success());
    let listed = cli_command(&server)
        .env(embers_client::BASE_INDEX_ENV_VAR, "1")
        .args(["list-windows", "-t", "alpha"])
        .output()
        .expect("cli runs");
    assert_eq!(stdout(&listed).trim(), "1\t1\teditor\n2\t0\tlogs");

    // The flag wins over the environment.
    let listed = cli_command(&server)
        .env(embers_client::BASE_INDEX_ENV_VAR, "1")
        .args(["--base-index", "0", "list-windows", "-t", "alpha"])
        .output()
        .expect("cli runs");
    assert_eq!(stdout(&listed).trim(), "0\t1\teditor\n1\t0\tlogs");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_base_index_numeric_targets_stay_ambiguity_checked() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    new_window(&server, &[], "editor");
    new_window(&server, &[], "logs");

    let ambiguous = cli_command(&server)
        .args(["select-window", "-t", "alpha:1"])
        .output()
        .expect("cli runs");
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("ambiguous"));

    // `0` can only be the 0-based first window and `2` the 1-based second.
    run_cli(&server, ["select-window", "-t", "alpha:0"]);
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t1\teditor\n1\t0\tlogs");
    run_cli(&server, ["select-window", "-t", "alpha:2"]);
    let listed = run_cli(&server, ["list-windows", "-t", "alpha"]);
    assert_eq!(stdout(&listed).trim(), "0\t0\teditor\n1\t1\tlogs");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killing_a_window_renumbers_the_windows_after_it() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    for title in ["one", "two", "three"] {
        new_window(&server, &["--base-index", "1"], title);
    }

    run_cli(
        &server,
        ["--base-index", "1", "kill-window", "-t", "alpha:2"],
    );

    let listed = run_cli(
        &server,
        ["--base-index", "1", "list-windows", "-t", "alpha"],
    );
    assert_eq!(stdout(&listed).trim(), "1\t0\tone\n2\t1\tthree");
    run_cli(
        &server,
        ["--base-index", "1", "select-window", "-t", "alpha:2"],
    );
    let listed = run_cli(
        &server,
        ["--base-index", "1", "list-windows", "-t", "alpha"],
    );
    assert_eq!(stdout(&listed).trim(), "1\t0\tone\n2\t1\tthree");

    server.shutdown().await.expect("shutdown server");
}
