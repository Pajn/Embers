//! tmux target syntax: every `-t` accepts sessions, `=exact` names, `$id`,
//! `session:window.pane`, `:window`, `.pane`, `@window` and `%buffer`.

use embers_test_support::TestServer;

use crate::support::{cli_command, run_cli, stdout};

fn print(server: &TestServer, target: &str, format: &str) -> String {
    stdout(&run_cli(
        server,
        [
            "--base-index",
            "0",
            "display-message",
            "-p",
            "-t",
            target,
            format,
        ],
    ))
    .trim_end()
    .to_owned()
}

fn fails(server: &TestServer, args: &[&str]) -> String {
    let output = cli_command(server)
        .args(args)
        .output()
        .expect("cli command runs");
    assert!(!output.status.success(), "{args:?} should fail");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `(pane id, buffer id)` of the panes `list-panes -t target` prints.
fn panes(server: &TestServer, target: &str) -> Vec<(String, String)> {
    stdout(&run_cli(server, ["list-panes", "-t", target]))
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            (
                fields.next().expect("pane id").to_owned(),
                fields.next().expect("buffer id").to_owned(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targets_accept_tmux_forms() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-session", "alpine"]);
    run_cli(&server, ["new-session", "beta"]);
    for (session, title) in [
        ("=alpha", "editor"),
        ("=alpha", "logs"),
        ("=alpine", "x"),
        ("beta", "main"),
    ] {
        run_cli(
            &server,
            [
                "new-window",
                "-t",
                session,
                "--title",
                title,
                "--",
                "/bin/sh",
            ],
        );
    }
    run_cli(
        &server,
        ["split-window", "-t", "=alpha:logs", "--", "/bin/sh"],
    );

    // Sessions: exact, `=exact`, unique prefix, `$id`.
    run_cli(&server, ["has-session", "-t", "alpha"]);
    run_cli(&server, ["has-session", "-t", "=alpha"]);
    run_cli(&server, ["has-session", "-t", "bet"]);
    run_cli(&server, ["has-session", "-t", "$3"]);
    let stderr = fails(&server, &["has-session", "-t", "=bet"]);
    assert!(stderr.contains("session 'bet' was not found"), "{stderr}");
    let stderr = fails(&server, &["has-session", "-t", "alp"]);
    assert!(stderr.contains("more than one session"), "{stderr}");
    let stderr = fails(&server, &["has-session", "-t", "$9"]);
    assert!(stderr.contains("was not found"), "{stderr}");

    // Windows by number, name, prefix, `=name`, `@id` and tokens.
    assert_eq!(print(&server, "alpha:1", "#S:#I:#W"), "alpha:1:logs");
    assert_eq!(print(&server, "=alpha:=editor", "#W"), "editor");
    assert_eq!(print(&server, "alpha:lo", "#W"), "logs");
    assert_eq!(print(&server, "alpha:^", "#W"), "editor");
    assert_eq!(print(&server, "alpha:$", "#W"), "logs");
    let stderr = fails(&server, &["select-window", "-t", "=alpha:=lo"]);
    assert!(stderr.contains("window 'lo' was not found"), "{stderr}");

    // Panes by index within a window, `%buffer` and the plain pane id.
    let logs = panes(&server, "alpha:logs");
    assert_eq!(logs.len(), 2);
    assert_eq!(print(&server, "alpha:logs.1", "#{node_id}"), logs[1].0);
    assert_eq!(print(&server, "alpha:1.0", "#{node_id}"), logs[0].0);
    assert_eq!(
        print(&server, &format!("%{}", logs[1].1), "#S:#W #{node_id}"),
        format!("alpha:logs {}", logs[1].0)
    );
    assert_eq!(print(&server, &logs[0].0, "#W"), "logs");
    let stderr = fails(
        &server,
        &["display-message", "-p", "-t", "alpha:logs.5", "#D"],
    );
    assert!(stderr.contains("pane index 5 is out of range"), "{stderr}");

    // `@N` is the window whose root node is N.
    let editor = panes(&server, "alpha:editor");
    let window_id = editor[0].0.clone();
    assert_eq!(print(&server, &format!("@{window_id}"), "#W"), "editor");

    // A word is a window of the current session, else a session.
    assert_eq!(print(&server, "beta", "#S:#W"), "beta:main");

    // Session commands take the session of a window or pane target.
    run_cli(
        &server,
        ["rename-session", "-t", &format!("%{}", logs[0].1), "gamma"],
    );
    run_cli(&server, ["has-session", "-t", "=gamma"]);

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_targets_start_from_the_callers_pane() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-session", "beta"]);
    for title in ["one", "two", "three"] {
        run_cli(
            &server,
            [
                "new-window",
                "-t",
                "alpha",
                "--title",
                title,
                "--",
                "/bin/sh",
            ],
        );
    }
    run_cli(&server, ["new-window", "-t", "beta", "--", "/bin/sh"]);
    run_cli(
        &server,
        ["split-window", "-t", "alpha:two", "--", "/bin/sh"],
    );
    let two = panes(&server, "alpha:two");

    // As if run in the first pane of window "two" of alpha.
    let in_pane = |args: &[&str]| {
        let output = cli_command(&server)
            .env("EMBERS_BUFFER", &two[0].1)
            .env("EMBERS_SOCKET", server.socket_path())
            .args(["--base-index", "0"])
            .args(args)
            .output()
            .expect("cli command runs");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    };
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", ":+", "#W"]),
        "three"
    );
    assert_eq!(in_pane(&["display-message", "-p", "-t", ":-", "#W"]), "one");
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", ":+2", "#W"]),
        "one"
    );
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", ".1", "#{node_id}"]),
        two[1].0
    );
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", ":", "#{node_id}"]),
        two[0].0
    );
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", "one", "#W"]),
        "one"
    );
    assert_eq!(
        in_pane(&["display-message", "-p", "-t", "=beta:", "#S"]),
        "beta"
    );

    server.shutdown().await.expect("shutdown server");
}
