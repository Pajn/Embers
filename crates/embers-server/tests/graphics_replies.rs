//! A program that detects kitty graphics the usual way (a graphics query, then
//! queries every terminal answers, read up to the last reply) sees the host
//! terminal's answer to the graphics query first while a client forwards such
//! queries to a host that answers them, and the emulator's own replies at once
//! otherwise.

use embers_core::{BufferId, PtySize, RequestId, init_test_tracing};
use embers_protocol::{
    BufferRequest, ClientMessage, ClientRequest, InputRequest, ProtocolClient, ServerResponse,
};
use embers_server::{Server, ServerConfig, ServerHandle};
use tempfile::{TempDir, tempdir};
use tokio::time::{Duration, Instant, sleep};

/// What ratatui-image's `Picker::from_query_stdio` writes: a kitty graphics
/// query, DA1, the cell size query and a status report. The pane prints
/// `sent` first, then shows what it reads with `cat -v`.
const DETECTION_SCRIPT: &str = "read _; stty raw -echo; \
     printf 'sent\\r\\n\\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\\033\\\\\\033[c\\033[16t\\033[5n'; \
     cat -v";

/// The emulator's replies to the detection queries as `cat -v` shows them, in
/// a pane of 9 by 18 pixel cells.
const EMULATOR_REPLIES: &str = "^[[?6c^[[6;18;9t^[[0n";

/// Which clients declare that they forward graphics queries to a host that
/// answers them, before the pane detects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HostGraphics {
    /// None.
    Off,
    /// The test's own connection.
    On,
    /// Another connection, which then goes away.
    Left,
}

struct Detection {
    _tempdir: TempDir,
    handle: ServerHandle,
    client: ProtocolClient,
    buffer_id: BufferId,
    next_request_id: u64,
}

impl Detection {
    /// Start a pane running [`DETECTION_SCRIPT`], set up clients as
    /// `host_graphics` says, and set the pane off.
    async fn start(host_graphics: HostGraphics) -> Self {
        init_test_tracing();
        let tempdir = tempdir().expect("tempdir");
        let socket_path = tempdir.path().join("mux.sock");
        let handle = Server::new(ServerConfig::new(socket_path.clone()))
            .start()
            .await
            .expect("start server");
        let client = ProtocolClient::connect(&socket_path)
            .await
            .expect("connect client");
        let mut detection = Self {
            _tempdir: tempdir,
            handle,
            client,
            buffer_id: BufferId(0),
            next_request_id: 1,
        };
        let request_id = detection.request_id();
        detection.buffer_id = match detection
            .client
            .request(&ClientMessage::Buffer(BufferRequest::Create {
                request_id,
                title: None,
                command: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    DETECTION_SCRIPT.to_owned(),
                ],
                cwd: None,
                env: Default::default(),
                size: Some(PtySize {
                    cols: 80,
                    rows: 24,
                    pixel_width: 720,
                    pixel_height: 432,
                }),
                session_id: None,
            }))
            .await
            .expect("create buffer")
        {
            ServerResponse::Buffer(response) => response.buffer.id,
            other => panic!("expected buffer response, got {other:?}"),
        };
        let declare = ClientMessage::Client(ClientRequest::SetHostGraphics {
            request_id: RequestId(1),
            enabled: true,
        });
        match host_graphics {
            HostGraphics::Off => {}
            HostGraphics::On => detection.ok(declare).await,
            HostGraphics::Left => {
                let mut other = ProtocolClient::connect(&socket_path)
                    .await
                    .expect("connect other client");
                match other.request(&declare).await.expect("declare") {
                    ServerResponse::Ok(_) => {}
                    other => panic!("expected ok response, got {other:?}"),
                }
                drop(other);
                // The server notices the closed connection on its own time.
                sleep(Duration::from_millis(200)).await;
            }
        }
        detection.send(b"\r").await;
        detection.wait_for(|text| text.contains("sent")).await;
        detection
    }

    fn request_id(&mut self) -> RequestId {
        self.next_request_id += 1;
        RequestId(self.next_request_id)
    }

    async fn ok(&mut self, message: ClientMessage) {
        match self.client.request(&message).await.expect("request") {
            ServerResponse::Ok(_) => {}
            other => panic!("expected ok response, got {other:?}"),
        }
    }

    async fn send(&mut self, bytes: &[u8]) {
        let request_id = self.request_id();
        let buffer_id = self.buffer_id;
        self.ok(ClientMessage::Input(InputRequest::Send {
            request_id,
            buffer_id,
            bytes: bytes.to_vec(),
        }))
        .await;
    }

    async fn text(&mut self) -> String {
        let request_id = self.request_id();
        match self
            .client
            .request(&ClientMessage::Buffer(BufferRequest::Capture {
                request_id,
                buffer_id: self.buffer_id,
            }))
            .await
            .expect("capture")
        {
            ServerResponse::Snapshot(snapshot) => snapshot.lines.join("\n"),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    async fn wait_for(&mut self, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = self.text().await;
            if done(&text) {
                return text;
            }
            assert!(Instant::now() < deadline, "pane never got there: {text:?}");
            sleep(Duration::from_millis(20)).await;
        }
    }

    async fn finish(self) {
        self.handle.shutdown().await.expect("shutdown server");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hosts_graphics_answer_comes_before_the_other_replies() {
    let mut detection = Detection::start(HostGraphics::On).await;
    sleep(Duration::from_millis(100)).await;
    let text = detection.text().await;
    assert!(!text.contains("^["), "replies were not held: {text:?}");

    // What the client does with the host's answer.
    detection.send(b"\x1b_Gi=31;OK\x1b\\").await;
    let expected = format!("^[_Gi=31;OK^[\\{EMULATOR_REPLIES}");
    let text = detection.wait_for(|text| text.contains("^[[0n")).await;
    assert!(
        text.contains(&expected),
        "expected {expected:?} in {text:?}"
    );
    detection.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_other_replies_come_after_a_timeout_when_the_host_is_silent() {
    let mut detection = Detection::start(HostGraphics::On).await;
    sleep(Duration::from_millis(100)).await;
    let text = detection.text().await;
    assert!(!text.contains("^["), "replies were not held: {text:?}");

    let text = detection.wait_for(|text| text.contains("^[[0n")).await;
    assert!(
        text.contains(EMULATOR_REPLIES),
        "expected {EMULATOR_REPLIES:?} in {text:?}"
    );
    detection.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replies_are_immediate_when_no_client_forwards_graphics() {
    for host_graphics in [HostGraphics::Off, HostGraphics::Left] {
        let mut detection = Detection::start(host_graphics).await;
        let started = Instant::now();
        let text = detection.wait_for(|text| text.contains("^[[0n")).await;
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "replies took {:?}",
            started.elapsed()
        );
        assert!(
            text.contains(EMULATOR_REPLIES),
            "expected {EMULATOR_REPLIES:?} in {text:?}"
        );
        detection.finish().await;
    }
}
