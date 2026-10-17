use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use embers_core::{
    ActivityState, BufferId, ClipboardWrite, MuxError, PassthroughWrite, PtySize, RequestId,
    Result, TerminalModes, TerminalSnapshot, new_request_id,
};
use embers_protocol::{
    BufferPipeRecord, BufferPipeState, BufferPipeStopReason, BufferRecord, BufferRecordState,
    BufferRequest, ClientMessage, ClientRequest, InputRequest, ServerEnvelope, ServerEvent,
    ServerResponse, SubscribeRequest,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::Upstream;
use crate::buffer_runtime::{
    BufferRuntimeCallbacks, BufferRuntimePipeStatus, BufferRuntimePipeStopReason,
    BufferRuntimeStatus, BufferRuntimeUpdate, RuntimeEnd, RuntimeScrollbackSlice, RuntimeSnapshot,
    status_update_delta,
};
use crate::config::REMOTE_HOST_PLACEHOLDER;

/// Reaches other hosts' embers servers: the seam between the host
/// connections and how their bytes travel.
pub(crate) trait HostConnector: Send + Sync {
    /// A connection to `host`'s server, its protocol version checked.
    fn connect<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Upstream>>;
}

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reaches a host by running a command whose standard input and output reach
/// its server, such as `ssh HOST embers proxy`.
pub(crate) struct CommandConnector {
    command: Vec<String>,
}

impl CommandConnector {
    /// Run `command`, with [`REMOTE_HOST_PLACEHOLDER`] replaced by the host.
    pub(crate) fn new(command: Vec<String>) -> Self {
        Self { command }
    }
}

impl HostConnector for CommandConnector {
    fn connect<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Upstream>> {
        let command = self
            .command
            .iter()
            .map(|part| part.replace(REMOTE_HOST_PLACEHOLDER, host))
            .collect::<Vec<_>>();
        Box::pin(async move { Upstream::connect(&command).await })
    }
}

/// The connections to other hosts' embers servers, one per host, opened when a
/// buffer first needs one.
pub(crate) struct RemoteHosts {
    connector: Arc<dyn HostConnector>,
    hosts: Mutex<HashMap<String, HostConnection>>,
    /// Whether some client here forwards kitty graphics queries to a terminal
    /// that answers them, as each connection tells its host.
    host_graphics: AtomicBool,
}

impl std::fmt::Debug for RemoteHosts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteHosts")
            .finish_non_exhaustive()
    }
}

impl RemoteHosts {
    /// Hosts reached through `connector`.
    pub(crate) fn new(connector: Arc<dyn HostConnector>) -> Self {
        Self {
            connector,
            hosts: Mutex::new(HashMap::new()),
            host_graphics: AtomicBool::new(false),
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
        let mut upstream = self.connector.connect(host).await?;
        // Every buffer's output is reported to every subscriber, detached
        // buffers included; the connection passes on those of its buffers.
        let subscribe = ClientMessage::Subscribe(SubscribeRequest {
            request_id: new_request_id(),
            session_id: None,
        });
        expect_ok(host, upstream.request(&subscribe).await?)?;
        // Like any client's, the setting is the connection's, so a new one
        // starts out without it.
        if self.host_graphics.load(Ordering::Acquire) {
            let enable = ClientMessage::Client(ClientRequest::SetHostGraphics {
                request_id: new_request_id(),
                enabled: true,
            });
            expect_ok(host, upstream.request(&enable).await?)?;
        }
        let connection = HostConnection::serve(host, upstream);
        hosts.insert(host.to_owned(), connection.clone());
        Ok(connection)
    }
}

impl RemoteHosts {
    /// Tell every host whether some client here forwards kitty graphics
    /// queries to a terminal that answers them, so its buffers hold their own
    /// replies to such a query until the terminal's arrives. Hosts reached
    /// later are told when they are.
    pub(crate) async fn set_host_graphics(&self, enabled: bool) {
        self.host_graphics.store(enabled, Ordering::Release);
        let connections: Vec<_> = self.hosts.lock().await.values().cloned().collect();
        for connection in connections {
            let request = ClientMessage::Client(ClientRequest::SetHostGraphics {
                request_id: new_request_id(),
                enabled,
            });
            if let Err(error) = connection.request(request).await {
                debug!(host = %connection.host(), %error, "failed to tell host about host graphics");
            }
        }
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
    /// Pass on what the host reports about `buffer_id`.
    Watch {
        buffer_id: BufferId,
        watcher: Watcher,
    },
    Unwatch {
        buffer_id: BufferId,
    },
}

/// Where the host connection passes on what the host reports about one
/// buffer.
struct Watcher {
    /// That the buffer changed, coalesced: one waiting report stands for any
    /// number.
    changes: mpsc::Sender<()>,
    /// What the buffer's program sent for the terminal, in order.
    writes: mpsc::UnboundedSender<HostWrite>,
}

/// Clipboard writes and passthrough from a buffer on the host, for the
/// terminals showing it here.
enum HostWrite {
    Clipboard(ClipboardWrite),
    Passthrough(Vec<PassthroughWrite>),
}

/// The receiving ends of a [`Watcher`], which close when the connection is
/// lost.
struct Watched {
    changes: mpsc::Receiver<()>,
    writes: mpsc::UnboundedReceiver<HostWrite>,
}

/// One host's server, shared by every buffer running there. A task owns the
/// connection, sending requests as they come, up to
/// [`MAX_REQUESTS_IN_FLIGHT`] unanswered, and passing on what the server
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

    /// What the host reports about its buffer `buffer_id`: that it changed,
    /// and what its program sends for the terminal. The host sends the latter
    /// only to connections watching the buffer, since no session of its own
    /// shows it; a host from before watching does not, and the buffer works
    /// without.
    async fn watch(&self, buffer_id: BufferId) -> Result<Watched> {
        let (changes, changes_receiver) = mpsc::channel(1);
        let (writes, writes_receiver) = mpsc::unbounded_channel();
        self.commands
            .send(HostCommand::Watch {
                buffer_id,
                watcher: Watcher { changes, writes },
            })
            .await
            .map_err(|_| self.lost())?;
        if let Err(error) = self
            .request(ClientMessage::Buffer(BufferRequest::Watch {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await
        {
            if !self.is_open() {
                return Err(error);
            }
            debug!(host = %self.host, %buffer_id, %error, "host does not pass on clipboard writes");
        }
        Ok(Watched {
            changes: changes_receiver,
            writes: writes_receiver,
        })
    }

    /// Stop passing on what the host reports about `buffer_id`, and tell the
    /// host, which then removes the buffer once its program has exited.
    fn unwatch(&self, buffer_id: BufferId) {
        let _ = self.commands.try_send(HostCommand::Unwatch { buffer_id });
        let connection = self.clone();
        tokio::spawn(async move {
            let _ = connection
                .request(ClientMessage::Buffer(BufferRequest::Unwatch {
                    request_id: new_request_id(),
                    buffer_id,
                }))
                .await;
        });
    }

    fn lost(&self) -> MuxError {
        MuxError::transport(format!("the connection to {} was lost", self.host))
    }
}

/// Most requests a host connection has sent and not yet had answered. More
/// wait for one to be answered: the server answers in order, and with no
/// bound a burst could fill both pipes, each side waiting for the other to
/// read.
const MAX_REQUESTS_IN_FLIGHT: usize = 32;

async fn serve_host(
    host: String,
    mut upstream: Upstream,
    mut commands: mpsc::Receiver<HostCommand>,
) {
    let mut watchers = HashMap::<BufferId, Watcher>::new();
    // Requests sent and not yet answered, by id. Requests go out as they
    // come rather than one round trip at a time, so a slow link costs one
    // round trip for a burst of requests, not one each.
    let mut in_flight = HashMap::<RequestId, oneshot::Sender<Result<ServerResponse>>>::new();
    loop {
        tokio::select! {
            command = commands.recv(), if in_flight.len() < MAX_REQUESTS_IN_FLIGHT => match command {
                None => break,
                Some(HostCommand::Request { message, reply }) => {
                    let request_id = message.request_id();
                    match upstream.send(&message).await {
                        Ok(()) => {
                            in_flight.insert(request_id, reply);
                        }
                        Err(error) => {
                            debug!(%host, %error, "connection to host failed");
                            let _ = reply.send(Err(error));
                            break;
                        }
                    }
                }
                Some(HostCommand::Watch { buffer_id, watcher }) => {
                    watchers.insert(buffer_id, watcher);
                }
                Some(HostCommand::Unwatch { buffer_id }) => {
                    watchers.remove(&buffer_id);
                }
            },
            envelope = upstream.recv() => match envelope {
                Ok(Some(ServerEnvelope::Response(response))) => {
                    // A response without an id answers a request the server
                    // could not read, so it cannot be told which.
                    let Some(reply) = response
                        .request_id()
                        .and_then(|request_id| in_flight.remove(&request_id))
                    else {
                        debug!(%host, ?response, "host answered no request in flight");
                        break;
                    };
                    let _ = reply.send(Ok(response));
                }
                Ok(Some(ServerEnvelope::Event(event))) => route_event(&watchers, event),
                Ok(None) => break,
                Err(error) => {
                    debug!(%host, %error, "connection to host failed");
                    break;
                }
            },
        }
    }
    debug!(%host, "connection to host closed");
    // Dropping the replies in flight fails their requests, and dropping the
    // watchers closes their channels, which tells each buffer the connection
    // is gone; dropping the receiver makes the connection report itself
    // closed.
}

/// Pass on what the host reports about a buffer to whoever watches it.
fn route_event(watchers: &HashMap<BufferId, Watcher>, event: ServerEvent) {
    match event {
        ServerEvent::RenderInvalidated(event) => {
            if let Some(watcher) = watchers.get(&event.buffer_id) {
                // Full means a report is already waiting.
                let _ = watcher.changes.try_send(());
            }
        }
        ServerEvent::Clipboard(event) => {
            if let Some(watcher) = watchers.get(&event.buffer_id) {
                let _ = watcher.writes.send(HostWrite::Clipboard(ClipboardWrite {
                    selection: event.selection,
                    text: event.text,
                }));
            }
        }
        ServerEvent::Passthrough(event) => {
            if let Some(watcher) = watchers.get(&event.buffer_id) {
                let _ = watcher.writes.send(HostWrite::Passthrough(event.writes));
            }
        }
        _ => {}
    }
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
    /// The buffer's options on the host as last read. Starts empty, so the
    /// first read passes on what the host has.
    options: StdMutex<BTreeMap<String, String>>,
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
        let watched = connection.watch(record.id).await?;
        let inner = Arc::new(RemoteInner {
            buffer_id,
            remote_id: record.id,
            connection,
            status: StdMutex::new(status_from_record(&record)),
            options: StdMutex::new(BTreeMap::new()),
            pump: StdMutex::new(None),
        });
        let pump = tokio::spawn(pump_changes(inner.clone(), watched, callbacks));
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

    /// Stop passing on the host's reports about this buffer, which the server
    /// here is done with. A program still running keeps running on the host
    /// until it is killed; an exited one's buffer is removed there.
    pub async fn release(&self) -> Result<()> {
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
/// on what changed as the keeper's status poller would, along with what the
/// program sends for the terminal, until the buffer exits, disappears from
/// the host, or the connection is lost.
async fn pump_changes(
    inner: Arc<RemoteInner>,
    mut watched: Watched,
    callbacks: BufferRuntimeCallbacks,
) {
    // The first read catches whatever changed between the record the buffer
    // started from and the watch starting.
    let mut changed = true;
    loop {
        if !changed {
            tokio::select! {
                change = watched.changes.recv() => {
                    if change.is_none() {
                        (callbacks.on_ended)(inner.buffer_id, RuntimeEnd::Interrupted);
                        return;
                    }
                }
                Some(write) = watched.writes.recv() => {
                    match write {
                        HostWrite::Clipboard(write) => (callbacks.on_clipboard)(inner.buffer_id, write),
                        HostWrite::Passthrough(writes) => {
                            (callbacks.on_passthrough)(inner.buffer_id, writes);
                        }
                    }
                    continue;
                }
            }
        }
        changed = false;
        let record = match get_record(&inner.connection, inner.remote_id).await {
            Ok(record) => record,
            Err(error) if error.is_not_found() => {
                (callbacks.on_ended)(inner.buffer_id, RuntimeEnd::Exited(None));
                return;
            }
            Err(_) if !inner.connection.is_open() => {
                (callbacks.on_ended)(inner.buffer_id, RuntimeEnd::Interrupted);
                return;
            }
            Err(error) => {
                debug!(buffer_id = %inner.buffer_id, %error, "reading remote buffer failed");
                continue;
            }
        };
        let status = status_from_record(&record);
        let options = options_delta(&mut lock(&inner.options), &record.user_options);
        let update = {
            let mut last = lock(&inner.status);
            let mut update = status_update_delta(&status, &last).map(|mut update| {
                // Whether output counts as activity here is for this server
                // to decide by who is looking, as for a local buffer.
                if update.sequence > last.sequence {
                    update.activity = ActivityState::Activity;
                }
                update
            });
            if let Some(options) = options {
                update
                    .get_or_insert(BufferRuntimeUpdate {
                        sequence: status.sequence,
                        activity: ActivityState::Idle,
                        title: None,
                        pipe: None,
                        cwd: None,
                        foreground_command: None,
                        bell_sequence: status.bell_sequence,
                        user_options: None,
                    })
                    .user_options = Some(options);
            }
            *last = status.clone();
            update
        };
        if let Some(update) = update {
            (callbacks.on_output)(inner.buffer_id, update);
        }
        if record.state == BufferRecordState::Exited {
            (callbacks.on_ended)(inner.buffer_id, RuntimeEnd::Exited(record.exit_code));
            return;
        }
    }
}

/// The options that changed between `last` and `current`, set (`Some`) or
/// unset (`None`), or `None` when none did; `last` becomes `current`.
fn options_delta(
    last: &mut BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, Option<String>>> {
    let mut delta = BTreeMap::new();
    for (key, value) in current {
        if last.get(key) != Some(value) {
            delta.insert(key.clone(), Some(value.clone()));
        }
    }
    for key in last.keys() {
        if !current.contains_key(key) {
            delta.insert(key.clone(), None);
        }
    }
    *last = current.clone();
    (!delta.is_empty()).then_some(delta)
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
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use std::time::Duration;

    use embers_core::{BufferId, ErrorCode, RequestId, WireError};
    use embers_protocol::{
        BufferRequest, ClientMessage, ErrorResponse, OkResponse, PingRequest, PingResponse,
        RenderInvalidatedEvent, ServerEvent, ServerResponse,
    };
    use tempfile::tempdir;

    use super::{
        HostConnection, MAX_REQUESTS_IN_FLIGHT, RemoteHosts, options_delta, validate_host,
    };
    use crate::testing_hosts::{ScriptedHost, SocketHosts};

    /// Long enough for a request the connection would send to reach the
    /// scripted host.
    const QUIET: Duration = Duration::from_millis(200);

    async fn scripted() -> (HostConnection, ScriptedHost) {
        let (upstream, host) = ScriptedHost::connect("devbox").await;
        (HostConnection::serve("devbox", upstream), host)
    }

    fn ping(id: u64) -> ClientMessage {
        ClientMessage::Ping(PingRequest {
            request_id: RequestId(id),
            payload: format!("ping {id}"),
        })
    }

    fn pong(request: ClientMessage) -> ServerResponse {
        let ClientMessage::Ping(ping) = request else {
            panic!("expected a ping, got {request:?}");
        };
        ServerResponse::Pong(PingResponse {
            request_id: ping.request_id,
            payload: ping.payload,
        })
    }

    #[tokio::test]
    async fn answers_reach_their_requests_in_whatever_order_they_come() {
        let (connection, mut host) = scripted().await;
        let first = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request(ping(1)).await }
        });
        let second = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request(ping(2)).await }
        });
        let a = host.next_request().await.expect("a request");
        let b = host.next_request().await.expect("another request");

        host.respond(pong(b)).await;
        host.respond(pong(a)).await;

        for (request, id) in [(first, 1), (second, 2)] {
            match request.await.expect("request task") {
                Ok(ServerResponse::Pong(pong)) => assert_eq!(pong.payload, format!("ping {id}")),
                other => panic!("expected the pong for ping {id}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn losing_the_host_fails_the_requests_in_flight_and_ends_the_watches() {
        let (connection, mut host) = scripted().await;
        let watching = tokio::spawn({
            let connection = connection.clone();
            async move { connection.watch(BufferId(5)).await }
        });
        let watch = host.next_request().await.expect("the watch request");
        host.respond(ServerResponse::Ok(OkResponse {
            request_id: watch.request_id(),
        }))
        .await;
        let mut watched = watching.await.expect("watch task").expect("watch");
        let pending = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request(ping(1)).await }
        });
        host.next_request().await.expect("the ping");

        drop(host);

        assert!(pending.await.expect("request task").is_err());
        assert_eq!(watched.changes.recv().await, None);
        assert!(!connection.is_open());
    }

    #[tokio::test]
    async fn an_answer_to_no_request_ends_the_connection() {
        let (connection, mut host) = scripted().await;
        let pending = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request(ping(1)).await }
        });
        host.next_request().await.expect("the ping");

        // A server answers a request it cannot read without its id, so the
        // connection cannot tell which request failed.
        host.respond(ServerResponse::Error(ErrorResponse {
            request_id: None,
            error: WireError::new(ErrorCode::ProtocolViolation, "unreadable request"),
        }))
        .await;

        assert!(pending.await.expect("request task").is_err());
        assert!(!connection.is_open());
    }

    #[tokio::test]
    async fn requests_wait_while_the_most_are_in_flight() {
        let (connection, mut host) = scripted().await;
        let requests: Vec<_> = (0..=MAX_REQUESTS_IN_FLIGHT as u64)
            .map(|id| {
                let connection = connection.clone();
                tokio::spawn(async move { connection.request(ping(id + 1)).await })
            })
            .collect();
        let mut received = Vec::new();
        for _ in 0..MAX_REQUESTS_IN_FLIGHT {
            received.push(host.next_request().await.expect("a request in flight"));
        }

        assert!(host.request_within(QUIET).await.is_none());
        host.respond(pong(received.remove(0))).await;
        received.push(host.next_request().await.expect("the request that waited"));
        for request in received {
            host.respond(pong(request)).await;
        }
        for request in requests {
            assert!(request.await.expect("request task").is_ok());
        }
    }

    #[tokio::test]
    async fn a_host_from_before_watching_still_reports_its_buffers_changing() {
        let (connection, mut host) = scripted().await;
        let watching = tokio::spawn({
            let connection = connection.clone();
            async move { connection.watch(BufferId(5)).await }
        });
        let Some(ClientMessage::Buffer(BufferRequest::Watch { request_id, .. })) =
            host.next_request().await
        else {
            panic!("expected a watch request");
        };

        host.respond(ServerResponse::Error(ErrorResponse {
            request_id: Some(request_id),
            error: WireError::new(ErrorCode::InvalidRequest, "unknown buffer request"),
        }))
        .await;
        let mut watched = watching.await.expect("watch task").expect("watch anyway");
        host.send_event(ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
            buffer_id: BufferId(5),
        }))
        .await;

        assert_eq!(watched.changes.recv().await, Some(()));
        assert!(connection.is_open());
    }
    use crate::{Server, ServerConfig};

    #[tokio::test]
    async fn a_host_is_reached_over_any_stream_to_its_server() {
        let dir = tempdir().expect("tempdir");
        let socket = dir.path().join("devbox.sock");
        let server = Server::new(ServerConfig::new(socket.clone()))
            .start()
            .await
            .expect("start the host's server");
        let sockets = SocketHosts::default();
        sockets.add("devbox", socket);
        let hosts = RemoteHosts::new(Arc::new(sockets));

        let connection = hosts.connect("devbox").await.expect("reach devbox");
        let response = connection
            .request(ClientMessage::Ping(PingRequest {
                request_id: RequestId(1),
                payload: "hello".to_owned(),
            }))
            .await
            .expect("ping devbox");

        assert!(matches!(response, ServerResponse::Pong(pong) if pong.payload == "hello"));
        server
            .shutdown()
            .await
            .expect("shut down the host's server");
    }

    #[tokio::test]
    async fn a_lost_host_is_reached_again_on_a_new_connection() {
        let dir = tempdir().expect("tempdir");
        let socket = dir.path().join("devbox.sock");
        let server = Server::new(ServerConfig::new(socket.clone()))
            .start()
            .await
            .expect("start the host's server");
        let sockets = SocketHosts::default();
        sockets.add("devbox", socket);
        let hosts = RemoteHosts::new(Arc::new(sockets.clone()));
        let lost = hosts.connect("devbox").await.expect("reach devbox");

        sockets.cut("devbox");
        let ping = ClientMessage::Ping(PingRequest {
            request_id: RequestId(2),
            payload: "anyone?".to_owned(),
        });
        assert!(lost.request(ping.clone()).await.is_err());
        assert!(!lost.is_open());
        assert!(hosts.connect("devbox").await.is_err());

        sockets.restore("devbox");
        let found = hosts.connect("devbox").await.expect("reach devbox again");
        assert!(found.request(ping).await.is_ok());
        server
            .shutdown()
            .await
            .expect("shut down the host's server");
    }

    #[test]
    fn option_changes_on_the_host_are_reported_once() {
        let mut last = BTreeMap::new();
        let current = BTreeMap::from([("is-vim".to_owned(), "1".to_owned())]);
        assert_eq!(
            options_delta(&mut last, &current),
            Some(BTreeMap::from([(
                "is-vim".to_owned(),
                Some("1".to_owned())
            )]))
        );
        assert_eq!(options_delta(&mut last, &current), None);
        assert_eq!(
            options_delta(&mut last, &BTreeMap::new()),
            Some(BTreeMap::from([("is-vim".to_owned(), None)]))
        );
    }

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
