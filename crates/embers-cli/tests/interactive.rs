#[cfg(target_os = "macos")]
use std::ffi::CStr;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use embers_core::{ActivityState, BufferId, NodeId, PtySize, new_request_id};
use embers_protocol::{
    BufferRequest, ClientMessage, InputRequest, ServerResponse, SessionRequest, SessionSnapshot,
    VisibleSnapshotResponse,
};
use embers_test_support::{
    PtyHarness, TestConnection, TestServer, acquire_test_lock, cargo_bin, cargo_bin_path,
};
use filetime::FileTime;

use crate::support::{require_pty, run_cli, session_snapshot_by_name, stdout};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const FILE_WAIT_POLL: Duration = Duration::from_millis(50);
const FILE_WAIT_ATTEMPTS: usize = 200;
const SCROLLBACK_SETTLE_DELAY: Duration = Duration::from_millis(750);
const QUIET_TIMEOUT: Duration = Duration::from_millis(500);
const PAGE_UP_ATTEMPTS: usize = 4;

/// A guard that owns the spawned embers process and ensures cleanup
/// of orphaned __serve processes when dropped.
struct SpawnedEmbers {
    socket_path: PathBuf,
    started_server: bool,
}

impl SpawnedEmbers {
    fn new(socket_path: PathBuf, started_server: bool) -> Self {
        Self {
            socket_path,
            started_server,
        }
    }
}

impl Drop for SpawnedEmbers {
    fn drop(&mut self) {
        if !self.started_server {
            return;
        }
        // Kill any orphaned __serve process for our socket
        kill_orphaned_server(&self.socket_path);
    }
}

/// Kill any orphaned embers __serve process for the given socket.
/// This is safe to call from Drop or any synchronous context.
fn kill_orphaned_server(socket_path: &Path) {
    let pid_path = socket_path.with_extension("pid");

    // A clean SIGTERM path returns here; failed waits fall through to the SIGKILL retry below.
    let matched_pid = try_signal_server(&pid_path, socket_path, libc::SIGTERM);
    if let Some(matched_pid) = matched_pid
        && wait_for_server_exit(matched_pid, socket_path)
    {
        if read_pid(&pid_path) == Some(matched_pid) {
            let _ = fs::remove_file(&pid_path);
        }
        return;
    }

    // Wait briefly for graceful shutdown
    std::thread::sleep(Duration::from_millis(50));

    let matched_pid = try_signal_server(&pid_path, socket_path, libc::SIGKILL);
    if let Some(matched_pid) = matched_pid
        && wait_for_server_exit(matched_pid, socket_path)
        && read_pid(&pid_path) == Some(matched_pid)
    {
        let _ = fs::remove_file(&pid_path);
    }
}

fn try_signal_server(pid_path: &Path, socket_path: &Path, signal: i32) -> Option<i32> {
    let pid = read_pid(pid_path)?;
    if !pid_matches_serve_process(pid, socket_path) {
        return None;
    }
    // SAFETY: pid was read from our pid file and verified against the active __serve command line.
    if unsafe { libc::kill(pid, signal) } != 0 {
        return None;
    }
    Some(pid)
}

fn wait_for_server_exit(pid: i32, socket_path: &Path) -> bool {
    for _ in 0..20 {
        if !pid_matches_serve_process(pid, socket_path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !pid_matches_serve_process(pid, socket_path)
}

fn read_pid(pid_path: &Path) -> Option<i32> {
    let pid = fs::read_to_string(pid_path).ok()?;
    let pid = pid.trim().parse::<i32>().ok()?;
    (pid > 0).then_some(pid)
}

fn pid_matches_serve_process(pid: i32, socket_path: &Path) -> bool {
    let expected_binary = cargo_bin_path("embers");
    let Some((exe_path, argv)) = process_exe_and_argv(pid) else {
        return false;
    };

    same_path(&exe_path, &expected_binary)
        && argv
            .first()
            .is_some_and(|arg| same_path(Path::new(arg.as_os_str()), &expected_binary))
        && argv.get(1).is_some_and(|arg| arg == OsStr::new("__serve"))
        && argv.windows(2).any(|window| {
            window[0] == OsStr::new("--socket")
                && same_path(Path::new(window[1].as_os_str()), socket_path)
        })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_exe_and_argv(pid: i32) -> Option<(PathBuf, Vec<OsString>)> {
    let exe_path = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv = split_nul_args(&cmdline)?;
    Some((exe_path, argv))
}

#[cfg(target_os = "macos")]
fn process_exe_and_argv(pid: i32) -> Option<(PathBuf, Vec<OsString>)> {
    let exe_path = process_exe_path(pid)?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0usize;
    // SAFETY: `mib` names the procargs sysctl and `size` is a valid output parameter.
    let size_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            u32::try_from(mib.len()).ok()?,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if size_result != 0 || size == 0 {
        return None;
    }

    let mut bytes = vec![0u8; size];
    // SAFETY: `bytes` is allocated to the kernel-reported size and all pointers remain valid.
    let read_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            u32::try_from(mib.len()).ok()?,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read_result != 0 || size == 0 {
        return None;
    }
    bytes.truncate(size);
    let argv = parse_macos_argv(&bytes)?;
    Some((exe_path, argv))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn process_exe_and_argv(_pid: i32) -> Option<(PathBuf, Vec<OsString>)> {
    None
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn split_nul_args(bytes: &[u8]) -> Option<Vec<OsString>> {
    let args = bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| OsString::from_vec(arg.to_vec()))
        .collect::<Vec<_>>();
    (!args.is_empty()).then_some(args)
}

#[cfg(target_os = "macos")]
fn parse_macos_argv(bytes: &[u8]) -> Option<Vec<OsString>> {
    let argc = i32::from_ne_bytes(bytes.get(..std::mem::size_of::<i32>())?.try_into().ok()?);
    if argc < 0 {
        return None;
    }
    let argc = usize::try_from(argc).ok()?;
    if argc > bytes.len() {
        return None;
    }
    let mut index = std::mem::size_of::<i32>();
    while bytes.get(index).is_some_and(|byte| *byte != 0) {
        index += 1;
    }
    while bytes.get(index).is_some_and(|byte| *byte == 0) {
        index += 1;
    }

    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        let start = index;
        while bytes.get(index).is_some_and(|byte| *byte != 0) {
            index += 1;
        }
        if start == index {
            return None;
        }
        argv.push(OsString::from_vec(bytes[start..index].to_vec()));
        while bytes.get(index).is_some_and(|byte| *byte == 0) {
            index += 1;
        }
    }

    (!argv.is_empty()).then_some(argv)
}

#[cfg(target_os = "macos")]
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut libc::c_void, buffersize: u32) -> i32;
}

#[cfg(target_os = "macos")]
fn process_exe_path(pid: i32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    // SAFETY: `buffer` is a valid writable output buffer for `proc_pidpath`.
    let length = unsafe {
        proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if length <= 0 {
        return None;
    }

    // SAFETY: successful `proc_pidpath` writes a NUL-terminated path into `buffer`.
    let path = unsafe { CStr::from_ptr(buffer.as_ptr().cast()) };
    Some(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

/// Spawn an embers client process with the given arguments and return a guard
/// that ensures cleanup of any orphaned __serve process when dropped.
/// The socket_path should be the path to the socket - if a server needs to be
/// spawned for it, the guard will clean it up.
fn spawn_embers(args: &[&str], socket_path: PathBuf) -> (SpawnedEmbers, PtyHarness) {
    let binary = cargo_bin_path("embers");
    let binary_dir = binary.parent().expect("binary dir");
    let started_server = !socket_path.exists() && !socket_path.with_extension("pid").exists();
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
    let harness = PtyHarness::spawn("/usr/bin/env", &argv, PtySize::new(80, 24))
        .expect("spawn embers in pty");
    (SpawnedEmbers::new(socket_path, started_server), harness)
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

fn focused_pane_id(snapshot: &SessionSnapshot) -> u64 {
    snapshot
        .session
        .focused_leaf_id
        .map(|leaf_id| leaf_id.0)
        .expect("session has a focused pane")
}

fn pane_buffer_id(snapshot: &SessionSnapshot, pane_id: u64) -> BufferId {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == NodeId(pane_id))
        .and_then(|node| node.buffer_view.as_ref())
        .map(|view| view.buffer_id)
        .unwrap_or_else(|| panic!("pane {pane_id} buffer view exists"))
}

fn root_tab_child_id(snapshot: &SessionSnapshot, title: &str) -> NodeId {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .and_then(|node| node.tabs.as_ref())
        .and_then(|tabs| {
            tabs.tabs
                .iter()
                .find(|tab| tab.title == title)
                .map(|tab| tab.child_id)
        })
        .unwrap_or_else(|| panic!("root tab `{title}` exists"))
}

fn split_child_order(
    snapshot: &SessionSnapshot,
    first_pane_id: u64,
    second_pane_id: u64,
) -> Option<[u64; 2]> {
    snapshot.nodes.iter().find_map(|node| {
        let split = node.split.as_ref()?;
        let child_ids = split
            .child_ids
            .iter()
            .map(|child| child.0)
            .collect::<Vec<_>>();
        if child_ids.len() == 2
            && child_ids.contains(&first_pane_id)
            && child_ids.contains(&second_pane_id)
        {
            Some([child_ids[0], child_ids[1]])
        } else {
            None
        }
    })
}

async fn disable_echo_in_pane(server: &TestServer, pane_id: u64) {
    let marker = format!("__ECHO_DISABLED_{pane_id}__");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_containing_pane(&mut connection, pane_id).await;
    let buffer_id = pane_buffer_id(&snapshot, pane_id);
    send_buffer_input(
        &mut connection,
        buffer_id,
        format!("stty -echo; printf '{marker}\\n'\r").as_bytes(),
    )
    .await;
    connection
        .wait_for_capture_contains(buffer_id, &marker, IO_TIMEOUT)
        .await
        .expect("echo-disable marker appears");
}

async fn send_buffer_input(connection: &mut TestConnection, buffer_id: BufferId, bytes: &[u8]) {
    let response = connection
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: new_request_id(),
            buffer_id,
            bytes: bytes.to_vec(),
        }))
        .await
        .expect("send input succeeds");
    assert!(
        matches!(response, ServerResponse::Ok(_)),
        "expected ok response to input send, got {response:?}"
    );
}

async fn wait_for_buffer_activity(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    expected: ActivityState,
) {
    let deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        let response = connection
            .request(&ClientMessage::Buffer(BufferRequest::Get {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await
            .expect("buffer get succeeds");
        let activity = match response {
            ServerResponse::Buffer(response) => response.buffer.activity,
            other => panic!("expected buffer response, got {other:?}"),
        };
        if activity == expected {
            return;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for buffer {buffer_id} activity {expected:?}; last activity {activity:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_target_pane_buffer(
    connection: &mut TestConnection,
    session_name: &str,
    pane_id: u64,
    expected_buffer_id: BufferId,
) -> SessionSnapshot {
    let deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        let snapshot = session_snapshot_by_name(connection, session_name).await;
        if pane_buffer_id(&snapshot, pane_id) == expected_buffer_id {
            return snapshot;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for pane {pane_id} to attach buffer {expected_buffer_id}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn session_snapshot_containing_pane(
    connection: &mut TestConnection,
    pane_id: u64,
) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::List {
            request_id: new_request_id(),
        }))
        .await
        .expect("list sessions succeeds");
    let sessions = match response {
        ServerResponse::Sessions(response) => response.sessions,
        other => panic!("expected sessions response, got {other:?}"),
    };
    for session in sessions {
        let snapshot = connection
            .session_snapshot(session.id)
            .await
            .expect("session snapshot succeeds");
        if snapshot.nodes.iter().any(|node| node.id == NodeId(pane_id)) {
            return snapshot;
        }
    }
    panic!("pane {pane_id} missing from all sessions");
}

async fn wait_for_split_child_order(
    connection: &mut TestConnection,
    session_name: &str,
    pane_a: u64,
    pane_b: u64,
    expected: [u64; 2],
) -> SessionSnapshot {
    let deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        let snapshot = session_snapshot_by_name(connection, session_name).await;
        let current_order = split_child_order(&snapshot, pane_a, pane_b);
        if current_order == Some(expected) {
            return snapshot;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for split order {:?}; last order {:?}",
            expected,
            current_order
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_visible_snapshot<F>(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    mut predicate: F,
) -> VisibleSnapshotResponse
where
    F: FnMut(&VisibleSnapshotResponse) -> bool,
{
    let deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        let snapshot = connection
            .capture_visible_buffer(buffer_id)
            .await
            .expect("visible capture succeeds");
        if predicate(&snapshot) {
            return snapshot;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for visible snapshot predicate; last snapshot: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embers_without_subcommand_starts_server_and_client() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let (_spawned, mut harness) = spawn_embers(&["--socket", &socket_arg], socket_path.clone());

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

    // spawned.drop() will clean up the orphaned __serve process
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_subcommand_connects_to_running_server() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(&["attach", "--socket", &socket_arg], socket_path);
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
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(
        &["attach", "--socket", &socket_arg, "-t", "main"],
        socket_path,
    );
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
async fn buffer_reveal_switches_the_attached_client_to_the_buffer_session() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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
            "logs",
            "--",
            "/bin/sh",
        ],
    );

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let ops_snapshot = session_snapshot_by_name(&mut connection, "ops").await;
    let ops_buffer_id = ops_snapshot
        .session
        .focused_leaf_id
        .and_then(|leaf_id| {
            ops_snapshot
                .nodes
                .iter()
                .find(|node| node.id == leaf_id)
                .and_then(|node| node.buffer_view.as_ref())
                .map(|view| view.buffer_id.0)
        })
        .expect("ops focused buffer id exists");

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(
        &["attach", "--socket", &socket_arg, "-t", "main"],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders main");

    run_cli(&server, ["buffer", "reveal", &ops_buffer_id.to_string()]);
    harness
        .read_until_contains("[ops]", IO_TIMEOUT)
        .expect("buffer reveal retargets the live client");

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_up_enters_local_scrollback() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let (_spawned, mut harness) = spawn_embers(&["--socket", &socket_arg], socket_path.clone());

    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("client starts and renders");
    populate_scrollback_or_wait(&mut harness, 40).await;

    page_up_until_visible(&mut harness, "line-1");

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");

    // spawned.drop() will clean up the orphaned __serve process
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_selection_yank_emits_osc52_clipboard_sequence() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("embers.sock");
    let socket_arg = socket_path.to_string_lossy().into_owned();
    let (_spawned, mut harness) = spawn_embers(&["--socket", &socket_arg], socket_path.clone());

    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("client starts and renders");
    populate_scrollback_or_wait(&mut harness, 40).await;

    page_up_until_visible(&mut harness, "line-1");
    harness
        .wait_for_quiet(Duration::from_millis(200), IO_TIMEOUT)
        .unwrap_or_else(|error| panic!("scrollback render settled: {error}"));
    harness.write_all("vly").expect("select and yank");
    let output = harness
        .read_until_contains("]52;c;", IO_TIMEOUT)
        .expect("osc52 emitted");
    assert!(output.contains("]52;c;"));

    harness.write_all("\x11").expect("quit client");
    harness.wait().expect("client exits");

    // spawned.drop() will clean up the orphaned __serve process
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scripted_input_bindings_reach_the_live_terminal_in_pty() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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

    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = tempdir.path().join("config.rhai");
    fs::write(
        &config_path,
        r#"bind("normal", "<C-g>", action.send_bytes_current("echo scripted-pty\r"))"#,
    )
    .expect("write config");

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let config_arg = config_path.to_string_lossy().into_owned();
    let (_spawned, mut harness) = spawn_embers(
        &[
            "attach",
            "--socket",
            &socket_arg,
            "--config",
            &config_arg,
            "-t",
            "main",
        ],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");
    harness
        .write_all("stty -echo\r")
        .expect("disable shell echo in focused pane");
    harness
        .wait_for_quiet(QUIET_TIMEOUT, IO_TIMEOUT)
        .expect("focused shell settles");

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "main").await;
    let buffer_id = pane_buffer_id(&snapshot, focused_pane_id(&snapshot));

    harness.write_all("\x07").expect("trigger scripted binding");
    harness
        .read_until_contains("scripted-pty", IO_TIMEOUT)
        .expect("scripted output renders");
    connection
        .wait_for_capture_contains(buffer_id, "scripted-pty", IO_TIMEOUT)
        .await
        .expect("scripted output reaches focused buffer");

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_reload_updates_live_bindings_without_breaking_terminal_io() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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

    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = tempdir.path().join("config.rhai");
    fs::write(
        &config_path,
        r#"bind("normal", "<C-g>", action.send_bytes_current("echo before-reload\r"))"#,
    )
    .expect("write initial config");

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let config_arg = config_path.to_string_lossy().into_owned();
    let (_spawned, mut harness) = spawn_embers(
        &[
            "attach",
            "--socket",
            &socket_arg,
            "--config",
            &config_arg,
            "-t",
            "main",
        ],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");
    harness
        .write_all("stty -echo\r")
        .expect("disable shell echo in focused pane");
    harness
        .wait_for_quiet(QUIET_TIMEOUT, IO_TIMEOUT)
        .expect("focused shell settles");

    harness.write_all("\x07").expect("trigger initial binding");
    harness
        .read_until_contains("before-reload", IO_TIMEOUT)
        .expect("initial binding renders");

    fs::write(
        &config_path,
        r#"bind("normal", "<C-g>", action.send_bytes_current("echo after-reload\r"))"#,
    )
    .expect("write reloaded config");
    filetime::set_file_mtime(&config_path, FileTime::now()).expect("bump reloaded config mtime");
    let reload_deadline = tokio::time::Instant::now() + IO_TIMEOUT;
    loop {
        harness.write_all("\x07").expect("trigger reloaded binding");
        if harness
            .read_until_contains("after-reload", Duration::from_millis(200))
            .is_ok()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < reload_deadline,
            "timed out waiting for reloaded binding to activate"
        );
    }

    let output = run_pane_command(&mut harness, "echo still-live", "still-live");
    assert!(
        output.contains("still-live"),
        "regular terminal input must still work after reload:\n{output}"
    );

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pty_client_preserves_buffers_across_layout_and_attachment_changes() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(
        &["attach", "--socket", &socket_arg, "-t", "main"],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");

    let split = run_cli(&server, ["split-window", "--", "/bin/sh"]);
    let moving_pane_id = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns new pane id");
    disable_echo_in_pane(&server, moving_pane_id).await;

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "main").await;
    let anchor_pane_id = snapshot
        .nodes
        .iter()
        .filter(|node| node.buffer_view.is_some())
        .map(|node| node.id.0)
        .find(|pane_id| *pane_id != moving_pane_id)
        .expect("anchor pane exists");
    let moving_buffer_id = pane_buffer_id(&snapshot, moving_pane_id);

    run_cli(
        &server,
        [
            "send-keys",
            "-t",
            &moving_pane_id.to_string(),
            "--enter",
            "echo",
            "split-live",
        ],
    );
    connection
        .wait_for_capture_contains(moving_buffer_id, "split-live", IO_TIMEOUT)
        .await
        .expect("split pane keeps running");
    harness
        .read_until_contains("split-live", IO_TIMEOUT)
        .expect("split output renders in attached client");

    let initial_order = split_child_order(&snapshot, anchor_pane_id, moving_pane_id)
        .expect("split containing anchor and moving panes exists");
    let expected_order = if initial_order == [anchor_pane_id, moving_pane_id] {
        run_cli(
            &server,
            [
                "node",
                "move-before",
                &moving_pane_id.to_string(),
                &anchor_pane_id.to_string(),
            ],
        );
        [moving_pane_id, anchor_pane_id]
    } else {
        run_cli(
            &server,
            [
                "node",
                "move-after",
                &moving_pane_id.to_string(),
                &anchor_pane_id.to_string(),
            ],
        );
        [anchor_pane_id, moving_pane_id]
    };
    let _moved_snapshot = wait_for_split_child_order(
        &mut connection,
        "main",
        anchor_pane_id,
        moving_pane_id,
        expected_order,
    )
    .await;

    run_cli(
        &server,
        [
            "send-keys",
            "-t",
            &moving_pane_id.to_string(),
            "--enter",
            "echo",
            "moved-live",
        ],
    );
    connection
        .wait_for_capture_contains(moving_buffer_id, "moved-live", IO_TIMEOUT)
        .await
        .expect("moved pane keeps running");
    harness
        .read_until_contains("moved-live", IO_TIMEOUT)
        .expect("moved pane output still renders");

    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Detach {
            request_id: new_request_id(),
            buffer_id: moving_buffer_id,
        }))
        .await
        .expect("detach buffer succeeds");
    assert!(
        matches!(response, ServerResponse::Ok(_)),
        "expected ok response to buffer detach, got {response:?}"
    );

    send_buffer_input(&mut connection, moving_buffer_id, b"echo detached-live\r").await;
    connection
        .wait_for_capture_contains(moving_buffer_id, "detached-live", IO_TIMEOUT)
        .await
        .expect("detached buffer continues receiving output");

    run_cli(
        &server,
        [
            "attach-buffer",
            &moving_buffer_id.to_string(),
            "-t",
            &anchor_pane_id.to_string(),
        ],
    );
    let _reattached_snapshot =
        wait_for_target_pane_buffer(&mut connection, "main", anchor_pane_id, moving_buffer_id)
            .await;

    send_buffer_input(&mut connection, moving_buffer_id, b"echo reattach-live\r").await;
    connection
        .wait_for_capture_contains(moving_buffer_id, "reattach-live", IO_TIMEOUT)
        .await
        .expect("reattached buffer continues receiving output");
    harness
        .read_until_contains("reattach-live", IO_TIMEOUT)
        .expect("reattached buffer output renders in attached client");

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_buffer_bells_surface_in_the_attached_client_and_reveal_buffered_output() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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
    run_cli(
        &server,
        ["new-window", "-t", "main", "--title", "bg", "--", "/bin/sh"],
    );
    run_cli(&server, ["select-window", "-t", "main:0"]);

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "main").await;
    let hidden_pane_id = root_tab_child_id(&snapshot, "bg").0;
    let hidden_buffer_id = pane_buffer_id(&snapshot, hidden_pane_id);
    disable_echo_in_pane(&server, hidden_pane_id).await;

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(
        &["attach", "--socket", &socket_arg, "-t", "main"],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");

    send_buffer_input(
        &mut connection,
        hidden_buffer_id,
        b"printf 'hidden-bell\\n\\a'; sleep 0.5\r",
    )
    .await;
    connection
        .wait_for_capture_contains(hidden_buffer_id, "hidden-bell", IO_TIMEOUT)
        .await
        .expect("hidden buffer output accumulates");
    wait_for_buffer_activity(&mut connection, hidden_buffer_id, ActivityState::Bell).await;
    harness
        .read_until_contains("!bg", IO_TIMEOUT)
        .expect("hidden bell updates tab marker in attached client");

    run_cli(&server, ["select-window", "-t", "main:bg"]);
    harness
        .read_until_contains("hidden-bell", IO_TIMEOUT)
        .expect("revealed hidden buffer shows accumulated output");

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fullscreen_terminal_transitions_render_in_the_live_client_pty() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    if !require_pty() {
        return;
    }
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

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect protocol client");
    let snapshot = session_snapshot_by_name(&mut connection, "main").await;
    let buffer_id = pane_buffer_id(&snapshot, focused_pane_id(&snapshot));

    let socket_arg = server.socket_path().to_string_lossy().into_owned();
    let socket_path = server.socket_path().to_path_buf();
    let (_spawned, mut harness) = spawn_embers(
        &["attach", "--socket", &socket_arg, "-t", "main"],
        socket_path,
    );
    harness
        .read_until_contains("[main]", STARTUP_TIMEOUT)
        .expect("attach client renders");
    harness
        .write_all("stty -echo\r")
        .expect("disable shell echo in focused pane");
    harness
        .wait_for_quiet(QUIET_TIMEOUT, IO_TIMEOUT)
        .expect("focused shell settles");

    harness
        .write_all(
            "printf '\\033[?1049h\\033[2J\\033[HPTY-FULLSCREEN'; sleep 1; printf '\\033[?1049lPTY-RESTORED\\n'\r",
        )
        .expect("run fullscreen fixture");
    harness
        .read_until_contains("PTY-FULLSCREEN", IO_TIMEOUT)
        .expect("fullscreen output renders in live client");

    let live = wait_for_visible_snapshot(&mut connection, buffer_id, |snapshot| {
        snapshot.alternate_screen
            && snapshot
                .lines
                .iter()
                .any(|line| line.contains("PTY-FULLSCREEN"))
    })
    .await;
    assert!(live.alternate_screen);

    harness
        .read_until_contains("PTY-RESTORED", IO_TIMEOUT)
        .expect("primary screen restoration renders in live client");
    let restored = wait_for_visible_snapshot(&mut connection, buffer_id, |snapshot| {
        !snapshot.alternate_screen
            && snapshot
                .lines
                .iter()
                .any(|line| line.contains("PTY-RESTORED"))
    })
    .await;
    assert!(!restored.alternate_screen);

    harness.write_all("\x11").expect("quit attached client");
    harness.wait().expect("client exits");
    server.shutdown().await.expect("shutdown server");
}
