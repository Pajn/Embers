//! `embers proxy` connects standard input and output to the server, for a
//! client that reaches it through a pipe such as `ssh HOST embers proxy`.

use std::path::Path;
use std::process::Stdio;

use embers_core::RequestId;
use embers_protocol::{
    ClientMessage, FrameType, HelloRequest, PROTOCOL_VERSION, PingRequest, RawFrame,
    SOFTWARE_VERSION, ServerEnvelope, ServerResponse, decode_server_envelope,
    encode_client_message, read_frame, write_frame,
};
use embers_test_support::{TestServer, cargo_bin_path};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, timeout};

fn spawn_proxy(socket: &Path) -> Child {
    Command::new(cargo_bin_path("embers"))
        .arg("--socket")
        .arg(socket)
        .arg("proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn embers proxy")
}

/// Send `message` through the proxy and read the server's answer, skipping
/// any events sent before it.
async fn request(
    stdin: &mut ChildStdin,
    stdout: &mut ChildStdout,
    message: &ClientMessage,
) -> ServerResponse {
    let payload = encode_client_message(message).expect("encode request");
    let frame = RawFrame::new(FrameType::Request, message.request_id(), payload);
    write_frame(stdin, &frame).await.expect("write request");
    loop {
        let frame = timeout(Duration::from_secs(10), read_frame(stdout))
            .await
            .expect("server answers in time")
            .expect("read response")
            .expect("proxy stays open");
        if let ServerEnvelope::Response(response) =
            decode_server_envelope(&frame.payload).expect("decode response")
        {
            return response;
        }
    }
}

/// The handshake and a ping, as a client at the other end of the pipe would
/// send them.
async fn check_server_answers(stdin: &mut ChildStdin, stdout: &mut ChildStdout) {
    let hello = ClientMessage::Hello(HelloRequest {
        request_id: RequestId(1),
        protocol_version: PROTOCOL_VERSION,
        software_version: SOFTWARE_VERSION.to_owned(),
    });
    match request(stdin, stdout, &hello).await {
        ServerResponse::Hello(response) => {
            assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        }
        other => panic!("expected the server's versions, got {other:?}"),
    }
    let ping = ClientMessage::Ping(PingRequest {
        request_id: RequestId(2),
        payload: "through the proxy".to_owned(),
    });
    match request(stdin, stdout, &ping).await {
        ServerResponse::Pong(response) => assert_eq!(response.payload, "through the proxy"),
        other => panic!("expected pong, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_passes_the_protocol_through_and_exits_when_input_ends() {
    let server = TestServer::start().await.expect("start server");
    let mut proxy = spawn_proxy(server.socket_path());
    let mut stdin = proxy.stdin.take().expect("proxy stdin");
    let mut stdout = proxy.stdout.take().expect("proxy stdout");

    check_server_answers(&mut stdin, &mut stdout).await;

    // The server closes the connection once the client's side ends, and the
    // proxy exits with it.
    drop(stdin);
    let status = timeout(Duration::from_secs(10), proxy.wait())
        .await
        .expect("proxy exits once its input ends")
        .expect("wait for proxy");
    assert!(status.success(), "proxy failed: {status}");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_exits_when_the_server_stops_while_input_stays_open() {
    let server = TestServer::start().await.expect("start server");
    let mut proxy = spawn_proxy(server.socket_path());
    let mut stdin = proxy.stdin.take().expect("proxy stdin");
    let mut stdout = proxy.stdout.take().expect("proxy stdout");

    check_server_answers(&mut stdin, &mut stdout).await;
    server.shutdown().await.expect("shutdown server");

    let status = timeout(Duration::from_secs(10), proxy.wait())
        .await
        .expect("proxy exits once the server is gone")
        .expect("wait for proxy");
    assert!(status.success(), "proxy failed: {status}");
    drop(stdin);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_starts_a_server_when_none_is_running() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket = tempdir.path().join("mux.sock");
    let mut proxy = spawn_proxy(&socket);
    let mut stdin = proxy.stdin.take().expect("proxy stdin");
    let mut stdout = proxy.stdout.take().expect("proxy stdout");

    check_server_answers(&mut stdin, &mut stdout).await;
    drop(stdin);
    proxy.wait().await.expect("wait for proxy");

    let output = embers_test_support::cargo_bin("embers")
        .arg("--socket")
        .arg(&socket)
        .arg("kill-server")
        .output()
        .expect("cli runs");
    assert!(
        output.status.success(),
        "kill-server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
