//! A pane's PTY takes the pixel size a client sends with its cells, so
//! programs that size images from `TIOCGWINSZ` or from the cell size reports
//! (`CSI 14 t`, `CSI 16 t`) get the host terminal's.

use embers_core::{PtySize, RequestId, init_test_tracing};
use embers_protocol::{BufferRequest, ClientMessage, InputRequest, ProtocolClient, ServerResponse};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, Instant, sleep};

/// Once a byte is entered, shows the size the pane's terminal reports:
/// `TIOCGWINSZ` when Python is there to read it, then the emulator's replies
/// to the text area and cell size queries.
const REPORT_SIZES: &str = "stty raw -echo; dd bs=1 count=1 >/dev/null 2>&1; \
     command -v python3 >/dev/null && python3 -c 'import fcntl, struct, termios; \
print(\"winsize=%d,%d,%d,%d\" % struct.unpack(\"HHHH\", \
fcntl.ioctl(0, termios.TIOCGWINSZ, bytes(8))), end=\"\\r\\n\")'; \
     printf '\\033[14t\\033[16t'; exec cat -v";

async fn capture(client: &mut ProtocolClient, buffer_id: embers_core::BufferId) -> String {
    match client
        .request(&ClientMessage::Buffer(BufferRequest::Capture {
            request_id: RequestId(100),
            buffer_id,
        }))
        .await
        .expect("capture")
    {
        ServerResponse::Snapshot(snapshot) => snapshot.lines.join("\n"),
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_carries_the_pixel_size_to_the_pane() {
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
    let buffer_id = match client
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: RequestId(1),
            title: None,
            command: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                REPORT_SIZES.to_owned(),
            ],
            cwd: None,
            env: Default::default(),
            size: Some(PtySize::new(80, 24)),
            session_id: None,
        }))
        .await
        .expect("create buffer")
    {
        ServerResponse::Buffer(response) => response.buffer.id,
        other => panic!("expected buffer response, got {other:?}"),
    };

    match client
        .request(&ClientMessage::Input(InputRequest::Resize {
            request_id: RequestId(2),
            buffer_id,
            cols: 100,
            rows: 30,
            pixel_width: 900,
            pixel_height: 540,
        }))
        .await
        .expect("resize")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
    // The server records it too, for clients deciding whether to resize.
    match client
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: RequestId(3),
            buffer_id,
        }))
        .await
        .expect("get buffer")
    {
        ServerResponse::Buffer(response) => assert_eq!(
            response.buffer.pty_size,
            PtySize {
                cols: 100,
                rows: 30,
                pixel_width: 900,
                pixel_height: 540,
            }
        ),
        other => panic!("expected buffer response, got {other:?}"),
    }

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(4),
            buffer_id,
            bytes: b"\r".to_vec(),
        }))
        .await
        .expect("send")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let text = capture(&mut client, buffer_id).await;
        if text.contains("^[[6;") {
            break text;
        }
        assert!(Instant::now() < deadline, "no size report: {text:?}");
        sleep(Duration::from_millis(20)).await;
    };

    // 9 by 18 pixel cells.
    assert!(
        text.contains("^[[4;540;900t^[[6;18;9t"),
        "unexpected reports: {text:?}"
    );
    if text.contains("winsize=") {
        assert!(
            text.contains("winsize=30,100,900,540"),
            "unexpected TIOCGWINSZ: {text:?}"
        );
    }
    handle.shutdown().await.expect("shutdown server");
}
