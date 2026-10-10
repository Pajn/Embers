//! Crash and reboot recovery with the real binary: the server is killed with
//! SIGKILL, so nothing is saved on the way down, and so are its keepers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use embers_core::BufferId;
use embers_server::ServerConfig;
use embers_test_support::{acquire_test_lock, cargo_bin, cargo_bin_path};
use tokio::time::{Instant, sleep};

/// A `__serve` process started directly, so the test can SIGKILL it.
struct ServeProcess {
    child: Child,
}

impl ServeProcess {
    async fn start(socket_path: &Path) -> Self {
        Self::start_with_env(socket_path, &[]).await
    }

    async fn start_with_env(socket_path: &Path, env: &[(&str, &str)]) -> Self {
        let _ = fs::remove_file(socket_path.with_extension("pid"));
        let child = Command::new(cargo_bin_path("embers"))
            .arg("__serve")
            .arg("--socket")
            .arg(socket_path)
            .env("SHELL", "/bin/sh")
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn embers server");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !(socket_path.exists() && try_cli(socket_path, &["list-sessions"]).status.success()) {
            assert!(Instant::now() < deadline, "server never came up");
            sleep(Duration::from_millis(50)).await;
        }
        Self { child }
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn try_cli(socket_path: &Path, args: &[&str]) -> Output {
    cargo_bin("embers")
        .arg("--socket")
        .arg(socket_path)
        .args(args)
        .output()
        .expect("cli command runs")
}

fn cli(socket_path: &Path, args: &[&str]) -> String {
    let output = try_cli(socket_path, args);
    assert!(
        output.status.success(),
        "embers {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

/// Directory holding this server's keeper sockets; every keeper names its
/// socket in its command line, so it also identifies this test's keepers.
fn keeper_dir(socket_path: &Path) -> PathBuf {
    ServerConfig::new(socket_path.to_path_buf())
        .keeper_socket_path(BufferId(1))
        .parent()
        .expect("keeper socket dir")
        .to_path_buf()
}

/// SIGKILL every keeper of this server, as a reboot would, and wait until
/// they are gone.
async fn kill_keepers(socket_path: &Path) {
    let pattern = format!(
        "__runtime-keeper --keeper-socket {}/",
        keeper_dir(socket_path).display()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let _ = Command::new("pkill")
            .args(["-KILL", "-f", &pattern])
            .status();
        let alive = Command::new("pgrep")
            .args(["-f", &pattern])
            .output()
            .expect("run pgrep");
        if alive.stdout.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "keepers survived SIGKILL");
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panes_come_back_in_place_after_the_server_and_keepers_are_killed() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    // Short and canonical: socket paths are length-limited, and the directory
    // must compare equal to the one the OS reports for the shells.
    let root = tempfile::Builder::new()
        .prefix("emb")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let root_path = root.path().canonicalize().expect("canonical tempdir");
    let left_dir = root_path.join("left");
    let right_dir = root_path.join("right");
    fs::create_dir(&left_dir).expect("create left dir");
    fs::create_dir(&right_dir).expect("create right dir");
    let socket_path = root_path.join("s.sock");
    let workspace_path = ServerConfig::new(socket_path.clone()).workspace_path;

    let server = ServeProcess::start(&socket_path).await;
    cli(&socket_path, &["new-session", "main"]);
    cli(&socket_path, &["new-window", "-t", "main", "--", "/bin/sh"]);
    let right_pane = cli(&socket_path, &["split-window", "--", "/bin/sh"]);
    let right_pane = right_pane.trim();
    let panes_before = cli(&socket_path, &["list-panes"]);
    let left_pane = panes_before
        .lines()
        .map(|line| line.split('\t').next().expect("pane id"))
        .find(|pane| *pane != right_pane)
        .expect("left pane")
        .to_owned();

    let cd_left = format!("cd {}", left_dir.display());
    let cd_right = format!("cd {}", right_dir.display());
    cli(
        &socket_path,
        &["send-keys", "-t", &left_pane, &cd_left, "--enter"],
    );
    cli(
        &socket_path,
        &["send-keys", "-t", right_pane, &cd_right, "--enter"],
    );

    // The server saves on its own shortly after a change; no clean shutdown.
    wait_until("both directories to be saved", || {
        fs::read_to_string(&workspace_path).is_ok_and(|contents| {
            contents.contains(left_dir.to_str().expect("utf-8"))
                && contents.contains(right_dir.to_str().expect("utf-8"))
        })
    })
    .await;

    server.kill();
    kill_keepers(&socket_path).await;

    let server = ServeProcess::start(&socket_path).await;
    let panes_after = cli(&socket_path, &["list-panes"]);
    assert_eq!(panes_after, panes_before, "same panes, ids and titles");

    for (pane, dir) in [(left_pane.as_str(), &left_dir), (right_pane, &right_dir)] {
        cli(&socket_path, &["send-keys", "-t", pane, "pwd", "--enter"]);
        let dir = dir.to_str().expect("utf-8").to_owned();
        wait_until(&format!("pane {pane} to print {dir}"), || {
            try_cli(&socket_path, &["capture-pane", "-t", pane])
                .stdout
                .split(|&byte| byte == b'\n')
                .any(|line| line == dir.as_bytes())
        })
        .await;
    }

    server.kill();
    kill_keepers(&socket_path).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_panes_whose_keepers_are_gone_stay_gone() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let root = tempfile::Builder::new()
        .prefix("emb")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let socket_path = root.path().join("s.sock");
    let workspace_path = ServerConfig::new(socket_path.clone()).workspace_path;

    let server = ServeProcess::start(&socket_path).await;
    cli(&socket_path, &["new-session", "main"]);
    cli(&socket_path, &["new-window", "-t", "main", "--", "/bin/sh"]);
    let right_pane = cli(&socket_path, &["split-window", "--", "/bin/sh"]);
    let right_pane = right_pane.trim().to_owned();
    cli(&socket_path, &["new-session", "other"]);
    cli(
        &socket_path,
        &["new-window", "-t", "other", "--", "/bin/sh"],
    );
    wait_until("the three panes to be saved running", || {
        fs::read_to_string(&workspace_path)
            .is_ok_and(|contents| contents.matches("\"running\"").count() >= 3)
    })
    .await;
    server.kill();
    kill_keepers(&socket_path).await;

    // With restore off the panes are left interrupted. Killing them must
    // remove them even though there is no program left to end.
    let server = ServeProcess::start_with_env(&socket_path, &[("EMBERS_RESTORE", "0")]).await;
    let buffers = cli(&socket_path, &["list-buffers"]);
    assert_eq!(
        buffers.matches("interrupted").count(),
        3,
        "every pane is interrupted:\n{buffers}"
    );
    cli(&socket_path, &["kill-pane", "-t", &right_pane]);
    cli(&socket_path, &["kill-session", "-t", "other"]);
    let buffers = cli(&socket_path, &["list-buffers"]);
    assert_eq!(
        buffers.lines().count(),
        1,
        "only main's remaining pane is left:\n{buffers}"
    );
    assert!(
        !buffers.contains("detached"),
        "orphaned buffers:\n{buffers}"
    );
    wait_until("the kills to be saved", || {
        fs::read_to_string(&workspace_path)
            .is_ok_and(|contents| contents.matches("\"interrupted\"").count() == 1)
    })
    .await;
    server.kill();

    // Restore on: only the pane still shown comes back.
    let server = ServeProcess::start(&socket_path).await;
    let buffers = cli(&socket_path, &["list-buffers"]);
    assert_eq!(
        buffers.lines().count(),
        1,
        "killed panes came back:\n{buffers}"
    );
    assert!(buffers.contains("attached"), "{buffers}");

    server.kill();
    kill_keepers(&socket_path).await;
}
