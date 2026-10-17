use std::collections::VecDeque;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use embers_core::{MuxError, Result};
use embers_protocol::{
    ClientMessage, ProtocolClient, ProtocolError, ServerEnvelope, ServerEvent, ServerResponse,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// How much of the command's standard error a failure reports: its end, where
/// a command such as SSH says why it gave up.
const STDERR_TAIL: usize = 4 * 1024;

/// How long a failed connection waits for the command to exit, and then for
/// the rest of its standard error, before reporting what it has.
const EXIT_WAIT: Duration = Duration::from_secs(2);

/// A connection from this server to another embers server: through a command
/// whose standard input and output reach it, such as `ssh HOST embers proxy`,
/// or over any stream that does.
///
/// The connection speaks the protocol as any client does, version check
/// included. A command runs as long as the connection, and is killed when the
/// connection is dropped.
pub struct Upstream {
    /// What the connection goes through, for its errors.
    through: String,
    client: ProtocolClient,
    process: Option<Process>,
    queued_events: VecDeque<ServerEvent>,
}

/// The command a connection runs through.
struct Process {
    child: Child,
    stderr: StderrTail,
}

impl Upstream {
    /// Run `command` and check that the server it reaches speaks this
    /// server's protocol. A command that fails, or exits before the server
    /// answers, is reported with the end of what it wrote to standard error.
    pub async fn connect(command: &[String]) -> Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| MuxError::invalid_input("upstream command is empty"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                MuxError::transport(format!("could not run `{}`: {error}", command.join(" ")))
            })?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = StderrTail::collect(child.stderr.take().expect("stderr is piped"));

        Self::start(
            format!("`{}`", command.join(" ")),
            ProtocolClient::from_split(stdout, stdin),
            Some(Process { child, stderr }),
        )
        .await
    }

    /// Check that the server reading what goes to `writer`, and writing what
    /// comes from `reader`, speaks this server's protocol. `through` names
    /// the stream in errors.
    pub async fn over<R, W>(through: impl Into<String>, reader: R, writer: W) -> Result<Self>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::start(
            through.into(),
            ProtocolClient::from_split(reader, writer),
            None,
        )
        .await
    }

    async fn start(
        through: String,
        client: ProtocolClient,
        process: Option<Process>,
    ) -> Result<Self> {
        let mut upstream = Self {
            through,
            client,
            process,
            queued_events: VecDeque::new(),
        };
        match upstream.client.handshake().await {
            Ok(()) => Ok(upstream),
            // The server answered, so the command is working: there is no
            // exit to wait for, and the refusal says it all.
            Err(
                error @ (ProtocolError::IncompatibleServer { .. }
                | ProtocolError::ServerPredatesHandshake { .. }),
            ) => Err(MuxError::transport(error.to_string())),
            Err(error) => Err(upstream.failure(error).await),
        }
    }

    /// Send `message` and wait for its response. Events that arrive first are
    /// kept for [`Self::next_event`].
    pub async fn request(&mut self, message: &ClientMessage) -> Result<ServerResponse> {
        let request_id = message.request_id();
        if let Err(error) = self.client.send(message).await {
            return Err(self.failure(error).await);
        }
        loop {
            match self.client.recv().await {
                Ok(Some(ServerEnvelope::Event(event))) => self.queued_events.push_back(event),
                Ok(Some(ServerEnvelope::Response(response))) => {
                    if let Some(response_id) = response.request_id()
                        && response_id != request_id
                    {
                        return Err(MuxError::protocol(format!(
                            "mismatched response id: expected {request_id}, got {response_id}"
                        )));
                    }
                    return Ok(response);
                }
                Ok(None) => {
                    return Err(self
                        .failure(ProtocolError::InvalidMessage(
                            "connection closed before response",
                        ))
                        .await);
                }
                Err(error) => return Err(self.failure(error).await),
            }
        }
    }

    /// Send `message` without waiting for its response, which [`Self::recv`]
    /// returns among the events, for a caller with several requests in
    /// flight. The server answers a connection's requests in order.
    pub async fn send(&mut self, message: &ClientMessage) -> Result<()> {
        if let Err(error) = self.client.send(message).await {
            return Err(self.failure(error).await);
        }
        Ok(())
    }

    /// The next response or event, or `None` once the connection closed.
    /// Events [`Self::request`] kept come first.
    pub async fn recv(&mut self) -> Result<Option<ServerEnvelope>> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(Some(ServerEnvelope::Event(event)));
        }
        match self.client.recv().await {
            Ok(envelope) => Ok(envelope),
            Err(error) => Err(self.failure(error).await),
        }
    }

    /// The next event the server sent, or `None` once the connection closed.
    pub async fn next_event(&mut self) -> Result<Option<ServerEvent>> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(Some(event));
        }
        match self.client.recv().await {
            Ok(Some(ServerEnvelope::Event(event))) => Ok(Some(event)),
            Ok(Some(ServerEnvelope::Response(response))) => Err(MuxError::protocol(format!(
                "received response without pending request: {response:?}"
            ))),
            Ok(None) => Ok(None),
            Err(error) => Err(self.failure(error).await),
        }
    }

    /// `error`, with what the command said about it if it exited: its status
    /// and the end of its standard error.
    async fn failure(&mut self, error: ProtocolError) -> MuxError {
        let Some(process) = &mut self.process else {
            return MuxError::transport(format!("{}: {error}", self.through));
        };
        let status = tokio::time::timeout(EXIT_WAIT, process.child.wait())
            .await
            .ok()
            .and_then(|status| status.ok());
        let Some(status) = status else {
            return MuxError::transport(format!("{}: {error}", self.through));
        };
        let stderr = process.stderr.finish().await;
        MuxError::transport(describe_exit(&self.through, status, &error, &stderr))
    }
}

fn describe_exit(command: &str, status: ExitStatus, error: &ProtocolError, stderr: &str) -> String {
    let mut message = format!("{command} exited ({status})");
    let stderr = stderr.trim();
    if stderr.is_empty() {
        message.push_str(&format!(": {error}"));
    } else {
        message.push_str(": ");
        message.push_str(stderr);
    }
    message
}

/// The end of what a command writes to standard error, read as it arrives so
/// the command never blocks on a full pipe.
struct StderrTail {
    tail: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

impl StderrTail {
    fn collect<R>(mut stderr: R) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let tail = Arc::new(Mutex::new(Vec::new()));
        let kept = tail.clone();
        let reader = tokio::spawn(async move {
            let mut buffer = [0; 1024];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    return;
                }
                let mut kept = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                kept.extend_from_slice(&buffer[..read]);
                let excess = kept.len().saturating_sub(STDERR_TAIL);
                kept.drain(..excess);
            }
        });
        Self {
            tail,
            reader: Some(reader),
        }
    }

    /// What was kept, once the command closed standard error or [`EXIT_WAIT`]
    /// passed.
    async fn finish(&mut self) -> String {
        if let Some(reader) = self.reader.take() {
            let _ = tokio::time::timeout(EXIT_WAIT, reader).await;
        }
        let tail = self
            .tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&tail).into_owned()
    }
}

impl Drop for StderrTail {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}
