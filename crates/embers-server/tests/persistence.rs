use std::path::Path;

use embers_core::{BufferId, RequestId, init_test_tracing};
use embers_protocol::{
    BufferRecordState, BufferRequest, BufferResponse, BuffersResponse, ClientMessage, InputRequest,
    ProtocolClient, ServerResponse, SessionRequest, SessionSnapshotResponse, SnapshotResponse,
};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, Instant, sleep};

async fn request_session_snapshot(
    client: &mut ProtocolClient,
    request: SessionRequest,
) -> SessionSnapshotResponse {
    match client
        .request(&ClientMessage::Session(request))
        .await
        .expect("session request succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response,
        other => panic!("expected session snapshot, got {other:?}"),
    }
}

async fn request_buffer(client: &mut ProtocolClient, request: BufferRequest) -> BufferResponse {
    match client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer request succeeds")
    {
        ServerResponse::Buffer(response) => response,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn request_buffers(client: &mut ProtocolClient, request: BufferRequest) -> BuffersResponse {
    match client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer list succeeds")
    {
        ServerResponse::Buffers(response) => response,
        other => panic!("expected buffers response, got {other:?}"),
    }
}

async fn wait_for_snapshot_line(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    expected: &str,
) -> SnapshotResponse {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(ServerResponse::Snapshot(snapshot)) = client
            .request(&ClientMessage::Buffer(BufferRequest::Capture {
                request_id,
                buffer_id,
            }))
            .await
            && snapshot.lines.iter().any(|line| line.contains(expected))
        {
            return snapshot;
        }

        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    panic!(
        "capture for buffer {buffer_id} did not contain expected line '{expected}' before timeout"
    );
}

async fn wait_for_running_buffer(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
) -> embers_protocol::BufferRecord {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let response = request_buffer(
            client,
            BufferRequest::Get {
                request_id,
                buffer_id,
            },
        )
        .await;
        if response.buffer.state == BufferRecordState::Running {
            return response.buffer;
        }
        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    panic!("buffer {buffer_id} did not reach Running before timeout");
}

#[tokio::test]
async fn clean_restart_restores_workspace_and_keeps_live_buffers_running() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());
    let workspace_path = config.workspace_path.clone();

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Create {
            request_id: RequestId(1),
            name: "main".to_owned(),
        },
    )
    .await;
    let session_id = session.snapshot.session.id;

    let attached = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(2),
            title: Some("attached".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;

    let detached = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(3),
            title: Some("detached".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;

    let attached_id = attached.id;
    let detached_id = detached.id;

    let restored_layout = request_session_snapshot(
        &mut client,
        SessionRequest::AddRootTab {
            request_id: RequestId(4),
            session_id,
            title: "shell".to_owned(),
            buffer_id: Some(attached_id),
            child_node_id: None,
        },
    )
    .await;
    assert_eq!(restored_layout.snapshot.session.id, session_id);

    let deadline = Instant::now() + Duration::from_secs(2);
    let attached_running = loop {
        let response = request_buffer(
            &mut client,
            BufferRequest::Get {
                request_id: RequestId(40),
                buffer_id: attached_id,
            },
        )
        .await;
        if response.buffer.state == BufferRecordState::Running {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(25)).await;
    };
    assert!(
        attached_running,
        "attached buffer {attached_id} did not reach Running before shutdown"
    );

    handle.shutdown().await.expect("shutdown server");
    assert!(workspace_path.exists());

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Get {
            request_id: RequestId(5),
            session_id,
        },
    )
    .await;
    assert_eq!(session.snapshot.session.name, "main");
    let attached_buffer = session
        .snapshot
        .buffers
        .iter()
        .find(|buffer| buffer.id == attached_id)
        .expect("attached buffer restored");
    assert_eq!(attached_buffer.state, BufferRecordState::Running);
    assert!(attached_buffer.attachment_node_id.is_some());

    let buffers = request_buffers(
        &mut client,
        BufferRequest::List {
            request_id: RequestId(6),
            session_id: None,
            attached_only: false,
            detached_only: false,
        },
    )
    .await;
    let detached_buffer = buffers
        .buffers
        .iter()
        .find(|buffer| buffer.id == detached_id)
        .expect("detached buffer restored");
    assert_eq!(detached_buffer.state, BufferRecordState::Running);
    assert_eq!(detached_buffer.attachment_node_id, None);

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(7),
            buffer_id: attached_id,
            bytes: b"printf restarted-attached\\n\r".to_vec(),
        }))
        .await
        .expect("send to attached buffer succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(8),
            buffer_id: detached_id,
            bytes: b"printf restarted-detached\\n\r".to_vec(),
        }))
        .await
        .expect("send to detached buffer succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }

    let _attached_capture =
        wait_for_snapshot_line(&mut client, RequestId(9), attached_id, "restarted-attached").await;

    let _detached_capture = wait_for_snapshot_line(
        &mut client,
        RequestId(10),
        detached_id,
        "restarted-detached",
    )
    .await;

    handle.shutdown().await.expect("shutdown restarted server");
}

#[tokio::test]
async fn clean_restart_stops_ephemeral_buffer_pipes() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let buffer = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(101),
            title: Some("piped".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;
    let buffer_id = buffer.id;
    let _ = wait_for_running_buffer(&mut client, RequestId(102), buffer_id).await;

    let started = request_buffer(
        &mut client,
        BufferRequest::StartPipe {
            request_id: RequestId(103),
            buffer_id,
            command: vec!["/bin/cat".to_owned()],
            cwd: None,
            env: Default::default(),
        },
    )
    .await
    .buffer;
    assert!(
        started
            .pipe
            .as_ref()
            .is_some_and(|pipe| pipe.state == embers_protocol::BufferPipeState::Running)
    );

    handle.shutdown().await.expect("shutdown server");

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");

    let restored = request_buffer(
        &mut client,
        BufferRequest::Get {
            request_id: RequestId(104),
            buffer_id,
        },
    )
    .await
    .buffer;
    assert_eq!(restored.state, BufferRecordState::Running);
    let restored_pipe = restored
        .pipe
        .expect("restored buffer keeps stopped pipe metadata");
    assert_eq!(
        restored_pipe.state,
        embers_protocol::BufferPipeState::Stopped
    );
    assert_eq!(
        restored_pipe.stop_reason,
        Some(embers_protocol::BufferPipeStopReason::Requested)
    );

    handle.shutdown().await.expect("shutdown restarted server");
}

/// `sockaddr_un.sun_path` size: 104 bytes on macOS, 108 on other Unix
/// platforms. A bindable path must leave room for the trailing NUL.
#[cfg(target_os = "macos")]
const SUN_PATH_LEN: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH_LEN: usize = 108;

#[tokio::test]
async fn long_server_socket_path_still_spawns_and_restores_keepers() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    init_test_tracing();

    // Pad the directory so the server socket path is a few bytes under the
    // limit: still bindable itself, but too long to nest keeper sockets in a
    // directory derived from it.
    let tempdir = tempdir().expect("tempdir");
    let base_len = tempdir.path().as_os_str().as_bytes().len();
    let socket_len = SUN_PATH_LEN - 8;
    let padding = socket_len
        .checked_sub(base_len + "/".len() + "/mux.sock".len())
        .expect("temp dir leaves room for a padded socket path");
    let socket_dir = tempdir.path().join("d".repeat(padding));
    std::fs::create_dir(&socket_dir).expect("create socket dir");
    let socket_path = socket_dir.join("mux.sock");
    assert_eq!(socket_path.as_os_str().as_bytes().len(), socket_len);

    let config = ServerConfig::new(socket_path.clone());

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let buffer_id = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(201),
            title: Some("long".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer
    .id;
    let _ = wait_for_running_buffer(&mut client, RequestId(202), buffer_id).await;

    let keeper_socket = config.keeper_socket_path(buffer_id);
    let keeper_dir = keeper_socket.parent().expect("keeper dir").to_path_buf();
    assert!(keeper_socket.as_os_str().as_bytes().len() < SUN_PATH_LEN);
    assert!(!keeper_socket.starts_with(&config.runtime_dir));
    assert!(
        keeper_socket.exists(),
        "keeper listens on the reported path"
    );
    let mode = std::fs::metadata(&keeper_dir)
        .expect("keeper dir exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "keeper dir must be private");

    handle.shutdown().await.expect("shutdown server");

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");
    let restored = request_buffer(
        &mut client,
        BufferRequest::Get {
            request_id: RequestId(203),
            buffer_id,
        },
    )
    .await
    .buffer;
    assert_eq!(restored.state, BufferRecordState::Running);

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(204),
            buffer_id,
            bytes: b"printf restored-long\\n\r".to_vec(),
        }))
        .await
        .expect("send to restored buffer succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
    let _ = wait_for_snapshot_line(&mut client, RequestId(205), buffer_id, "restored-long").await;

    // Keepers outlive the server, and this keeper directory is outside the
    // temp dir, so stop the keeper and remove the directory here.
    handle.shutdown().await.expect("shutdown restarted server");
    let _ = std::process::Command::new("pkill")
        .arg("-f")
        .arg(keeper_socket.as_os_str())
        .status();
    let _ = std::fs::remove_dir_all(&keeper_dir);
}

async fn send_input(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    bytes: &[u8],
) {
    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id,
            buffer_id,
            bytes: bytes.to_vec(),
        }))
        .await
        .expect("send input succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
}

async fn create_shell_buffer(
    client: &mut ProtocolClient,
    request_id: RequestId,
    title: &str,
    cwd: &Path,
) -> BufferId {
    request_buffer(
        client,
        BufferRequest::Create {
            request_id,
            title: Some(title.to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: Some(cwd.display().to_string()),
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer
    .id
}

/// Polls the workspace file on disk until `predicate` holds for its contents.
async fn wait_for_saved_workspace(path: &Path, what: &str, predicate: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && predicate(&contents)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "workspace file never saved {what} while the server was running: {}",
            std::fs::read_to_string(path).unwrap_or_default()
        );
        sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_buffer_cwd(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    expected: &Path,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let buffer = request_buffer(
            client,
            BufferRequest::Get {
                request_id,
                buffer_id,
            },
        )
        .await
        .buffer;
        if buffer.cwd.as_deref() == expected.to_str() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "buffer {buffer_id} cwd stayed {:?}, expected {}",
            buffer.cwd,
            expected.display()
        );
        sleep(Duration::from_millis(25)).await;
    }
}

/// Polls captures of `buffer_id` until a line contains `expected`. Slower than
/// [`wait_for_snapshot_line`] because a restored shell has to start first.
async fn wait_for_restored_line(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    expected: &str,
) -> SnapshotResponse {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(ServerResponse::Snapshot(snapshot)) = client
            .request(&ClientMessage::Buffer(BufferRequest::Capture {
                request_id,
                buffer_id,
            }))
            .await
            && snapshot.lines.iter().any(|line| line.contains(expected))
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "capture for buffer {buffer_id} never contained '{expected}'"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// Simulates a reboot once the server is gone: every keeper and its child stop,
/// so a restarted server has nothing to reattach to. Removing a keeper's socket
/// makes it stop its child and exit, whether it runs in its own process or in
/// the test process.
async fn stop_keepers(config: &ServerConfig, buffers: &[(BufferId, u32)]) {
    for (buffer_id, _) in buffers {
        let _ = std::fs::remove_file(config.keeper_socket_path(*buffer_id));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    for (buffer_id, pid) in buffers {
        while process_alive(*pid) {
            assert!(
                Instant::now() < deadline,
                "child {pid} of buffer {buffer_id} outlived its keeper socket"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test]
async fn workspace_is_saved_while_the_server_runs() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());
    let workspace_path = config.workspace_path.clone();

    let handle = Server::new(config).start().await.expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Create {
            request_id: RequestId(501),
            name: "autosaved".to_owned(),
        },
    )
    .await;
    wait_for_saved_workspace(&workspace_path, "a new session", |contents| {
        contents.contains("\"autosaved\"")
    })
    .await;

    let _ = request_session_snapshot(
        &mut client,
        SessionRequest::Rename {
            request_id: RequestId(502),
            session_id: session.snapshot.session.id,
            name: "renamed".to_owned(),
        },
    )
    .await;
    wait_for_saved_workspace(&workspace_path, "a session rename", |contents| {
        contents.contains("\"renamed\"") && !contents.contains("\"autosaved\"")
    })
    .await;

    // Saves go through a temp file renamed into place, which must not linger.
    let leftovers: Vec<_> = std::fs::read_dir(tempdir.path())
        .expect("read socket dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );

    handle.shutdown().await.expect("shutdown server");
}

#[tokio::test]
async fn restore_starts_a_fresh_shell_in_the_last_directory_when_the_keeper_is_gone() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    // Canonical so it compares equal to the directory the OS reports for the shell.
    let root = tempdir.path().canonicalize().expect("canonical tempdir");
    let start_dir = root.join("start");
    let moved_dir = root.join("moved");
    std::fs::create_dir(&start_dir).expect("create start dir");
    std::fs::create_dir(&moved_dir).expect("create moved dir");
    let socket_path = root.join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());
    let workspace_path = config.workspace_path.clone();

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Create {
            request_id: RequestId(601),
            name: "main".to_owned(),
        },
    )
    .await;
    let session_id = session.snapshot.session.id;
    let buffer_id = create_shell_buffer(&mut client, RequestId(602), "work", &start_dir).await;
    let _ = request_session_snapshot(
        &mut client,
        SessionRequest::AddRootTab {
            request_id: RequestId(603),
            session_id,
            title: "shell".to_owned(),
            buffer_id: Some(buffer_id),
            child_node_id: None,
        },
    )
    .await;
    let running = wait_for_running_buffer(&mut client, RequestId(604), buffer_id).await;
    let old_pid = running.pid.expect("running buffer has a pid");
    let node_id = running.attachment_node_id.expect("buffer is attached");

    send_input(
        &mut client,
        RequestId(605),
        buffer_id,
        format!("cd {}\r", moved_dir.display()).as_bytes(),
    )
    .await;
    wait_for_buffer_cwd(&mut client, RequestId(606), buffer_id, &moved_dir).await;
    wait_for_saved_workspace(&workspace_path, "the new cwd", |contents| {
        contents.contains(moved_dir.to_str().expect("utf-8 path"))
    })
    .await;

    drop(client);
    handle.shutdown().await.expect("shutdown server");
    stop_keepers(&config, &[(buffer_id, old_pid)]).await;

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");

    let restored = wait_for_running_buffer(&mut client, RequestId(607), buffer_id).await;
    assert_ne!(restored.pid, Some(old_pid), "a fresh shell was started");
    assert_eq!(restored.attachment_node_id, Some(node_id));
    assert_eq!(restored.title, "work");
    assert_eq!(restored.cwd.as_deref(), moved_dir.to_str());

    send_input(&mut client, RequestId(608), buffer_id, b"pwd\r").await;
    let _ = wait_for_restored_line(
        &mut client,
        RequestId(609),
        buffer_id,
        moved_dir.to_str().expect("utf-8 path"),
    )
    .await;

    let restored_pid = restored.pid.expect("restored buffer has a pid");
    drop(client);
    handle.shutdown().await.expect("shutdown restarted server");
    stop_keepers(&config, &[(buffer_id, restored_pid)]).await;
}

#[tokio::test]
async fn restore_reruns_allowlisted_foreground_commands_only() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let root = tempdir.path().canonicalize().expect("canonical tempdir");
    std::fs::write(root.join("followed.log"), "followed-line\n").expect("write followed file");
    let socket_path = root.join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());
    let workspace_path = config.workspace_path.clone();

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let tail_id = create_shell_buffer(&mut client, RequestId(701), "tail", &root).await;
    let sleep_id = create_shell_buffer(&mut client, RequestId(702), "sleep", &root).await;
    let tail_pid = wait_for_running_buffer(&mut client, RequestId(703), tail_id)
        .await
        .pid
        .expect("tail buffer pid");
    let sleep_pid = wait_for_running_buffer(&mut client, RequestId(704), sleep_id)
        .await
        .pid
        .expect("sleep buffer pid");

    // `tail` is on the default allowlist; `sleep` is not.
    send_input(
        &mut client,
        RequestId(705),
        tail_id,
        b"tail -f followed.log\r",
    )
    .await;
    send_input(&mut client, RequestId(706), sleep_id, b"sleep 1000\r").await;
    let _ = wait_for_restored_line(&mut client, RequestId(707), tail_id, "followed-line").await;
    wait_for_saved_workspace(&workspace_path, "both foreground commands", |contents| {
        contents.contains("\"followed.log\"") && contents.contains("\"1000\"")
    })
    .await;

    drop(client);
    handle.shutdown().await.expect("shutdown server");
    stop_keepers(&config, &[(tail_id, tail_pid), (sleep_id, sleep_pid)]).await;

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");

    let tail_pid = wait_for_running_buffer(&mut client, RequestId(708), tail_id)
        .await
        .pid
        .expect("restored tail pid");
    let sleep_pid = wait_for_running_buffer(&mut client, RequestId(709), sleep_id)
        .await
        .pid
        .expect("restored sleep pid");

    // The restored shell re-runs tail, which prints the file again.
    let _ = wait_for_restored_line(&mut client, RequestId(710), tail_id, "followed-line").await;

    // The other shell sits at a prompt, so input it receives runs right away.
    // The quotes keep the echoed command line from matching the output.
    send_input(
        &mut client,
        RequestId(711),
        sleep_id,
        b"echo shell-''idle\r",
    )
    .await;
    let capture = wait_for_restored_line(&mut client, RequestId(712), sleep_id, "shell-idle").await;
    assert!(
        !capture.lines.iter().any(|line| line.contains("sleep")),
        "sleep must not be re-run: {:?}",
        capture.lines
    );

    drop(client);
    handle.shutdown().await.expect("shutdown restarted server");
    stop_keepers(&config, &[(tail_id, tail_pid), (sleep_id, sleep_pid)]).await;
}

#[tokio::test]
async fn restore_can_be_turned_off() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let mut config = ServerConfig::new(socket_path.clone());
    config.restore.enabled = false;

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");
    let buffer_id = create_shell_buffer(&mut client, RequestId(801), "off", tempdir.path()).await;
    let pid = wait_for_running_buffer(&mut client, RequestId(802), buffer_id)
        .await
        .pid
        .expect("buffer pid");

    drop(client);
    handle.shutdown().await.expect("shutdown server");
    stop_keepers(&config, &[(buffer_id, pid)]).await;

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");
    let restored = request_buffer(
        &mut client,
        BufferRequest::Get {
            request_id: RequestId(803),
            buffer_id,
        },
    )
    .await
    .buffer;
    assert_eq!(restored.state, BufferRecordState::Interrupted);
    handle.shutdown().await.expect("shutdown restarted server");
}
