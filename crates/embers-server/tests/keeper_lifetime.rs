//! Runtime keepers deliberately outlive the server so a restart can reattach,
//! but must exit once nothing can ever reattach to them.

use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{
    BufferRecordState, BufferRequest, BufferResponse, ClientMessage, KillServerRequest,
    ProtocolClient, ServerResponse,
};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, Instant, sleep};

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

async fn wait_for_running_buffer(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: embers_core::BufferId,
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
        assert!(
            Instant::now() < deadline,
            "buffer {buffer_id} did not reach Running before timeout"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

/// Returns whether `pid` names a live process. An exited keeper stays a zombie
/// until its parent reaps it, so zombies count as gone.
fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn parent_pid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

async fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    !process_alive(pid)
}

#[tokio::test]
async fn keepers_outlive_shutdown_but_exit_once_their_socket_is_removed() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let buffer = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(201),
            title: Some("orphan".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;
    let running = wait_for_running_buffer(&mut client, RequestId(202), buffer.id).await;
    let shell_pid = running.pid.expect("running buffer reports its pid");
    // The keeper runs out of process when the embers binary is available and
    // in-process otherwise; only an out-of-process keeper has a pid of its own.
    let keeper_pid = parent_pid(shell_pid).filter(|&pid| pid != std::process::id());

    drop(client);
    handle.shutdown().await.expect("shutdown server");

    // Keepers deliberately survive a server shutdown so a restart can reattach.
    assert!(
        process_alive(shell_pid),
        "shell should outlive server shutdown"
    );
    if let Some(keeper_pid) = keeper_pid {
        assert!(
            process_alive(keeper_pid),
            "keeper should outlive server shutdown"
        );
    }

    // Once the keeper socket is gone no server can reattach, so the keeper must
    // stop its shell and exit rather than run unreachable forever.
    drop(tempdir);
    assert!(
        wait_for_process_exit(shell_pid, Duration::from_secs(10)).await,
        "shell {shell_pid} still running after its keeper socket was removed"
    );
    if let Some(keeper_pid) = keeper_pid {
        assert!(
            wait_for_process_exit(keeper_pid, Duration::from_secs(10)).await,
            "keeper {keeper_pid} still running after its socket was removed"
        );
    }
}

#[tokio::test]
async fn keeper_of_killed_buffer_exits_once_no_server_is_attached() {
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
            request_id: RequestId(301),
            title: Some("killed".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;
    let running = wait_for_running_buffer(&mut client, RequestId(302), buffer.id).await;
    let shell_pid = running.pid.expect("running buffer reports its pid");
    let keeper_pid = parent_pid(shell_pid).filter(|&pid| pid != std::process::id());

    match client
        .request(&ClientMessage::Buffer(BufferRequest::Kill {
            request_id: RequestId(303),
            buffer_id: buffer.id,
            force: false,
        }))
        .await
        .expect("kill request succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
    assert!(
        wait_for_process_exit(shell_pid, Duration::from_secs(10)).await,
        "shell {shell_pid} still running after kill"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let record = request_buffer(
            &mut client,
            BufferRequest::Get {
                request_id: RequestId(304),
                buffer_id: buffer.id,
            },
        )
        .await
        .buffer;
        if record.state == BufferRecordState::Exited {
            break;
        }
        assert!(Instant::now() < deadline, "buffer did not reach Exited");
        sleep(Duration::from_millis(25)).await;
    }

    drop(client);
    handle.shutdown().await.expect("shutdown server");

    // The child is gone and no server is attached, so there is nothing left to
    // reattach to: the keeper exits even though its socket is still in place.
    if let Some(keeper_pid) = keeper_pid {
        assert!(
            wait_for_process_exit(keeper_pid, Duration::from_secs(10)).await,
            "keeper {keeper_pid} still running after its buffer was killed"
        );
    }

    // A restarted server cleans the exited, detached record up instead of
    // keeping it.
    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");
    let restored = client
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: RequestId(305),
            buffer_id: buffer.id,
        }))
        .await
        .expect("get request round-trips");
    assert!(
        matches!(&restored, ServerResponse::Error(error) if error.error.code == embers_core::ErrorCode::NotFound),
        "exited buffer should not be restored, got {restored:?}"
    );
    handle.shutdown().await.expect("shutdown restarted server");
}

/// Unlike a clean shutdown, `kill-server` ends every buffer's program and
/// keeper and leaves nothing for a restarted server to restore.
#[tokio::test]
async fn kill_server_ends_programs_and_keepers_and_forgets_the_workspace() {
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

    let buffer = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(401),
            title: Some("doomed".to_owned()),
            // Ignores SIGHUP, so the keeper has to escalate to SIGKILL.
            command: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "trap '' HUP; while :; do sleep 1; done".to_owned(),
            ],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;
    let running = wait_for_running_buffer(&mut client, RequestId(402), buffer.id).await;
    let shell_pid = running.pid.expect("running buffer reports its pid");
    let keeper_pid = parent_pid(shell_pid).filter(|&pid| pid != std::process::id());

    match client
        .request(&ClientMessage::KillServer(KillServerRequest {
            request_id: RequestId(403),
        }))
        .await
        .expect("kill-server request succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
    tokio::time::timeout(Duration::from_secs(10), handle.stopped())
        .await
        .expect("server stops after kill-server");
    drop(client);
    handle.shutdown().await.expect("server finishes stopping");

    assert!(
        wait_for_process_exit(shell_pid, Duration::from_secs(10)).await,
        "shell {shell_pid} still running after kill-server"
    );
    if let Some(keeper_pid) = keeper_pid {
        assert!(
            wait_for_process_exit(keeper_pid, Duration::from_secs(10)).await,
            "keeper {keeper_pid} still running after kill-server"
        );
    }
    assert!(
        !workspace_path.exists(),
        "kill-server leaves no workspace to restore"
    );

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");
    match client
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: RequestId(404),
            session_id: None,
            attached_only: false,
            detached_only: false,
        }))
        .await
        .expect("list buffers")
    {
        ServerResponse::Buffers(response) => assert!(response.buffers.is_empty()),
        other => panic!("expected buffers response, got {other:?}"),
    }
    drop(client);
    handle.shutdown().await.expect("shutdown restarted server");
}

/// Whether `pid` still has a process table entry, zombie or not.
fn process_listed(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

/// A keeper that exits while the server that spawned it keeps running must be
/// reaped, not left behind as a zombie.
#[tokio::test]
async fn exited_keepers_are_reaped_while_the_server_runs() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let session_id = match client
        .request(&ClientMessage::Session(
            embers_protocol::SessionRequest::Create {
                request_id: RequestId(501),
                name: "main".to_owned(),
            },
        ))
        .await
        .expect("create session")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot.session.id,
        other => panic!("expected session snapshot, got {other:?}"),
    };
    let buffer = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(502),
            title: Some("short".to_owned()),
            command: vec!["/bin/sh".to_owned(), "-c".to_owned(), "sleep 1".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
        },
    )
    .await
    .buffer;
    let running = wait_for_running_buffer(&mut client, RequestId(503), buffer.id).await;
    let shell_pid = running.pid.expect("running buffer reports its pid");
    let Some(keeper_pid) = parent_pid(shell_pid).filter(|&pid| pid != std::process::id()) else {
        // An in-process keeper thread has no process to reap.
        drop(client);
        handle.shutdown().await.expect("shutdown server");
        return;
    };
    // Shown in a pane, so its exit closes the pane and releases the keeper.
    match client
        .request(&ClientMessage::Session(
            embers_protocol::SessionRequest::AddRootTab {
                request_id: RequestId(504),
                session_id,
                title: "short".to_owned(),
                buffer_id: Some(buffer.id),
                child_node_id: None,
            },
        ))
        .await
        .expect("add root tab")
    {
        ServerResponse::SessionSnapshot(_) => {}
        other => panic!("expected session snapshot, got {other:?}"),
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while process_listed(keeper_pid) {
        assert!(
            Instant::now() < deadline,
            "keeper {keeper_pid} was not reaped (zombie: {})",
            !process_alive(keeper_pid)
        );
        sleep(Duration::from_millis(50)).await;
    }

    drop(client);
    handle.shutdown().await.expect("shutdown server");
}
