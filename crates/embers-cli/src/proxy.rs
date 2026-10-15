use std::io::Read;
use std::path::Path;
use std::thread;

use embers_core::{MuxError, Result};
use tokio::io::{AsyncWriteExt, copy};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// Most bytes one read of standard input passes on at a time.
const STDIN_CHUNK: usize = 64 * 1024;

/// Pass standard input on to the server at `socket`, and what the server sends
/// to standard output, until the server closes the connection. Once standard
/// input ends, the proxy stops writing to the server but still passes on what
/// it sends, so the answers to the last requests arrive.
///
/// The proxy does not read the frames it passes on: the client at the other
/// end of the pipe speaks the protocol with the server, handshake included.
pub(crate) async fn run(socket: &Path) -> Result<()> {
    let stream = UnixStream::connect(socket).await?;
    let (mut from_server, mut to_server) = stream.into_split();

    let mut stdin = read_stdin();
    tokio::spawn(async move {
        while let Some(chunk) = stdin.recv().await {
            if to_server.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = to_server.shutdown().await;
    });

    let mut stdout = tokio::io::stdout();
    copy(&mut from_server, &mut stdout)
        .await
        .map_err(|error| MuxError::transport(format!("passing on server output: {error}")))?;
    stdout.flush().await?;
    Ok(())
}

/// Standard input in chunks, closed when it ends or fails. Read on a thread of
/// its own rather than tokio's, whose blocking reads would keep the runtime,
/// and so the proxy, from exiting after the server closed the connection while
/// standard input was still open.
fn read_stdin() -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel(4);
    thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = vec![0; STDIN_CHUNK];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if sender.blocking_send(buffer[..read].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    receiver
}
