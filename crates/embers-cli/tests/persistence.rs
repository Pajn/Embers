//! Crash and reboot recovery with the real binary: the server is killed with
//! SIGKILL, so nothing is saved on the way down, and so are its keepers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use embers_core::{BufferId, RequestId};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferRequest, ClientMessage, ProtocolClient, ServerResponse,
};
use embers_server::{REMAIN_ON_EXIT_OPTION, ServerConfig};
use embers_test_support::{acquire_test_lock, cargo_bin, cargo_bin_path};
use tokio::time::{Instant, sleep};

/// Test hook: while the file this names exists, the server does not save its
/// workspace on its own, which stands in for a crash landing between reading
/// a change and saving it.
const HOLD_SAVES_ENV_VAR: &str = "EMBERS_TEST_HOLD_WORKSPACE_SAVES";

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

async fn buffer_request(socket_path: &Path, request: BufferRequest) -> ServerResponse {
    let mut client = ProtocolClient::connect(socket_path)
        .await
        .expect("connect protocol client");
    client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer request is answered")
}

/// The server's record of `buffer_id`, or `None` once it has none.
async fn buffer_record(socket_path: &Path, buffer_id: BufferId) -> Option<BufferRecord> {
    match buffer_request(
        socket_path,
        BufferRequest::Get {
            request_id: RequestId(1),
            buffer_id,
        },
    )
    .await
    {
        ServerResponse::Buffer(response) => Some(response.buffer),
        ServerResponse::Error(_) => None,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

/// The workspace file's record of `buffer_id`.
fn saved_buffer(workspace_path: &Path, buffer_id: BufferId) -> Option<serde_json::Value> {
    let contents = fs::read_to_string(workspace_path).ok()?;
    let workspace: serde_json::Value = serde_json::from_str(&contents).ok()?;
    workspace["buffers"]
        .as_array()?
        .iter()
        .find(|buffer| buffer["id"].as_u64() == Some(buffer_id.0))
        .cloned()
}

/// The state the workspace file records for `buffer_id`, e.g. `"running"`.
fn saved_buffer_state(workspace_path: &Path, buffer_id: BufferId) -> Option<String> {
    saved_buffer(workspace_path, buffer_id)?["state"]["kind"]
        .as_str()
        .map(str::to_owned)
}

fn keeper_running(socket_path: &Path, buffer_id: BufferId) -> bool {
    let keeper_socket = ServerConfig::new(socket_path.to_path_buf()).keeper_socket_path(buffer_id);
    let pattern = format!(
        "__runtime-keeper --keeper-socket {} ",
        keeper_socket.display()
    );
    !Command::new("pgrep")
        .args(["-f", &pattern])
        .output()
        .expect("run pgrep")
        .stdout
        .is_empty()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panes_that_exited_just_before_a_server_crash_are_not_resurrected() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let root = tempfile::Builder::new()
        .prefix("emb")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let root_path = root.path().canonicalize().expect("canonical tempdir");
    let socket_path = root_path.join("s.sock");
    let release = root_path.join("release");
    let hold = root_path.join("hold");
    let workspace_path = ServerConfig::new(socket_path.clone()).workspace_path;

    let server = ServeProcess::start_with_env(
        &socket_path,
        &[(HOLD_SAVES_ENV_VAR, hold.to_str().expect("utf-8 temp path"))],
    )
    .await;
    cli(&socket_path, &["new-session", "main"]);
    cli(&socket_path, &["new-window", "-t", "main", "--", "/bin/sh"]);
    // Two panes whose commands exit 3 once the test creates `release`: one
    // remains on exit as a dead pane, the other is cleaned up.
    let waits_then_exits = [
        "/bin/sh",
        "-c",
        r#"while [ ! -e "$1" ]; do sleep 0.05; done; exit 3"#,
        "sh",
        release.to_str().expect("utf-8 temp path"),
    ];
    let mut split = vec!["split-window", "--"];
    split.extend(waits_then_exits);
    let kept_pane = cli(&socket_path, &split).trim().to_owned();
    let removed_pane = cli(&socket_path, &split).trim().to_owned();
    let panes = cli(&socket_path, &["list-panes"]);
    let buffer_of = |pane: &str| {
        panes
            .lines()
            .find_map(|line| {
                let mut fields = line.split('\t');
                (fields.next() == Some(pane)).then(|| fields.next().map(str::to_owned))
            })
            .flatten()
            .map(|id| BufferId(id.parse().expect("numeric buffer id")))
            .expect("pane is listed")
    };
    let kept = buffer_of(&kept_pane);
    let removed = buffer_of(&removed_pane);
    match buffer_request(
        &socket_path,
        BufferRequest::SetUserOption {
            request_id: RequestId(2),
            buffer_id: kept,
            key: REMAIN_ON_EXIT_OPTION.to_owned(),
            value: Some("on".to_owned()),
        },
    )
    .await
    {
        ServerResponse::Buffer(_) => {}
        other => panic!("expected buffer response, got {other:?}"),
    }
    wait_until(
        "both panes to be saved running, one remaining on exit",
        || {
            [kept, removed].iter().all(|&buffer_id| {
                saved_buffer_state(&workspace_path, buffer_id).as_deref() == Some("running")
            }) && saved_buffer(&workspace_path, kept).is_some_and(|buffer| {
                buffer["user_options"][REMAIN_ON_EXIT_OPTION].as_str() == Some("on")
            })
        },
    )
    .await;

    // Hold saves, then let the commands exit and wait until the server has
    // read both exits (keeping one dead pane, removing the other): it now
    // knows, but has not saved either.
    fs::write(&hold, b"").expect("hold workspace saves");
    fs::write(&release, b"").expect("release the commands");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let kept_exited = buffer_record(&socket_path, kept)
            .await
            .is_some_and(|record| record.state == BufferRecordState::Exited);
        if kept_exited && buffer_record(&socket_path, removed).await.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server never saw the commands exit"
        );
        sleep(Duration::from_millis(25)).await;
    }
    for buffer_id in [kept, removed] {
        assert_eq!(
            saved_buffer_state(&workspace_path, buffer_id).as_deref(),
            Some("running"),
            "the exit of buffer {buffer_id} must not have been saved before the crash"
        );
    }

    server.kill();
    // Restore reruns a pane's command, so withdraw the release: a resurrected
    // command would keep running instead of exiting 3. Then give the keepers
    // several socket-check intervals to (wrongly) leave.
    fs::remove_file(&release).expect("withdraw the release");
    fs::remove_file(&hold).expect("stop holding saves");
    sleep(Duration::from_millis(1500)).await;
    assert!(keeper_running(&socket_path, kept));
    assert!(keeper_running(&socket_path, removed));

    // The restarted server reattaches to both keepers and learns how the
    // commands ended instead of running them again.
    let server = ServeProcess::start(&socket_path).await;
    let restored = buffer_record(&socket_path, kept)
        .await
        .expect("the dead pane is kept");
    assert_eq!(restored.state, BufferRecordState::Exited);
    assert_eq!(restored.exit_code, Some(3));
    assert!(
        buffer_record(&socket_path, removed).await.is_none(),
        "the exited pane without remain-on-exit came back"
    );
    // Once its removal is saved, the removed pane's keeper is released.
    wait_until("the removed pane's keeper to exit", || {
        !keeper_running(&socket_path, removed)
    })
    .await;

    server.kill();
    kill_keepers(&socket_path).await;
}
