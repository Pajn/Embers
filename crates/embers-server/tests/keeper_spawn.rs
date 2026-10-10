//! Spawning a buffer waits until its runtime keeper reports that it is ready,
//! so creation neither races the keeper's socket setup nor hides why a keeper
//! failed to start.

use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{BufferRequest, ClientMessage, ProtocolClient, ServerResponse};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, Instant};

const WORKERS: u64 = 8;
const BUFFERS_PER_WORKER: u64 = 25;

fn create_request(request_id: u64, command: &[&str]) -> ClientMessage {
    ClientMessage::Buffer(BufferRequest::Create {
        request_id: RequestId(request_id),
        title: None,
        command: command.iter().map(|part| (*part).to_owned()).collect(),
        cwd: None,
        env: Default::default(),
    })
}

/// Creating many buffers at once used to fail now and then with "Connection
/// refused": the keeper's socket file exists from `bind` until `listen`, and a
/// connect in that window is refused rather than retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_buffer_creation_always_reaches_the_keeper() {
    init_test_tracing();
    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let handle = Server::new(ServerConfig::new(socket_path.clone()))
        .start()
        .await
        .expect("start server");

    let mut workers = Vec::new();
    for worker in 0..WORKERS {
        let socket_path = socket_path.clone();
        workers.push(tokio::spawn(async move {
            let mut client = ProtocolClient::connect(&socket_path)
                .await
                .expect("connect client");
            let mut failures = Vec::new();
            for index in 0..BUFFERS_PER_WORKER {
                let request_id = worker * BUFFERS_PER_WORKER + index + 1;
                let response = client
                    .request(&create_request(request_id, &["/bin/sh", "-c", "sleep 5"]))
                    .await
                    .expect("create request round-trips");
                if !matches!(response, ServerResponse::Buffer(_)) {
                    failures.push(format!("{response:?}"));
                }
            }
            failures
        }));
    }
    let mut failures = Vec::new();
    for worker in workers {
        failures.extend(worker.await.expect("worker task"));
    }

    handle.shutdown().await.expect("shutdown server");
    assert!(
        failures.is_empty(),
        "{} of {} buffer creations failed: {failures:#?}",
        failures.len(),
        WORKERS * BUFFERS_PER_WORKER
    );
}

#[tokio::test]
async fn keeper_start_failure_is_reported_promptly() {
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

    let missing = tempdir.path().join("no-such-program");
    let missing = missing.to_str().expect("utf-8 temp path");
    let started = Instant::now();
    let response = client
        .request(&create_request(1, &[missing]))
        .await
        .expect("create request round-trips");
    let elapsed = started.elapsed();

    match response {
        ServerResponse::Error(error) => assert!(
            error.error.message.contains("no-such-program"),
            "error should say which program failed to start: {error:?}"
        ),
        other => panic!("expected an error response, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "keeper start failure took {elapsed:?} to surface"
    );

    handle.shutdown().await.expect("shutdown server");
}
