use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use embers_core::{
    ActivityState, BufferId, MuxError, PtySize, Result, TerminalModes, TerminalSnapshot,
    new_request_id,
};
use embers_protocol::{
    BufferPipeRecord, BufferPipeState, BufferPipeStopReason, BufferRecord, BufferRecordState,
    BufferRequest, ClientMessage, InputRequest, ServerEvent, ServerResponse, SubscribeRequest,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::Upstream;
use crate::buffer_runtime::{
    BufferRuntimeCallbacks, BufferRuntimePipeStatus, BufferRuntimePipeStopReason,
    BufferRuntimeStatus, RuntimeScrollbackSlice, RuntimeSnapshot, status_update_delta,
};
use crate::config::REMOTE_HOST_PLACEHOLDER;

/// The connections to other hosts' embers servers, one per host, opened when a
/// buffer first needs one.
#[derive(Debug)]
pub(crate) struct RemoteHosts {
    command: Vec<String>,
    hosts: Mutex<HashMap<String, HostConnection>>,
}

impl RemoteHosts {
    /// Hosts reached by running `command`, with
    /// [`REMOTE_HOST_PLACEHOLDER`] replaced by the host.
    pub(crate) fn new(command: Vec<String>) -> Self {
        Self {
            command,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// The connection to `host`'s server: the open one, or a new one when
    /// there is none or it was lost.
    pub(crate) async fn connect(&self, host: &str) -> Result<HostConnection> {
        validate_host(host)?;
        // Held while connecting, so buffers created on a new host at once
        // share one connection.
        let mut hosts = self.hosts.lock().await;
        if let Some(connection) = hosts.get(host)
            && connection.is_open()
        {
            return Ok(connection.clone());
        }
        let command = self
            .command
            .iter()
            .map(|part| part.replace(REMOTE_HOST_PLACEHOLDER, host))
            .collect::<Vec<_>>();
        let mut upstream = Upstream::connect(&command).await?;
        // Every buffer's output is reported to every subscriber, detached
        // buffers included; the connection passes on those of its buffers.
        let subscribe = ClientMessage::Subscribe(SubscribeRequest {
            request_id: new_request_id(),
            session_id: None,
        });
        expect_ok(host, upstream.request(&subscribe).await?)?;
        let connection = HostConnection::serve(host, upstream);
        hosts.insert(host.to_owned(), connection.clone());
        Ok(connection)
    }
}

/// A host name is passed to the remote command as one argument, which SSH
/// would read as an option if it started with `-`.
pub(crate) fn validate_host(host: &str) -> Result<()> {
    if host.is_empty()
        || host.starts_with('-')
        || host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(MuxError::invalid_input(format!(
            "invalid host {host:?}: a host must not be empty, start with '-', or contain spaces"
        )));
    }
    Ok(())
}

enum HostCommand {
    Request {
        message: ClientMessage,
        reply: oneshot::Sender<Result<ServerResponse>>,
    },
    /// Pass on the host's reports that `buffer_id` changed.
    Watch {
        buffer_id: BufferId,
        changes: mpsc::Sender<()>,
    },
    Unwatch {
        buffer_id: BufferId,
    },
}

/// One host's server, shared by every buffer running there. A task owns the
/// connection, sending requests one at a time and passing on what the server
/// reports about each buffer to that buffer's runtime.
#[derive(Clone, Debug)]
pub(crate) struct HostConnection {
    host: Arc<str>,
    commands: mpsc::Sender<HostCommand>,
}

impl HostConnection {
    fn serve(host: &str, upstream: Upstream) -> Self {
        let (commands, receiver) = mpsc::channel(64);
        tokio::spawn(serve_host(host.to_owned(), upstream, receiver));
        Self {
            host: host.into(),
            commands,
        }
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// Whether the connection still works. Once lost it stays lost; a new one
    /// comes from [`RemoteHosts::connect`].
    pub(crate) fn is_open(&self) -> bool {
        !self.commands.is_closed()
    }

    /// Send `message` to the host's server. Its error responses come back as
    /// errors, keeping their code.
    pub(crate) async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(HostCommand::Request { message, reply })
            .await
            .map_err(|_| self.lost())?;
        match response.await.map_err(|_| self.lost())?? {
            ServerResponse::Error(response) => Err(MuxError::Wire(response.error)),
            response => Ok(response),
        }
    }

    /// Reports that the host's buffer `buffer_id` changed, coalesced: one
    /// waiting report stands for any number. The channel closes when the
    /// connection is lost.
    async fn watch(&self, buffer_id: BufferId) -> Result<mpsc::Receiver<()>> {
        let (changes, receiver) = mpsc::channel(1);
        self.commands
            .send(HostCommand::Watch { buffer_id, changes })
            .await
            .map_err(|_| self.lost())?;
        Ok(receiver)
    }

    fn unwatch(&self, buffer_id: BufferId) {
        let _ = self.commands.try_send(HostCommand::Unwatch { buffer_id });
    }

    fn lost(&self) -> MuxError {
        MuxError::transport(format!("the connection to {} was lost", self.host))
    }
}

async fn serve_host(
    host: String,
    mut upstream: Upstream,
    mut commands: mpsc::Receiver<HostCommand>,
) {
    let mut watchers = HashMap::<BufferId, mpsc::Sender<()>>::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => break,
                Some(HostCommand::Request { message, reply }) => {
                    let response = upstream.request(&message).await;
                    // Errors are the connection failing; the server's own
                    // refusals are responses.
                    let failed = response.is_err();
                    let _ = reply.send(response);
                    if failed {
                        break;
                    }
                }
                Some(HostCommand::Watch { buffer_id, changes }) => {
                    watchers.insert(buffer_id, changes);
                }
                Some(HostCommand::Unwatch { buffer_id }) => {
                    watchers.remove(&buffer_id);
                }
            },
            event = upstream.next_event() => match event {
                Ok(Some(ServerEvent::RenderInvalidated(event))) => {
                    if let Some(changes) = watchers.get(&event.buffer_id) {
                        // Full means a report is already waiting.
                        let _ = changes.try_send(());
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    debug!(%host, %error, "connection to host failed");
                    break;
                }
            },
        }
    }
    debug!(%host, "connection to host closed");
    // Dropping the watchers closes their channels, which tells each buffer
    // the connection is gone; dropping the receiver makes the connection
    // report itself closed.
}

fn expect_ok(host: &str, response: ServerResponse) -> Result<()> {
    match response {
        ServerResponse::Error(response) => Err(MuxError::Wire(response.error)),
        ServerResponse::SubscriptionAck(_) | ServerResponse::Ok(_) => Ok(()),
        other => Err(MuxError::protocol(format!(
            "unexpected response from {host}: {other:?}"
        ))),
    }
}

/// A buffer whose program runs on another host, as a detached buffer of that
/// host's embers server.
#[derive(Clone)]
pub struct RemoteRuntimeHandle {
    inner: Arc<RemoteInner>,
}

struct RemoteInner {
    buffer_id: BufferId,
    remote_id: BufferId,
    connection: HostConnection,
    /// The status as last read from the host, which the server reads on every
    /// capture, so it costs no round trip.
    status: StdMutex<BufferRuntimeStatus>,
    pump: StdMutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for RemoteRuntimeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteRuntimeHandle")
            .field("buffer_id", &self.inner.buffer_id)
            .field("host", &self.inner.connection.host())
            .field("remote_id", &self.inner.remote_id)
            .finish_non_exhaustive()
    }
}

impl RemoteRuntimeHandle {
    /// Start `command` in a new detached buffer on `connection`'s host. The
    /// environment is `env` alone: the host's server adds its own, so the
    /// program's `EMBERS_SOCKET` and `EMBERS_BUFFER` name that server and its
    /// buffer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn(
        buffer_id: BufferId,
        connection: HostConnection,
        title: Option<String>,
        command: &[String],
        cwd: Option<&Path>,
        env: BTreeMap<String, String>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let response = connection
            .request(ClientMessage::Buffer(BufferRequest::Create {
                request_id: new_request_id(),
                title,
                command: command.to_vec(),
                cwd: cwd.map(|cwd| cwd.display().to_string()),
                env,
                size: Some(size),
                session_id: None,
                host: None,
            }))
            .await?;
        let record = expect_buffer(connection.host(), response)?;
        Self::start(buffer_id, connection, record, callbacks).await
    }

    /// Take up the host's buffer `remote_id` again, as after a restart.
    pub(crate) async fn attach(
        buffer_id: BufferId,
        connection: HostConnection,
        remote_id: BufferId,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let record = get_record(&connection, remote_id).await?;
        Self::start(buffer_id, connection, record, callbacks).await
    }

    async fn start(
        buffer_id: BufferId,
        connection: HostConnection,
        record: BufferRecord,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let changes = connection.watch(record.id).await?;
        let inner = Arc::new(RemoteInner {
            buffer_id,
            remote_id: record.id,
            connection,
            status: StdMutex::new(status_from_record(&record)),
            pump: StdMutex::new(None),
        });
        let pump = tokio::spawn(pump_changes(inner.clone(), changes, callbacks));
        *lock(&inner.pump) = Some(pump);
        Ok(Self { inner })
    }

    pub fn host(&self) -> &str {
        self.inner.connection.host()
    }

    /// The buffer's id on its host's server.
    pub fn remote_id(&self) -> BufferId {
        self.inner.remote_id
    }

    pub async fn status(&self) -> Result<BufferRuntimeStatus> {
        Ok(lock(&self.inner.status).clone())
    }

    pub async fn capture_snapshot(&self) -> Result<RuntimeSnapshot> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::Capture {
                request_id: new_request_id(),
                buffer_id: self.inner.remote_id,
            }))
            .await?;
        match response {
            ServerResponse::Snapshot(snapshot) => Ok(RuntimeSnapshot {
                sequence: snapshot.sequence,
                size: snapshot.size,
                lines: snapshot.lines,
                title: snapshot.title,
                cwd: snapshot.cwd.map(PathBuf::from),
            }),
            other => Err(self.unexpected(&other)),
        }
    }

    pub async fn capture_visible_snapshot(&self) -> Result<TerminalSnapshot> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::CaptureVisible {
                request_id: new_request_id(),
                buffer_id: self.inner.remote_id,
            }))
            .await?;
        match response {
            ServerResponse::VisibleSnapshot(snapshot) => Ok(TerminalSnapshot {
                sequence: snapshot.sequence,
                size: snapshot.size,
                cursor: snapshot.cursor,
                lines: snapshot.lines,
                title: snapshot.title,
                cwd: snapshot.cwd.map(PathBuf::from),
                viewport_top_line: snapshot.viewport_top_line,
                total_lines: snapshot.total_lines,
                modes: TerminalModes {
                    alternate_screen: snapshot.alternate_screen,
                    mouse_reporting: snapshot.mouse_reporting,
                    focus_reporting: snapshot.focus_reporting,
                    bracketed_paste: snapshot.bracketed_paste,
                    keyboard_mode: snapshot.keyboard_mode,
                },
            }),
            other => Err(self.unexpected(&other)),
        }
    }

    pub async fn capture_scrollback_slice(
        &self,
        start_line: u64,
        line_count: u32,
    ) -> Result<RuntimeScrollbackSlice> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::ScrollbackSlice {
                request_id: new_request_id(),
                buffer_id: self.inner.remote_id,
                start_line,
                line_count,
            }))
            .await?;
        match response {
            ServerResponse::ScrollbackSlice(slice) => {
                Ok(RuntimeScrollbackSlice::from_snapshot_lines(
                    slice.start_line,
                    slice.total_lines,
                    slice.lines,
                ))
            }
            other => Err(self.unexpected(&other)),
        }
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.request(ClientMessage::Input(InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: self.inner.remote_id,
            bytes,
        }))
        .await
        .map(drop)
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        self.request(ClientMessage::Input(InputRequest::Resize {
            request_id: new_request_id(),
            buffer_id: self.inner.remote_id,
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }))
        .await
        .map(drop)
    }

    /// End the program. Its exit arrives like any other, once the host
    /// reports it.
    pub async fn kill(&self) -> Result<()> {
        self.request(ClientMessage::Buffer(BufferRequest::Kill {
            request_id: new_request_id(),
            buffer_id: self.inner.remote_id,
            force: false,
        }))
        .await
        .map(drop)
    }

    /// Start a pipe on the host: `command` runs there, in `cwd` on the host.
    pub async fn start_pipe(
        &self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<BufferRuntimePipeStatus> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::StartPipe {
                request_id: new_request_id(),
                buffer_id: self.inner.remote_id,
                command,
                cwd: cwd.map(|cwd| cwd.display().to_string()),
                env,
            }))
            .await?;
        self.pipe_status(response)
    }

    pub async fn stop_pipe(&self) -> Result<BufferRuntimePipeStatus> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::StopPipe {
                request_id: new_request_id(),
                buffer_id: self.inner.remote_id,
            }))
            .await?;
        self.pipe_status(response)
    }

    /// Stop passing on the host's reports about this buffer. The buffer keeps
    /// running on the host until it is killed.
    pub async fn join_threads(&self) -> Result<()> {
        if let Some(pump) = lock(&self.inner.pump).take() {
            pump.abort();
        }
        self.inner.connection.unwatch(self.inner.remote_id);
        Ok(())
    }

    async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        self.inner.connection.request(message).await
    }

    fn pipe_status(&self, response: ServerResponse) -> Result<BufferRuntimePipeStatus> {
        let record = expect_buffer(self.host(), response)?;
        record.pipe.as_ref().map(pipe_status).ok_or_else(|| {
            MuxError::protocol(format!(
                "{} answered a pipe request without a pipe",
                self.host()
            ))
        })
    }

    fn unexpected(&self, response: &ServerResponse) -> MuxError {
        MuxError::protocol(format!(
            "unexpected response from {}: {response:?}",
            self.host()
        ))
    }
}

/// Read the host's record whenever it reports the buffer changed, and pass
/// on what changed as the keeper's status poller would, until the buffer
/// exits, disappears from the host, or the connection is lost.
async fn pump_changes(
    inner: Arc<RemoteInner>,
    mut changes: mpsc::Receiver<()>,
    callbacks: BufferRuntimeCallbacks,
) {
    // The first read catches whatever changed between the record the buffer
    // started from and the watch starting.
    let mut changed = true;
    loop {
        if !changed && changes.recv().await.is_none() {
            (callbacks.on_lost)(inner.buffer_id);
            return;
        }
        changed = false;
        let record = match get_record(&inner.connection, inner.remote_id).await {
            Ok(record) => record,
            Err(error) if error.is_not_found() => {
                (callbacks.on_exit)(inner.buffer_id, None);
                return;
            }
            Err(_) if !inner.connection.is_open() => {
                (callbacks.on_lost)(inner.buffer_id);
                return;
            }
            Err(error) => {
                debug!(buffer_id = %inner.buffer_id, %error, "reading remote buffer failed");
                continue;
            }
        };
        let status = status_from_record(&record);
        let update = {
            let mut last = lock(&inner.status);
            let update = status_update_delta(&status, &last).map(|mut update| {
                // Whether output counts as activity here is for this server
                // to decide by who is looking, as for a local buffer.
                if update.sequence > last.sequence {
                    update.activity = ActivityState::Activity;
                }
                update
            });
            *last = status.clone();
            update
        };
        if let Some(update) = update {
            (callbacks.on_output)(inner.buffer_id, update);
        }
        if record.state == BufferRecordState::Exited {
            (callbacks.on_exit)(inner.buffer_id, record.exit_code);
            return;
        }
    }
}

async fn get_record(connection: &HostConnection, remote_id: BufferId) -> Result<BufferRecord> {
    let response = connection
        .request(ClientMessage::Buffer(BufferRequest::Get {
            request_id: new_request_id(),
            buffer_id: remote_id,
        }))
        .await?;
    expect_buffer(connection.host(), response)
}

fn expect_buffer(host: &str, response: ServerResponse) -> Result<BufferRecord> {
    match response {
        ServerResponse::Buffer(response) => Ok(response.buffer),
        other => Err(MuxError::protocol(format!(
            "unexpected response from {host}: {other:?}"
        ))),
    }
}

/// What the host's record says, in the terms of a local runtime's status.
fn status_from_record(record: &BufferRecord) -> BufferRuntimeStatus {
    BufferRuntimeStatus {
        // The process runs on another host.
        pid: None,
        sequence: record.last_snapshot_seq,
        // The host flags a detached buffer for any output, so its activity
        // says nothing; output is reported as the sequence advances.
        activity: ActivityState::Idle,
        title: (!record.title.is_empty()).then(|| record.title.clone()),
        running: record.state != BufferRecordState::Exited,
        exit_code: record.exit_code,
        pipe: record.pipe.as_ref().map(pipe_status),
        cwd: record.cwd.as_ref().map(PathBuf::from),
        clipboard_sequence: 0,
        bell_sequence: record.bell_sequence,
        passthrough_sequence: 0,
        foreground_command: record.foreground_command.clone(),
    }
}

fn pipe_status(pipe: &BufferPipeRecord) -> BufferRuntimePipeStatus {
    BufferRuntimePipeStatus {
        command: pipe.command.clone(),
        running: pipe.state == BufferPipeState::Running,
        pid: pipe.pid,
        exit_code: pipe.exit_code,
        stop_reason: pipe.stop_reason.and_then(|reason| match reason {
            BufferPipeStopReason::Requested => Some(BufferRuntimePipeStopReason::Requested),
            BufferPipeStopReason::PipeExited => Some(BufferRuntimePipeStopReason::PipeExited),
            BufferPipeStopReason::WriteFailed => Some(BufferRuntimePipeStopReason::WriteFailed),
            BufferPipeStopReason::BufferExited => Some(BufferRuntimePipeStopReason::BufferExited),
            BufferPipeStopReason::RuntimeInterrupted => None,
        }),
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::validate_host;

    #[test]
    fn hosts_that_ssh_would_read_as_options_or_split_are_refused() {
        assert!(validate_host("devbox").is_ok());
        assert!(validate_host("user@devbox.example.org").is_ok());
        assert!(validate_host("").is_err());
        assert!(validate_host("-oProxyCommand=sh").is_err());
        assert!(validate_host("dev box").is_err());
        assert!(validate_host("devbox\n").is_err());
    }
}
