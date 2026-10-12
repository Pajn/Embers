//! Commands shaped like tmux's, for scripts written against tmux:
//! `new-session` with a first window, `-c` and `-P -F` on the commands that
//! create panes, `switch-client` from inside a pane, a `display-popup` that
//! waits, directional `select-pane` and `capture-pane -p -S -E -J -e`.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use embers_core::{FloatGeometry, Size, new_request_id};
use embers_protocol::{
    ClientMessage, ClientRequest, ServerResponse, SessionRequest, SessionSnapshot,
};
use embers_test_support::{TestConnection, TestServer};
use tempfile::tempdir;
use tokio::time::sleep;

use crate::support::{cli_command, run_cli, session_snapshot_by_name, stdout};

fn out(output: &Output) -> String {
    stdout(output).trim_end().to_owned()
}

fn print(server: &TestServer, target: &str, format: &str) -> String {
    out(&run_cli(
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
}

/// A directory the way `pwd -P` prints it.
fn real_dir(path: &Path) -> String {
    path.canonicalize()
        .expect("canonical path")
        .to_string_lossy()
        .into_owned()
}

/// Poll `capture-pane -p -S - -t target` until it shows `needle`.
async fn wait_for_text(server: &TestServer, target: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let captured = out(&run_cli(
            server,
            [
                "--base-index",
                "0",
                "capture-pane",
                "-p",
                "-S",
                "-",
                "-t",
                target,
            ],
        ));
        if captured.contains(needle) {
            return captured;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?} in {target}; it shows:\n{captured}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// A command that prints the directory it starts in and stays.
const PRINT_DIR: &str = "echo \"dir:$(pwd -P)\"; sleep 30";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_session_creates_a_first_window_in_the_directory() {
    let server = TestServer::start().await.expect("start server");
    let dir = tempdir().expect("tempdir");
    let dir_arg = dir.path().to_string_lossy().into_owned();

    let created = run_cli(
        &server,
        [
            "new-session",
            "-d",
            "-s",
            "t1",
            "-c",
            &dir_arg,
            "-n",
            "work",
            "-P",
            "-F",
            "#{session_name}:#{window_index}:#{window_name} #{pane_index}",
            "--",
            "/bin/sh",
            "-c",
            PRINT_DIR,
        ],
    );
    assert_eq!(out(&created), "t1:0:work 0");
    wait_for_text(&server, "=t1:", &format!("dir:{}", real_dir(dir.path()))).await;

    // Without -P it prints nothing, like tmux, and names sessions by number.
    let unnamed = run_cli(&server, ["new-session", "-d"]);
    assert_eq!(out(&unnamed), "");
    run_cli(&server, ["has-session", "-t", "=0"]);
    let default_format = run_cli(&server, ["new-session", "-d", "-P"]);
    assert_eq!(out(&default_format), "1:");

    // A second session of the same name is refused unless -A.
    let duplicate = cli_command(&server)
        .args(["new-session", "-d", "-s", "t1"])
        .output()
        .expect("cli runs");
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("duplicate session: t1"));
    let existing = run_cli(&server, ["new-session", "-d", "-A", "-s", "t1", "-P"]);
    assert_eq!(out(&existing), "t1:");
    let listed = out(&run_cli(
        &server,
        ["list-sessions", "-F", "#{session_name}"],
    ));
    assert_eq!(listed, "t1\n0\n1");

    // Without -c the window starts where the command runs, like tmux.
    let elsewhere = tempdir().expect("tempdir");
    let created = cli_command(&server)
        .current_dir(elsewhere.path())
        .args([
            "new-session",
            "-d",
            "-s",
            "t2",
            "--",
            "/bin/sh",
            "-c",
            PRINT_DIR,
        ])
        .output()
        .expect("cli runs");
    assert!(created.status.success());
    wait_for_text(
        &server,
        "=t2:",
        &format!("dir:{}", real_dir(elsewhere.path())),
    )
    .await;

    // The old form still makes an empty session and prints its id and name.
    let legacy = run_cli(&server, ["new-session", "legacy"]);
    assert!(out(&legacy).ends_with("\tlegacy"), "{}", out(&legacy));
    let windows = out(&run_cli(&server, ["list-windows", "-t", "=legacy"]));
    assert_eq!(windows, "");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panes_start_in_the_directory_given_with_c() {
    let server = TestServer::start().await.expect("start server");
    let dir = tempdir().expect("tempdir");
    let other = tempdir().expect("tempdir");
    let dir_arg = dir.path().to_string_lossy().into_owned();
    run_cli(
        &server,
        [
            "new-session",
            "-d",
            "-s",
            "alpha",
            "-c",
            &dir_arg,
            "--",
            "/bin/sh",
        ],
    );

    // -c with a format, expanded for the pane being split.
    let split = run_cli(
        &server,
        [
            "split-window",
            "-v",
            "-t",
            "alpha:0.0",
            "-c",
            "#{pane_current_path}",
            "-P",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index} #{pane_id} #{buffer_id}",
            "--",
            "/bin/sh",
            "-c",
            PRINT_DIR,
        ],
    );
    let printed = out(&split);
    let (address, ids) = printed.split_once(' ').expect("format output");
    assert_eq!(address, "alpha:0.1");
    let (pane_id, buffer_id) = ids.split_once(' ').expect("ids");
    wait_for_text(&server, pane_id, &format!("dir:{}", real_dir(dir.path()))).await;
    // `#{pane_id}` is tmux's `%N`, and `%N` names the same pane back.
    assert_eq!(pane_id, format!("%{buffer_id}"));
    assert_eq!(print(&server, &format!("%{buffer_id}"), "#D"), pane_id);
    // -v stacks the new pane below.
    let snapshot = {
        let mut connection = TestConnection::connect(server.socket_path())
            .await
            .expect("connect");
        session_snapshot_by_name(&mut connection, "alpha").await
    };
    let split_node = snapshot
        .nodes
        .iter()
        .find(|node| node.split.is_some())
        .expect("a split");
    assert_eq!(
        split_node.split.as_ref().unwrap().direction,
        embers_core::SplitDirection::Horizontal
    );

    // new-window -c, and -P with its default format.
    let other_arg = other.path().to_string_lossy().into_owned();
    let window = run_cli(
        &server,
        [
            "new-window",
            "-t",
            "alpha",
            "-c",
            &other_arg,
            "-P",
            "--",
            "/bin/sh",
            "-c",
            PRINT_DIR,
        ],
    );
    assert_eq!(out(&window), "alpha:1.0");
    wait_for_text(
        &server,
        "alpha:1",
        &format!("dir:{}", real_dir(other.path())),
    )
    .await;

    // Without -c, where the command runs; -d keeps the current window.
    let elsewhere = tempdir().expect("tempdir");
    let window = cli_command(&server)
        .current_dir(elsewhere.path())
        .args([
            "new-window",
            "-d",
            "-t",
            "alpha",
            "-P",
            "-F",
            "#{window_index} #{window_active}",
            "--",
            "/bin/sh",
            "-c",
            PRINT_DIR,
        ])
        .output()
        .expect("cli runs");
    assert!(window.status.success());
    assert_eq!(out(&window), "2 0");
    wait_for_text(
        &server,
        "alpha:2",
        &format!("dir:{}", real_dir(elsewhere.path())),
    )
    .await;

    // Window and pane listings take formats too.
    let windows = out(&run_cli(
        &server,
        [
            "list-windows",
            "-t",
            "alpha",
            "-F",
            "#{window_index} #{window_panes} #{window_active}",
        ],
    ));
    assert_eq!(windows, "0 2 0\n1 1 1\n2 1 0");
    let panes = out(&run_cli(
        &server,
        [
            "list-panes",
            "-s",
            "-t",
            "alpha",
            "-F",
            "#{window_index}.#{pane_index}",
        ],
    ));
    assert_eq!(panes, "0.0\n0.1\n1.0\n2.0");

    // -d keeps the split pane focused.
    let detached = run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "alpha:1.0",
            "-P",
            "-F",
            "#{pane_index} #{pane_active}",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(out(&detached), "1 0");
    assert_eq!(print(&server, "alpha:1.0", "#{pane_active}"), "1");

    server.shutdown().await.expect("shutdown server");
}

/// A protocol client showing `session`, like an attached `embers`. Returns
/// the connection (keep it alive) and its client id.
async fn attached_client(server: &TestServer, session: &str) -> (TestConnection, u64) {
    let mut client = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let session_id = session_snapshot_by_name(&mut client, session)
        .await
        .session
        .id;
    let response = client
        .request(&ClientMessage::Client(ClientRequest::Switch {
            request_id: new_request_id(),
            client_id: None,
            session_id,
            node_id: None,
        }))
        .await
        .expect("switch");
    match response {
        ServerResponse::Client(response) => (client, response.client.id),
        other => panic!("unexpected response {other:?}"),
    }
}

/// The name of the session client `client_id` shows, per `list-clients`.
fn shown_session(server: &TestServer, client_id: u64) -> String {
    let clients = out(&run_cli(server, ["list-clients"]));
    clients
        .lines()
        .find_map(|line| {
            let mut fields = line.split('\t');
            (fields.next() == Some(&client_id.to_string())).then(|| {
                let session = fields.next().expect("session field");
                session
                    .split_once(':')
                    .map_or(session, |(_, name)| name)
                    .to_owned()
            })
        })
        .unwrap_or_else(|| panic!("client {client_id} not listed in {clients}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switch_client_from_a_pane_moves_the_client_showing_it() {
    let server = TestServer::start().await.expect("start server");
    let output_dir = tempdir().expect("tempdir");
    for session in ["alpha", "beta", "gamma"] {
        run_cli(
            &server,
            ["new-session", "-d", "-s", session, "--", "/bin/sh"],
        );
    }
    let (_alpha_client, alpha_client) = attached_client(&server, "alpha").await;
    let (_gamma_client, gamma_client) = attached_client(&server, "gamma").await;

    // Run in alpha's pane, which has only EMBERS_SOCKET and EMBERS_BUFFER.
    let embers = embers_test_support::cargo_bin_path("embers");
    let done = output_dir.path().join("done");
    let script = format!(
        "'{embers}' switch-client -t =beta && touch '{done}'",
        embers = embers.display(),
        done = done.display(),
    );
    run_cli(&server, ["send-keys", "-t", "=alpha:", "--enter", &script]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done.exists() {
        assert!(Instant::now() < deadline, "switch-client did not finish");
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(shown_session(&server, alpha_client), "beta");
    assert_eq!(shown_session(&server, gamma_client), "gamma");

    // Outside any pane, with two clients attached, it has to be told which.
    let ambiguous = cli_command(&server)
        .env_remove("EMBERS_BUFFER")
        .env_remove("EMBERS_CLIENT")
        .args(["switch-client", "-t", "alpha"])
        .output()
        .expect("cli runs");
    assert!(!ambiguous.status.success());
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("several clients are attached"),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
    run_cli(
        &server,
        [
            "switch-client",
            "-c",
            &gamma_client.to_string(),
            "-t",
            "alpha",
        ],
    );
    assert_eq!(shown_session(&server, gamma_client), "alpha");

    // EMBERS_CLIENT (set for run_shell children) names the client.
    let by_env = cli_command(&server)
        .env("EMBERS_SOCKET", server.socket_path())
        .env("EMBERS_CLIENT", gamma_client.to_string())
        .args(["switch-client", "-n"])
        .output()
        .expect("cli runs");
    assert!(by_env.status.success());
    assert_eq!(shown_session(&server, gamma_client), "beta");
    let by_env = cli_command(&server)
        .env("EMBERS_SOCKET", server.socket_path())
        .env("EMBERS_CLIENT", gamma_client.to_string())
        .args(["switch-client", "-p"])
        .output()
        .expect("cli runs");
    assert!(by_env.status.success());
    assert_eq!(shown_session(&server, gamma_client), "alpha");

    server.shutdown().await.expect("shutdown server");
}

async fn set_viewport(server: &TestServer, session: &str, width: u16, height: u16) {
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let session_id = session_snapshot_by_name(&mut connection, session)
        .await
        .session
        .id;
    connection
        .request(&ClientMessage::Session(SessionRequest::SetViewport {
            request_id: new_request_id(),
            session_id,
            viewport: Size { width, height },
        }))
        .await
        .expect("set viewport");
}

async fn popup_count(server: &TestServer, session: &str) -> usize {
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    session_snapshot_by_name(&mut connection, session)
        .await
        .floating
        .len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_popup_waits_for_the_popup_and_exits_with_its_status() {
    let server = TestServer::start().await.expect("start server");
    let dir = tempdir().expect("tempdir");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "alpha", "--", "/bin/sh"],
    );
    set_viewport(&server, "alpha", 100, 40).await;

    // Blocks until the command exits, then closes the popup and exits with
    // its status.
    let started = Instant::now();
    let waited = cli_command(&server)
        .args([
            "display-popup",
            "-E",
            "-w",
            "90%",
            "-h",
            "90%",
            "-d",
            &dir.path().to_string_lossy(),
            "--",
            "/bin/sh",
            "-c",
            "pwd -P > out; sleep 1; exit 3",
        ])
        .output()
        .expect("cli runs");
    let elapsed = started.elapsed();
    assert_eq!(waited.status.code(), Some(3), "{waited:?}");
    assert!(
        elapsed >= Duration::from_millis(900),
        "returned after {elapsed:?}"
    );
    assert_eq!(popup_count(&server, "alpha").await, 0);
    let pwd = std::fs::read_to_string(dir.path().join("out")).expect("popup wrote out");
    assert_eq!(pwd.trim_end(), real_dir(dir.path()));

    // A command that succeeds exits 0.
    let ok = cli_command(&server)
        .args(["display-popup", "-E", "--", "/bin/sh", "-c", "exit 0"])
        .output()
        .expect("cli runs");
    assert_eq!(ok.status.code(), Some(0));

    // --no-wait returns the id at once; sizes are percentages of the view,
    // centred by default, and cells place it.
    let popup = run_cli(
        &server,
        [
            "display-popup",
            "--no-wait",
            "-w",
            "90%",
            "-h",
            "90%",
            "--",
            "/bin/sh",
        ],
    );
    let popup_id = out(&popup);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(
        snapshot.floating[0].geometry,
        FloatGeometry::new(5, 2, 90, 36)
    );
    run_cli(&server, ["kill-popup", "-t", &popup_id]);
    let popup = run_cli(
        &server,
        [
            "display-popup",
            "--no-wait",
            "-x",
            "R",
            "-y",
            "0",
            "-w",
            "20",
            "-h",
            "50%",
            "--",
            "/bin/sh",
        ],
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert_eq!(
        snapshot.floating[0].geometry,
        FloatGeometry::new(80, 0, 20, 20)
    );
    run_cli(&server, ["kill-popup", "-t", &out(&popup)]);

    // Closing the popup ends the wait (and the command), with status 0.
    let socket = server.socket_path().to_path_buf();
    let waiting = std::thread::spawn(move || {
        embers_test_support::cargo_bin("embers")
            .arg("--socket")
            .arg(&socket)
            .env_remove("EMBERS_BUFFER")
            .args(["display-popup", "-E", "--", "/bin/sh", "-c", "sleep 30"])
            .output()
            .expect("cli runs")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while popup_count(&server, "alpha").await == 0 {
        assert!(Instant::now() < deadline, "the popup never opened");
        sleep(Duration::from_millis(50)).await;
    }
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    run_cli(
        &server,
        ["kill-popup", "-t", &snapshot.floating[0].id.to_string()],
    );
    let output = waiting.join().expect("waiting cli");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // -EE keeps a popup whose command failed open until it is closed.
    let socket = server.socket_path().to_path_buf();
    let waiting = std::thread::spawn(move || {
        embers_test_support::cargo_bin("embers")
            .arg("--socket")
            .arg(&socket)
            .args(["display-popup", "-EE", "--", "/bin/sh", "-c", "exit 5"])
            .output()
            .expect("cli runs")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while popup_count(&server, "alpha").await == 0 {
        assert!(Instant::now() < deadline, "the popup never opened");
        sleep(Duration::from_millis(50)).await;
    }
    sleep(Duration::from_millis(500)).await;
    assert_eq!(
        popup_count(&server, "alpha").await,
        1,
        "the failed popup stays"
    );
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    run_cli(
        &server,
        ["kill-popup", "-t", &snapshot.floating[0].id.to_string()],
    );
    let output = waiting.join().expect("waiting cli");
    assert_eq!(output.status.code(), Some(5), "{output:?}");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_popup_fails_when_the_server_goes_away() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "alpha", "--", "/bin/sh"],
    );
    let socket: PathBuf = server.socket_path().to_path_buf();
    let waiting = std::thread::spawn(move || {
        embers_test_support::cargo_bin("embers")
            .arg("--socket")
            .arg(&socket)
            .args(["display-popup", "--", "/bin/sh", "-c", "sleep 30"])
            .output()
            .expect("cli runs")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while popup_count(&server, "alpha").await == 0 {
        assert!(Instant::now() < deadline, "the popup never opened");
        sleep(Duration::from_millis(50)).await;
    }
    server.shutdown().await.expect("shutdown server");
    let output = waiting.join().expect("waiting cli");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("went away"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The id of the focused pane in `session`.
fn focused_pane(server: &TestServer, session: &str) -> String {
    out(&run_cli(
        server,
        [
            "list-panes",
            "-s",
            "-t",
            session,
            "-F",
            "#{pane_active} #{node_id}",
        ],
    ))
    .lines()
    .find_map(|line| line.strip_prefix("1 ").map(str::to_owned))
    .expect("a focused pane")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_pane_moves_to_the_pane_on_that_side() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "alpha", "--", "/bin/sh"],
    );
    // left | right-top
    //      | right-bottom
    let left = print(&server, "alpha:0.0", "#{node_id}");
    let top_right = out(&run_cli(
        &server,
        ["split-window", "-h", "-t", &left, "--", "/bin/sh"],
    ));
    let bottom_right = out(&run_cli(
        &server,
        ["split-window", "-v", "-t", &top_right, "--", "/bin/sh"],
    ));

    run_cli(&server, ["select-pane", "-t", &bottom_right, "-U"]);
    assert_eq!(focused_pane(&server, "alpha"), top_right);
    run_cli(&server, ["select-pane", "-t", &top_right, "-L"]);
    assert_eq!(focused_pane(&server, "alpha"), left);
    // Nothing to the left of the left pane: focus stays.
    run_cli(&server, ["select-pane", "-L"]);
    assert_eq!(focused_pane(&server, "alpha"), left);
    run_cli(&server, ["select-pane", "-R"]);
    assert!([top_right.clone(), bottom_right.clone()].contains(&focused_pane(&server, "alpha")));

    // From inside a pane, without -t, it moves from that pane.
    let buffer = print(&server, &top_right, "#{buffer_id}");
    let moved = cli_command(&server)
        .env("EMBERS_SOCKET", server.socket_path())
        .env("EMBERS_BUFFER", &buffer)
        .args(["select-pane", "-D"])
        .output()
        .expect("cli runs");
    assert!(moved.status.success());
    assert_eq!(focused_pane(&server, "alpha"), bottom_right);

    // What smart-splits asks before handing off at an edge.
    let edges = "#{pane_at_left}#{pane_at_right}#{pane_at_top}#{pane_at_bottom}";
    assert_eq!(print(&server, &left, edges), "1011");
    assert_eq!(print(&server, &top_right, edges), "0110");
    assert_eq!(print(&server, &bottom_right, edges), "0101");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_pane_prints_ranges_joined_lines_and_escapes() {
    let server = TestServer::start().await.expect("start server");
    // 60 numbered lines, a line of 100 characters (wrapped in 80 columns)
    // and a red word, on the 80x24 screen panes get before a client draws
    // them: line1..line40 scroll into the history.
    let script = "i=1; while [ $i -le 60 ]; do echo line$i; i=$((i+1)); done; \
                  printf '%0100d\\n' 0; printf '\\033[31mred\\033[0m\\n'; sleep 30";
    run_cli(
        &server,
        [
            "new-session",
            "-d",
            "-s",
            "alpha",
            "--",
            "/bin/sh",
            "-c",
            script,
        ],
    );
    wait_for_text(&server, "alpha:", "red").await;

    let screen = out(&run_cli(&server, ["capture-pane", "-p", "-t", "alpha:"]));
    let lines = stdout(&run_cli(&server, ["capture-pane", "-p", "-t", "alpha:"]));
    assert_eq!(lines.lines().count(), 24, "{lines:?}");
    assert!(screen.starts_with("line41\n"), "{screen}");
    assert!(!screen.contains("line40\n"));

    let history = out(&run_cli(
        &server,
        ["capture-pane", "-p", "-t", "alpha:", "-S", "-3", "-E", "-1"],
    ));
    assert_eq!(history, "line38\nline39\nline40");
    let everything = out(&run_cli(
        &server,
        ["capture-pane", "-p", "-t", "alpha:", "-S", "-", "-E", "-"],
    ));
    assert!(everything.starts_with("line1\nline2\n"), "{everything}");
    assert!(everything.contains("line60\n"));

    let zeros = "0".repeat(100);
    let wrapped = out(&run_cli(&server, ["capture-pane", "-p", "-t", "alpha:"]));
    assert!(
        wrapped.lines().any(|line| line == "0".repeat(80)),
        "{wrapped}"
    );
    let joined = out(&run_cli(
        &server,
        ["capture-pane", "-p", "-J", "-t", "alpha:"],
    ));
    assert!(joined.lines().any(|line| line == zeros), "{joined}");

    let escaped = out(&run_cli(
        &server,
        ["capture-pane", "-p", "-e", "-t", "alpha:"],
    ));
    assert!(escaped.contains("\x1b[0;31mred\x1b[0m"), "{escaped:?}");

    server.shutdown().await.expect("shutdown server");
}

/// Run `args` and return its stderr, asserting that it failed.
fn fails(server: &TestServer, args: &[&str]) -> String {
    let output = cli_command(server).args(args).output().expect("cli runs");
    assert!(
        !output.status.success(),
        "{args:?} succeeded: {}",
        stdout(&output)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_options_expand_in_formats_and_set_with_set_option_p() {
    let server = TestServer::start().await.expect("start server");
    for session in ["main", "t1"] {
        run_cli(
            &server,
            ["new-session", "-d", "-s", session, "--", "/bin/sh"],
        );
    }
    let main_pane = print(&server, "=main:", "#{pane_id}");
    let t1_pane = print(&server, "=t1:", "#{pane_id}");

    // What tria does: mark a pane, then list every pane with the mark.
    run_cli(
        &server,
        [
            "set-option",
            "-p",
            "-t",
            &t1_pane,
            "@tria-open-socket",
            "/tmp/tria.sock",
        ],
    );
    let listed = out(&run_cli(
        &server,
        [
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} [#{@tria-open-socket}]",
        ],
    ));
    let mut lines = listed.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    let mut expected = vec![
        format!("{main_pane} []"),
        format!("{t1_pane} [/tmp/tria.sock]"),
    ];
    expected.sort_unstable();
    assert_eq!(lines, expected);

    // Every format context reads the pane's option.
    assert_eq!(
        print(&server, &t1_pane, "#{@tria-open-socket}"),
        "/tmp/tria.sock"
    );
    assert_eq!(
        out(&run_cli(
            &server,
            ["list-windows", "-t", "=t1", "-F", "#{@tria-open-socket}"]
        )),
        "/tmp/tria.sock"
    );
    assert_eq!(
        out(&run_cli(
            &server,
            [
                "list-sessions",
                "-F",
                "#{session_name}=#{@tria-open-socket}"
            ]
        )),
        "main=\nt1=/tmp/tria.sock"
    );
    let created = out(&run_cli(
        &server,
        [
            "new-window",
            "-d",
            "-t",
            "=t1",
            "-P",
            "-F",
            "[#{@tria-open-socket}]",
            "--",
            "/bin/sh",
        ],
    ));
    assert_eq!(created, "[]");

    // `@name` is the buffer option `name`, which `buffer set-option` sets
    // and the config reads with `buffer.user_option("name")`.
    run_cli(
        &server,
        ["buffer", "set-option", "-t", &main_pane, "is-vim", "1"],
    );
    assert_eq!(print(&server, &main_pane, "#{@is-vim}"), "1");
    run_cli(
        &server,
        ["set-option", "-p", "-t", &main_pane, "@is-vim", "0"],
    );
    assert_eq!(
        out(&run_cli(
            &server,
            ["buffer", "show-options", "-t", &main_pane]
        )),
        "\"is-vim\"\t\"0\""
    );

    // show-options -p prints like tmux, quoting values that need it.
    run_cli(&server, ["set", "-p", "-t", &t1_pane, "@spaced", "a b"]);
    run_cli(
        &server,
        ["set-option", "-p", "-t", &t1_pane, "remain-on-exit", "on"],
    );
    assert_eq!(
        out(&run_cli(&server, ["show-options", "-p", "-t", &t1_pane])),
        "@spaced \"a b\"\n@tria-open-socket /tmp/tria.sock\nremain-on-exit on"
    );
    assert_eq!(
        out(&run_cli(
            &server,
            ["show", "-pv", "-t", &t1_pane, "@spaced"]
        )),
        "a b"
    );
    assert!(
        fails(&server, &["show-options", "-p", "-t", &t1_pane, "@nope"])
            .contains("invalid option: @nope")
    );
    assert_eq!(
        out(&run_cli(
            &server,
            ["show-options", "-pq", "-t", &t1_pane, "@nope"]
        )),
        ""
    );

    // -u unsets; -o leaves a set option alone.
    run_cli(&server, ["set-option", "-pu", "-t", &t1_pane, "@spaced"]);
    assert_eq!(print(&server, &t1_pane, "[#{@spaced}]"), "[]");
    assert!(
        fails(
            &server,
            &[
                "set-option",
                "-po",
                "-t",
                &t1_pane,
                "@tria-open-socket",
                "x"
            ]
        )
        .contains("already set: @tria-open-socket")
    );
    assert_eq!(
        print(&server, &t1_pane, "#{@tria-open-socket}"),
        "/tmp/tria.sock"
    );

    // Only pane options exist, and only user options and remain-on-exit.
    assert!(fails(&server, &["set-option", "-p", "bogus", "1"]).contains("invalid option: bogus"));
    assert!(fails(&server, &["set-option", "-p", "@empty"]).contains("empty value"));
    assert!(fails(&server, &["set-option", "@foo", "1"]).contains("-p"));
    assert!(fails(&server, &["set-option", "-w", "@foo", "1"]).contains("-p"));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switch_client_to_a_pane_shows_its_window_and_pane() {
    let server = TestServer::start().await.expect("start server");
    let output_dir = tempdir().expect("tempdir");
    for session in ["main", "t1"] {
        run_cli(
            &server,
            ["new-session", "-d", "-s", session, "--", "/bin/sh"],
        );
    }
    // t1: window 0, and window 1 split in two, with window 0 current.
    run_cli(&server, ["new-window", "-d", "-t", "=t1", "--", "/bin/sh"]);
    let target = out(&run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "=t1:1",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "/bin/sh",
        ],
    ));
    run_cli(
        &server,
        ["--base-index", "0", "select-window", "-t", "=t1:0"],
    );
    assert_eq!(print(&server, "=t1:", "#{window_index}"), "0");
    assert_eq!(
        print(&server, &target, "#{window_index}.#{pane_index}"),
        "1.1"
    );

    let (_main_client, main_client) = attached_client(&server, "main").await;

    // From main's pane, as tria does.
    let embers = embers_test_support::cargo_bin_path("embers");
    let done = output_dir.path().join("done");
    let script = format!(
        "'{embers}' switch-client -t '{target}' && touch '{done}'",
        embers = embers.display(),
        done = done.display(),
    );
    run_cli(&server, ["send-keys", "-t", "=main:", "--enter", &script]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done.exists() {
        assert!(Instant::now() < deadline, "switch-client did not finish");
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(shown_session(&server, main_client), "t1");
    assert_eq!(
        print(&server, "=t1:", "#{window_index}.#{pane_index} #{pane_id}"),
        format!("1.1 {target}")
    );

    // A window target shows that window with the pane it last focused.
    let client = main_client.to_string();
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "switch-client",
            "-c",
            &client,
            "-t",
            "=main:0",
        ],
    );
    assert_eq!(shown_session(&server, main_client), "main");
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "switch-client",
            "-c",
            &client,
            "-t",
            "t1:0",
        ],
    );
    assert_eq!(shown_session(&server, main_client), "t1");
    assert_eq!(print(&server, "=t1:", "#{window_index}"), "0");
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "switch-client",
            "-c",
            &client,
            "-t",
            "t1:1",
        ],
    );
    assert_eq!(
        print(&server, "=t1:", "#{window_index}.#{pane_index} #{pane_id}"),
        format!("1.1 {target}")
    );
    // `sess:win.pane` too.
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "switch-client",
            "-c",
            &client,
            "-t",
            "=t1:1.0",
        ],
    );
    assert_eq!(
        print(&server, "=t1:", "#{window_index}.#{pane_index}"),
        "1.0"
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_pane_and_select_window_elsewhere_do_not_move_the_client() {
    let server = TestServer::start().await.expect("start server");
    for session in ["alpha", "beta"] {
        run_cli(
            &server,
            ["new-session", "-d", "-s", session, "--", "/bin/sh"],
        );
        run_cli(
            &server,
            ["new-window", "-d", "-t", session, "--", "/bin/sh"],
        );
    }
    let first = print(&server, "=alpha:0", "#{pane_id}");
    let other = out(&run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "=alpha:1",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "/bin/sh",
        ],
    ));
    run_cli(
        &server,
        ["--base-index", "0", "select-window", "-t", "=alpha:0"],
    );
    let (_client, client_id) = attached_client(&server, "alpha").await;

    // Like tmux, select-pane makes the pane its window's active pane but
    // leaves the session's current window alone.
    run_cli(&server, ["select-pane", "-t", &other]);
    assert_eq!(
        print(&server, "=alpha:", "#{window_index} #{pane_id}"),
        format!("0 {first}")
    );
    run_cli(
        &server,
        ["--base-index", "0", "select-window", "-t", "=alpha:1"],
    );
    assert_eq!(
        print(
            &server,
            "=alpha:",
            "#{window_index}.#{pane_index} #{pane_id}"
        ),
        format!("1.1 {other}")
    );

    // In another session's window, the client stays where it is.
    let beta_pane = print(&server, "=beta:1", "#{pane_id}");
    run_cli(&server, ["select-pane", "-t", &beta_pane]);
    assert_eq!(print(&server, "=beta:", "#{window_index}"), "0");
    run_cli(
        &server,
        ["--base-index", "0", "select-window", "-t", "=beta:1"],
    );
    assert_eq!(print(&server, "=beta:", "#{window_index}"), "1");
    assert_eq!(shown_session(&server, client_id), "alpha");

    // In the current window, select-pane focuses the pane.
    let first_of_window = print(&server, "=alpha:1.0", "#{pane_id}");
    run_cli(&server, ["select-pane", "-t", &first_of_window]);
    assert_eq!(
        print(&server, "=alpha:", "#{window_index}.#{pane_index}"),
        "1.0"
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_split_of_another_window_leaves_the_current_window_alone() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "work", "--", "/bin/sh"],
    );
    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    let first = print(&server, "=work:1", "#{pane_id}");
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "=work:1",
            "--",
            "/bin/sh",
        ],
    );

    assert_eq!(print(&server, "=work:", "#{window_index}"), "0");
    assert_eq!(
        print(&server, "=work:1", "#{pane_index} #{pane_id}"),
        format!("0 {first}")
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_target_means_the_pane_select_pane_made_active() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "work", "--", "/bin/sh"],
    );
    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "=work:1",
            "--",
            "/bin/sh",
        ],
    );
    let second = print(&server, "=work:1.1", "#{pane_id}");

    // Window 0 stays current, but window 1 now means its second pane.
    run_cli(
        &server,
        ["--base-index", "0", "select-pane", "-t", "=work:1.1"],
    );
    assert_eq!(print(&server, "=work:", "#{window_index}"), "0");
    assert_eq!(
        print(&server, "=work:1", "#{pane_index} #{pane_id}"),
        format!("1 {second}")
    );
    let windows = out(&run_cli(
        &server,
        [
            "--base-index",
            "0",
            "list-windows",
            "-t",
            "=work",
            "-F",
            "#{window_index} #{pane_id}",
        ],
    ));
    assert_eq!(windows.lines().nth(1), Some(format!("1 {second}").as_str()));

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_pane_moves_from_a_pane_of_another_window() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "work", "--", "/bin/sh"],
    );
    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-h",
            "-t",
            "=work:1",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(
        print(&server, "=work:1.0", "#{pane_at_left}#{pane_at_right}"),
        "10"
    );

    run_cli(
        &server,
        ["--base-index", "0", "select-pane", "-R", "-t", "=work:1.0"],
    );
    assert_eq!(print(&server, "=work:1", "#{pane_index}"), "1");
    assert_eq!(print(&server, "=work:", "#{window_index}"), "0");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detached_new_window_keeps_a_lone_window_current() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "work", "--", "/bin/sh"],
    );
    let first = print(&server, "=work:0", "#{pane_id}");
    // Killing the second window leaves the first as the session's root,
    // outside any tabs.
    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    run_cli(
        &server,
        ["--base-index", "0", "kill-window", "-t", "=work:1"],
    );

    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    assert_eq!(
        print(&server, "=work:", "#{window_index} #{pane_id}"),
        format!("0 {first}")
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_creation_keeps_a_focused_popup_and_the_current_window() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "work", "--", "/bin/sh"],
    );
    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    run_cli(
        &server,
        ["display-popup", "--no-wait", "-t", "work", "--", "/bin/sh"],
    );
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let current = |snapshot: &SessionSnapshot| {
        let root = snapshot
            .nodes
            .iter()
            .find(|node| node.id == snapshot.session.root_node_id)
            .and_then(|node| node.tabs.as_ref())
            .expect("root tabs");
        (
            root.active,
            snapshot.session.focused_floating_id,
            snapshot.session.focused_leaf_id,
        )
    };
    let before = current(&session_snapshot_by_name(&mut connection, "work").await);
    assert_eq!(before.0, 0);
    assert!(before.1.is_some(), "the popup has focus");

    run_cli(&server, ["new-window", "-d", "-t", "work", "--", "/bin/sh"]);
    assert_eq!(
        current(&session_snapshot_by_name(&mut connection, "work").await),
        before
    );
    run_cli(
        &server,
        [
            "--base-index",
            "0",
            "split-window",
            "-d",
            "-t",
            "=work:1",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(
        current(&session_snapshot_by_name(&mut connection, "work").await),
        before
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_clients_takes_a_format() {
    let server = TestServer::start().await.expect("start server");
    run_cli(
        &server,
        ["new-session", "-d", "-s", "alpha", "--", "/bin/sh"],
    );
    run_cli(
        &server,
        ["new-session", "-d", "-s", "beta", "--", "/bin/sh"],
    );
    let (_alpha_client, alpha_id) = attached_client(&server, "alpha").await;
    let (_beta_client, beta_id) = attached_client(&server, "beta").await;

    let listed = out(&run_cli(
        &server,
        [
            "list-clients",
            "-F",
            "#{client_name} #{client_session} #{session_name}:#{window_index} #{pane_id}",
        ],
    ));
    let alpha_pane = print(&server, "alpha:0.0", "#{pane_id}");
    let beta_pane = print(&server, "beta:0.0", "#{pane_id}");
    assert!(
        listed
            .lines()
            .any(|line| line == format!("{alpha_id} alpha alpha:0 {alpha_pane}")),
        "{listed}"
    );
    assert!(
        listed
            .lines()
            .any(|line| line == format!("{beta_id} beta beta:0 {beta_pane}")),
        "{listed}"
    );

    // -t limits the list to the clients showing that session.
    let only_beta = out(&run_cli(
        &server,
        ["list-clients", "-t", "beta", "-F", "#{client_name}"],
    ));
    assert_eq!(only_beta.trim_end(), beta_id.to_string());

    server.shutdown().await.expect("shutdown server");
}
