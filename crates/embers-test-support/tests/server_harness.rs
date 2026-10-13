use std::process::Command;
use std::time::{Duration, Instant};

use crate::support::integration_test_lock;
use embers_core::new_request_id;
use embers_protocol::{BufferRecordState, BufferRequest, ClientMessage, ServerResponse};
use embers_test_support::{TestConnection, TestServer};
use tokio::time::sleep;

#[tokio::test]
async fn harness_starts_server_and_pings_it() {
    let _guard = integration_test_lock().lock().await;
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect to server");

    let payload = connection.ping("harness").await.expect("ping server");
    assert_eq!(payload, "harness");

    server.shutdown().await.expect("shutdown server");
}

/// Returns whether `pid` names a live process. An exited keeper stays a zombie
/// until its parent reaps it, so zombies count as gone.
fn process_alive(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
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

async fn running_buffer_pid(connection: &mut TestConnection) -> u32 {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some("shell".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
            session_id: None,
            host: None,
        }))
        .await
        .expect("create buffer request succeeds");
    let buffer_id = match response {
        ServerResponse::Buffer(response) => response.buffer.id,
        other => panic!("expected buffer response, got {other:?}"),
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = connection
            .request(&ClientMessage::Buffer(BufferRequest::Get {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await
            .expect("get buffer request succeeds");
        if let ServerResponse::Buffer(response) = response
            && response.buffer.state == BufferRecordState::Running
            && let Some(pid) = response.buffer.pid
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "buffer {buffer_id} did not reach Running"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

/// Dropping a `TestServer` without `shutdown` is what happens when a test
/// panics; the runtime keepers it spawned must not outlive it.
#[tokio::test]
async fn dropping_harness_server_stops_its_runtime_keepers() {
    let _guard = integration_test_lock().lock().await;
    let server = TestServer::start().await.expect("start server");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect to server");

    let shell_pid = running_buffer_pid(&mut connection).await;
    // The keeper runs out of process when the embers binary is available and
    // in-process otherwise; only an out-of-process keeper has a pid of its own.
    let keeper_pid = parent_pid(shell_pid).filter(|&pid| pid != std::process::id());

    drop(connection);
    drop(server);

    // Dropping the harness only stops an out-of-process keeper: an in-process
    // one lives on a thread of this test process, which the drop does not end.
    if let Some(keeper_pid) = keeper_pid {
        assert!(
            wait_for_process_exit(shell_pid, Duration::from_secs(10)).await,
            "shell {shell_pid} still running after the harness server was dropped"
        );
        assert!(
            wait_for_process_exit(keeper_pid, Duration::from_secs(10)).await,
            "keeper {keeper_pid} still running after the harness server was dropped"
        );
    }
}
