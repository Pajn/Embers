mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use embers_core::PtySize;
use embers_test_support::{PtyHarness, TestServer, cargo_bin, cargo_bin_path};
use tempfile::tempdir;

use support::run_cli;

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
    let pid = fs::read_to_string(&pid_path)
        .unwrap_or_else(|error| panic!("read pid file {}: {error}", pid_path.display()))
        .trim()
        .parse::<i32>()
        .expect("pid parses");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embers_without_subcommand_starts_server_and_client() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["--socket", &socket_arg]);

    harness
        .read_until_contains("[main]", Duration::from_secs(5))
        .expect("client starts and renders");

    harness
        .write_all("embers list-sessions\r")
        .expect("send command inside pane");
    let output = harness
        .read_until_contains("1\tmain", Duration::from_secs(5))
        .expect("pane command output");
    assert!(output.contains("1\tmain"));

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");
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
        .read_until_contains("[main]", Duration::from_secs(5))
        .expect("attach client renders");

    harness
        .write_all("embers list-sessions\r")
        .expect("send command inside attached pane");
    let output = harness
        .read_until_contains("1\tmain", Duration::from_secs(5))
        .expect("attached pane command output");
    assert!(output.contains("1\tmain"));

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_up_enters_local_scrollback_and_shows_indicator() {
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let mut harness = spawn_embers(&["--socket", &socket_arg]);

    harness
        .read_until_contains("[main]", Duration::from_secs(5))
        .expect("client starts and renders");
    let long_output = format!(
        "printf '{}\\n'; echo DONE\r",
        (1..=40)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\\n")
    );
    harness
        .write_all(&long_output)
        .expect("write scrolling command");
    harness
        .read_until_contains("line-39", Duration::from_secs(5))
        .expect("long output rendered");
    tokio::time::sleep(Duration::from_millis(200)).await;

    harness.write_all("\x1b[5~").expect("page up");
    let output = harness
        .read_until_contains("line-1", Duration::from_secs(5))
        .expect("page up reveals earlier scrollback");
    assert!(output.contains("line-1"));

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
        .read_until_contains("[main]", Duration::from_secs(5))
        .expect("client starts and renders");
    let long_output = format!(
        "printf '{}\\n'; echo DONE\r",
        (1..=40)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\\n")
    );
    harness
        .write_all(&long_output)
        .expect("write scrolling command");
    harness
        .read_until_contains("line-39", Duration::from_secs(5))
        .expect("long output rendered");
    tokio::time::sleep(Duration::from_millis(200)).await;

    harness.write_all("\x1b[5~").expect("page up");
    harness
        .read_until_contains("line-1", Duration::from_secs(5))
        .expect("page up reveals earlier scrollback");
    harness.write_all("vly").expect("select and yank");
    let output = harness
        .read_until_contains("]52;c;", Duration::from_secs(5))
        .expect("osc52 emitted");
    assert!(output.contains("]52;c;"));

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");
    shutdown_spawned_server(&socket_path).await;
}
