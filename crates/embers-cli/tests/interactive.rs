use std::fs;
use std::path::Path;
use std::time::Duration;

use embers_core::PtySize;
use embers_test_support::{PtyHarness, TestServer, cargo_bin, cargo_bin_path};
use tempfile::tempdir;

use crate::support::{run_cli, stdout};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const FILE_WAIT_POLL: Duration = Duration::from_millis(50);
const FILE_WAIT_ATTEMPTS: usize = 200;
const SCROLLBACK_SETTLE_DELAY: Duration = Duration::from_millis(750);
const QUIET_TIMEOUT: Duration = Duration::from_millis(500);
const PAGE_UP_ATTEMPTS: usize = 4;

fn spawn_embers(args: &[&str]) -> PtyHarness {
    let binary = cargo_bin_path("embers");
    let binary_dir = binary.parent().expect("binary dir");
    let path = format!(
        "PATH={}:{}",
        binary_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut env_and_args = vec![
        path,
        "SHELL=/bin/sh".to_owned(),
        binary.to_string_lossy().into_owned(),
    ];
    env_and_args.extend(args.iter().map(|arg| (*arg).to_owned()));
    let argv = env_and_args.iter().map(String::as_str).collect::<Vec<_>>();
    PtyHarness::spawn("/usr/bin/env", &argv, PtySize::new(80, 24)).expect("spawn embers in pty")
}

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

    for _ in 0..FILE_WAIT_ATTEMPTS {
        if !socket_path.exists() && !pid_path.exists() {
            return;
        }
        tokio::time::sleep(FILE_WAIT_POLL).await;
    }

    panic!(
        "timed out waiting for spawned server shutdown (socket: {}, pid file: {})",
        socket_path.display(),
        pid_path.display()
    );
}

async fn wait_for_socket(socket_path: &Path) {
    for _ in 0..FILE_WAIT_ATTEMPTS {
        if socket_path.exists() {
            return;
        }
        tokio::time::sleep(FILE_WAIT_POLL).await;
    }

    panic!("timed out waiting for socket {}", socket_path.display());
}

async fn wait_for_pid(pid_path: &Path) -> String {
    for _ in 0..FILE_WAIT_ATTEMPTS {
        if let Ok(pid) = fs::read_to_string(pid_path) {
            return pid;
        }
        tokio::time::sleep(FILE_WAIT_POLL).await;
    }

    panic!("timed out waiting for pid file {}", pid_path.display());
}

async fn populate_scrollback_or_wait(harness: &mut PtyHarness, lines: usize) {
    harness
        .write_all("echo READY\r")
        .expect("write ready command");
    harness
        .read_until_contains("READY", IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("pane ready handshake: {error}"));

    let long_output =
        format!("i=1; while [ $i -le {lines} ]; do echo line-$i; i=$((i+1)); done; echo DONE\r");
    harness
        .write_all(&long_output)
        .expect("write scrolling command");
    harness
        .read_until_contains("line-1", IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("long output started: {error}"));
    harness
        .wait_for_quiet(QUIET_TIMEOUT, IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("long output settled: {error}"));
    tokio::time::sleep(SCROLLBACK_SETTLE_DELAY).await;
}

fn page_up_until_visible(harness: &mut PtyHarness, needle: &str) {
    let mut last_err = None;
    for _ in 0..PAGE_UP_ATTEMPTS {
        harness.write_all("\x1b[5~").expect("page up");
        match harness.read_until_contains(needle, IO_TIMEOUT) {
            Ok(_) => return,
            Err(error) => last_err = Some(error),
        }
    }

    let last_err = last_err
        .map(|error| error.to_string())
        .unwrap_or_else(|| "no read error captured".to_owned());
    panic!("page up did not reveal `{needle}` within {PAGE_UP_ATTEMPTS} attempts: {last_err}");
}

fn run_pane_command(harness: &mut PtyHarness, command: &str, expected: &str) -> String {
    harness
        .write_all(&format!("{command}\r"))
        .unwrap_or_else(|error| panic!("send pane command `{command}`: {error}"));

    let output = harness
        .read_until_contains(expected, IO_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!("pane command `{command}` did not print `{expected}`: {error}")
        });

    assert!(
        output.contains(expected),
        "pane command `{command}` did not print `{expected}`:\n{output}"
    );

    output
}

fn first_client_id(output: &str) -> u64 {
    output
        .lines()
        .find_map(|line| {
            let mut columns = line.split('\t');
            let client_id = columns.next()?;
            let current_session = columns.next()?;
            if current_session == "-" {
                return None;
            }
            Some(client_id)
        })
        .expect("attached client row present")
        .parse::<u64>()
        .expect("client id parses")
}

#[test]
fn first_client_id_finds_attached_row() {
    let output = "10\t-\t-\n42\t1:main\tall\n";
    assert_eq!(first_client_id(output), 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embers_without_subcommand_starts_server_and_client() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["--socket", &socket_arg]);

    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("client starts and renders");

    let output = run_pane_command(&mut harness, "embers list-sessions", "1\tmain");
    assert!(
        output.contains("1\tmain"),
        "expected list-sessions output in pane:\n{output}"
    );

    wait_for_socket(&socket_path).await;

    let output = cargo_bin("embers")
        .arg("list-sessions")
        .arg("--socket")
        .arg(&socket_path)
        .output()
        .expect("cli command runs");
    assert!(
        output.status.success(),
        "list-sessions failed after client exit:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1\tmain"));

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");

    shutdown_spawned_server(&socket_path).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_subcommand_connects_to_running_server() {
    let server = TestServer::start().await.expect("start server");
    let binary = cargo_bin_path("embers");
    let binary_dir = binary.parent().expect("binary dir");
    let shell_path = format!(
        "{}:{}",
        binary_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    run_cli(&server, ["new-session", "main"]);
    run_cli(
        &server,
        vec![
            "new-window".to_owned(),
            "-t".to_owned(),
            "main".to_owned(),
            "--title".to_owned(),
            "shell".to_owned(),
            "--".to_owned(),
            "/usr/bin/env".to_owned(),
            format!("PATH={shell_path}"),
            "/bin/sh".to_owned(),
        ],
    );

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["attach", "--socket", &socket_arg]);
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");

    let output = run_pane_command(&mut harness, "embers list-sessions", "1\tmain");
    assert!(
        output.contains("1\tmain"),
        "expected list-sessions output in attached pane:\n{output}"
    );

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_commands_can_switch_and_detach_a_live_attached_client() {
    let server = TestServer::start().await.expect("start server");

    run_cli(&server, ["new-session", "main"]);
    run_cli(
        &server,
        [
            "new-window",
            "-t",
            "main",
            "--title",
            "shell",
            "--",
            "/bin/sh",
        ],
    );
    run_cli(&server, ["new-session", "ops"]);
    run_cli(
        &server,
        [
            "new-window",
            "-t",
            "ops",
            "--title",
            "shell",
            "--",
            "/bin/sh",
        ],
    );

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["attach", "--socket", &socket_arg, "-t", "main"]);
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders main");

    let clients = run_cli(&server, ["list-clients"]);
    let client_id = first_client_id(&stdout(&clients));

    run_cli(
        &server,
        ["switch-client", &client_id.to_string(), "-t", "ops"],
    );
    harness
        .read_until_contains("[ops]", IO_TIMEOUT)
        .expect("switch-client retargets the live client");

    run_cli(&server, ["detach-client", &client_id.to_string()]);
    harness.wait().expect("client exits after detach");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_up_enters_local_scrollback_and_shows_indicator() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["--socket", &socket_arg]);

    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("client starts and renders");
    populate_scrollback_or_wait(&mut harness, 40).await;

    page_up_until_visible(&mut harness, "line-1");

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");
    shutdown_spawned_server(&socket_path).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_selection_yank_emits_osc52_clipboard_sequence() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["--socket", &socket_arg]);

    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("client starts and renders");
    populate_scrollback_or_wait(&mut harness, 40).await;

    page_up_until_visible(&mut harness, "line-1");
    harness.write_all("vly").expect("select and yank");
    let output = harness
        .read_until_contains("]52;c;", IO_TIMEOUT)
        .expect("osc52 emitted");
    assert!(output.contains("]52;c;"));

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");
    shutdown_spawned_server(&socket_path).await;
}
