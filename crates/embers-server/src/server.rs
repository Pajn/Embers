use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use embers_core::{
    BufferId, ClipboardWrite, ErrorCode, MuxError, PassthroughWrite, PtySize, RequestId, Result,
    SnapshotLine, WireError, request_span,
};
use embers_protocol::{
    BufferCreatedEvent, BufferDetachedEvent, BufferHistoryPlacement, BufferHistoryScope,
    BufferLocation, BufferLocationAttachment, BufferLocationResponse, BufferPipeChangedEvent,
    BufferRequest, BufferResponse, BufferWithLocationResponse, BuffersResponse, ClientChangedEvent,
    ClientMessage, ClientRecord, ClientRequest, ClientResponse, ClientsResponse, ClipboardEvent,
    DisplayMessageEvent, EnvironmentResponse, ErrorResponse, FloatingChangedEvent, FloatingRequest,
    FloatingResponse, FocusChangedEvent, FrameType, HelloResponse, InputRequest, NodeChangedEvent,
    OkResponse, PROTOCOL_VERSION, PassthroughEvent, PingResponse, ProtocolError, RawFrame,
    RenderInvalidatedEvent, SOFTWARE_VERSION, ScrollbackSliceResponse, ServerEnvelope, ServerEvent,
    ServerResponse, SessionClosedEvent, SessionCreatedEvent, SessionRenamedEvent, SessionRequest,
    SessionSnapshotResponse, SessionsResponse, SnapshotResponse, SubscriptionAckResponse,
    SynchronizePanesMode, VisibleSnapshotResponse, decode_client_message, encode_server_envelope,
    read_frame, write_frame_no_flush,
};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, info, warn};

use crate::config::{DEFAULT_REMOTE_COMMAND, ResourceLimits, RestoreConfig};
use crate::model::{
    BufferKind, BufferPipe, BufferPipeState, BufferPipeStopReason, HelperBufferScope,
    IS_VIM_OPTION, Node, Placement,
};
use crate::persist::{
    PersistedWorkspace, encode_persisted_workspace, load_workspace, same_saved_state,
    write_workspace,
};
use crate::protocol::{
    buffer_location, buffer_record, floating_record, session_record, session_snapshot,
};
use crate::remote::{CommandConnector, validate_host};
use crate::restore::{default_shell, plan_restore};
use crate::runtimes::{BufferRuntimes, Callbacks, KeeperSettings, Restored, StartRequest, Started};
use crate::{
    BufferAttachment, BufferRuntimeCallbacks, BufferRuntimeHandle, BufferRuntimePipeStatus,
    BufferRuntimePipeStopReason, BufferRuntimeStatus, BufferRuntimeUpdate, BufferState, RuntimeEnd,
    ServerConfig, ServerState, TabEntry,
};

/// Capacity of each client's outbound envelope queue. A client that cannot keep
/// up (suspended terminal, stalled socket) fills this queue; rather than letting
/// it grow unbounded and exhaust the shared daemon, [`Runtime::broadcast`] drops
/// such a client. Sized to absorb normal render bursts while bounding per-client
/// memory.
///
/// Note the response path in [`handle_connection`] does not drop on a full queue:
/// a response is awaited by the client, so it backpressures (awaits capacity) and
/// only fails on a genuinely closed channel. Only best-effort events, which a
/// reattached client recovers via resync, are dropped when the queue is full.
const OUTBOUND_CHANNEL_CAPACITY: usize = 1024;

/// How long the workspace saver waits after the first change of a burst before
/// it writes the workspace file. Changes made meanwhile land in the same write,
/// so a burst costs one write, and a crash loses at most this much.
const WORKSPACE_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// [`WORKSPACE_SAVE_DELAY`] for changes to a buffer's title, working directory
/// or foreground command. Programs change these continuously (a progress
/// display in the title, a shell prompt that sets it), and losing the last
/// moment of them in a crash costs little.
const WORKSPACE_METADATA_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// How long a connection gets to write the shutdown notice to its client when
/// the server stops, before it is closed anyway.
const SHUTDOWN_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Test hook: while the file this variable names exists, the workspace saver
/// skips its saves (and with them the exit acknowledgements that follow a
/// save). Tests use it to stand in for a crash that lands between the server
/// learning of a change and saving it.
const HOLD_WORKSPACE_SAVES_ENV_VAR: &str = "EMBERS_TEST_HOLD_WORKSPACE_SAVES";

/// How long a restored shell gets to print its prompt before the command it
/// should run again is typed into it anyway.
const RESTORED_SHELL_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Caps on runtime-settable pane user options. Each option is re-encoded into
/// every `BufferRecord` (list/get responses and RenderInvalidated broadcasts),
/// so an unbounded map would let a client inflate frames without limit — and a
/// record that exceeds the frame size limit becomes permanently un-listable.
const MAX_USER_OPTIONS_PER_BUFFER: usize = 256;
const MAX_USER_OPTION_KEY_LEN: usize = 256;
const MAX_USER_OPTION_VALUE_LEN: usize = 4096;

/// Aggregate size budget for a `BuffersResponse`. Per-buffer caps alone don't
/// bound the *total* list: with up to `max_buffers` records (default 2048), each
/// carrying env and user options, the encoded response can exceed the protocol
/// frame cap (`MAX_FRAME_LEN`, 8 MiB), which makes the whole list un-sendable and
/// fails the request. We stop adding records past this budget (kept comfortably
/// under the frame cap to absorb estimation slack) rather than emit a dead frame.
const BUFFERS_RESPONSE_BUDGET: usize = 7 * 1024 * 1024;

/// Payload budget for one `PassthroughEvent`. One write is at most
/// `MAX_PASSTHROUGH_BYTES` (4 MiB), so a lone write always fits; several
/// smaller ones share an event up to this size, which leaves room for framing
/// under the protocol frame cap (`MAX_FRAME_LEN`, 8 MiB).
const PASSTHROUGH_EVENT_BUDGET: usize = 6 * 1024 * 1024;

/// Split passthrough writes, in order, into groups that each fit one event.
fn batch_passthrough_writes(writes: Vec<PassthroughWrite>) -> Vec<Vec<PassthroughWrite>> {
    // Per-write cost of the cursor, vector entry and table overhead.
    const WRITE_OVERHEAD: usize = 32;
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut batch_bytes = 0;
    for write in writes {
        let cost = write.bytes.len() + WRITE_OVERHEAD;
        if !batch.is_empty() && batch_bytes + cost > PASSTHROUGH_EVENT_BUDGET {
            batches.push(std::mem::take(&mut batch));
            batch_bytes = 0;
        }
        batch_bytes += cost;
        batch.push(write);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

/// Conservative upper bound on a `BufferRecord`'s flatbuffers-encoded size, used
/// to keep `BuffersResponse` under [`BUFFERS_RESPONSE_BUDGET`]. It overestimates
/// (generous fixed and per-field overhead) so the real encoded frame always lands
/// within budget even though the exact flatbuffers layout isn't computed here.
fn estimated_buffer_record_size(record: &embers_protocol::BufferRecord) -> usize {
    // Fixed scalar fields, the table, and its vtable.
    const RECORD_OVERHEAD: usize = 256;
    // Length prefix, offset, and padding per variable-length string/vector entry.
    const FIELD_OVERHEAD: usize = 16;
    let mut size = RECORD_OVERHEAD;
    size += record.title.len() + FIELD_OVERHEAD;
    size += record.cwd.as_ref().map_or(0, String::len) + FIELD_OVERHEAD;
    for arg in &record.command {
        size += arg.len() + FIELD_OVERHEAD;
    }
    if let Some(pipe) = &record.pipe {
        for arg in &pipe.command {
            size += arg.len() + FIELD_OVERHEAD;
        }
    }
    for (key, value) in &record.env {
        size += key.len() + value.len() + 2 * FIELD_OVERHEAD;
    }
    for (key, value) in &record.user_options {
        size += key.len() + value.len() + 2 * FIELD_OVERHEAD;
    }
    size
}

#[derive(Debug)]
pub struct Server {
    config: ServerConfig,
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub async fn start(self) -> Result<ServerHandle> {
        if self.config.socket_path.exists() {
            std::fs::remove_file(&self.config.socket_path)?;
        }

        let mut state = load_workspace(&self.config.workspace_path)?.unwrap_or_default();
        state.remain_on_exit = self.config.remain_on_exit;
        let socket_path = self.config.socket_path.clone();
        let mut runtime = Runtime::new(
            state,
            self.config.socket_path.clone(),
            self.config.workspace_path.clone(),
            self.config.runtime_dir.clone(),
            self.config.buffer_env.clone(),
            self.config.limits.clone(),
        );
        runtime.restore = self.config.restore.clone();
        runtime.buffer_runtimes = BufferRuntimes::new(
            KeeperSettings {
                server_socket: self.config.socket_path.clone(),
                runtime_dir: self.config.runtime_dir.clone(),
                env: self.config.buffer_env.clone(),
            },
            Arc::new(CommandConnector::new(self.config.remote_command.clone())),
        );
        runtime.hold_saves_while =
            std::env::var_os(HOLD_WORKSPACE_SAVES_ENV_VAR).map(PathBuf::from);
        let runtime = Arc::new(runtime);
        runtime.restore_buffer_runtimes().await?;
        // Clean up buffers that are done: exited while the server was away, or
        // saved exited and detached. No client is connected yet to tell.
        runtime.dispose_exited_buffers(&mut Vec::new()).await;
        // Record what the restore changed (reattached pids, restarted panes).
        runtime.mark_workspace_dirty();
        let workspace_saver = runtime.spawn_workspace_saver();
        let listener = UnixListener::bind(&self.config.socket_path)?;
        set_socket_permissions(&self.config.socket_path)?;
        let shutdown_signal = runtime.shutdown.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let mut stop_requested = runtime.shutdown.subscribe();

        let join = tokio::spawn(async move {
            let _cleanup = SocketCleanup::new(socket_path.clone());
            info!(socket_path = %socket_path.display(), "mux server listening");

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        debug!("server shutdown requested");
                        break;
                    }
                    // A client asked the server to stop (kill-server).
                    () = wait_for_shutdown(&mut stop_requested) => {
                        debug!("server stop requested by a client");
                        break;
                    }
                    result = listener.accept() => {
                        let (stream, _) = result?;
                        let connection_id = runtime.next_connection_id.fetch_add(1, Ordering::Relaxed);
                        let (reader, writer) = stream.into_split();
                        let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
                        let (shutdown_tx, shutdown_rx) = oneshot::channel();
                        let (stopped_tx, stopped_rx) = oneshot::channel();
                        runtime
                            .register_client(connection_id, shutdown_tx, stopped_rx)
                            .await;

                        let write_runtime = runtime.clone();
                        let mut write_handle = tokio::spawn(write_loop(writer, outbound_rx));
                        // Held until the shutdown notice is flushed, so the
                        // server does not exit first; released right away
                        // otherwise.
                        let flush_task = runtime.connection_tasks.enter();
                        let read_runtime = runtime.clone();
                        let connection_task = runtime.connection_tasks.enter();
                        let read_handle = tokio::spawn(async move {
                            let _connection_task = connection_task;
                            handle_connection(
                                read_runtime,
                                connection_id,
                                reader,
                                outbound_tx,
                                shutdown_rx,
                            )
                            .await
                        });
                        tokio::spawn(async move {
                            let exit = match read_handle.await {
                                Ok(Ok(exit)) => exit,
                                Ok(Err(error)) => {
                                    error!(%error, connection_id, "connection failed");
                                    ConnectionExit::Closed
                                }
                                Err(error) => {
                                    error!(%error, connection_id, "read_loop panicked");
                                    ConnectionExit::Closed
                                }
                            };
                            let _ = stopped_tx.send(());

                            if exit == ConnectionExit::ServerStopped {
                                // Dropping the connection's subscriptions drops
                                // the last senders, so the writer ends once it
                                // has written what is queued.
                                write_runtime.cleanup_connection(connection_id).await;
                                if tokio::time::timeout(SHUTDOWN_FLUSH_TIMEOUT, &mut write_handle)
                                    .await
                                    .is_err()
                                {
                                    write_handle.abort();
                                }
                                return;
                            }
                            drop(flush_task);
                            match exit {
                                ConnectionExit::ServerStopped => {}
                                ConnectionExit::SelfDetached => match write_handle.await {
                                    Ok(Ok(())) => {}
                                    Ok(Err(error)) => {
                                        error!(%error, connection_id, "write_loop failed");
                                    }
                                    Err(error) if error.is_cancelled() => {}
                                    Err(error) => {
                                        error!(%error, connection_id, "write_loop panicked");
                                    }
                                },
                                ConnectionExit::Closed => {
                                    write_handle.abort();
                                    match write_handle.await {
                                        Ok(Ok(())) => {}
                                        Ok(Err(error)) => {
                                            error!(%error, connection_id, "write_loop failed");
                                        }
                                        Err(error) if error.is_cancelled() => {}
                                        Err(error) => {
                                            error!(%error, connection_id, "write_loop panicked");
                                        }
                                    }
                                }
                            };
                            write_runtime.cleanup_connection(connection_id).await;
                        });
                    }
                }
            }

            runtime.shutdown.trigger();
            if let Err(error) = workspace_saver.await {
                error!(%error, "workspace saver panicked");
            }
            let kill = runtime.kill_on_shutdown.load(Ordering::Acquire);
            runtime.buffer_runtimes.shut_down(kill).await;
            runtime.quiesce_state_tasks().await;
            if kill {
                // Every program is gone, so there is nothing for a restarted
                // server to restore.
                return match std::fs::remove_file(&runtime.workspace_path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                };
            }
            let persisted = runtime.persist_workspace().await;
            // Keepers the final save did not release stay up for the next server.
            runtime.buffer_runtimes.close();
            if let Err(error) = persisted {
                error!(%error, "failed to persist workspace during shutdown");
                return Err(error);
            }
            Ok(())
        });

        Ok(ServerHandle {
            socket_path: self.config.socket_path,
            shutdown: Some(shutdown_tx),
            shutdown_signal,
            join,
        })
    }
}

#[derive(Debug)]
pub struct ServerHandle {
    socket_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    shutdown_signal: ShutdownSignal,
    join: JoinHandle<Result<()>>,
}

impl ServerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Resolves once the server has been asked to stop, whether through
    /// [`Self::shutdown`] or by a client (`kill-server`). The caller still
    /// calls [`Self::shutdown`] to wait for it to finish.
    pub async fn stopped(&self) {
        let mut stopped = self.shutdown_signal.subscribe();
        wait_for_shutdown(&mut stopped).await;
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.shutdown_signal.trigger();
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }

        self.join
            .await
            .map_err(|error| MuxError::internal(error.to_string()))?
    }
}

struct SocketCleanup {
    socket_path: PathBuf,
}

impl SocketCleanup {
    fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[derive(Clone)]
struct ShutdownSignal {
    inner: Arc<ShutdownSignalInner>,
}

struct ShutdownSignalInner {
    active: AtomicBool,
    tx: watch::Sender<bool>,
}

impl std::fmt::Debug for ShutdownSignal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShutdownSignal")
            .field("active", &self.inner.active.load(Ordering::Relaxed))
            .finish()
    }
}

impl ShutdownSignal {
    fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self {
            inner: Arc::new(ShutdownSignalInner {
                active: AtomicBool::new(false),
                tx,
            }),
        }
    }

    fn trigger(&self) {
        if !self.inner.active.swap(true, Ordering::AcqRel) {
            self.inner.tx.send_replace(true);
        }
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.tx.subscribe()
    }
}

#[derive(Debug)]
struct Subscription {
    connection_id: u64,
    session_id: Option<embers_core::SessionId>,
    sender: mpsc::Sender<ServerEnvelope>,
}

struct ClientConnection {
    current_session_id: Option<embers_core::SessionId>,
    /// Whether the client forwards kitty graphics queries to a host terminal
    /// that answers them (`ClientRequest::SetHostGraphics`).
    host_graphics: bool,
    /// Buffers whose clipboard writes and passthrough the client also gets
    /// (`BufferRequest::Watch`), whether or not its session shows them.
    watched_buffers: BTreeSet<BufferId>,
    shutdown: Option<oneshot::Sender<()>>,
    stopped: Option<oneshot::Receiver<()>>,
}

impl std::fmt::Debug for ClientConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClientConnection")
            .field("current_session_id", &self.current_session_id)
            .finish_non_exhaustive()
    }
}

struct DetachedClient {
    shutdown: Option<oneshot::Sender<()>>,
    stopped: Option<oneshot::Receiver<()>>,
}

#[derive(Debug)]
struct Runtime {
    state: Mutex<ServerState>,
    buffer_runtimes: BufferRuntimes,
    workspace_path: PathBuf,
    limits: ResourceLimits,
    restore: RestoreConfig,
    /// See [`HOLD_WORKSPACE_SAVES_ENV_VAR`].
    hold_saves_while: Option<PathBuf>,
    /// Signalled whenever the workspace changes in a way worth saving; the
    /// workspace saver batches these into one write.
    workspace_dirty: Notify,
    /// Signalled when only buffer metadata (title, working directory,
    /// foreground command) changed; saved after a longer delay.
    workspace_metadata_dirty: Notify,
    /// The workspace last written to the workspace file. Held across a whole
    /// save so saves never interleave, and used to skip writing a workspace
    /// that has not changed in a way worth saving (see [`same_saved_state`]).
    last_saved_workspace: Mutex<Option<PersistedWorkspace>>,
    subscriptions: Mutex<BTreeMap<u64, Subscription>>,
    clients: Mutex<BTreeMap<u64, ClientConnection>>,
    /// Serializes [`Runtime::sync_host_graphics`].
    host_graphics_sync: Mutex<()>,
    next_connection_id: AtomicU64,
    next_subscription_id: AtomicU64,
    shutdown: ShutdownSignal,
    /// Set by `kill-server`: shutting down then ends every buffer's program and
    /// keeper and discards the workspace instead of leaving them to restore.
    kill_on_shutdown: AtomicBool,
    connection_tasks: TaskCounter,
    state_tasks: TaskCounter,
}

#[derive(Clone, Default)]
struct TaskCounter {
    inner: Arc<TaskCounterInner>,
}

struct TaskCounterInner {
    active: AtomicUsize,
    drained: Notify,
}

impl Default for TaskCounterInner {
    fn default() -> Self {
        Self {
            active: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }
}

impl std::fmt::Debug for TaskCounter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskCounter")
            .field("active", &self.inner.active.load(Ordering::Relaxed))
            .finish()
    }
}

#[must_use]
struct TaskTicket {
    inner: Arc<TaskCounterInner>,
}

impl TaskCounter {
    fn enter(&self) -> TaskTicket {
        self.inner.active.fetch_add(1, Ordering::Relaxed);
        TaskTicket {
            inner: self.inner.clone(),
        }
    }

    async fn wait_for_idle(&self) {
        loop {
            let notified = self.inner.drained.notified();
            if self.inner.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for TaskTicket {
    fn drop(&mut self) {
        if self.inner.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.drained.notify_waiters();
        }
    }
}

impl Runtime {
    fn new(
        state: ServerState,
        socket_path: PathBuf,
        workspace_path: PathBuf,
        runtime_dir: PathBuf,
        buffer_env: BTreeMap<String, OsString>,
        limits: ResourceLimits,
    ) -> Self {
        Self {
            state: Mutex::new(state),
            // `Server::start` applies the configured remote command.
            buffer_runtimes: BufferRuntimes::new(
                KeeperSettings {
                    server_socket: socket_path,
                    runtime_dir,
                    env: buffer_env,
                },
                Arc::new(CommandConnector::new(
                    DEFAULT_REMOTE_COMMAND.map(str::to_owned).to_vec(),
                )),
            ),
            workspace_path,
            limits,
            // `Server::start` applies the configured policy; a bare runtime
            // leaves panes without a keeper `Interrupted`.
            restore: RestoreConfig {
                enabled: false,
                ..RestoreConfig::default()
            },
            hold_saves_while: None,
            workspace_dirty: Notify::new(),
            workspace_metadata_dirty: Notify::new(),
            last_saved_workspace: Mutex::new(None),
            subscriptions: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
            host_graphics_sync: Mutex::new(()),
            next_connection_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
            shutdown: ShutdownSignal::new(),
            kill_on_shutdown: AtomicBool::new(false),
            connection_tasks: TaskCounter::default(),
            state_tasks: TaskCounter::default(),
        }
    }
}

impl Runtime {
    /// Write the workspace file unless it already holds the current state,
    /// then tell the keepers this save released (see
    /// [`BufferRuntimes::awaiting_save`]) that they are no longer needed. Only a
    /// snapshot of the state is taken under the state lock; encoding and the
    /// write happen outside it.
    async fn persist_workspace(&self) -> Result<()> {
        let mut last_saved = self.last_saved_workspace.lock().await;
        let (persisted, released) = {
            let state = self.state.lock().await;
            (state.to_persisted(), self.exits_recorded_in(&state))
        };
        if !last_saved
            .as_ref()
            .is_some_and(|saved| same_saved_state(saved, &persisted))
        {
            let path = self.workspace_path.clone();
            let persisted = tokio::task::spawn_blocking(move || {
                write_workspace(&path, &encode_persisted_workspace(&persisted)?).map(|()| persisted)
            })
            .await
            .map_err(|error| MuxError::internal(error.to_string()))??;
            *last_saved = Some(persisted);
        }
        drop(last_saved);
        self.buffer_runtimes.saved(released).await;
        Ok(())
    }

    /// The buffers whose keepers wait for a save that `state`, about to be
    /// saved, no longer needs them for: the buffer is exited, gone, or no
    /// longer refers to a keeper. Exited buffers never run again, so a later
    /// state cannot undo this.
    fn exits_recorded_in(&self, state: &ServerState) -> Vec<BufferId> {
        self.buffer_runtimes
            .awaiting_save()
            .into_iter()
            .filter(|buffer_id| {
                state.buffers.get(buffer_id).is_none_or(|buffer| {
                    matches!(buffer.state, BufferState::Exited(_))
                        || buffer.placement().socket_path().is_none()
                })
            })
            .collect()
    }

    /// Ask the workspace saver to write the workspace soon: a change to the
    /// layout, sessions or buffers, saved within [`WORKSPACE_SAVE_DELAY`].
    fn mark_workspace_dirty(&self) {
        self.workspace_dirty.notify_one();
    }

    /// Ask the workspace saver to write the workspace eventually: a change to
    /// a buffer's title, working directory or foreground command, which
    /// programs can make many times a second. These are saved within
    /// [`WORKSPACE_METADATA_SAVE_DELAY`], so a burst of them costs one write.
    fn mark_workspace_metadata_dirty(&self) {
        self.workspace_metadata_dirty.notify_one();
    }

    /// Saves the workspace after changes, so a crash or SIGKILL of the server
    /// loses at most the last [`WORKSPACE_SAVE_DELAY`] of layout changes and
    /// [`WORKSPACE_METADATA_SAVE_DELAY`] of title, directory and foreground
    /// command changes. The wait starts at the first change and is not
    /// extended by later ones, so a pane that never stops changing its title
    /// is still saved regularly. Stops at shutdown, which writes the final
    /// state itself.
    fn spawn_workspace_saver(self: &Arc<Self>) -> JoinHandle<()> {
        let runtime = self.clone();
        let mut shutdown = self.shutdown.subscribe();
        tokio::spawn(async move {
            loop {
                let delay = tokio::select! {
                    () = runtime.workspace_dirty.notified() => WORKSPACE_SAVE_DELAY,
                    () = runtime.workspace_metadata_dirty.notified() => {
                        WORKSPACE_METADATA_SAVE_DELAY
                    }
                    () = wait_for_shutdown(&mut shutdown) => return,
                };
                let mut deadline = tokio::time::Instant::now() + delay;
                loop {
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => break,
                        // A layout change during a long wait is saved as
                        // promptly as any other.
                        () = runtime.workspace_dirty.notified() => {
                            deadline = deadline
                                .min(tokio::time::Instant::now() + WORKSPACE_SAVE_DELAY);
                        }
                        () = wait_for_shutdown(&mut shutdown) => return,
                    }
                }
                if runtime
                    .hold_saves_while
                    .as_deref()
                    .is_some_and(Path::exists)
                {
                    continue;
                }
                if let Err(error) = runtime.persist_workspace().await {
                    warn!(%error, "failed to save workspace");
                }
            }
        })
    }

    async fn quiesce_state_tasks(&self) {
        self.connection_tasks.wait_for_idle().await;
        self.state_tasks.wait_for_idle().await;
    }

    /// The server's buffer runtimes, set up to report to it.
    fn runtimes(self: &Arc<Self>) -> &BufferRuntimes {
        self.buffer_runtimes.set_callbacks(|| {
            // Runtimes outliving the server report to nothing.
            let runtime = Arc::downgrade(self);
            let taken_up_runtime = runtime.clone();
            Callbacks {
                runtime: Arc::new(move || {
                    runtime
                        .upgrade()
                        .map(|runtime| runtime.buffer_runtime_callbacks())
                        .unwrap_or_default()
                }),
                taken_up: Arc::new(move |buffer_id, status| {
                    let Some(runtime) = taken_up_runtime.upgrade() else {
                        return;
                    };
                    let task = runtime.state_tasks.enter();
                    tokio::spawn(async move {
                        let _task = task;
                        runtime.record_buffer_taken_up(buffer_id, status).await;
                    });
                }),
            }
        });
        &self.buffer_runtimes
    }

    fn buffer_runtime_callbacks(self: &Arc<Self>) -> BufferRuntimeCallbacks {
        let output_handle = tokio::runtime::Handle::current();
        let exit_handle = output_handle.clone();
        let clipboard_handle = output_handle.clone();
        let output_runtime = self.clone();
        let exit_runtime = self.clone();
        let clipboard_runtime = self.clone();
        let output_tasks = self.state_tasks.clone();
        let exit_tasks = self.state_tasks.clone();
        let clipboard_tasks = self.state_tasks.clone();
        let passthrough_tasks = self.state_tasks.clone();
        // Passthrough writes must reach clients in the order the program sent
        // them (an image is many sequences), so unlike the other callbacks they
        // go through one task per runtime instead of a task per call. The task
        // ends when the callbacks, and with them the sender, are dropped.
        let (passthrough_tx, mut passthrough_rx) =
            tokio::sync::mpsc::unbounded_channel::<(BufferId, Vec<PassthroughWrite>, TaskTicket)>();
        let passthrough_runtime = self.clone();
        std::mem::drop(output_handle.spawn(async move {
            while let Some((buffer_id, writes, task)) = passthrough_rx.recv().await {
                passthrough_runtime
                    .forward_passthrough(buffer_id, writes)
                    .await;
                drop(task);
            }
        }));

        BufferRuntimeCallbacks {
            on_output: Arc::new(move |buffer_id, update| {
                let runtime = output_runtime.clone();
                let task = output_tasks.enter();
                std::mem::drop(output_handle.spawn(async move {
                    let _task = task;
                    runtime.record_buffer_update(buffer_id, update).await;
                }));
            }),
            on_ended: Arc::new(move |buffer_id, end| {
                let runtime = exit_runtime.clone();
                let task = exit_tasks.enter();
                std::mem::drop(exit_handle.spawn(async move {
                    let _task = task;
                    runtime.record_buffer_end(buffer_id, end).await;
                }));
            }),
            on_clipboard: Arc::new(move |buffer_id, write| {
                let runtime = clipboard_runtime.clone();
                let task = clipboard_tasks.enter();
                std::mem::drop(clipboard_handle.spawn(async move {
                    let _task = task;
                    runtime.forward_clipboard(buffer_id, write).await;
                }));
            }),
            on_passthrough: Arc::new(move |buffer_id, writes| {
                let task = passthrough_tasks.enter();
                if passthrough_tx.send((buffer_id, writes, task)).is_err() {
                    debug!(%buffer_id, "dropping passthrough: forwarder stopped");
                }
            }),
        }
    }

    async fn restore_buffer_runtimes(self: &Arc<Self>) -> Result<()> {
        let buffers = {
            let state = self.state.lock().await;
            state.buffers.values().cloned().collect::<Vec<_>>()
        };

        let mut placements = Vec::new();
        let mut pid_hints = BTreeMap::new();
        for buffer in buffers {
            pid_hints.insert(buffer.id, buffer_pid_hint(&buffer.state));
            match buffer.placement() {
                // Interrupted until its host is reached again, in the
                // background (see `BufferRuntimes::restore`).
                Placement::Remote { .. } => {
                    if !matches!(buffer.state, BufferState::Exited(_)) {
                        let mut state = self.state.lock().await;
                        let _ = state.mark_buffer_interrupted(buffer.id, None);
                    }
                    placements.push((buffer.id, buffer.placement().clone()));
                }
                Placement::Local {
                    socket_path: Some(_),
                } => placements.push((buffer.id, buffer.placement().clone())),
                Placement::Local { socket_path: None } => {
                    if matches!(buffer.state, BufferState::Running(_) | BufferState::Created) {
                        let mut state = self.state.lock().await;
                        let _ = state
                            .mark_buffer_interrupted(buffer.id, buffer_pid_hint(&buffer.state));
                    }
                }
            }
        }

        for (buffer_id, restored) in self.runtimes().restore(placements).await {
            let mut state = self.state.lock().await;
            match restored {
                Restored::Running(status) => apply_runtime_status(&mut state, buffer_id, &status),
                Restored::Gone => {
                    let _ = state.set_buffer_placement(buffer_id, Placement::default());
                    let pid = pid_hints.get(&buffer_id).copied().flatten();
                    let _ = state.mark_buffer_interrupted(buffer_id, pid);
                }
            }
        }

        if self.restore.enabled {
            self.restart_interrupted_buffers().await;
        }
        Ok(())
    }

    /// Record that `buffer_id`, interrupted on its host, runs there again, as
    /// its runtime reported in `status` if it did.
    async fn record_buffer_taken_up(
        &self,
        buffer_id: BufferId,
        status: Option<BufferRuntimeStatus>,
    ) {
        let known = {
            let mut state = self.state.lock().await;
            if let Some(status) = &status {
                apply_runtime_status(&mut state, buffer_id, status);
            }
            state.buffers.contains_key(&buffer_id)
        };
        if !known {
            // Removed while its host was being reached.
            self.buffer_runtimes.release(buffer_id).await;
            return;
        }
        self.mark_workspace_dirty();
        self.broadcast(
            vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                buffer_id,
            })],
            &[],
        )
        .await;
    }

    /// Start a fresh process for every PTY buffer left `Interrupted` that a
    /// pane shows: its keeper is gone, so it cannot be reattached. Restarts run
    /// concurrently so a large workspace does not hold up startup.
    ///
    /// Like tmux-resurrect, only what is in the layout is restored. A detached
    /// buffer has no place to come back to and its terminal contents are not
    /// restored, so a fresh shell for it would only be a hidden process
    /// nobody asked for; its record is dropped instead.
    async fn restart_interrupted_buffers(self: &Arc<Self>) {
        let interrupted: Vec<_> = {
            let mut state = self.state.lock().await;
            let (attached, detached): (Vec<_>, Vec<_>) = state
                .buffers
                .values()
                // A buffer on another host is not this server's to restart:
                // its program may still run there.
                .filter(|buffer| {
                    matches!(buffer.state, BufferState::Interrupted(_))
                        && matches!(buffer.kind, BufferKind::Pty)
                        && buffer.placement().host().is_none()
                })
                .map(|buffer| (buffer.id, buffer.attachment.clone()))
                .partition(|(_, attachment)| matches!(attachment, BufferAttachment::Attached(_)));
            for (buffer_id, _) in detached {
                if state.remove_buffer(buffer_id).is_ok() {
                    info!(%buffer_id, "dropped detached buffer whose keeper was gone");
                }
            }
            attached
                .into_iter()
                .map(|(buffer_id, _)| buffer_id)
                .collect()
        };
        let mut restarts = tokio::task::JoinSet::new();
        for buffer_id in interrupted {
            let runtime = self.clone();
            restarts.spawn(async move {
                if let Err(error) = runtime.restart_buffer(buffer_id).await {
                    warn!(%buffer_id, %error, "failed to restore buffer");
                }
            });
        }
        while restarts.join_next().await.is_some() {}
    }

    /// Restore one buffer whose keeper is gone: start the shell (or allowlisted
    /// program) its [`RestorePlan`](crate::restore::RestorePlan) names in its
    /// last directory, keeping its id, title and place in the layout.
    async fn restart_buffer(self: &Arc<Self>, buffer_id: BufferId) -> Result<()> {
        let plan = {
            let state = self.state.lock().await;
            plan_restore(state.buffer(buffer_id)?, &self.restore, &default_shell())
        };
        // The record keeps its command and directory until the new keeper
        // runs: a restore that fails leaves (and saves) the pane as it was,
        // so a later restore can still bring back what it ran.
        self.spawn_buffer_runtime_with(buffer_id, plan.command.clone(), plan.cwd.clone())
            .await?;
        if let Some(buffer) = self.state.lock().await.buffers.get_mut(&buffer_id) {
            // The program that set it is gone; a rerun Neovim sets it again.
            buffer.user_options.remove(IS_VIM_OPTION);
        }
        info!(%buffer_id, command = ?plan.command, "restored buffer whose keeper was gone");

        if let Some(command_line) = plan.typed_command {
            let runtime = self.buffer_runtime(buffer_id).await?;
            let task = self.state_tasks.enter();
            tokio::spawn(async move {
                let _task = task;
                // Wait for the shell's first output (its prompt) so the command
                // is not echoed ahead of it, then type it like a user would.
                let deadline = tokio::time::Instant::now() + RESTORED_SHELL_READY_TIMEOUT;
                while tokio::time::Instant::now() < deadline {
                    match runtime.status().await {
                        Ok(status) if status.sequence > 0 || !status.running => break,
                        Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
                        Err(_) => return,
                    }
                }
                let mut bytes = command_line.into_bytes();
                bytes.push(b'\r');
                if let Err(error) = runtime.write(bytes).await {
                    debug!(%buffer_id, %error, "failed to rerun command in restored buffer");
                }
            });
        }
        Ok(())
    }

    async fn register_client(
        &self,
        connection_id: u64,
        shutdown: oneshot::Sender<()>,
        stopped: oneshot::Receiver<()>,
    ) {
        self.clients.lock().await.insert(
            connection_id,
            ClientConnection {
                current_session_id: None,
                host_graphics: false,
                watched_buffers: BTreeSet::new(),
                shutdown: Some(shutdown),
                stopped: Some(stopped),
            },
        );
    }

    async fn dispatch_request(
        self: &Arc<Self>,
        connection_id: u64,
        outbound: &mpsc::Sender<ServerEnvelope>,
        request: ClientMessage,
    ) -> (
        ServerResponse,
        Vec<ServerEvent>,
        Option<oneshot::Sender<()>>,
    ) {
        match request {
            // Answered whatever the client speaks: the client decides whether
            // it can work with this server, and says why if not.
            ClientMessage::Hello(request) => (
                ServerResponse::Hello(HelloResponse {
                    request_id: request.request_id,
                    protocol_version: PROTOCOL_VERSION,
                    software_version: SOFTWARE_VERSION.to_owned(),
                }),
                Vec::new(),
                None,
            ),
            ClientMessage::Ping(request) => (
                ServerResponse::Pong(PingResponse {
                    request_id: request.request_id,
                    payload: request.payload,
                }),
                Vec::new(),
                None,
            ),
            // The connection triggers the stop once this response is sent.
            ClientMessage::KillServer(request) => {
                self.kill_on_shutdown.store(true, Ordering::Release);
                (
                    ServerResponse::Ok(OkResponse {
                        request_id: request.request_id,
                    }),
                    Vec::new(),
                    None,
                )
            }
            // These can attach or detach buffers, which can make an exited
            // buffer due for cleanup (a dead pane closed, a buffer whose command
            // exited before it was first shown now attached).
            // Closing a view (the kill commands do) can also close a session's
            // last pane, which destroys the session.
            ClientMessage::Session(request) => {
                let closes = matches!(request, SessionRequest::CloseRootTab { .. });
                let had_panes = self.sessions_with_panes_if(closes).await;
                let (resp, mut events) = self.dispatch_session(request).await;
                self.dispose_exited_buffers(&mut events).await;
                self.destroy_emptied_sessions(had_panes, &mut events).await;
                (resp, events, None)
            }
            ClientMessage::Buffer(request) => {
                let (resp, mut events) = self.dispatch_buffer(connection_id, request).await;
                self.dispose_exited_buffers(&mut events).await;
                (resp, events, None)
            }
            ClientMessage::Node(request) => {
                let closes = matches!(request, embers_protocol::NodeRequest::Close { .. });
                let had_panes = self.sessions_with_panes_if(closes).await;
                let (resp, mut events) = self.dispatch_node(request).await;
                self.dispose_exited_buffers(&mut events).await;
                self.destroy_emptied_sessions(had_panes, &mut events).await;
                (resp, events, None)
            }
            ClientMessage::Floating(request) => {
                let closes = matches!(request, FloatingRequest::Close { .. });
                let had_panes = self.sessions_with_panes_if(closes).await;
                let (resp, mut events) = self.dispatch_floating(request).await;
                self.dispose_exited_buffers(&mut events).await;
                self.destroy_emptied_sessions(had_panes, &mut events).await;
                (resp, events, None)
            }
            ClientMessage::Input(request) => {
                let (resp, events) = self.dispatch_input(request).await;
                (resp, events, None)
            }
            ClientMessage::Subscribe(request) => {
                let subscription_id = self.next_subscription_id.fetch_add(1, Ordering::Relaxed);
                self.subscriptions.lock().await.insert(
                    subscription_id,
                    Subscription {
                        connection_id,
                        session_id: request.session_id,
                        sender: outbound.clone(),
                    },
                );
                (
                    ServerResponse::SubscriptionAck(SubscriptionAckResponse {
                        request_id: request.request_id,
                        subscription_id,
                    }),
                    Vec::new(),
                    None,
                )
            }
            ClientMessage::Unsubscribe(request) => {
                let mut subscriptions = self.subscriptions.lock().await;
                match subscriptions.get(&request.subscription_id) {
                    Some(subscription) if subscription.connection_id == connection_id => {
                        subscriptions.remove(&request.subscription_id);
                        (
                            ServerResponse::Ok(OkResponse {
                                request_id: request.request_id,
                            }),
                            Vec::new(),
                            None,
                        )
                    }
                    Some(_) => (
                        error_response(
                            Some(request.request_id),
                            ErrorCode::Conflict,
                            format!(
                                "subscription {} does not belong to this connection",
                                request.subscription_id
                            ),
                        ),
                        Vec::new(),
                        None,
                    ),
                    None => (
                        error_response(
                            Some(request.request_id),
                            ErrorCode::NotFound,
                            format!("unknown subscription {}", request.subscription_id),
                        ),
                        Vec::new(),
                        None,
                    ),
                }
            }
            ClientMessage::Client(request) => self.dispatch_client(connection_id, request).await,
        }
    }

    async fn dispatch_client(
        &self,
        connection_id: u64,
        request: ClientRequest,
    ) -> (
        ServerResponse,
        Vec<ServerEvent>,
        Option<oneshot::Sender<()>>,
    ) {
        match request {
            ClientRequest::List { request_id } => (
                ServerResponse::Clients(ClientsResponse {
                    request_id,
                    clients: self.list_clients().await,
                }),
                Vec::new(),
                None,
            ),
            ClientRequest::Get {
                request_id,
                client_id,
            } => {
                let target_id = client_id.map(|id| id.get()).unwrap_or(connection_id);
                match self.client_record(target_id).await {
                    Some(client) => (
                        ServerResponse::Client(ClientResponse { request_id, client }),
                        Vec::new(),
                        None,
                    ),
                    None => (
                        error_response(
                            Some(request_id),
                            ErrorCode::NotFound,
                            format!("unknown client {}", target_id),
                        ),
                        Vec::new(),
                        None,
                    ),
                }
            }
            ClientRequest::Detach {
                request_id,
                client_id,
            } => {
                let target_id = client_id.map(|id| id.get()).unwrap_or(connection_id);
                let is_self_detach = target_id == connection_id;
                match self.detach_client(target_id).await {
                    Ok(detached) => match (is_self_detach, detached) {
                        (true, DetachedClient { shutdown, .. }) => (
                            ServerResponse::Ok(OkResponse { request_id }),
                            Vec::new(),
                            shutdown,
                        ),
                        (
                            false,
                            DetachedClient {
                                shutdown,
                                mut stopped,
                            },
                        ) => {
                            if let Some(shutdown) = shutdown {
                                let _ = shutdown.send(());
                            }
                            if let Some(stopped) = stopped.take() {
                                let _ = stopped.await;
                            }
                            (
                                ServerResponse::Ok(OkResponse { request_id }),
                                Vec::new(),
                                None,
                            )
                        }
                    },
                    Err(error) => (
                        mux_error_response(Some(request_id), error),
                        Vec::new(),
                        None,
                    ),
                }
            }
            ClientRequest::DisplayMessage {
                request_id,
                client_id,
                session_id,
                message,
                duration_ms,
            } => {
                let event = ServerEvent::DisplayMessage(DisplayMessageEvent {
                    message,
                    duration_ms,
                });
                let response = match self.display_message(client_id, session_id, event).await {
                    Ok(()) => ServerResponse::Ok(OkResponse { request_id }),
                    Err(error) => mux_error_response(Some(request_id), error),
                };
                (response, Vec::new(), None)
            }
            ClientRequest::SetHostGraphics {
                request_id,
                enabled,
            } => {
                if let Some(client) = self.clients.lock().await.get_mut(&connection_id) {
                    client.host_graphics = enabled;
                }
                self.sync_host_graphics().await;
                (
                    ServerResponse::Ok(OkResponse { request_id }),
                    Vec::new(),
                    None,
                )
            }
            ClientRequest::Switch {
                request_id,
                client_id,
                session_id,
                node_id,
            } => {
                let target_id = client_id.map(|id| id.get()).unwrap_or(connection_id);
                // The window and pane change before the client switches, so
                // it never shows the session as it was, and every client of
                // the session sees the same change.
                let mut events = Vec::new();
                if let Some(node_id) = node_id {
                    if self.client_record(target_id).await.is_none() {
                        return (
                            mux_error_response(
                                Some(request_id),
                                MuxError::not_found(format!("client {target_id} was not found")),
                            ),
                            Vec::new(),
                            None,
                        );
                    }
                    let state = &mut *self.state.lock().await;
                    match state
                        .show_node(session_id, node_id)
                        .and_then(|()| layout_changed_events(state, session_id))
                    {
                        Ok(layout_events) => events = layout_events,
                        Err(error) => {
                            return (
                                mux_error_response(Some(request_id), error),
                                Vec::new(),
                                None,
                            );
                        }
                    }
                }
                match self.set_client_session(target_id, Some(session_id)).await {
                    Ok((client, client_events)) => (
                        ServerResponse::Client(ClientResponse { request_id, client }),
                        {
                            events.extend(client_events);
                            events
                        },
                        None,
                    ),
                    Err(error) => (
                        mux_error_response(Some(request_id), error),
                        Vec::new(),
                        None,
                    ),
                }
            }
        }
    }

    async fn dispatch_session(
        &self,
        request: SessionRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        let mut state = self.state.lock().await;

        match request {
            SessionRequest::Create { request_id, name } => {
                if state.sessions.len() >= self.limits.max_sessions {
                    return (
                        error_response(
                            Some(request_id),
                            ErrorCode::Conflict,
                            limit_reached_message(
                                "session",
                                self.limits.max_sessions,
                                crate::config::MAX_SESSIONS_ENV_VAR,
                            ),
                        ),
                        Vec::new(),
                    );
                }
                let session_id = state.create_session(name);
                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => (
                        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                            request_id,
                            snapshot: snapshot.clone(),
                        }),
                        vec![ServerEvent::SessionCreated(SessionCreatedEvent {
                            session: snapshot.session,
                        })],
                    ),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            SessionRequest::List { request_id } => (
                ServerResponse::Sessions(SessionsResponse {
                    request_id,
                    sessions: state.sessions.values().map(session_record).collect(),
                }),
                Vec::new(),
            ),
            SessionRequest::Get {
                request_id,
                session_id,
            } => match session_snapshot(&state, session_id) {
                Ok(snapshot) => (
                    ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                        request_id,
                        snapshot,
                    }),
                    Vec::new(),
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            SessionRequest::Close {
                request_id,
                session_id,
                force: _,
            } => {
                let changed_clients = {
                    let mut clients = self.clients.lock().await;
                    match state.close_session(session_id) {
                        Ok(()) => Self::clear_client_session(&mut clients, session_id),
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    }
                };
                drop(state);
                let mut events = vec![ServerEvent::SessionClosed(SessionClosedEvent {
                    session_id,
                })];
                events.extend(self.client_changed_events(changed_clients).await);
                (ServerResponse::Ok(OkResponse { request_id }), events)
            }
            SessionRequest::Rename {
                request_id,
                session_id,
                name,
            } => match state.rename_session(session_id, name) {
                Ok(()) => match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let name = snapshot.session.name.clone();
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            vec![ServerEvent::SessionRenamed(SessionRenamedEvent {
                                session_id,
                                name,
                            })],
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                },
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            SessionRequest::AddRootTab {
                request_id,
                session_id,
                title,
                buffer_id,
                child_node_id,
            } => {
                let result = match (buffer_id, child_node_id) {
                    (Some(buffer_id), None) => {
                        state.add_root_tab_from_buffer(session_id, title, buffer_id)
                    }
                    (None, Some(child_node_id)) => {
                        state.add_root_tab_from_subtree(session_id, title, child_node_id)
                    }
                    (Some(_), Some(_)) => Err(MuxError::invalid_input(
                        "add-root-tab requires either buffer_id or child_node_id, not both",
                    )),
                    (None, None) => Err(MuxError::invalid_input(
                        "add-root-tab requires either buffer_id or child_node_id",
                    )),
                };
                match result {
                    Ok(_) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            SessionRequest::SelectRootTab {
                request_id,
                session_id,
                index,
            } => match protocol_tab_index(index)
                .and_then(|index| state.select_root_tab(session_id, index))
            {
                Ok(()) => layout_snapshot_response(&state, request_id, session_id),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            SessionRequest::RenameRootTab {
                request_id,
                session_id,
                index,
                title,
            } => match protocol_tab_index(index)
                .and_then(|index| state.rename_root_tab(session_id, index, title))
            {
                Ok(()) => layout_snapshot_response(&state, request_id, session_id),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            SessionRequest::CloseRootTab {
                request_id,
                session_id,
                index,
            } => match protocol_tab_index(index)
                .and_then(|index| state.close_root_tab(session_id, index))
            {
                Ok(()) => layout_snapshot_response(&state, request_id, session_id),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            // No event: the viewport only seeds the size of panes created
            // later, and other clients draw at their own size anyway. Without
            // one the workspace is not saved on its own, so ask for that here.
            SessionRequest::SetViewport {
                request_id,
                session_id,
                viewport,
                pane_titles,
            } => match state.set_session_viewport(session_id, viewport, pane_titles) {
                Ok(()) => {
                    self.mark_workspace_dirty();
                    (ServerResponse::Ok(OkResponse { request_id }), Vec::new())
                }
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            // No event either: clients number windows from their own config.
            SessionRequest::SetBaseIndex {
                request_id,
                session_id,
                base_index,
            } => match state.set_session_base_index(session_id, base_index) {
                Ok(()) => {
                    self.mark_workspace_dirty();
                    (ServerResponse::Ok(OkResponse { request_id }), Vec::new())
                }
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            SessionRequest::SetRootTabSynchronizePanes {
                request_id,
                session_id,
                index,
                mode,
            } => {
                let enabled = match mode {
                    SynchronizePanesMode::Toggle => None,
                    SynchronizePanesMode::On => Some(true),
                    SynchronizePanesMode::Off => Some(false),
                };
                match protocol_tab_index(index).and_then(|index| {
                    state.set_root_tab_synchronize_panes(session_id, index, enabled)
                }) {
                    Ok(_) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            SessionRequest::SetEnvironment {
                request_id,
                session_id,
                name,
                value,
            } => {
                if let Some(session_id) = session_id
                    && let Err(error) = state.session(session_id)
                {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                // Sessions are never reused, so forget the environments of
                // closed ones here rather than when they close.
                let ServerState {
                    sessions,
                    environment,
                    ..
                } = &mut *state;
                environment.retain_sessions(|session_id| sessions.contains_key(&session_id));
                match environment.set(session_id, &name, value) {
                    Ok(()) => (ServerResponse::Ok(OkResponse { request_id }), Vec::new()),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            SessionRequest::ShowEnvironment {
                request_id,
                session_id,
            } => {
                if let Some(session_id) = session_id
                    && let Err(error) = state.session(session_id)
                {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                (
                    ServerResponse::Environment(EnvironmentResponse {
                        request_id,
                        env: state.environment.get(session_id),
                    }),
                    Vec::new(),
                )
            }
        }
    }

    async fn dispatch_buffer(
        self: &Arc<Self>,
        connection_id: u64,
        request: BufferRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        match request {
            BufferRequest::Create {
                request_id,
                title,
                command,
                cwd,
                env,
                size,
                session_id,
                host,
            } => {
                if let Some(host) = &host
                    && let Err(error) = validate_host(host)
                {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                if command.is_empty() {
                    return (
                        error_response(
                            Some(request_id),
                            ErrorCode::InvalidRequest,
                            "buffer command must not be empty",
                        ),
                        Vec::new(),
                    );
                }

                let buffer_id = {
                    let mut state = self.state.lock().await;
                    if state.buffers.len() >= self.limits.max_buffers {
                        return (
                            error_response(
                                Some(request_id),
                                ErrorCode::Conflict,
                                limit_reached_message(
                                    "buffer",
                                    self.limits.max_buffers,
                                    crate::config::MAX_BUFFERS_ENV_VAR,
                                ),
                            ),
                            Vec::new(),
                        );
                    }
                    if let Some(session_id) = session_id
                        && let Err(error) = state.session(session_id)
                    {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                    // The session's environment goes under the creator's own
                    // variables and is recorded with the buffer, so a restored
                    // pane gets it again. It describes this machine, so a
                    // buffer on another host gets the creator's variables
                    // alone, under that host's own environment.
                    let mut buffer_env = if host.is_some() {
                        BTreeMap::new()
                    } else {
                        state.environment.for_new_buffer(session_id)
                    };
                    buffer_env.extend(env);
                    let buffer_id = state.create_buffer_with_env(
                        title.unwrap_or_else(|| "buffer".to_owned()),
                        command,
                        cwd.map(Into::into),
                        buffer_env,
                    );
                    if let Some(host) = host {
                        let _ = state.set_buffer_placement(
                            buffer_id,
                            Placement::Remote {
                                host,
                                buffer_id: None,
                            },
                        );
                    }
                    // Start the command at the size it will be drawn at, so it
                    // never sees the default size first.
                    if let Some(size) = size
                        && let Err(error) = state.set_buffer_size(buffer_id, size)
                    {
                        let _ = state.remove_buffer(buffer_id);
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                    buffer_id
                };

                if let Err(error) = self.spawn_buffer_runtime(buffer_id).await {
                    let mut state = self.state.lock().await;
                    let _ = state.remove_buffer(buffer_id);
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }

                let record = self
                    .state
                    .lock()
                    .await
                    .buffer(buffer_id)
                    .map(buffer_record)
                    .map_err(|error| mux_error_response(Some(request_id), error));
                match record {
                    Ok(record) => (
                        ServerResponse::Buffer(BufferResponse {
                            request_id,
                            buffer: record.clone(),
                        }),
                        vec![ServerEvent::BufferCreated(BufferCreatedEvent {
                            buffer: record,
                        })],
                    ),
                    Err(error) => (error, Vec::new()),
                }
            }
            BufferRequest::List {
                request_id,
                session_id,
                attached_only,
                detached_only,
            } => {
                if attached_only && detached_only {
                    return (
                        error_response(
                            Some(request_id),
                            ErrorCode::InvalidRequest,
                            "attached_only and detached_only cannot both be true",
                        ),
                        Vec::new(),
                    );
                }

                let state = self.state.lock().await;
                let matching: Vec<_> = state
                    .buffers
                    .values()
                    .filter(|buffer| {
                        if attached_only && matches!(buffer.attachment, BufferAttachment::Detached)
                        {
                            return false;
                        }
                        if detached_only && !matches!(buffer.attachment, BufferAttachment::Detached)
                        {
                            return false;
                        }
                        match session_id {
                            Some(session_id) => match buffer.attachment {
                                BufferAttachment::Attached(node_id) => state
                                    .node(node_id)
                                    .map(|node| node.session_id() == session_id)
                                    .unwrap_or(false),
                                BufferAttachment::Detached => false,
                            },
                            None => true,
                        }
                    })
                    .collect();
                let total = matching.len();
                let mut buffers = Vec::with_capacity(total);
                let mut budget = 0usize;
                for buffer in matching {
                    let record = buffer_record(buffer);
                    let size = estimated_buffer_record_size(&record);
                    // Always include at least one record; past that, stop before
                    // the aggregate response could exceed the frame cap.
                    if !buffers.is_empty() && budget.saturating_add(size) > BUFFERS_RESPONSE_BUDGET
                    {
                        break;
                    }
                    budget += size;
                    buffers.push(record);
                }
                if buffers.len() < total {
                    warn!(
                        included = buffers.len(),
                        total, "buffer list truncated to stay under the response frame budget"
                    );
                }

                (
                    ServerResponse::Buffers(BuffersResponse {
                        request_id,
                        buffers,
                    }),
                    Vec::new(),
                )
            }
            BufferRequest::Get {
                request_id,
                buffer_id,
            } => match self.state.lock().await.buffer(buffer_id) {
                Ok(buffer) => (
                    ServerResponse::Buffer(BufferResponse {
                        request_id,
                        buffer: buffer_record(buffer),
                    }),
                    Vec::new(),
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::Watch {
                request_id,
                buffer_id,
            } => {
                match self.state.lock().await.buffers.get_mut(&buffer_id) {
                    Some(buffer) => {
                        buffer.watched = true;
                        buffer.ever_attached = true;
                    }
                    None => {
                        let error = MuxError::not_found(format!("unknown buffer {buffer_id}"));
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                }
                if let Some(client) = self.clients.lock().await.get_mut(&connection_id) {
                    client.watched_buffers.insert(buffer_id);
                }
                (ServerResponse::Ok(OkResponse { request_id }), Vec::new())
            }
            BufferRequest::Unwatch {
                request_id,
                buffer_id,
            } => {
                if let Some(client) = self.clients.lock().await.get_mut(&connection_id) {
                    client.watched_buffers.remove(&buffer_id);
                }
                let mut events = Vec::new();
                self.sync_watched_buffers([buffer_id], &mut events).await;
                (ServerResponse::Ok(OkResponse { request_id }), events)
            }
            BufferRequest::Inspect {
                request_id,
                buffer_id,
            } => {
                let state = self.state.lock().await;
                match state.buffer(buffer_id) {
                    Ok(buffer) => match buffer_location(&state, buffer_id).and_then(|location| {
                        location_is_root_tab(&state, &location)
                            .map(|at_root_tab| (location, at_root_tab))
                    }) {
                        Ok((location, at_root_tab)) => match BufferWithLocationResponse::new(
                            request_id,
                            buffer_record(buffer),
                            location,
                            at_root_tab,
                        ) {
                            Ok(response) => {
                                (ServerResponse::BufferWithLocation(response), Vec::new())
                            }
                            Err(error) => (
                                mux_error_response(Some(request_id), MuxError::protocol(error)),
                                Vec::new(),
                            ),
                        },
                        Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                    },
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            BufferRequest::Detach {
                request_id,
                buffer_id,
            } => {
                let mut state = self.state.lock().await;
                let attached_view = match state.buffer(buffer_id) {
                    Ok(buffer) => match buffer.attachment {
                        BufferAttachment::Attached(node_id) => Some(node_id),
                        BufferAttachment::Detached => None,
                    },
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };

                let mut events = vec![ServerEvent::BufferDetached(BufferDetachedEvent {
                    buffer_id,
                })];
                if let Some(view_id) = attached_view {
                    let session_id = match state.node(view_id) {
                        Ok(node) => node.session_id(),
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    };
                    if let Err(error) = state.close_node(view_id) {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                    if let Some(focus_event) = focus_changed_event(&state, session_id) {
                        events.push(ServerEvent::FocusChanged(focus_event));
                    }
                    events.push(ServerEvent::NodeChanged(NodeChangedEvent { session_id }));
                }

                (ServerResponse::Ok(OkResponse { request_id }), events)
            }
            BufferRequest::Kill {
                request_id,
                buffer_id,
                force: _,
            } => match self.kill_buffer(buffer_id).await {
                Ok(()) => (ServerResponse::Ok(OkResponse { request_id }), Vec::new()),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::Capture {
                request_id,
                buffer_id,
            } => match self.capture_snapshot(request_id, buffer_id).await {
                Ok(snapshot) => (ServerResponse::Snapshot(snapshot), Vec::new()),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::CaptureVisible {
                request_id,
                buffer_id,
            } => match self.capture_visible_snapshot(request_id, buffer_id).await {
                Ok(snapshot) => (ServerResponse::VisibleSnapshot(snapshot), Vec::new()),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::ScrollbackSlice {
                request_id,
                buffer_id,
                start_line,
                line_count,
            } => match self
                .capture_scrollback_slice(request_id, buffer_id, start_line, line_count)
                .await
            {
                Ok(snapshot) => (ServerResponse::ScrollbackSlice(snapshot), Vec::new()),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::GetLocation {
                request_id,
                buffer_id,
            } => {
                let state = self.state.lock().await;
                match buffer_location(&state, buffer_id) {
                    Ok(location) => (
                        ServerResponse::BufferLocation(BufferLocationResponse {
                            request_id,
                            location,
                        }),
                        Vec::new(),
                    ),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            BufferRequest::Reveal {
                request_id,
                buffer_id,
                client_id,
            } => match self
                .reveal_buffer(
                    connection_id,
                    client_id.map(std::num::NonZeroU64::get),
                    buffer_id,
                )
                .await
            {
                Ok((location, events)) => (
                    ServerResponse::BufferLocation(BufferLocationResponse {
                        request_id,
                        location,
                    }),
                    events,
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::OpenHistory {
                request_id,
                buffer_id,
                scope,
                placement,
                client_id,
            } => match self
                .open_history_buffer(
                    connection_id,
                    client_id.map(std::num::NonZeroU64::get),
                    buffer_id,
                    scope,
                    placement,
                )
                .await
            {
                Ok((location, mut reveal_events)) => {
                    let mut events = Vec::new();
                    let (buffer, at_root_tab) = {
                        let state = self.state.lock().await;
                        match state.buffer(location.buffer_id) {
                            Ok(buffer) => match location_is_root_tab(&state, &location) {
                                Ok(at_root_tab) => (buffer_record(buffer), at_root_tab),
                                Err(error) => {
                                    return (
                                        mux_error_response(Some(request_id), error),
                                        Vec::new(),
                                    );
                                }
                            },
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        }
                    };
                    events.push(ServerEvent::BufferCreated(BufferCreatedEvent {
                        buffer: buffer.clone(),
                    }));
                    match &location.attachment {
                        BufferLocationAttachment::Floating {
                            session_id,
                            floating_id,
                            ..
                        } => {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id: *session_id,
                                floating_id: Some(*floating_id),
                            }));
                        }
                        BufferLocationAttachment::Session { session_id, .. }
                            if !reveal_events.iter().any(|event| {
                            matches!(
                                event,
                                ServerEvent::NodeChanged(NodeChangedEvent { session_id: changed })
                                    if *changed == *session_id
                            )
                        }) =>
                        {
                            events.push(ServerEvent::NodeChanged(NodeChangedEvent {
                                session_id: *session_id,
                            }));
                        }
                        BufferLocationAttachment::Detached
                        | BufferLocationAttachment::Session { .. } => {}
                    }
                    events.append(&mut reveal_events);
                    match BufferWithLocationResponse::new(request_id, buffer, location, at_root_tab)
                    {
                        Ok(response) => (ServerResponse::BufferWithLocation(response), events),
                        Err(error) => (
                            mux_error_response(Some(request_id), MuxError::protocol(error)),
                            Vec::new(),
                        ),
                    }
                }
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::StartPipe {
                request_id,
                buffer_id,
                command,
                cwd,
                env,
            } => match self.start_buffer_pipe(buffer_id, command, cwd, env).await {
                Ok((buffer, events)) => (
                    ServerResponse::Buffer(BufferResponse { request_id, buffer }),
                    events,
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::StopPipe {
                request_id,
                buffer_id,
            } => match self.stop_buffer_pipe(buffer_id).await {
                Ok((buffer, events)) => (
                    ServerResponse::Buffer(BufferResponse { request_id, buffer }),
                    events,
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            BufferRequest::SetUserOption {
                request_id,
                buffer_id,
                key,
                value,
            } => match self.set_buffer_user_option(buffer_id, key, value).await {
                Ok((buffer, events)) => (
                    ServerResponse::Buffer(BufferResponse { request_id, buffer }),
                    events,
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
        }
    }

    async fn dispatch_input(
        self: &Arc<Self>,
        request: InputRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        match request {
            InputRequest::Send {
                request_id,
                buffer_id,
                bytes,
            } => {
                if let Err(error) = self.ensure_buffer_accepts_input(buffer_id).await {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                match self.buffer_runtime(buffer_id).await {
                    Ok(runtime) => match runtime.write(bytes).await {
                        Ok(()) => (ServerResponse::Ok(OkResponse { request_id }), Vec::new()),
                        Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                    },
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            InputRequest::Resize {
                request_id,
                buffer_id,
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                // The pixel size comes with the cells: a client that does not
                // know its terminal's cell size sends 0, which the PTY reports
                // as unknown rather than a stale size for other cells.
                let size = PtySize {
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                };
                let is_helper = {
                    let state = self.state.lock().await;
                    match state.buffer(buffer_id) {
                        Ok(buffer) => matches!(&buffer.kind, BufferKind::Helper(_)),
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    }
                };

                if !is_helper {
                    let runtime = match self.buffer_runtime(buffer_id).await {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    };
                    if let Err(error) = runtime.resize(size).await {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                }

                // The view showing the buffer records the size too, so its node
                // record tells API users (popups sized from the session) the
                // size it is drawn at.
                let resized_session = {
                    let mut state = self.state.lock().await;
                    match state
                        .set_buffer_size(buffer_id, size)
                        .and_then(|()| state.sync_buffer_view_size(buffer_id))
                    {
                        Ok(session_id) => session_id,
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    }
                };

                let mut events = vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                    buffer_id,
                })];
                if let Some(session_id) = resized_session {
                    self.mark_workspace_metadata_dirty();
                    events.push(ServerEvent::NodeChanged(NodeChangedEvent { session_id }));
                }
                (ServerResponse::Ok(OkResponse { request_id }), events)
            }
        }
    }

    async fn dispatch_node(
        &self,
        request: embers_protocol::NodeRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        let mut state = self.state.lock().await;

        match request {
            embers_protocol::NodeRequest::GetTree {
                request_id,
                session_id,
            } => match session_snapshot(&state, session_id) {
                Ok(snapshot) => (
                    ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                        request_id,
                        snapshot,
                    }),
                    Vec::new(),
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            embers_protocol::NodeRequest::Split {
                request_id,
                leaf_node_id,
                direction,
                new_buffer_id,
            } => {
                let session_id = match state.node(leaf_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) =
                    state.split_leaf_with_new_buffer(leaf_node_id, direction, new_buffer_id)
                {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }

                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let mut events =
                            vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::CreateSplit {
                request_id,
                session_id,
                direction,
                child_node_ids,
                sizes,
            } => match state.create_split_node(session_id, direction, child_node_ids) {
                Ok(split_id) => {
                    if !sizes.is_empty()
                        && let Err(error) = state.resize_split_children(split_id, sizes)
                    {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                    layout_snapshot_response(&state, request_id, session_id)
                }
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            embers_protocol::NodeRequest::CreateTabs {
                request_id,
                session_id,
                child_node_ids,
                titles,
                active,
            } => {
                if child_node_ids.len() != titles.len() {
                    return (
                        mux_error_response(
                            Some(request_id),
                            MuxError::invalid_input(
                                "create-tabs requires the same number of titles and child ids",
                            ),
                        ),
                        Vec::new(),
                    );
                }
                let tabs = titles
                    .into_iter()
                    .zip(child_node_ids)
                    .map(|(title, child)| TabEntry::new(title, child))
                    .collect();
                match protocol_tab_index(active)
                    .and_then(|active| state.create_tabs_node(session_id, tabs, active))
                {
                    Ok(_) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::ReplaceNode {
                request_id,
                node_id,
                child_node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.replace_node(node_id, child_node_id) {
                    Ok(()) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::WrapInSplit {
                request_id,
                node_id,
                child_node_id,
                direction,
                insert_before,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.wrap_node_in_split(node_id, direction, child_node_id, insert_before) {
                    Ok(_) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::WrapInTabs {
                request_id,
                node_id,
                title,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.wrap_node_in_tabs(node_id, title) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                layout_snapshot_response(&state, request_id, session_id)
            }
            embers_protocol::NodeRequest::AddTab {
                request_id,
                tabs_node_id,
                title,
                buffer_id,
                child_node_id,
                index,
            } => {
                let session_id = match state.node(tabs_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                let result =
                    protocol_tab_index(index).and_then(|index| match (buffer_id, child_node_id) {
                        (Some(buffer_id), None) => {
                            state.add_tab_from_buffer_at(tabs_node_id, index, title, buffer_id)
                        }
                        (None, Some(child_node_id)) => {
                            state.add_tab_sibling_at(tabs_node_id, index, title, child_node_id)
                        }
                        (Some(_), Some(_)) => Err(MuxError::invalid_input(
                            "add-tab requires either buffer_id or child_node_id, not both",
                        )),
                        (None, None) => Err(MuxError::invalid_input(
                            "add-tab requires either buffer_id or child_node_id",
                        )),
                    });
                match result {
                    Ok(_) => layout_snapshot_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::SelectTab {
                request_id,
                tabs_node_id,
                index,
            } => {
                let session_id = match state.node(tabs_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                let index = match protocol_tab_index(index) {
                    Ok(index) => index,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.switch_tab(tabs_node_id, index) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let mut events =
                            vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::Focus {
                request_id,
                session_id,
                node_id,
            } => {
                let target_leaf = match state.node(node_id) {
                    Ok(crate::Node::BufferView(_)) => Some(node_id),
                    Ok(_) => state
                        .resolve_visible_leaf(node_id)
                        .or_else(|_| state.resolve_first_leaf(node_id))
                        .ok()
                        .flatten(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };

                let Some(target_leaf) = target_leaf else {
                    return (
                        error_response(
                            Some(request_id),
                            ErrorCode::InvalidRequest,
                            format!("node {node_id} has no focusable leaf"),
                        ),
                        Vec::new(),
                    );
                };

                if let Err(error) = state.focus_leaf(session_id, target_leaf) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }

                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let mut events =
                            vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::SelectPane {
                request_id,
                session_id,
                node_id,
            } => match state
                .select_pane(session_id, node_id)
                .and_then(|()| layout_changed_events(&state, session_id))
                .and_then(|events| Ok((session_snapshot(&state, session_id)?, events)))
            {
                Ok((snapshot, events)) => (
                    ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                        request_id,
                        snapshot,
                    }),
                    events,
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            embers_protocol::NodeRequest::FocusLast {
                request_id,
                session_id,
            } => {
                if let Err(error) = state.focus_last_pane(session_id) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }

                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let mut events =
                            vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::Close {
                request_id,
                node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.close_node(node_id) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                match session_snapshot(&state, session_id) {
                    Ok(snapshot) => {
                        let mut events =
                            vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                                request_id,
                                snapshot,
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::Resize {
                request_id,
                node_id,
                sizes,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.resize_split_children(node_id, sizes) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                layout_snapshot_response(&state, request_id, session_id)
            }
            embers_protocol::NodeRequest::MoveBufferToNode {
                request_id,
                buffer_id,
                target_leaf_node_id,
            } => {
                let session_id = match state.node(target_leaf_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.move_buffer_to_leaf(buffer_id, target_leaf_node_id) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                layout_snapshot_response(&state, request_id, session_id)
            }
            embers_protocol::NodeRequest::Zoom {
                request_id,
                node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.zoom_node(node_id) {
                    Ok(()) => layout_ok_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::Unzoom {
                request_id,
                session_id,
            } => match state.unzoom_session(session_id) {
                Ok(()) => layout_ok_response(&state, request_id, session_id),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            embers_protocol::NodeRequest::ToggleZoom {
                request_id,
                node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.toggle_zoom_node(node_id) {
                    Ok(()) => layout_ok_response(&state, request_id, session_id),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::SwapSiblings {
                request_id,
                first_node_id,
                second_node_id,
            } => {
                let session_id = match state.node(first_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.swap_sibling_nodes(first_node_id, second_node_id) {
                    Ok(()) => {
                        let current_floating_id = match state.floating_id_for_node(first_node_id) {
                            Ok(floating_id) => floating_id,
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        };
                        let (response, mut events) =
                            layout_ok_response(&state, request_id, session_id);
                        if current_floating_id.is_some() {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id,
                                floating_id: current_floating_id,
                            }));
                        }
                        (response, events)
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::BreakNode {
                request_id,
                node_id,
                destination,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                let into_floating =
                    matches!(destination, embers_protocol::NodeBreakDestination::Floating);
                let previous_floating_id = match state.floating_id_for_node(node_id) {
                    Ok(floating_id) => floating_id,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.break_node(node_id, into_floating) {
                    Ok(()) => {
                        let current_floating_id = match state.floating_id_for_node(node_id) {
                            Ok(floating_id) => floating_id,
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        };
                        let (response, mut events) =
                            layout_ok_response(&state, request_id, session_id);
                        if previous_floating_id != current_floating_id
                            || current_floating_id.is_some()
                        {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id,
                                floating_id: current_floating_id,
                            }));
                        }
                        (response, events)
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::JoinBufferAtNode {
                request_id,
                node_id,
                buffer_id,
                placement,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.join_buffer_at_node(node_id, buffer_id, placement) {
                    Ok(()) => {
                        let current_floating_id = match state.floating_id_for_node(node_id) {
                            Ok(floating_id) => floating_id,
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        };
                        let (response, mut events) =
                            layout_ok_response(&state, request_id, session_id);
                        if current_floating_id.is_some() {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id,
                                floating_id: current_floating_id,
                            }));
                        }
                        (response, events)
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::MoveNodeBefore {
                request_id,
                node_id,
                sibling_node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.move_node_before(node_id, sibling_node_id) {
                    Ok(()) => {
                        let current_floating_id = match state.floating_id_for_node(node_id) {
                            Ok(floating_id) => floating_id,
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        };
                        let (response, mut events) =
                            layout_ok_response(&state, request_id, session_id);
                        if current_floating_id.is_some() {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id,
                                floating_id: current_floating_id,
                            }));
                        }
                        (response, events)
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
            embers_protocol::NodeRequest::MoveNodeAfter {
                request_id,
                node_id,
                sibling_node_id,
            } => {
                let session_id = match state.node(node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                match state.move_node_after(node_id, sibling_node_id) {
                    Ok(()) => {
                        let current_floating_id = match state.floating_id_for_node(node_id) {
                            Ok(floating_id) => floating_id,
                            Err(error) => {
                                return (mux_error_response(Some(request_id), error), Vec::new());
                            }
                        };
                        let (response, mut events) =
                            layout_ok_response(&state, request_id, session_id);
                        if current_floating_id.is_some() {
                            events.push(ServerEvent::FloatingChanged(FloatingChangedEvent {
                                session_id,
                                floating_id: current_floating_id,
                            }));
                        }
                        (response, events)
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                }
            }
        }
    }

    async fn dispatch_floating(
        &self,
        request: FloatingRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        let mut state = self.state.lock().await;

        match request {
            FloatingRequest::Create {
                request_id,
                session_id,
                root_node_id,
                buffer_id,
                geometry,
                title,
                focus,
                close_on_empty,
                border,
            } => match match (root_node_id, buffer_id) {
                (Some(root_node_id), None) => state.create_floating_window_with_options(
                    session_id,
                    root_node_id,
                    geometry,
                    title,
                    focus,
                    close_on_empty,
                ),
                (None, Some(buffer_id)) => state.create_floating_from_buffer_with_options(
                    session_id,
                    buffer_id,
                    geometry,
                    title,
                    focus,
                    close_on_empty,
                ),
                (Some(_), Some(_)) => Err(MuxError::invalid_input(
                    "create-floating requires either root_node_id or buffer_id, not both",
                )),
                (None, None) => Err(MuxError::invalid_input(
                    "create-floating requires either root_node_id or buffer_id",
                )),
            }
            .and_then(|floating_id| {
                state.set_floating_border(floating_id, border)?;
                Ok(floating_id)
            }) {
                Ok(floating_id) => match state.floating_window(floating_id) {
                    Ok(floating) => {
                        let mut events = vec![ServerEvent::FloatingChanged(FloatingChangedEvent {
                            session_id,
                            floating_id: Some(floating_id),
                        })];
                        if let Some(focus_event) = focus_changed_event(&state, session_id) {
                            events.push(ServerEvent::FocusChanged(focus_event));
                        }
                        (
                            ServerResponse::Floating(FloatingResponse {
                                request_id,
                                floating: floating_record(floating),
                            }),
                            events,
                        )
                    }
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                },
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            FloatingRequest::Close {
                request_id,
                floating_id,
            } => {
                let session_id = match state.floating_window(floating_id) {
                    Ok(floating) => floating.session_id,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.close_floating(floating_id) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                let mut events = vec![ServerEvent::FloatingChanged(FloatingChangedEvent {
                    session_id,
                    floating_id: Some(floating_id),
                })];
                if let Some(focus_event) = focus_changed_event(&state, session_id) {
                    events.push(ServerEvent::FocusChanged(focus_event));
                }
                (ServerResponse::Ok(OkResponse { request_id }), events)
            }
            FloatingRequest::Move {
                request_id,
                floating_id,
                geometry,
            } => match state.move_floating(floating_id, geometry) {
                Ok(()) => {
                    let floating = match state.floating_window(floating_id) {
                        Ok(floating) => floating,
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    };
                    (
                        ServerResponse::Floating(FloatingResponse {
                            request_id,
                            floating: floating_record(floating),
                        }),
                        vec![ServerEvent::FloatingChanged(FloatingChangedEvent {
                            session_id: floating.session_id,
                            floating_id: Some(floating_id),
                        })],
                    )
                }
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            FloatingRequest::Focus {
                request_id,
                floating_id,
            } => {
                let session_id = match state.floating_window(floating_id) {
                    Ok(floating) => floating.session_id,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.focus_floating(floating_id) {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }
                let floating = match state.floating_window(floating_id) {
                    Ok(floating) => floating,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                let mut events = Vec::new();
                if let Some(focus_event) = focus_changed_event(&state, session_id) {
                    events.push(ServerEvent::FocusChanged(focus_event));
                }
                (
                    ServerResponse::Floating(FloatingResponse {
                        request_id,
                        floating: floating_record(floating),
                    }),
                    events,
                )
            }
        }
    }

    async fn spawn_buffer_runtime(self: &Arc<Self>, buffer_id: BufferId) -> Result<()> {
        let (command, cwd) = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?;
            (buffer.command.clone(), buffer.cwd.clone())
        };
        self.spawn_buffer_runtime_with(buffer_id, command, cwd)
            .await
    }

    /// Start a runtime for `buffer_id` running `command` in `cwd`: a keeper,
    /// or a buffer on the host the record names. The buffer record takes them
    /// as its command and directory only once the runtime runs, so a spawn
    /// that fails leaves the record as it was.
    async fn spawn_buffer_runtime_with(
        self: &Arc<Self>,
        buffer_id: BufferId,
        command: Vec<String>,
        cwd: Option<PathBuf>,
    ) -> Result<()> {
        let (size, env_hints, title, host) = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?;
            (
                buffer.pty_size,
                buffer.env.clone(),
                buffer.title.clone(),
                buffer.placement().host().map(str::to_owned),
            )
        };

        let started = self
            .runtimes()
            .start(
                buffer_id,
                StartRequest {
                    command: &command,
                    cwd: cwd.as_deref(),
                    env: &env_hints,
                    size,
                    title: &title,
                    host: host.as_deref(),
                },
            )
            .await;
        // A runtime that started and was stopped again waits for a save.
        let Started { placement, status } = started.inspect_err(|_| self.mark_workspace_dirty())?;

        {
            let mut state = self.state.lock().await;
            // The status poller starts with the runtime and may already have
            // recorded the exit of a command that ended at once. The buffer was
            // still created, so keep it as exited instead of failing creation.
            let already_exited = state
                .buffer(buffer_id)
                .is_ok_and(|buffer| matches!(buffer.state, BufferState::Exited(_)));
            if !already_exited && let Err(error) = state.mark_buffer_running(buffer_id, status.pid)
            {
                drop(state);
                // The caller drops the buffer record on error, so stop the
                // keeper too rather than leave it running with nothing
                // referring to it.
                self.buffer_runtimes.discard(buffer_id).await;
                self.mark_workspace_dirty();
                return Err(error);
            }
            state.set_buffer_placement(buffer_id, placement)?;
            if let Some(buffer) = state.buffers.get_mut(&buffer_id) {
                buffer.command = command;
                buffer.cwd = cwd;
            }
            apply_runtime_status(&mut state, buffer_id, &status);
        }
        Ok(())
    }

    async fn buffer_runtime(&self, buffer_id: BufferId) -> Result<BufferRuntimeHandle> {
        if let Some(runtime) = self.buffer_runtimes.get(buffer_id) {
            return Ok(runtime);
        }

        let state = self.state.lock().await;
        let buffer = state.buffer(buffer_id)?;
        match buffer.state {
            BufferState::Created => Err(MuxError::conflict(format!(
                "buffer {buffer_id} is not running"
            ))),
            BufferState::Running(_) => Err(MuxError::internal(format!(
                "buffer {buffer_id} is marked running without an active runtime"
            ))),
            BufferState::Interrupted(_) => Err(MuxError::conflict(format!(
                "buffer {buffer_id} was restored without a running runtime"
            ))),
            BufferState::Exited(_) => Err(MuxError::conflict(format!(
                "buffer {buffer_id} has already exited"
            ))),
        }
    }

    /// End `buffer_id`'s program. A buffer left `Interrupted` (its keeper is
    /// gone and restore is off or failed) has no program to end, so killing it
    /// ends the buffer itself: a detached one is removed, and an attached one
    /// is marked exited, which the exit cleanup then handles like any other
    /// (closing its view unless it remains on exit). Without this, killing
    /// its view would leave an orphaned record that a later restore brings
    /// back.
    async fn kill_buffer(&self, buffer_id: BufferId) -> Result<()> {
        if let Some(runtime) = self.buffer_runtimes.get(buffer_id) {
            return runtime.kill().await;
        }
        {
            let mut state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?;
            if matches!(buffer.state, BufferState::Interrupted(_)) {
                if matches!(buffer.attachment, BufferAttachment::Detached) {
                    state.remove_buffer(buffer_id)?;
                } else {
                    state.mark_buffer_exited(buffer_id, None)?;
                }
                drop(state);
                info!(%buffer_id, "removed killed buffer whose keeper was gone");
                self.mark_workspace_dirty();
                return Ok(());
            }
        }
        self.buffer_runtime(buffer_id).await?.kill().await
    }

    async fn ensure_buffer_accepts_input(&self, buffer_id: BufferId) -> Result<()> {
        let state = self.state.lock().await;
        let buffer = state.buffer(buffer_id)?;
        if matches!(&buffer.kind, BufferKind::Helper(_)) {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} is read-only"
            )));
        }
        Ok(())
    }

    async fn ensure_buffer_accepts_pipe(&self, buffer_id: BufferId) -> Result<()> {
        let state = self.state.lock().await;
        let buffer = state.buffer(buffer_id)?;
        if matches!(&buffer.kind, BufferKind::Helper(_)) {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} is read-only"
            )));
        }
        if matches!(buffer.state, BufferState::Exited(_)) {
            return Err(MuxError::conflict(format!(
                "buffer {buffer_id} has already exited"
            )));
        }
        Ok(())
    }

    async fn start_buffer_pipe(
        &self,
        buffer_id: BufferId,
        command: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
    ) -> Result<(embers_protocol::BufferRecord, Vec<ServerEvent>)> {
        if command.is_empty() {
            return Err(MuxError::invalid_input(
                "buffer pipe command must not be empty",
            ));
        }
        self.ensure_buffer_accepts_pipe(buffer_id).await?;
        let runtime = self.buffer_runtime(buffer_id).await?;
        let pipe = runtime
            .start_pipe(command, cwd.map(Into::into), env)
            .await?;
        let (buffer, session_id) = {
            let mut state = self.state.lock().await;
            let attachment = state.buffer(buffer_id)?.attachment.clone();
            {
                let Some(record) = state.buffers.get_mut(&buffer_id) else {
                    return Err(MuxError::not_found(format!(
                        "buffer {buffer_id} was not found"
                    )));
                };
                record.pipe = Some(model_pipe_from_runtime(&pipe));
            }
            let session_id = match attachment {
                BufferAttachment::Attached(node_id) => Some(state.node(node_id)?.session_id()),
                BufferAttachment::Detached => None,
            };
            let buffer = buffer_record(state.buffer(buffer_id)?);
            (buffer, session_id)
        };
        Ok((
            buffer.clone(),
            vec![ServerEvent::BufferPipeChanged(BufferPipeChangedEvent {
                session_id,
                buffer,
            })],
        ))
    }

    async fn stop_buffer_pipe(
        &self,
        buffer_id: BufferId,
    ) -> Result<(embers_protocol::BufferRecord, Vec<ServerEvent>)> {
        self.ensure_buffer_accepts_pipe(buffer_id).await?;
        let runtime = self.buffer_runtime(buffer_id).await?;
        let pipe = runtime.stop_pipe().await?;
        let (buffer, session_id) = {
            let mut state = self.state.lock().await;
            let attachment = state.buffer(buffer_id)?.attachment.clone();
            {
                let Some(record) = state.buffers.get_mut(&buffer_id) else {
                    return Err(MuxError::not_found(format!(
                        "buffer {buffer_id} was not found"
                    )));
                };
                record.pipe = Some(model_pipe_from_runtime(&pipe));
            }
            let session_id = match attachment {
                BufferAttachment::Attached(node_id) => Some(state.node(node_id)?.session_id()),
                BufferAttachment::Detached => None,
            };
            let buffer = buffer_record(state.buffer(buffer_id)?);
            (buffer, session_id)
        };
        Ok((
            buffer.clone(),
            vec![ServerEvent::BufferPipeChanged(BufferPipeChangedEvent {
                session_id,
                buffer,
            })],
        ))
    }

    async fn set_buffer_user_option(
        &self,
        buffer_id: BufferId,
        key: String,
        value: Option<String>,
    ) -> Result<(embers_protocol::BufferRecord, Vec<ServerEvent>)> {
        let (buffer, changed) = {
            let mut state = self.state.lock().await;
            let Some(record) = state.buffers.get_mut(&buffer_id) else {
                return Err(MuxError::not_found(format!(
                    "buffer {buffer_id} was not found"
                )));
            };
            let changed = match value {
                Some(value) => {
                    if key.len() > MAX_USER_OPTION_KEY_LEN {
                        return Err(MuxError::invalid_input(format!(
                            "user option key exceeds {MAX_USER_OPTION_KEY_LEN} bytes"
                        )));
                    }
                    if value.len() > MAX_USER_OPTION_VALUE_LEN {
                        return Err(MuxError::invalid_input(format!(
                            "user option value exceeds {MAX_USER_OPTION_VALUE_LEN} bytes"
                        )));
                    }
                    // Setting a new key is capped; updating an existing key (or
                    // removing, below) is always allowed so clients can't get
                    // wedged once at the limit.
                    if !record.user_options.contains_key(&key)
                        && record.user_options.len() >= MAX_USER_OPTIONS_PER_BUFFER
                    {
                        return Err(MuxError::invalid_input(format!(
                            "buffer already has the maximum of {MAX_USER_OPTIONS_PER_BUFFER} user options"
                        )));
                    }
                    // Setting a key to the value it already holds changes nothing.
                    if record
                        .user_options
                        .get(&key)
                        .is_some_and(|existing| *existing == value)
                    {
                        false
                    } else {
                        record.user_options.insert(key, value);
                        true
                    }
                }
                // `remove` returns the old value, so `is_some()` is true only when
                // an entry actually existed to remove.
                None => record.user_options.remove(&key).is_some(),
            };
            (buffer_record(state.buffer(buffer_id)?), changed)
        };
        // A no-op set/unset leaves the record untouched, so don't invalidate
        // renders for it; only broadcast when the options actually changed.
        // The only event is a render invalidation, which does not save the
        // workspace by itself, so ask for that here.
        let events = if changed {
            self.mark_workspace_dirty();
            vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                buffer_id,
            })]
        } else {
            Vec::new()
        };
        Ok((buffer, events))
    }

    async fn resolve_reveal_client_id(
        &self,
        connection_id: u64,
        requested: Option<u64>,
    ) -> Result<u64> {
        if let Some(client_id) = requested {
            if self.clients.lock().await.contains_key(&client_id) {
                return Ok(client_id);
            }
            return Err(MuxError::not_found(format!("unknown client {client_id}")));
        }

        let clients = self.clients.lock().await;
        if clients
            .get(&connection_id)
            .is_some_and(|client| client.current_session_id.is_some())
        {
            return Ok(connection_id);
        }

        let mut attached_clients = clients
            .iter()
            .filter_map(|(client_id, client)| client.current_session_id.map(|_| *client_id));
        match (attached_clients.next(), attached_clients.next()) {
            (Some(client_id), None) => Ok(client_id),
            (None, _) => Err(MuxError::conflict(
                "no interactive client is currently attached",
            )),
            (Some(_), Some(_)) => Err(MuxError::conflict(
                "multiple interactive clients are attached; specify a client",
            )),
        }
    }

    async fn has_attached_client(&self) -> bool {
        self.clients
            .lock()
            .await
            .values()
            .any(|client| client.current_session_id.is_some())
    }

    async fn resolve_optional_reveal_client_id(
        &self,
        connection_id: u64,
        requested_client_id: Option<u64>,
    ) -> Result<Option<u64>> {
        if requested_client_id.is_some() || self.has_attached_client().await {
            self.resolve_reveal_client_id(connection_id, requested_client_id)
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    }

    async fn focus_revealed_buffer(
        &self,
        buffer_id: BufferId,
        client_id: Option<u64>,
    ) -> Result<(embers_protocol::BufferLocation, Vec<ServerEvent>)> {
        let (location, mut events, client_event) = {
            let mut state = self.state.lock().await;
            let location = buffer_location(&state, buffer_id)?;
            let (session_id, node_id) = match location.attachment {
                BufferLocationAttachment::Detached => {
                    return Ok((location, Vec::new()));
                }
                BufferLocationAttachment::Session {
                    session_id,
                    node_id,
                }
                | BufferLocationAttachment::Floating {
                    session_id,
                    node_id,
                    ..
                } => (session_id, node_id),
            };
            if let Some(client_id) = client_id {
                let mut clients = self.clients.lock().await;
                let previous_session_id = clients
                    .get(&client_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!("client {client_id} was not found"))
                    })?
                    .current_session_id;
                state.focus_leaf(session_id, node_id)?;
                let mut events = vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                if let Some(focus_event) = focus_changed_event(&state, session_id) {
                    events.push(ServerEvent::FocusChanged(focus_event));
                }
                clients
                    .get_mut(&client_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!("client {client_id} was not found"))
                    })?
                    .current_session_id = Some(session_id);
                let subscriptions = self.subscriptions.lock().await;
                let mut subscribed_all_sessions = false;
                let mut subscribed_session_ids = Vec::new();
                for subscription in subscriptions.values() {
                    if subscription.connection_id != client_id {
                        continue;
                    }
                    match subscription.session_id {
                        Some(session_id) => subscribed_session_ids.push(session_id),
                        None => subscribed_all_sessions = true,
                    }
                }
                subscribed_session_ids.sort_by_key(|session_id| session_id.0);
                subscribed_session_ids.dedup();
                let client_event = ServerEvent::ClientChanged(ClientChangedEvent {
                    client: ClientRecord {
                        id: client_id,
                        current_session_id: Some(session_id),
                        subscribed_all_sessions,
                        subscribed_session_ids,
                    },
                    previous_session_id,
                });
                let location = buffer_location(&state, buffer_id)?;
                (location, events, Some(client_event))
            } else {
                state.focus_leaf(session_id, node_id)?;
                let mut events = vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
                if let Some(focus_event) = focus_changed_event(&state, session_id) {
                    events.push(ServerEvent::FocusChanged(focus_event));
                }
                let location = buffer_location(&state, buffer_id)?;
                (location, events, None)
            }
        };
        if let Some(client_event) = client_event {
            events.push(client_event);
        }
        Ok((location, events))
    }

    async fn reveal_buffer(
        &self,
        connection_id: u64,
        requested_client_id: Option<u64>,
        buffer_id: BufferId,
    ) -> Result<(embers_protocol::BufferLocation, Vec<ServerEvent>)> {
        let target_client_id = self
            .resolve_optional_reveal_client_id(connection_id, requested_client_id)
            .await?;
        let location = {
            let state = self.state.lock().await;
            buffer_location(&state, buffer_id)?
        };
        if matches!(location.attachment, BufferLocationAttachment::Detached) {
            return Ok((location, Vec::new()));
        }
        self.focus_revealed_buffer(buffer_id, target_client_id)
            .await
    }

    async fn open_history_buffer(
        &self,
        connection_id: u64,
        requested_client_id: Option<u64>,
        source_buffer_id: BufferId,
        scope: BufferHistoryScope,
        placement: BufferHistoryPlacement,
    ) -> Result<(embers_protocol::BufferLocation, Vec<ServerEvent>)> {
        let (source_title, source_cwd, source_location, helper_lines) = {
            let state = self.state.lock().await;
            let buffer = state.buffer(source_buffer_id)?;
            let location = buffer_location(&state, source_buffer_id)?;
            let helper_lines = match &buffer.kind {
                BufferKind::Helper(helper) => Some(match scope {
                    BufferHistoryScope::Full => helper.lines.clone(),
                    BufferHistoryScope::Visible => {
                        let visible_limit = usize::from(buffer.pty_size.rows).max(1);
                        let start = helper.lines.len().saturating_sub(visible_limit);
                        helper.lines[start..].to_vec()
                    }
                }),
                BufferKind::Pty => None,
            };
            (
                buffer.title.clone(),
                buffer.cwd.clone(),
                location,
                helper_lines,
            )
        };
        let target_client_id = self
            .resolve_optional_reveal_client_id(connection_id, requested_client_id)
            .await?;

        let target_session_id = match source_location.attachment {
            BufferLocationAttachment::Session { session_id, .. }
            | BufferLocationAttachment::Floating { session_id, .. } => session_id,
            BufferLocationAttachment::Detached => {
                let client_id = target_client_id.ok_or_else(|| {
                    MuxError::conflict(
                        "history for detached buffers requires an attached client session",
                    )
                })?;
                let clients = self.clients.lock().await;
                clients
                    .get(&client_id)
                    .and_then(|client| client.current_session_id)
                    .ok_or_else(|| {
                        MuxError::conflict(
                            "history for detached buffers requires an attached client session",
                        )
                    })?
            }
        };

        let lines = match helper_lines {
            Some(lines) => lines,
            None => match scope {
                BufferHistoryScope::Full => {
                    self.capture_snapshot(RequestId(0), source_buffer_id)
                        .await?
                        .lines
                }
                BufferHistoryScope::Visible => {
                    // Helper buffers store plain text; project the styled visible
                    // snapshot down to text at this boundary.
                    self.capture_visible_snapshot(RequestId(0), source_buffer_id)
                        .await?
                        .lines
                        .into_iter()
                        .map(|line| line.text)
                        .collect()
                }
            },
        };

        let helper_scope = match scope {
            BufferHistoryScope::Full => HelperBufferScope::Full,
            BufferHistoryScope::Visible => HelperBufferScope::Visible,
        };
        let helper_title = format!("{} history", source_title);
        let helper_buffer_id = {
            let mut state = self.state.lock().await;
            if state.buffers.len() >= self.limits.max_buffers {
                return Err(MuxError::conflict(limit_reached_message(
                    "buffer",
                    self.limits.max_buffers,
                    crate::config::MAX_BUFFERS_ENV_VAR,
                )));
            }
            let helper_buffer_id = state.create_helper_buffer(
                helper_title.clone(),
                source_buffer_id,
                helper_scope,
                source_cwd,
                lines,
            )?;
            let placement_result = match placement {
                BufferHistoryPlacement::Tab => {
                    state.add_root_tab_from_buffer(
                        target_session_id,
                        helper_title,
                        helper_buffer_id,
                    )?;
                    Ok(())
                }
                BufferHistoryPlacement::Floating => {
                    let geometry = state.default_floating_geometry(target_session_id)?;
                    state.create_floating_from_buffer(
                        target_session_id,
                        helper_buffer_id,
                        geometry,
                        Some(helper_title),
                    )?;
                    Ok(())
                }
            };
            if let Err(error) = placement_result {
                let _ = state.remove_buffer(helper_buffer_id);
                return Err(error);
            }
            helper_buffer_id
        };

        match self
            .focus_revealed_buffer(helper_buffer_id, target_client_id)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let mut state = self.state.lock().await;
                let rollback_result = (|| -> Result<()> {
                    let helper_location = buffer_location(&state, helper_buffer_id)?;
                    match helper_location.attachment {
                        BufferLocationAttachment::Detached => {}
                        BufferLocationAttachment::Session { node_id, .. } => {
                            state.close_node(node_id)?;
                        }
                        BufferLocationAttachment::Floating { floating_id, .. } => {
                            state.close_floating(floating_id)?;
                        }
                    }
                    state.remove_buffer(helper_buffer_id)?;
                    Ok(())
                })();
                match rollback_result {
                    Ok(()) => Err(error),
                    Err(rollback_error) => {
                        debug!(
                            %helper_buffer_id,
                            %error,
                            %rollback_error,
                            "failed to roll back helper buffer after focus failure"
                        );
                        Err(MuxError::internal(format!(
                            "{error}; rollback also failed: {rollback_error}"
                        )))
                    }
                }
            }
        }
    }

    async fn capture_snapshot(
        &self,
        request_id: RequestId,
        buffer_id: BufferId,
    ) -> Result<SnapshotResponse> {
        enum SnapshotSource {
            Helper {
                sequence: u64,
                size: PtySize,
                lines: Vec<String>,
                title: String,
                cwd: Option<String>,
            },
            Runtime {
                title: String,
                cwd: Option<PathBuf>,
            },
        }

        let source = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?;
            match &buffer.kind {
                BufferKind::Helper(helper) => SnapshotSource::Helper {
                    sequence: buffer.last_snapshot_seq,
                    size: buffer.pty_size,
                    lines: helper.lines.clone(),
                    title: buffer.title.clone(),
                    cwd: buffer.cwd.as_ref().map(|path| path.display().to_string()),
                },
                _ => SnapshotSource::Runtime {
                    title: buffer.title.clone(),
                    cwd: buffer.cwd.clone(),
                },
            }
        };
        let (buffer_title, buffer_cwd) = match source {
            SnapshotSource::Helper {
                sequence,
                size,
                lines,
                title,
                cwd,
            } => {
                return Ok(SnapshotResponse {
                    request_id,
                    buffer_id,
                    sequence,
                    size,
                    lines,
                    title: Some(title),
                    cwd,
                });
            }
            SnapshotSource::Runtime { title, cwd } => (title, cwd),
        };
        let runtime = self.buffer_runtime(buffer_id).await?;
        let snapshot = runtime.capture_snapshot(buffer_cwd.clone()).await?;
        self.sync_buffer_runtime_status(buffer_id, &runtime).await?;
        // Re-read cwd after the sync: the runtime may have reported a new working
        // directory (e.g. via OSC 7) that `sync_buffer_runtime_status` just wrote
        // into the buffer record. `buffer_cwd` captured before the sync is stale.
        let response_cwd = {
            let state = self.state.lock().await;
            match state.buffers.get(&buffer_id) {
                // Preserve a deliberately-cleared cwd (`None`) on an existing
                // buffer; only fall back to the captured value when the buffer is
                // gone entirely.
                Some(buffer) => buffer.cwd.clone(),
                None => buffer_cwd,
            }
        };

        Ok(SnapshotResponse {
            request_id,
            buffer_id,
            sequence: snapshot.sequence,
            size: snapshot.size,
            lines: snapshot.lines,
            title: snapshot.title.or(Some(buffer_title)),
            cwd: response_cwd.map(|path| path.display().to_string()),
        })
    }

    async fn capture_visible_snapshot(
        &self,
        request_id: RequestId,
        buffer_id: BufferId,
    ) -> Result<VisibleSnapshotResponse> {
        enum VisibleSnapshotSource {
            Helper {
                sequence: u64,
                size: PtySize,
                lines: Vec<String>,
                title: String,
                cwd: Option<String>,
                viewport_top_line: u64,
                total_lines: u64,
            },
            Runtime {
                cwd: Option<PathBuf>,
            },
        }

        let source = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?;
            match &buffer.kind {
                BufferKind::Helper(helper) => {
                    let viewport_height = usize::from(buffer.pty_size.rows.max(1));
                    let viewport_start = helper.lines.len().saturating_sub(viewport_height);
                    VisibleSnapshotSource::Helper {
                        sequence: buffer.last_snapshot_seq,
                        size: buffer.pty_size,
                        lines: helper.lines.iter().skip(viewport_start).cloned().collect(),
                        title: buffer.title.clone(),
                        cwd: buffer.cwd.as_ref().map(|path| path.display().to_string()),
                        viewport_top_line: u64::try_from(viewport_start).unwrap_or(u64::MAX),
                        total_lines: u64::try_from(helper.lines.len()).unwrap_or(u64::MAX),
                    }
                }
                _ => VisibleSnapshotSource::Runtime {
                    cwd: buffer.cwd.clone(),
                },
            }
        };
        let buffer_cwd = match source {
            VisibleSnapshotSource::Helper {
                sequence,
                size,
                lines,
                title,
                cwd,
                viewport_top_line,
                total_lines,
            } => {
                return Ok(VisibleSnapshotResponse {
                    request_id,
                    buffer_id,
                    sequence,
                    size,
                    lines: lines.into_iter().map(SnapshotLine::plain).collect(),
                    title: Some(title),
                    cwd,
                    viewport_top_line,
                    total_lines,
                    alternate_screen: false,
                    mouse_reporting: false,
                    focus_reporting: false,
                    bracketed_paste: false,
                    keyboard_mode: 0,
                    cursor: None,
                });
            }
            VisibleSnapshotSource::Runtime { cwd } => cwd,
        };
        let runtime = self.buffer_runtime(buffer_id).await?;
        let snapshot = runtime.capture_visible_snapshot(buffer_cwd.clone()).await?;
        self.sync_buffer_runtime_status(buffer_id, &runtime).await?;
        // Re-read cwd after the sync: the runtime may have reported a new working
        // directory (e.g. via OSC 7) that `sync_buffer_runtime_status` just wrote
        // into the buffer record; the pre-sync `snapshot.cwd`/`buffer_cwd` is stale.
        let response_cwd = {
            let state = self.state.lock().await;
            match state.buffers.get(&buffer_id) {
                // Preserve a deliberately-cleared cwd (`None`) on an existing
                // buffer; only fall back to the captured value when the buffer is
                // gone entirely.
                Some(buffer) => buffer.cwd.clone(),
                None => buffer_cwd,
            }
        };

        Ok(VisibleSnapshotResponse {
            request_id,
            buffer_id,
            sequence: snapshot.sequence,
            size: snapshot.size,
            lines: snapshot.lines,
            title: snapshot.title,
            cwd: response_cwd.map(|path| path.display().to_string()),
            viewport_top_line: snapshot.viewport_top_line,
            total_lines: snapshot.total_lines,
            alternate_screen: snapshot.modes.alternate_screen,
            mouse_reporting: snapshot.modes.mouse_reporting,
            focus_reporting: snapshot.modes.focus_reporting,
            bracketed_paste: snapshot.modes.bracketed_paste,
            keyboard_mode: snapshot.modes.keyboard_mode,
            cursor: snapshot.cursor,
        })
    }

    async fn capture_scrollback_slice(
        &self,
        request_id: RequestId,
        buffer_id: BufferId,
        start_line: u64,
        line_count: u32,
    ) -> Result<ScrollbackSliceResponse> {
        let helper_slice = {
            let state = self.state.lock().await;
            match &state.buffer(buffer_id)?.kind {
                BufferKind::Helper(helper) => {
                    let total_lines = u64::try_from(helper.lines.len()).unwrap_or(u64::MAX);
                    let start = usize::try_from(start_line)
                        .unwrap_or(usize::MAX)
                        .min(helper.lines.len());
                    let end = start
                        .saturating_add(usize::try_from(line_count).unwrap_or(usize::MAX))
                        .min(helper.lines.len());
                    Some((
                        u64::try_from(start).unwrap_or(u64::MAX),
                        total_lines,
                        helper.lines[start..end].to_vec(),
                    ))
                }
                BufferKind::Pty => None,
            }
        };
        if let Some((start_line, total_lines, lines)) = helper_slice {
            return Ok(ScrollbackSliceResponse {
                request_id,
                buffer_id,
                start_line,
                total_lines,
                lines: lines.into_iter().map(SnapshotLine::plain).collect(),
            });
        }
        let runtime = self.buffer_runtime(buffer_id).await?;
        let slice = runtime
            .capture_scrollback_slice(start_line, line_count)
            .await?;
        self.sync_buffer_runtime_status(buffer_id, &runtime).await?;

        Ok(ScrollbackSliceResponse {
            request_id,
            buffer_id,
            start_line: slice.start_line,
            total_lines: slice.total_lines,
            lines: slice.into_snapshot_lines(),
        })
    }

    async fn record_buffer_update(&self, buffer_id: BufferId, update: BufferRuntimeUpdate) {
        let shown_sessions: BTreeSet<embers_core::SessionId> = self
            .clients
            .lock()
            .await
            .values()
            .filter_map(|client| client.current_session_id)
            .collect();
        let (render_invalidated, pipe_event, workspace_changed) = {
            let mut state = self.state.lock().await;
            // Like tmux, output and bells flag only a window no client is
            // looking at: the window a client shows is not flagged.
            let shown = state
                .session_showing_buffer(buffer_id)
                .is_some_and(|session_id| shown_sessions.contains(&session_id));
            let Some(buffer) = state.buffers.get_mut(&buffer_id) else {
                return;
            };
            let previous_pipe = buffer.pipe.clone();
            let mut render_invalidated = false;
            // Title, cwd and foreground command are what a restore brings back,
            // so a change to any of them is saved. Output and activity are not.
            let mut workspace_changed = false;
            {
                let sequence_advanced = update.sequence > buffer.last_snapshot_seq;
                let sequence_current = update.sequence >= buffer.last_snapshot_seq;
                // A bell counts even when its output's sequence was already
                // applied (a capture syncs the status too) and later output
                // replaced the runtime's activity before this update.
                if update.bell_sequence > buffer.runtime_bell_sequence {
                    buffer.runtime_bell_sequence = update.bell_sequence;
                    if !shown {
                        buffer.activity = embers_core::ActivityState::Bell;
                    }
                    render_invalidated = true;
                }
                if sequence_advanced {
                    buffer.last_snapshot_seq = update.sequence;
                    if !shown {
                        buffer.activity = max_activity(buffer.activity, update.activity);
                    }
                    if let Some(title) = update.title {
                        let next_title = title.unwrap_or_default();
                        if next_title != buffer.title {
                            buffer.title = next_title;
                            workspace_changed = true;
                        }
                    }
                    render_invalidated = true;
                }
                // The shell can change directory without advancing the snapshot
                // sequence, so accept same-sequence cwd updates; reject only
                // stale, lower-sequence ones (consistent with the pipe branch).
                // A `None` from the runtime means "cwd currently unresolved" (no
                // OSC 7 yet, pid lookup failed, unsupported platform) — not
                // "cleared" — so never let it wipe a directory we already know.
                if sequence_current
                    && let Some(cwd) = update.cwd
                    && cwd.is_some()
                    && cwd != buffer.cwd
                {
                    buffer.cwd = cwd;
                    render_invalidated = true;
                    workspace_changed = true;
                }
                if sequence_current
                    && let Some(foreground_command) = update.foreground_command
                    && foreground_command != buffer.foreground_command
                {
                    buffer.foreground_command = foreground_command;
                    // Clients read it to tell what runs in the pane (smart
                    // navigation detecting Neovim), so they refetch the record.
                    render_invalidated = true;
                    workspace_changed = true;
                }
                if sequence_current && let Some(pipe) = update.pipe {
                    let next_pipe = pipe.as_ref().map(model_pipe_from_runtime);
                    if next_pipe != buffer.pipe {
                        buffer.pipe = next_pipe;
                        render_invalidated = true;
                    }
                }
                // Options are saved, and clients read them (`is-vim` for
                // smart navigation), so a change is both.
                if sequence_current && let Some(options) = update.user_options {
                    for (key, value) in options {
                        let changed = match value {
                            Some(value) => {
                                buffer.user_options.insert(key, value.clone()) != Some(value)
                            }
                            None => buffer.user_options.remove(&key).is_some(),
                        };
                        if changed {
                            render_invalidated = true;
                            workspace_changed = true;
                        }
                    }
                }
            }
            let pipe_event = match state.buffer(buffer_id) {
                Ok(buffer) if buffer.pipe != previous_pipe => {
                    buffer_pipe_changed_event(&state, buffer_id).ok()
                }
                _ => None,
            };
            (render_invalidated, pipe_event, workspace_changed)
        };
        if workspace_changed {
            self.mark_workspace_metadata_dirty();
        }

        let mut events = Vec::new();
        if render_invalidated {
            events.push(ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                buffer_id,
            }));
        }
        if let Some(pipe_event) = pipe_event {
            events.push(ServerEvent::BufferPipeChanged(pipe_event));
        }
        if !events.is_empty() {
            self.broadcast(events, &[]).await;
        }
    }

    /// Send an OSC 52 clipboard write from `buffer_id` to every client whose
    /// current session contains the buffer, whether or not its pane is on
    /// screen, the way tmux sends it to every client attached to the pane's
    /// session. Each of those clients is a terminal the user may be looking at;
    /// clients on other sessions are not. A write from a detached buffer, or
    /// one whose session no client is showing, is dropped.
    async fn forward_clipboard(&self, buffer_id: BufferId, write: ClipboardWrite) {
        let Some(client_ids) = self.clients_showing_buffer(buffer_id).await else {
            debug!(%buffer_id, "dropping clipboard write: no client shows the buffer");
            return;
        };
        let event = ServerEnvelope::Event(ServerEvent::Clipboard(ClipboardEvent {
            buffer_id,
            selection: write.selection,
            text: write.text,
        }));
        self.send_to_clients(&client_ids, event).await;
    }

    /// Send passthrough writes from `buffer_id` to the same clients as a
    /// clipboard write. Each client decides from its own layout whether the
    /// pane is on screen (tmux `allow-passthrough on`) or forwards the writes
    /// anyway (`all`). The writes go out in order, in as few events as fit a
    /// protocol frame.
    async fn forward_passthrough(&self, buffer_id: BufferId, writes: Vec<PassthroughWrite>) {
        let Some(client_ids) = self.clients_showing_buffer(buffer_id).await else {
            debug!(%buffer_id, "dropping passthrough: no client shows the buffer");
            return;
        };
        for writes in batch_passthrough_writes(writes) {
            let event = ServerEnvelope::Event(ServerEvent::Passthrough(PassthroughEvent {
                buffer_id,
                writes,
            }));
            self.send_to_clients(&client_ids, event).await;
        }
    }

    /// The clients that show `buffer_id`: those whose current session
    /// contains it, and those watching it (`BufferRequest::Watch`), such as a
    /// server showing it in a pane of its own. `None` when there are none.
    async fn clients_showing_buffer(&self, buffer_id: BufferId) -> Option<BTreeSet<u64>> {
        let session_id = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id).ok()?;
            match buffer.attachment {
                BufferAttachment::Attached(node_id) => Some(state.node(node_id).ok()?.session_id()),
                BufferAttachment::Detached => None,
            }
        };
        let client_ids: BTreeSet<u64> = self
            .clients
            .lock()
            .await
            .iter()
            .filter(|(_, client)| {
                session_id.is_some_and(|session_id| client.current_session_id == Some(session_id))
                    || client.watched_buffers.contains(&buffer_id)
            })
            .map(|(&client_id, _)| client_id)
            .collect();
        (!client_ids.is_empty()).then_some(client_ids)
    }

    /// Send a `display-message` event to `client_id`, or to every client whose
    /// current session is `session_id`. Fails when no such client exists, as
    /// tmux does without a client to show the message on.
    async fn display_message(
        &self,
        client_id: Option<std::num::NonZeroU64>,
        session_id: Option<embers_core::SessionId>,
        event: ServerEvent,
    ) -> Result<()> {
        let client_ids: BTreeSet<u64> = {
            let clients = self.clients.lock().await;
            match (client_id, session_id) {
                (Some(client_id), _) => {
                    if !clients.contains_key(&client_id.get()) {
                        return Err(MuxError::not_found(format!("unknown client {client_id}")));
                    }
                    BTreeSet::from([client_id.get()])
                }
                (None, Some(session_id)) => clients
                    .iter()
                    .filter(|(_, client)| client.current_session_id == Some(session_id))
                    .map(|(&client_id, _)| client_id)
                    .collect(),
                (None, None) => BTreeSet::new(),
            }
        };
        if client_ids.is_empty() {
            return Err(MuxError::not_found(
                "no client is attached to show the message on",
            ));
        }
        self.send_to_clients(&client_ids, ServerEnvelope::Event(event))
            .await;
        Ok(())
    }

    /// Send `event` once to each of `client_ids`, best effort: unlike
    /// `broadcast`, a client too slow to take it misses it but stays attached.
    async fn send_to_clients(&self, client_ids: &BTreeSet<u64>, event: ServerEnvelope) {
        // Events reach a client through its subscriptions; send through one per
        // client so a client with several subscriptions gets it once.
        let mut delivered = BTreeSet::new();
        let subscriptions = self.subscriptions.lock().await;
        for subscription in subscriptions.values() {
            if !client_ids.contains(&subscription.connection_id)
                || !delivered.insert(subscription.connection_id)
            {
                continue;
            }
            if let Err(error) = subscription.sender.try_send(event.clone()) {
                debug!(
                    connection_id = subscription.connection_id,
                    %error,
                    "dropping event for slow client"
                );
            }
        }
    }

    /// Record how `buffer_id`'s runtime ended.
    async fn record_buffer_end(&self, buffer_id: BufferId, end: RuntimeEnd) {
        let (should_interrupt, exit_code) = match end {
            RuntimeEnd::Exited(exit_code) => (false, exit_code),
            RuntimeEnd::Interrupted => (true, None),
        };
        let runtime = self.buffer_runtimes.get(buffer_id);
        // An interrupted runtime is no longer registered: one that is was
        // taken up again since, and runs.
        if should_interrupt && runtime.is_some() {
            return;
        }
        let (updated, pipe_changed) = {
            let mut state = self.state.lock().await;
            let result = if should_interrupt {
                let pid = state
                    .buffers
                    .get(&buffer_id)
                    .and_then(|buffer| buffer_pid_hint(&buffer.state));
                state.mark_buffer_interrupted(buffer_id, pid)
            } else {
                state.mark_buffer_exited(buffer_id, exit_code)
            };
            match result {
                Ok(()) => {
                    let pipe_reason = if should_interrupt {
                        BufferPipeStopReason::RuntimeInterrupted
                    } else {
                        BufferPipeStopReason::BufferExited
                    };
                    let pipe_changed = state
                        .buffers
                        .get_mut(&buffer_id)
                        .is_some_and(|buffer| stop_buffer_pipe(buffer, pipe_reason, exit_code));
                    (true, pipe_changed)
                }
                Err(error) => {
                    debug!(%buffer_id, %error, "buffer exited after state cleanup");
                    (false, false)
                }
            }
        };
        if !should_interrupt {
            // Whether this marked the buffer exited or found its record already
            // gone, the next save no longer needs the keeper.
            self.mark_workspace_dirty();
        }

        if updated && pipe_changed && !should_interrupt {
            match runtime {
                Some(runtime) => match runtime.stop_pipe_after_exit().await {
                    Ok(pipe) => {
                        let mut state = self.state.lock().await;
                        if let Some(buffer) = state.buffers.get_mut(&buffer_id) {
                            buffer.pipe = Some(model_pipe_from_runtime(&pipe));
                        }
                    }
                    Err(error) => {
                        debug!(%buffer_id, %error, "failed to stop buffer pipe after buffer exit");
                    }
                },
                None => {
                    debug!(%buffer_id, "buffer exited with a running pipe but no runtime handle");
                }
            }
        }

        let pipe_event = if updated && pipe_changed {
            let state = self.state.lock().await;
            buffer_pipe_changed_event(&state, buffer_id).ok()
        } else {
            None
        };

        if updated {
            self.mark_workspace_dirty();
            let mut events = vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                buffer_id,
            })];
            if let Some(pipe_event) = pipe_event {
                events.push(ServerEvent::BufferPipeChanged(pipe_event));
            }
            if !should_interrupt {
                self.dispose_exited_buffers(&mut events).await;
            }
            let retired_session_ids = closed_session_ids(&events);
            self.broadcast(events, &retired_session_ids).await;
        }
    }

    /// The sessions that show a pane, when a request is about to close a view
    /// (`closes`); none otherwise.
    async fn sessions_with_panes_if(&self, closes: bool) -> BTreeSet<embers_core::SessionId> {
        if closes {
            self.state.lock().await.sessions_with_panes()
        } else {
            BTreeSet::new()
        }
    }

    /// Destroy every session in `had_panes` that no longer shows any pane,
    /// like tmux destroys a session whose last window closed: its last pane's
    /// command exited, or its view was closed (`kill-pane`, `kill-window` and
    /// `kill-popup` close views, with or without `--detach`). Detaching or
    /// moving a buffer out of its session does not count. A session that
    /// never had a pane, such as one just created, is left alone.
    ///
    /// Clients on a destroyed session are left without one; the interactive
    /// client then moves to another session or exits, per its
    /// `detach-on-destroy` setting. Layout and focus events about a destroyed
    /// session are dropped from `events`, so no client asks about it after
    /// learning it is gone, and `SessionClosed` and `ClientChanged` events are
    /// added.
    async fn destroy_emptied_sessions(
        &self,
        had_panes: BTreeSet<embers_core::SessionId>,
        events: &mut Vec<ServerEvent>,
    ) {
        let (destroyed, changed_clients) = {
            let mut state = self.state.lock().await;
            let mut clients = self.clients.lock().await;
            let mut destroyed = Vec::new();
            let mut changed_clients = Vec::new();
            for session_id in had_panes {
                if !state.sessions.contains_key(&session_id) || state.session_has_panes(session_id)
                {
                    continue;
                }
                if let Err(error) = state.close_session(session_id) {
                    warn!(%session_id, %error, "failed to destroy session without panes");
                    continue;
                }
                changed_clients.extend(Self::clear_client_session(&mut clients, session_id));
                destroyed.push(session_id);
            }
            (destroyed, changed_clients)
        };
        if destroyed.is_empty() {
            return;
        }
        info!(sessions = ?destroyed, "destroyed sessions whose last pane closed");
        self.mark_workspace_dirty();
        events.retain(|event| match event {
            ServerEvent::NodeChanged(NodeChangedEvent { session_id })
            | ServerEvent::FocusChanged(FocusChangedEvent { session_id, .. })
            | ServerEvent::FloatingChanged(FloatingChangedEvent { session_id, .. }) => {
                !destroyed.contains(session_id)
            }
            _ => true,
        });
        events.extend(
            destroyed
                .iter()
                .map(|&session_id| ServerEvent::SessionClosed(SessionClosedEvent { session_id })),
        );
        events.extend(self.client_changed_events(changed_clients).await);
    }

    /// Apply [`ServerState::dispose_exited_buffers`]: close the views of
    /// exited buffers and remove their records, then release the runtimes of
    /// the removed buffers, whose keepers can exit once a save records the
    /// removal (see [`BufferRuntimes::awaiting_save`]). Adds the events that
    /// describe the closed views to `events`, and destroys sessions whose last
    /// pane this closed.
    async fn dispose_exited_buffers(&self, events: &mut Vec<ServerEvent>) {
        let (removed, changed_sessions) = {
            let mut state = self.state.lock().await;
            let disposed = state.dispose_exited_buffers();
            for &session_id in &disposed.changed_sessions {
                if !state.sessions.contains_key(&session_id) {
                    continue;
                }
                events.push(ServerEvent::NodeChanged(NodeChangedEvent { session_id }));
                if let Some(focus_event) = focus_changed_event(&state, session_id) {
                    events.push(ServerEvent::FocusChanged(focus_event));
                }
            }
            (disposed.removed_buffers, disposed.changed_sessions)
        };
        self.destroy_emptied_sessions(changed_sessions, events)
            .await;
        if removed.is_empty() {
            return;
        }
        self.mark_workspace_dirty();
        // Until a save without the record has completed, a crash would leave
        // it saved as running, so the keeper waits to be told. Dropping the
        // last handle after that closes the keeper connection; with its child
        // gone and no server attached, the keeper then exits.
        for buffer_id in removed {
            self.buffer_runtimes.release(buffer_id).await;
        }
    }

    async fn sync_buffer_runtime_status(
        &self,
        buffer_id: BufferId,
        runtime: &BufferRuntimeHandle,
    ) -> Result<()> {
        let status = runtime.status().await?;
        self.record_buffer_update(
            buffer_id,
            BufferRuntimeUpdate {
                sequence: status.sequence,
                activity: status.activity,
                title: Some(status.title.clone()),
                pipe: Some(status.pipe.clone()),
                cwd: Some(status.cwd.clone()),
                foreground_command: Some(status.foreground_command.clone()),
                bell_sequence: status.bell_sequence,
                user_options: None,
            },
        )
        .await;
        if !status.running {
            self.record_buffer_end(buffer_id, RuntimeEnd::Exited(status.exit_code))
                .await;
        }
        Ok(())
    }

    async fn broadcast(
        &self,
        events: Vec<ServerEvent>,
        retired_session_ids: &[embers_core::SessionId],
    ) {
        if events.is_empty() && retired_session_ids.is_empty() {
            return;
        }

        let mut subscriptions = self.subscriptions.lock().await;
        if !events.is_empty() {
            subscriptions.retain(|_, subscription| {
                for event in &events {
                    let event_session_ids = event.all_session_ids();
                    let event_matches = event_session_ids.is_empty()
                        || subscription.session_id.is_none()
                        || subscription
                            .session_id
                            .is_some_and(|session_id| event_session_ids.contains(&session_id));

                    if event_matches {
                        match subscription
                            .sender
                            .try_send(ServerEnvelope::Event(event.clone()))
                        {
                            Ok(()) => {}
                            // Client is gone: drop the subscription.
                            Err(mpsc::error::TrySendError::Closed(_)) => return false,
                            // Client cannot keep up: detach it rather than let
                            // its queue grow without bound and exhaust the
                            // daemon. It can reattach and resync from a fresh
                            // snapshot.
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                warn!(
                                    connection_id = subscription.connection_id,
                                    "client outbound queue full; detaching slow client"
                                );
                                return false;
                            }
                        }
                    }
                }
                true
            });
        }

        if !retired_session_ids.is_empty() {
            let retired_session_ids = retired_session_ids.iter().copied().collect::<BTreeSet<_>>();
            subscriptions.retain(|_, subscription| {
                !matches!(
                    subscription.session_id,
                    Some(session_id) if retired_session_ids.contains(&session_id)
                )
            });
        }
    }

    async fn list_clients(&self) -> Vec<ClientRecord> {
        let clients = self.clients.lock().await;
        let subscriptions = self.subscriptions.lock().await;
        let mut records = clients
            .iter()
            .map(|(&client_id, client)| {
                let mut subscribed_all_sessions = false;
                let mut subscribed_session_ids = Vec::new();
                for subscription in subscriptions.values() {
                    if subscription.connection_id != client_id {
                        continue;
                    }
                    match subscription.session_id {
                        Some(session_id) => subscribed_session_ids.push(session_id),
                        None => subscribed_all_sessions = true,
                    }
                }
                subscribed_session_ids.sort_by_key(|session_id| session_id.0);
                subscribed_session_ids.dedup();
                ClientRecord {
                    id: client_id,
                    current_session_id: client.current_session_id,
                    subscribed_all_sessions,
                    subscribed_session_ids,
                }
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        records
    }

    async fn client_record(&self, client_id: u64) -> Option<ClientRecord> {
        let clients = self.clients.lock().await;
        let current_session_id = clients.get(&client_id)?.current_session_id;
        let subscriptions = self.subscriptions.lock().await;
        let mut subscribed_all_sessions = false;
        let mut subscribed_session_ids = Vec::new();
        for subscription in subscriptions.values() {
            if subscription.connection_id != client_id {
                continue;
            }
            match subscription.session_id {
                Some(session_id) => subscribed_session_ids.push(session_id),
                None => subscribed_all_sessions = true,
            }
        }
        subscribed_session_ids.sort_by_key(|session_id| session_id.0);
        subscribed_session_ids.dedup();
        Some(ClientRecord {
            id: client_id,
            current_session_id,
            subscribed_all_sessions,
            subscribed_session_ids,
        })
    }

    async fn detach_client(&self, client_id: u64) -> Result<DetachedClient> {
        let detached = {
            let mut clients = self.clients.lock().await;
            let Some(mut client) = clients.remove(&client_id) else {
                return Err(MuxError::not_found(format!(
                    "client {client_id} was not found"
                )));
            };
            DetachedClient {
                shutdown: client.shutdown.take(),
                stopped: client.stopped.take(),
            }
        };
        self.subscriptions
            .lock()
            .await
            .retain(|_, subscription| subscription.connection_id != client_id);
        Ok(detached)
    }

    async fn set_client_session(
        &self,
        client_id: u64,
        session_id: Option<embers_core::SessionId>,
    ) -> Result<(ClientRecord, Vec<ServerEvent>)> {
        let (previous_session_id, acknowledged) = {
            let mut state = self.state.lock().await;
            if let Some(session_id) = session_id
                && !state.sessions.contains_key(&session_id)
            {
                return Err(MuxError::not_found(format!(
                    "session {session_id} was not found"
                )));
            }
            let mut clients = self.clients.lock().await;
            let client = clients
                .get_mut(&client_id)
                .ok_or_else(|| MuxError::not_found(format!("client {client_id} was not found")))?;
            let previous = client.current_session_id;
            client.current_session_id = session_id;
            // A client now looks at the session's current window, so its
            // flags are acknowledged, as tmux clears them on attach. The
            // client has switched already, so a failure here does not undo it.
            let acknowledged = session_id.is_some_and(|session_id| {
                state
                    .acknowledge_shown_activity(session_id)
                    .unwrap_or(false)
            });
            (previous, acknowledged)
        };
        let record = self
            .client_record(client_id)
            .await
            .ok_or_else(|| MuxError::not_found(format!("client {client_id} was not found")))?;
        let mut events = vec![ServerEvent::ClientChanged(ClientChangedEvent {
            client: record.clone(),
            previous_session_id,
        })];
        // Other clients and automation refetch the session to see the cleared
        // flags.
        if acknowledged && let Some(session_id) = session_id {
            events.push(ServerEvent::NodeChanged(NodeChangedEvent { session_id }));
        }
        Ok((record, events))
    }

    fn clear_client_session(
        clients: &mut BTreeMap<u64, ClientConnection>,
        session_id: embers_core::SessionId,
    ) -> Vec<(u64, Option<embers_core::SessionId>)> {
        let mut changed = Vec::new();
        for (&client_id, client) in clients.iter_mut() {
            if client.current_session_id == Some(session_id) {
                client.current_session_id = None;
                changed.push((client_id, Some(session_id)));
            }
        }
        changed
    }

    async fn client_changed_events(
        &self,
        changed: Vec<(u64, Option<embers_core::SessionId>)>,
    ) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        for (client_id, previous_session_id) in changed {
            if let Some(client) = self.client_record(client_id).await {
                events.push(ServerEvent::ClientChanged(ClientChangedEvent {
                    client,
                    previous_session_id,
                }));
            }
        }
        events
    }

    async fn cleanup_connection(&self, connection_id: u64) {
        let client = self.clients.lock().await.remove(&connection_id);
        self.subscriptions
            .lock()
            .await
            .retain(|_, subscription| subscription.connection_id != connection_id);
        self.sync_host_graphics().await;
        if let Some(client) = client
            && !client.watched_buffers.is_empty()
        {
            let mut events = Vec::new();
            self.sync_watched_buffers(client.watched_buffers, &mut events)
                .await;
            let retired_session_ids = closed_session_ids(&events);
            self.broadcast(events, &retired_session_ids).await;
        }
    }

    /// Mark `buffer_ids` watched or not by whether a client still watches
    /// them, and clean up the exited ones nothing watches any more.
    async fn sync_watched_buffers(
        &self,
        buffer_ids: impl IntoIterator<Item = BufferId>,
        events: &mut Vec<ServerEvent>,
    ) {
        let watched: BTreeSet<BufferId> = self
            .clients
            .lock()
            .await
            .values()
            .flat_map(|client| client.watched_buffers.iter().copied())
            .collect();
        {
            let mut state = self.state.lock().await;
            for buffer_id in buffer_ids {
                if let Some(buffer) = state.buffers.get_mut(&buffer_id) {
                    buffer.watched = watched.contains(&buffer_id);
                }
            }
        }
        self.dispose_exited_buffers(events).await;
    }

    /// Tell every keeper whether any client now forwards kitty graphics
    /// queries to a host terminal that answers them, when that changed. While
    /// one does, a keeper holds a pane's replies to the queries that follow a
    /// graphics query until the host's answer arrives (see
    /// `crate::reply_hold`); otherwise it answers at once, as before. It is not
    /// worked out per pane: a pane no such client shows waits out the hold's
    /// timeout instead.
    async fn sync_host_graphics(&self) {
        // Held from reading the clients until the runtimes were told, so they
        // hear changes in order.
        let _sync = self.host_graphics_sync.lock().await;
        let enabled = self
            .clients
            .lock()
            .await
            .values()
            .any(|client| client.host_graphics);
        self.buffer_runtimes.set_host_graphics(enabled).await;
    }
}

fn location_is_root_tab(state: &ServerState, location: &BufferLocation) -> Result<bool> {
    let BufferLocationAttachment::Session {
        session_id,
        node_id,
    } = location.attachment
    else {
        return Ok(false);
    };
    let Node::Tabs(tabs) = state.node(state.root_tabs(session_id)?)? else {
        return Ok(false);
    };
    Ok(tabs.tabs.iter().any(|tab| tab.child == node_id))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionExit {
    Closed,
    SelfDetached,
    /// The server is stopping; the connection's last event,
    /// [`ServerEvent::ServerShutdown`], is queued and should be flushed.
    ServerStopped,
}

fn closed_session_ids(events: &[ServerEvent]) -> Vec<embers_core::SessionId> {
    let mut session_ids = BTreeSet::new();
    for event in events {
        if let ServerEvent::SessionClosed(event) = event {
            session_ids.insert(event.session_id);
        }
    }
    session_ids.into_iter().collect()
}

async fn handle_connection(
    runtime: Arc<Runtime>,
    connection_id: u64,
    mut reader: OwnedReadHalf,
    outbound: mpsc::Sender<ServerEnvelope>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<ConnectionExit> {
    let mut server_shutdown = runtime.shutdown.subscribe();
    loop {
        let frame = tokio::select! {
            _ = wait_for_shutdown(&mut server_shutdown) => {
                // Tell the client the server is stopping on purpose, so it can
                // exit cleanly rather than report a lost server. Best effort:
                // a client too slow to take it sees the connection close.
                let _ = outbound.try_send(ServerEnvelope::Event(ServerEvent::ServerShutdown));
                return Ok(ConnectionExit::ServerStopped);
            }
            _ = &mut shutdown => {
                debug!(connection_id, "client detach requested");
                return Ok(ConnectionExit::Closed);
            }
            frame = read_frame(&mut reader) => frame.map_err(protocol_error_to_mux)?,
        };
        let Some(frame) = frame else {
            debug!(connection_id, "client disconnected");
            return Ok(ConnectionExit::Closed);
        };

        if frame.frame_type != FrameType::Request {
            if outbound
                .send(ServerEnvelope::Response(protocol_error_response(
                    Some(frame.request_id),
                    ProtocolError::UnexpectedFrameType(frame.frame_type),
                )))
                .await
                .is_err()
            {
                return Err(MuxError::transport("connection writer closed"));
            }
            continue;
        }

        let request = match decode_client_message(&frame.payload) {
            Ok(request) => {
                if request.request_id() != frame.request_id {
                    if outbound
                        .send(ServerEnvelope::Response(protocol_error_response(
                            Some(frame.request_id),
                            ProtocolError::MismatchedRequestId {
                                expected: frame.request_id,
                                actual: request.request_id(),
                            },
                        )))
                        .await
                        .is_err()
                    {
                        return Err(MuxError::transport("connection writer closed"));
                    }
                    continue;
                }
                request
            }
            Err(error) => {
                if outbound
                    .send(ServerEnvelope::Response(protocol_error_response(
                        Some(frame.request_id),
                        error,
                    )))
                    .await
                    .is_err()
                {
                    return Err(MuxError::transport("connection writer closed"));
                }
                continue;
            }
        };

        let request_id = request.request_id();
        let kill_server = matches!(request, ClientMessage::KillServer(_));
        let (response, events, deferred_shutdown) = runtime
            .dispatch_request(connection_id, &outbound, request)
            .instrument(request_span("handle_request", request_id))
            .await;

        if outbound
            .send(ServerEnvelope::Response(response))
            .await
            .is_err()
        {
            return Err(MuxError::transport("connection writer closed"));
        }
        // Every request that changes the workspace tells clients so through an
        // event. Render invalidations alone only report new output.
        if events
            .iter()
            .any(|event| !matches!(event, ServerEvent::RenderInvalidated(_)))
        {
            runtime.mark_workspace_dirty();
        }
        let retired_session_ids = closed_session_ids(&events);
        runtime.broadcast(events, &retired_session_ids).await;

        // Handle self-detach: trigger shutdown after response is sent
        if let Some(shutdown) = deferred_shutdown {
            let _ = shutdown.send(());
            return Ok(ConnectionExit::SelfDetached);
        }
        // Stop the server only once the kill-server reply is queued, and let
        // the writer flush it rather than abort it with the connection.
        if kill_server {
            runtime.shutdown.trigger();
            return Ok(ConnectionExit::SelfDetached);
        }
    }
}

async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut outbound: mpsc::Receiver<ServerEnvelope>,
) -> Result<()> {
    while let Some(envelope) = outbound.recv().await {
        let payload = encode_server_envelope(&envelope).map_err(protocol_error_to_mux)?;
        let (frame_type, request_id) = match &envelope {
            ServerEnvelope::Response(response) => (
                FrameType::Response,
                response.request_id().unwrap_or(RequestId(0)),
            ),
            ServerEnvelope::Event(_) => (FrameType::Event, RequestId(0)),
        };
        let frame = RawFrame::new(frame_type, request_id, payload);
        write_frame_no_flush(&mut writer, &frame)
            .await
            .map_err(protocol_error_to_mux)?;
    }

    Ok(())
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow_and_update() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow_and_update() {
            return;
        }
    }
}

fn set_socket_permissions(socket_path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn protocol_tab_index(index: u32) -> Result<usize> {
    usize::try_from(index)
        .map_err(|_| MuxError::invalid_input(format!("tab index {index} exceeds platform limits")))
}

fn focus_changed_event(
    state: &ServerState,
    session_id: embers_core::SessionId,
) -> Option<FocusChangedEvent> {
    state
        .session(session_id)
        .ok()
        .map(|session| FocusChangedEvent {
            session_id,
            focused_leaf_id: session.focused_leaf,
            focused_floating_id: session.focused_floating,
        })
}

fn layout_changed_events(
    state: &ServerState,
    session_id: embers_core::SessionId,
) -> Result<Vec<ServerEvent>> {
    state.session(session_id)?;
    let mut events = vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
    if let Some(focus_event) = focus_changed_event(state, session_id) {
        events.push(ServerEvent::FocusChanged(focus_event));
    }
    Ok(events)
}

fn layout_ok_response(
    state: &ServerState,
    request_id: RequestId,
    session_id: embers_core::SessionId,
) -> (ServerResponse, Vec<ServerEvent>) {
    match layout_changed_events(state, session_id) {
        Ok(events) => (ServerResponse::Ok(OkResponse { request_id }), events),
        Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
    }
}

fn layout_snapshot_response(
    state: &ServerState,
    request_id: RequestId,
    session_id: embers_core::SessionId,
) -> (ServerResponse, Vec<ServerEvent>) {
    match session_snapshot(state, session_id) {
        Ok(snapshot) => match layout_changed_events(state, session_id) {
            Ok(events) => (
                ServerResponse::SessionSnapshot(SessionSnapshotResponse {
                    request_id,
                    snapshot,
                }),
                events,
            ),
            Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
        },
        Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
    }
}

fn error_response(
    request_id: Option<RequestId>,
    code: ErrorCode,
    message: impl Into<String>,
) -> ServerResponse {
    ServerResponse::Error(ErrorResponse {
        request_id,
        error: WireError::new(code, message),
    })
}

fn protocol_error_response(request_id: Option<RequestId>, error: ProtocolError) -> ServerResponse {
    error_response(request_id, ErrorCode::ProtocolViolation, error.to_string())
}

/// Rejection message shared by the session/buffer creation paths when a resource
/// ceiling is hit. `resource` is the singular noun ("session"/"buffer"); `limit`
/// and `env_var` name the active ceiling and the override knob.
fn limit_reached_message(resource: &str, limit: usize, env_var: &str) -> String {
    format!("{resource} limit reached ({limit}); close existing {resource}s or raise {env_var}")
}

fn mux_error_response(request_id: Option<RequestId>, error: MuxError) -> ServerResponse {
    let (code, message) = match error {
        MuxError::Wire(wire) => (wire.code, wire.message),
        MuxError::Io(io) => (ErrorCode::Transport, io.to_string()),
        MuxError::Protocol(message) => (ErrorCode::ProtocolViolation, message),
        MuxError::Transport(message) => (ErrorCode::Transport, message),
        MuxError::InvalidInput(message) => (ErrorCode::InvalidRequest, message),
        MuxError::NotFound(message) => (ErrorCode::NotFound, message),
        MuxError::Conflict(message) => (ErrorCode::Conflict, message),
        MuxError::Unsupported(message) => (ErrorCode::Unsupported, message),
        MuxError::Timeout(message) => (ErrorCode::Timeout, message),
        MuxError::Pty(message) => (ErrorCode::Transport, message),
        MuxError::Internal(message) => (ErrorCode::Internal, message),
    };

    error_response(request_id, code, message)
}

fn protocol_error_to_mux(error: ProtocolError) -> MuxError {
    MuxError::protocol(error.to_string())
}

fn model_pipe_from_runtime(status: &BufferRuntimePipeStatus) -> BufferPipe {
    BufferPipe {
        command: status.command.clone(),
        state: if status.running {
            BufferPipeState::Running { pid: status.pid }
        } else {
            BufferPipeState::Stopped {
                exit_code: status.exit_code,
                reason: match status
                    .stop_reason
                    .unwrap_or(BufferRuntimePipeStopReason::PipeExited)
                {
                    BufferRuntimePipeStopReason::Requested => BufferPipeStopReason::Requested,
                    BufferRuntimePipeStopReason::PipeExited => BufferPipeStopReason::PipeExited,
                    BufferRuntimePipeStopReason::WriteFailed => BufferPipeStopReason::WriteFailed,
                    BufferRuntimePipeStopReason::BufferExited => BufferPipeStopReason::BufferExited,
                },
            }
        },
    }
}

fn stop_buffer_pipe(
    buffer: &mut crate::Buffer,
    reason: BufferPipeStopReason,
    exit_code: Option<i32>,
) -> bool {
    let Some(pipe) = buffer.pipe.as_mut() else {
        return false;
    };
    if matches!(pipe.state, BufferPipeState::Stopped { .. }) {
        return false;
    }
    pipe.state = BufferPipeState::Stopped { exit_code, reason };
    true
}

fn buffer_pipe_changed_event(
    state: &ServerState,
    buffer_id: BufferId,
) -> Result<BufferPipeChangedEvent> {
    let buffer = state.buffer(buffer_id)?;
    let session_id = match buffer.attachment {
        BufferAttachment::Attached(node_id) => Some(state.node(node_id)?.session_id()),
        BufferAttachment::Detached => None,
    };
    Ok(BufferPipeChangedEvent {
        session_id,
        buffer: buffer_record(buffer),
    })
}

fn apply_runtime_status(
    state: &mut ServerState,
    buffer_id: BufferId,
    status: &BufferRuntimeStatus,
) {
    if let Some(buffer) = state.buffers.get_mut(&buffer_id) {
        buffer.last_snapshot_seq = status.sequence;
        // A newly attached runtime counts bells from its own start; its
        // current activity below already says whether one is pending.
        buffer.runtime_bell_sequence = status.bell_sequence;
        buffer.pipe = status.pipe.as_ref().map(model_pipe_from_runtime);
        buffer.foreground_command = status.foreground_command.clone();
    }
    if let Some(title) = &status.title {
        let _ = state.set_buffer_title(buffer_id, title.clone());
    }
    if let Some(buffer) = state.buffers.get(&buffer_id) {
        let current_activity = buffer.activity;
        let _ =
            state.set_buffer_activity(buffer_id, max_activity(current_activity, status.activity));
    }
    if status.running {
        let _ = state.mark_buffer_running(buffer_id, status.pid);
    } else {
        let _ = state.mark_buffer_exited(buffer_id, status.exit_code);
    }
}

fn max_activity(
    left: embers_core::ActivityState,
    right: embers_core::ActivityState,
) -> embers_core::ActivityState {
    use embers_core::ActivityState;

    match (left, right) {
        (ActivityState::Bell, _) | (_, ActivityState::Bell) => ActivityState::Bell,
        (ActivityState::Activity, _) | (_, ActivityState::Activity) => ActivityState::Activity,
        _ => ActivityState::Idle,
    }
}

fn buffer_pid_hint(state: &BufferState) -> Option<u32> {
    match state {
        BufferState::Running(running) => running.pid,
        BufferState::Interrupted(interrupted) => interrupted.last_known_pid,
        BufferState::Created | BufferState::Exited(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use embers_core::{
        ActivityState, ErrorCode, FloatGeometry, MuxError, RequestId, SplitDirection,
    };
    use embers_protocol::{
        BufferHistoryPlacement, BufferHistoryScope, BufferRequest, InputRequest,
        NodeBreakDestination, NodeJoinPlacement, NodeRequest, ServerEnvelope, ServerEvent,
        ServerResponse, SessionRequest,
    };
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::{
        MAX_USER_OPTION_KEY_LEN, MAX_USER_OPTION_VALUE_LEN, MAX_USER_OPTIONS_PER_BUFFER,
        OUTBOUND_CHANNEL_CAPACITY, Runtime, ShutdownSignal, Subscription, wait_for_shutdown,
    };
    use crate::ResourceLimits;
    use crate::model::HelperBufferScope;
    use crate::{
        BufferRuntimePipeStatus, BufferRuntimePipeStopReason, BufferRuntimeUpdate, ServerState,
    };

    use tokio::time::{Duration, timeout};

    fn create_leaf(
        state: &mut ServerState,
        session_id: embers_core::SessionId,
        title: &str,
    ) -> embers_core::NodeId {
        let buffer_id = state.create_buffer(title, vec!["/bin/sh".to_owned()], None);
        state
            .create_buffer_view(session_id, buffer_id)
            .expect("create leaf")
    }

    fn floating_split_runtime() -> (
        Runtime,
        embers_core::SessionId,
        embers_core::FloatingId,
        embers_core::NodeId,
        embers_core::NodeId,
    ) {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let session_id = state.create_session("alpha");
        let root_leaf = create_leaf(&mut state, session_id, "root");
        state
            .add_root_tab(session_id, "main", root_leaf)
            .expect("attach root leaf");
        let split_left = create_leaf(&mut state, session_id, "left");
        let split_right = create_leaf(&mut state, session_id, "right");
        let split_root = state
            .create_split_node(
                session_id,
                SplitDirection::Vertical,
                vec![split_left, split_right],
            )
            .expect("create floating split");
        let floating_id = state
            .create_floating_window_with_options(
                session_id,
                split_root,
                FloatGeometry::new(4, 4, 20, 10),
                Some("popup".to_owned()),
                false,
                true,
            )
            .expect("create floating");
        (
            Runtime::new(
                state,
                tempdir.path().join("server.sock"),
                tempdir.path().join("workspace.json"),
                tempdir.path().join("runtime"),
                BTreeMap::new(),
                ResourceLimits::default(),
            ),
            session_id,
            floating_id,
            split_left,
            split_right,
        )
    }

    fn hidden_floating_tabs_runtime() -> (
        Runtime,
        embers_core::SessionId,
        embers_core::FloatingId,
        embers_core::NodeId,
    ) {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let session_id = state.create_session("alpha");
        let root_leaf = create_leaf(&mut state, session_id, "root");
        state
            .add_root_tab(session_id, "main", root_leaf)
            .expect("attach root leaf");
        let split_left = create_leaf(&mut state, session_id, "left");
        let split_right = create_leaf(&mut state, session_id, "right");
        let split_root = state
            .create_split_node(
                session_id,
                SplitDirection::Vertical,
                vec![split_left, split_right],
            )
            .expect("create hidden split");
        let other_leaf = create_leaf(&mut state, session_id, "other");
        let hidden_tabs = state
            .create_tabs_node(
                session_id,
                vec![
                    crate::TabEntry::new("split", split_root),
                    crate::TabEntry::new("other", other_leaf),
                ],
                0,
            )
            .expect("create hidden tabs");
        let floating_id = state
            .create_floating_window_with_options(
                session_id,
                hidden_tabs,
                FloatGeometry::new(4, 4, 20, 10),
                Some("popup".to_owned()),
                false,
                true,
            )
            .expect("create floating");
        state
            .floating
            .get_mut(&floating_id)
            .expect("floating exists")
            .visible = false;
        (
            Runtime::new(
                state,
                tempdir.path().join("server.sock"),
                tempdir.path().join("workspace.json"),
                tempdir.path().join("runtime"),
                BTreeMap::new(),
                ResourceLimits::default(),
            ),
            session_id,
            floating_id,
            split_right,
        )
    }

    #[tokio::test]
    async fn shutdown_signal_is_latched_for_new_receivers() {
        let signal = ShutdownSignal::new();
        signal.trigger();
        let mut shutdown = signal.subscribe();

        timeout(Duration::from_millis(50), wait_for_shutdown(&mut shutdown))
            .await
            .expect("latched shutdown should resolve immediately");
    }

    #[tokio::test]
    async fn create_session_is_rejected_when_session_limit_reached() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits {
                max_sessions: 1,
                ..ResourceLimits::default()
            },
        );

        let (first, _events) = runtime
            .dispatch_session(SessionRequest::Create {
                request_id: RequestId(1),
                name: "alpha".to_owned(),
            })
            .await;
        assert!(matches!(first, ServerResponse::SessionSnapshot(_)));

        let (second, events) = runtime
            .dispatch_session(SessionRequest::Create {
                request_id: RequestId(2),
                name: "beta".to_owned(),
            })
            .await;
        assert!(events.is_empty());
        match second {
            ServerResponse::Error(response) => {
                assert_eq!(response.error.code, ErrorCode::Conflict);
            }
            other => panic!("expected conflict error, got {other:?}"),
        }
        assert_eq!(runtime.state.lock().await.sessions.len(), 1);
    }

    #[tokio::test]
    async fn session_viewport_is_kept_and_seeds_new_sessions() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let create = |request_id, name: &str| SessionRequest::Create {
            request_id: RequestId(request_id),
            name: name.to_owned(),
        };
        let session_viewport = |response| match response {
            ServerResponse::SessionSnapshot(response) => (
                response.snapshot.session.viewport,
                response.snapshot.session.pane_titles,
            ),
            other => panic!("expected session snapshot, got {other:?}"),
        };

        let (response, _) = runtime.dispatch_session(create(1, "alpha")).await;
        assert_eq!(session_viewport(response), (None, false));
        let alpha = runtime
            .state
            .lock()
            .await
            .sessions
            .keys()
            .copied()
            .next()
            .expect("session");
        let viewport = embers_core::Size {
            width: 120,
            height: 40,
        };
        let (response, events) = runtime
            .dispatch_session(SessionRequest::SetViewport {
                request_id: RequestId(2),
                session_id: alpha,
                viewport,
                pane_titles: true,
            })
            .await;
        assert!(matches!(response, ServerResponse::Ok(_)));
        assert!(events.is_empty());
        let (response, _) = runtime
            .dispatch_session(SessionRequest::Get {
                request_id: RequestId(3),
                session_id: alpha,
            })
            .await;
        assert_eq!(session_viewport(response), (Some(viewport), true));

        // A session no client has drawn yet starts with the last viewport,
        // and the title rows it was drawn with.
        let (response, _) = runtime.dispatch_session(create(4, "beta")).await;
        assert_eq!(session_viewport(response), (Some(viewport), true));
    }

    #[tokio::test]
    async fn create_buffer_is_rejected_when_buffer_limit_reached() {
        let runtime = Arc::new(Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits {
                max_buffers: 1,
                ..ResourceLimits::default()
            },
        ));

        // Pre-seed the single allowed buffer directly so the dispatch guard
        // rejects the next create before any PTY process is spawned.
        runtime
            .state
            .lock()
            .await
            .create_buffer("existing", vec!["/bin/sh".to_owned()], None);

        let (response, events) = runtime
            .dispatch_buffer(
                1,
                BufferRequest::Create {
                    request_id: RequestId(1),
                    title: None,
                    command: vec!["/bin/sh".to_owned()],
                    cwd: None,
                    env: BTreeMap::new(),
                    size: None,
                    session_id: None,
                    host: None,
                },
            )
            .await;
        assert!(events.is_empty());
        match response {
            ServerResponse::Error(response) => {
                assert_eq!(response.error.code, ErrorCode::Conflict);
            }
            other => panic!("expected conflict error, got {other:?}"),
        }
        assert_eq!(runtime.state.lock().await.buffers.len(), 1);
    }

    #[tokio::test]
    async fn set_buffer_user_option_enforces_caps() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            state.create_buffer("shell", vec!["/bin/sh".to_owned()], None)
        };

        // Over-long value and key are rejected.
        assert!(
            runtime
                .set_buffer_user_option(
                    buffer_id,
                    "k".to_owned(),
                    Some("x".repeat(MAX_USER_OPTION_VALUE_LEN + 1)),
                )
                .await
                .is_err()
        );
        assert!(
            runtime
                .set_buffer_user_option(
                    buffer_id,
                    "k".repeat(MAX_USER_OPTION_KEY_LEN + 1),
                    Some("v".to_owned()),
                )
                .await
                .is_err()
        );

        // Fill to the per-buffer cap with distinct keys.
        for i in 0..MAX_USER_OPTIONS_PER_BUFFER {
            runtime
                .set_buffer_user_option(buffer_id, format!("k{i}"), Some("v".to_owned()))
                .await
                .expect("within cap");
        }
        // A brand-new key beyond the cap is rejected...
        assert!(
            runtime
                .set_buffer_user_option(buffer_id, "overflow".to_owned(), Some("v".to_owned()))
                .await
                .is_err()
        );
        // ...but updating an existing key stays allowed even at the cap...
        runtime
            .set_buffer_user_option(buffer_id, "k0".to_owned(), Some("updated".to_owned()))
            .await
            .expect("update existing key at cap");
        // ...and removing is always allowed.
        runtime
            .set_buffer_user_option(buffer_id, "k0".to_owned(), None)
            .await
            .expect("remove at cap");
    }

    #[tokio::test]
    async fn set_buffer_user_option_emits_events_only_on_change() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            state.create_buffer("shell", vec!["/bin/sh".to_owned()], None)
        };

        // Setting a new key changes the record → RenderInvalidated emitted.
        let (_, events) = runtime
            .set_buffer_user_option(buffer_id, "is-vim".to_owned(), Some("1".to_owned()))
            .await
            .expect("set new key");
        assert_eq!(events.len(), 1);

        // Setting the same key to the same value is a no-op → no events.
        let (_, events) = runtime
            .set_buffer_user_option(buffer_id, "is-vim".to_owned(), Some("1".to_owned()))
            .await
            .expect("no-op set");
        assert!(events.is_empty());

        // Changing the value emits again.
        let (_, events) = runtime
            .set_buffer_user_option(buffer_id, "is-vim".to_owned(), Some("0".to_owned()))
            .await
            .expect("change value");
        assert_eq!(events.len(), 1);

        // Removing an existing key changes the record → event.
        let (_, events) = runtime
            .set_buffer_user_option(buffer_id, "is-vim".to_owned(), None)
            .await
            .expect("remove existing key");
        assert_eq!(events.len(), 1);

        // Removing an absent key is a no-op → no events.
        let (_, events) = runtime
            .set_buffer_user_option(buffer_id, "is-vim".to_owned(), None)
            .await
            .expect("no-op remove");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn buffer_list_is_truncated_under_frame_budget() {
        let runtime = Arc::new(Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        ));

        // Give each buffer ~1 MiB of env so a handful of them together would push
        // the encoded BuffersResponse past the frame budget.
        let big_value = "x".repeat(1024 * 1024);
        let created = 10usize;
        {
            let mut state = runtime.state.lock().await;
            for i in 0..created {
                let env = BTreeMap::from([(format!("BIG{i}"), big_value.clone())]);
                state.create_buffer_with_env(
                    format!("buf{i}"),
                    vec!["/bin/sh".to_owned()],
                    None,
                    env,
                );
            }
        }

        let (response, _events) = runtime
            .dispatch_buffer(
                1,
                BufferRequest::List {
                    request_id: RequestId(1),
                    session_id: None,
                    attached_only: false,
                    detached_only: false,
                },
            )
            .await;

        match response {
            ServerResponse::Buffers(response) => {
                assert!(
                    response.buffers.len() < created,
                    "expected the list to be truncated under the budget, got {} of {created}",
                    response.buffers.len()
                );
                assert!(
                    !response.buffers.is_empty(),
                    "at least one buffer should always be returned"
                );
            }
            other => panic!("expected buffers response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn open_history_buffer_is_rejected_when_buffer_limit_reached() {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let session_id = state.create_session("alpha");
        // A helper source carries its lines inline, so opening its history does
        // not need a live PTY capture and exercises the limit guard directly.
        let base_buffer_id = state.create_buffer("base", vec!["/bin/sh".to_owned()], None);
        let source_buffer_id = state
            .create_helper_buffer(
                "history",
                base_buffer_id,
                HelperBufferScope::Full,
                None,
                vec!["line-1".to_owned()],
            )
            .expect("create source helper buffer");
        let leaf = state
            .create_buffer_view(session_id, source_buffer_id)
            .expect("create leaf");
        state
            .add_root_tab(session_id, "main", leaf)
            .expect("attach leaf");

        let buffer_count = state.buffers.len();
        let runtime = Arc::new(Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
            ResourceLimits {
                max_buffers: buffer_count,
                ..ResourceLimits::default()
            },
        ));

        let error = runtime
            .open_history_buffer(
                1,
                None,
                source_buffer_id,
                BufferHistoryScope::Full,
                BufferHistoryPlacement::Tab,
            )
            .await
            .expect_err("helper buffer creation must respect the buffer limit");

        assert!(matches!(error, MuxError::Conflict(_)), "got {error:?}");
        assert_eq!(runtime.state.lock().await.buffers.len(), buffer_count);
    }

    #[tokio::test]
    async fn clipboard_writes_reach_only_clients_showing_the_buffer_session() {
        let mut state = ServerState::new();
        let alpha = state.create_session("alpha");
        let beta = state.create_session("beta");
        let leaf = create_leaf(&mut state, alpha, "nvim");
        state
            .add_root_tab(alpha, "main", leaf)
            .expect("attach leaf");
        let attached = match state.node(leaf).expect("leaf exists") {
            crate::Node::BufferView(view) => view.buffer_id,
            other => panic!("expected buffer view, got {other:?}"),
        };
        let detached = state.create_buffer("detached", vec!["/bin/sh".to_owned()], None);
        let runtime = Runtime::new(
            state,
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );

        // Client 1 shows alpha through two subscriptions, client 2 shows beta,
        // client 3 is connected but on no session.
        let mut receivers = BTreeMap::new();
        for (connection_id, session) in [(1, Some(alpha)), (2, Some(beta)), (3, None)] {
            runtime.clients.lock().await.insert(
                connection_id,
                super::ClientConnection {
                    current_session_id: session,
                    host_graphics: false,
                    watched_buffers: std::collections::BTreeSet::new(),
                    shutdown: None,
                    stopped: None,
                },
            );
        }
        for (subscription_id, connection_id, session_id) in [
            (1, 1, None),
            (2, 1, Some(alpha)),
            (3, 2, None),
            (4, 3, None),
        ] {
            let (sender, receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
            receivers.insert(subscription_id, receiver);
            runtime.subscriptions.lock().await.insert(
                subscription_id,
                Subscription {
                    connection_id,
                    session_id,
                    sender,
                },
            );
        }

        let write = |text: &str| embers_core::ClipboardWrite {
            selection: embers_core::ClipboardSelection::Clipboard,
            text: text.to_owned(),
        };
        runtime.forward_clipboard(attached, write("yank")).await;
        runtime.forward_clipboard(detached, write("lost")).await;

        let mut received = Vec::new();
        for (subscription_id, receiver) in &mut receivers {
            while let Ok(envelope) = receiver.try_recv() {
                match envelope {
                    ServerEnvelope::Event(ServerEvent::Clipboard(event)) => {
                        received.push((*subscription_id, event.buffer_id, event.text));
                    }
                    other => panic!("unexpected envelope {other:?}"),
                }
            }
        }
        // Exactly one copy, to the client on alpha; nothing from the detached
        // buffer; nothing for the clients elsewhere.
        assert_eq!(received, vec![(1, attached, "yank".to_owned())]);
    }

    #[tokio::test]
    async fn bells_flag_only_windows_no_client_is_looking_at() {
        let mut state = ServerState::new();
        let session = state.create_session("alpha");
        let shown_leaf = create_leaf(&mut state, session, "shown");
        let hidden_leaf = create_leaf(&mut state, session, "hidden");
        state
            .add_root_tab(session, "one", shown_leaf)
            .expect("attach shown leaf");
        state
            .add_root_tab(session, "two", hidden_leaf)
            .expect("attach hidden leaf");
        state
            .focus_leaf(session, shown_leaf)
            .expect("show the first window");
        let buffer_of = |state: &ServerState, leaf| match state.node(leaf).expect("leaf exists") {
            crate::Node::BufferView(view) => view.buffer_id,
            other => panic!("expected buffer view, got {other:?}"),
        };
        let shown = buffer_of(&state, shown_leaf);
        let hidden = buffer_of(&state, hidden_leaf);
        let root = state.session(session).expect("session").root_node;
        let runtime = Runtime::new(
            state,
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let runtime = &runtime;
        let ring = |buffer_id, bells| {
            runtime.record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: bells,
                    activity: ActivityState::Bell,
                    title: None,
                    pipe: None,
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: bells,
                    user_options: None,
                },
            )
        };
        let activity = |buffer_id| async move {
            runtime
                .state
                .lock()
                .await
                .buffer(buffer_id)
                .expect("buffer exists")
                .activity
        };

        // No client shows the session: like tmux, a bell flags even the
        // current window.
        ring(shown, 1).await;
        assert_eq!(activity(shown).await, ActivityState::Bell);
        runtime
            .state
            .lock()
            .await
            .set_buffer_activity(shown, ActivityState::Idle)
            .expect("clear the bell");

        runtime.clients.lock().await.insert(
            1,
            super::ClientConnection {
                current_session_id: Some(session),
                shutdown: None,
                stopped: None,
                host_graphics: false,
                watched_buffers: std::collections::BTreeSet::new(),
            },
        );

        // A bell in the window the client shows does not flag it; one in a
        // background window flags that window.
        ring(shown, 2).await;
        ring(hidden, 1).await;
        assert_eq!(activity(shown).await, ActivityState::Idle);
        assert_eq!(activity(hidden).await, ActivityState::Bell);

        // Until the window is visited.
        runtime
            .state
            .lock()
            .await
            .switch_tab(root, 1)
            .expect("visit the second window");
        assert_eq!(activity(hidden).await, ActivityState::Idle);
    }

    #[test]
    fn visiting_a_window_clears_the_flags_of_all_its_panes() {
        let mut state = ServerState::new();
        let session = state.create_session("alpha");
        let first = create_leaf(&mut state, session, "first");
        let left = create_leaf(&mut state, session, "left");
        state
            .add_root_tab(session, "one", first)
            .expect("attach first window");
        state
            .add_root_tab(session, "two", left)
            .expect("attach second window");
        let right_buffer = state.create_buffer("right", vec!["/bin/sh".to_owned()], None);
        state
            .split_leaf_with_new_buffer(left, SplitDirection::Vertical, right_buffer)
            .expect("split the second window");
        state
            .focus_leaf(session, first)
            .expect("show the first window");
        let buffer_of = |leaf| match state.node(leaf).expect("leaf exists") {
            crate::Node::BufferView(view) => view.buffer_id,
            other => panic!("expected buffer view, got {other:?}"),
        };
        let left_buffer = buffer_of(left);
        state
            .set_buffer_activity(left_buffer, ActivityState::Bell)
            .expect("ring");
        state
            .set_buffer_activity(right_buffer, ActivityState::Activity)
            .expect("output");

        // Focusing one pane of the window acknowledges the flag of the other
        // pane too: tmux clears the window's flags, not a pane's.
        state.focus_leaf(session, left).expect("visit the window");
        let activity = |buffer_id| state.buffer(buffer_id).expect("buffer").activity;
        assert_eq!(activity(left_buffer), ActivityState::Idle);
        assert_eq!(activity(right_buffer), ActivityState::Idle);
    }

    #[tokio::test]
    async fn output_flags_activity_only_in_windows_no_client_is_looking_at() {
        let mut state = ServerState::new();
        let session = state.create_session("alpha");
        let shown_leaf = create_leaf(&mut state, session, "shown");
        let hidden_leaf = create_leaf(&mut state, session, "hidden");
        state
            .add_root_tab(session, "one", shown_leaf)
            .expect("attach shown leaf");
        state
            .add_root_tab(session, "two", hidden_leaf)
            .expect("attach hidden leaf");
        state
            .focus_leaf(session, shown_leaf)
            .expect("show the first window");
        let popup = state.create_buffer("popup", vec!["/bin/sh".to_owned()], None);
        let floating_id = state
            .create_floating_from_buffer_with_options(
                session,
                popup,
                FloatGeometry::new(0, 0, 20, 5),
                None,
                false,
                true,
            )
            .expect("open a popup");
        let buffer_of = |state: &ServerState, leaf| match state.node(leaf).expect("leaf exists") {
            crate::Node::BufferView(view) => view.buffer_id,
            other => panic!("expected buffer view, got {other:?}"),
        };
        let shown = buffer_of(&state, shown_leaf);
        let hidden = buffer_of(&state, hidden_leaf);
        let root = state.session(session).expect("session").root_node;
        let runtime = Runtime::new(
            state,
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let runtime = &runtime;
        let output = |buffer_id, sequence| {
            runtime.record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence,
                    activity: ActivityState::Activity,
                    title: None,
                    pipe: None,
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
        };
        let activity = |buffer_id| async move {
            runtime
                .state
                .lock()
                .await
                .buffer(buffer_id)
                .expect("buffer exists")
                .activity
        };

        // No client shows the session: like tmux, output flags even the
        // current window.
        output(shown, 1).await;
        assert_eq!(activity(shown).await, ActivityState::Activity);

        // A client attaching acknowledges the window it shows, as tmux clears
        // the current window's flags on attach.
        runtime.clients.lock().await.insert(
            1,
            super::ClientConnection {
                current_session_id: None,
                shutdown: None,
                stopped: None,
                host_graphics: false,
                watched_buffers: std::collections::BTreeSet::new(),
            },
        );
        runtime
            .set_client_session(1, Some(session))
            .await
            .expect("attach the client");
        assert_eq!(activity(shown).await, ActivityState::Idle);

        // Output in the window or popup the client shows does not flag them;
        // output in a background window flags that window.
        output(shown, 2).await;
        output(popup, 1).await;
        output(hidden, 1).await;
        assert_eq!(activity(shown).await, ActivityState::Idle);
        assert_eq!(activity(popup).await, ActivityState::Idle);
        assert_eq!(activity(hidden).await, ActivityState::Activity);

        // A hidden popup is not looked at.
        runtime
            .state
            .lock()
            .await
            .floating
            .get_mut(&floating_id)
            .expect("popup exists")
            .visible = false;
        output(popup, 2).await;
        assert_eq!(activity(popup).await, ActivityState::Activity);

        // Visiting the window clears its flag, and the window left behind is
        // flagged by its next output.
        runtime
            .state
            .lock()
            .await
            .switch_tab(root, 1)
            .expect("visit the second window");
        assert_eq!(activity(hidden).await, ActivityState::Idle);
        output(hidden, 2).await;
        output(shown, 3).await;
        assert_eq!(activity(hidden).await, ActivityState::Idle);
        assert_eq!(activity(shown).await, ActivityState::Activity);
    }

    #[tokio::test]
    async fn record_buffer_update_ignores_stale_sequences() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer("current-title", vec!["/bin/sh".to_owned()], None);
            let buffer = state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created");
            buffer.last_snapshot_seq = 5;
            buffer.activity = ActivityState::Activity;
            buffer_id
        };
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        runtime.subscriptions.lock().await.insert(
            1,
            Subscription {
                connection_id: 1,
                session_id: None,
                sender,
            },
        );

        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 5,
                    activity: ActivityState::Bell,
                    title: Some(Some("stale-title".to_owned())),
                    pipe: None,
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert_eq!(buffer.last_snapshot_seq, 5);
        assert_eq!(buffer.activity, ActivityState::Activity);
        assert_eq!(buffer.title, "current-title");
        assert!(receiver.try_recv().is_err());

        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 6,
                    activity: ActivityState::Bell,
                    title: Some(Some("fresh-title".to_owned())),
                    pipe: None,
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert_eq!(buffer.last_snapshot_seq, 6);
        assert_eq!(buffer.activity, ActivityState::Bell);
        assert_eq!(buffer.title, "fresh-title");
        assert!(matches!(
            receiver.try_recv(),
            Ok(ServerEnvelope::Event(ServerEvent::RenderInvalidated(event)))
                if event.buffer_id == buffer_id
        ));
    }

    #[tokio::test]
    async fn record_buffer_update_ignores_stale_pipe_updates() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer("current-title", vec!["/bin/sh".to_owned()], None);
            let buffer = state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created");
            buffer.last_snapshot_seq = 5;
            buffer_id
        };
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        runtime.subscriptions.lock().await.insert(
            1,
            Subscription {
                connection_id: 1,
                session_id: None,
                sender,
            },
        );

        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 4,
                    activity: ActivityState::Idle,
                    title: None,
                    pipe: Some(Some(BufferRuntimePipeStatus {
                        command: vec!["tee".to_owned()],
                        running: false,
                        pid: None,
                        exit_code: Some(0),
                        stop_reason: Some(BufferRuntimePipeStopReason::PipeExited),
                    })),
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert!(buffer.pipe.is_none());
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn record_buffer_update_applies_pipe_only_updates() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer("current-title", vec!["/bin/sh".to_owned()], None);
            let buffer = state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created");
            buffer.last_snapshot_seq = 5;
            buffer_id
        };
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        runtime.subscriptions.lock().await.insert(
            1,
            Subscription {
                connection_id: 1,
                session_id: None,
                sender,
            },
        );

        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 5,
                    activity: ActivityState::Idle,
                    title: None,
                    pipe: Some(Some(BufferRuntimePipeStatus {
                        command: vec!["tee".to_owned()],
                        running: false,
                        pid: None,
                        exit_code: Some(0),
                        stop_reason: Some(BufferRuntimePipeStopReason::PipeExited),
                    })),
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert!(buffer.pipe.is_some());
        assert!(matches!(
            receiver.try_recv(),
            Ok(ServerEnvelope::Event(ServerEvent::RenderInvalidated(event)))
                if event.buffer_id == buffer_id
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(ServerEnvelope::Event(ServerEvent::BufferPipeChanged(event)))
                if event.buffer.id == buffer_id
        ));
    }

    #[tokio::test]
    async fn record_buffer_update_clears_title() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer("current-title", vec!["/bin/sh".to_owned()], None);
            let buffer = state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created");
            buffer.last_snapshot_seq = 5;
            buffer_id
        };

        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 6,
                    activity: ActivityState::Idle,
                    title: Some(None),
                    pipe: None,
                    cwd: None,
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert_eq!(buffer.last_snapshot_seq, 6);
        assert_eq!(buffer.title, "");
    }

    #[tokio::test]
    async fn record_buffer_update_keeps_a_bell_that_later_output_overwrote() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            state.create_buffer("shell", vec!["/bin/sh".to_owned()], None)
        };
        let update = |sequence, bell_sequence| BufferRuntimeUpdate {
            sequence,
            activity: ActivityState::Activity,
            title: None,
            pipe: None,
            cwd: None,
            foreground_command: None,
            bell_sequence,
            user_options: None,
        };
        let activity = || async {
            runtime
                .state
                .lock()
                .await
                .buffer(buffer_id)
                .expect("buffer exists")
                .activity
        };

        // A capture synced output after the bell before the poller reported
        // it: the sequence is already applied, and the runtime's activity no
        // longer says Bell, but its bell count advanced.
        runtime.record_buffer_update(buffer_id, update(2, 0)).await;
        assert_eq!(activity().await, ActivityState::Activity);
        runtime.record_buffer_update(buffer_id, update(2, 1)).await;
        assert_eq!(activity().await, ActivityState::Bell);

        // Once focus clears it, the same bell is not applied again.
        runtime
            .state
            .lock()
            .await
            .set_buffer_activity(buffer_id, ActivityState::Idle)
            .expect("clear activity");
        runtime.record_buffer_update(buffer_id, update(2, 1)).await;
        assert_eq!(activity().await, ActivityState::Idle);
    }

    #[tokio::test]
    async fn record_buffer_update_tells_clients_about_a_new_foreground_job() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer("shell", vec!["/bin/sh".to_owned()], None);
            state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created")
                .last_snapshot_seq = 5;
            buffer_id
        };
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        runtime.subscriptions.lock().await.insert(
            1,
            Subscription {
                connection_id: 1,
                session_id: None,
                sender,
            },
        );

        // A job taking over the foreground prints nothing by itself, so the
        // update does not advance the sequence.
        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 5,
                    activity: ActivityState::Idle,
                    title: None,
                    pipe: None,
                    cwd: None,
                    foreground_command: Some(Some(vec!["nvim".to_owned()])),
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let record = crate::protocol::buffer_record(
            runtime
                .state
                .lock()
                .await
                .buffer(buffer_id)
                .expect("buffer exists"),
        );
        assert_eq!(record.foreground_command, Some(vec!["nvim".to_owned()]));
        assert!(matches!(
            receiver.try_recv(),
            Ok(ServerEnvelope::Event(ServerEvent::RenderInvalidated(event)))
                if event.buffer_id == buffer_id
        ));
    }

    #[tokio::test]
    async fn record_buffer_update_keeps_known_cwd_when_runtime_reports_none() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );
        let buffer_id = {
            let mut state = runtime.state.lock().await;
            let buffer_id = state.create_buffer(
                "shell",
                vec!["/bin/sh".to_owned()],
                Some(PathBuf::from("/home/user")),
            );
            state
                .buffers
                .get_mut(&buffer_id)
                .expect("buffer is created")
                .last_snapshot_seq = 5;
            buffer_id
        };

        // The runtime couldn't resolve a live cwd (no OSC 7 yet, pid lookup
        // failed, etc.). That is "unknown", not "cleared", so the spawn cwd must
        // survive rather than being wiped to None.
        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 6,
                    activity: ActivityState::Idle,
                    title: None,
                    pipe: None,
                    cwd: Some(None),
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;

        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert_eq!(buffer.cwd, Some(PathBuf::from("/home/user")));

        // A resolved directory still updates it.
        runtime
            .record_buffer_update(
                buffer_id,
                BufferRuntimeUpdate {
                    sequence: 6,
                    activity: ActivityState::Idle,
                    title: None,
                    pipe: None,
                    cwd: Some(Some(PathBuf::from("/tmp/work"))),
                    foreground_command: None,
                    bell_sequence: 0,
                    user_options: None,
                },
            )
            .await;
        let buffer = runtime
            .state
            .lock()
            .await
            .buffer(buffer_id)
            .expect("buffer exists")
            .clone();
        assert_eq!(buffer.cwd, Some(PathBuf::from("/tmp/work")));
    }

    #[tokio::test]
    async fn open_history_for_detached_buffers_requires_attached_session_before_capture() {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let buffer_id = state.create_buffer("detached", vec!["/bin/sh".to_owned()], None);
        let runtime = Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );

        let error = runtime
            .open_history_buffer(
                1,
                None,
                buffer_id,
                BufferHistoryScope::Visible,
                BufferHistoryPlacement::Tab,
            )
            .await
            .expect_err("detached history requires an attached client session");

        assert!(matches!(
            error,
            MuxError::Conflict(message)
                if message == "history for detached buffers requires an attached client session"
        ));
    }

    #[tokio::test]
    async fn resizing_helper_buffers_updates_visible_snapshot_viewport() {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let source_buffer_id = state.create_buffer("source", vec!["/bin/sh".to_owned()], None);
        let helper_buffer_id = state
            .create_helper_buffer(
                "source history",
                source_buffer_id,
                HelperBufferScope::Visible,
                None,
                vec![
                    "line-1".to_owned(),
                    "line-2".to_owned(),
                    "line-3".to_owned(),
                    "line-4".to_owned(),
                    "line-5".to_owned(),
                ],
            )
            .expect("create helper buffer");
        let runtime = Arc::new(Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        ));

        let (response, events) = runtime
            .dispatch_input(InputRequest::Resize {
                request_id: RequestId(1),
                buffer_id: helper_buffer_id,
                cols: 80,
                rows: 3,
                pixel_width: 0,
                pixel_height: 0,
            })
            .await;

        assert!(matches!(
            response,
            ServerResponse::Ok(response) if response.request_id == RequestId(1)
        ));
        assert!(matches!(
            events.as_slice(),
            [ServerEvent::RenderInvalidated(event)] if event.buffer_id == helper_buffer_id
        ));

        let snapshot = runtime
            .capture_visible_snapshot(RequestId(2), helper_buffer_id)
            .await
            .expect("capture visible snapshot succeeds");

        assert_eq!(snapshot.size.rows, 3);
        assert_eq!(snapshot.total_lines, 5);
        assert_eq!(snapshot.viewport_top_line, 2);
        let snapshot_text: Vec<_> = snapshot
            .lines
            .iter()
            .map(|line| line.text.clone())
            .collect();
        assert_eq!(
            snapshot_text,
            vec![
                "line-3".to_owned(),
                "line-4".to_owned(),
                "line-5".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn join_buffer_at_node_emits_floating_changed_for_existing_floating() {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let session_id = state.create_session("alpha");
        let root_buffer_id = state.create_buffer("root", vec!["/bin/sh".to_owned()], None);
        let root_leaf = state
            .create_buffer_view(session_id, root_buffer_id)
            .expect("create root leaf");
        state
            .add_root_tab(session_id, "main", root_leaf)
            .expect("attach root leaf");

        let popup_buffer_id = state.create_buffer("popup", vec!["/bin/sh".to_owned()], None);
        let popup_leaf = state
            .create_buffer_view(session_id, popup_buffer_id)
            .expect("create popup leaf");
        let floating_id = state
            .create_floating_window_with_options(
                session_id,
                popup_leaf,
                FloatGeometry::new(4, 4, 20, 10),
                Some("popup".to_owned()),
                false,
                true,
            )
            .expect("create floating");

        let detached_buffer_id = state.create_buffer("logs", vec!["/bin/sh".to_owned()], None);
        let runtime = Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
            ResourceLimits::default(),
        );

        let (response, events) = runtime
            .dispatch_node(NodeRequest::JoinBufferAtNode {
                request_id: RequestId(1),
                node_id: popup_leaf,
                buffer_id: detached_buffer_id,
                placement: NodeJoinPlacement::Right,
            })
            .await;

        assert!(matches!(response, ServerResponse::Ok(_)));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::FloatingChanged(changed)
                if changed.session_id == session_id
                        && changed.floating_id == Some(floating_id)
            )
        }));
    }

    #[tokio::test]
    async fn swap_siblings_emits_floating_changed_for_existing_floating() {
        let (runtime, session_id, floating_id, split_left, split_right) = floating_split_runtime();

        let (response, events) = runtime
            .dispatch_node(NodeRequest::SwapSiblings {
                request_id: RequestId(1),
                first_node_id: split_left,
                second_node_id: split_right,
            })
            .await;

        assert!(matches!(response, ServerResponse::Ok(_)));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::FloatingChanged(changed)
                    if changed.session_id == session_id
                        && changed.floating_id == Some(floating_id)
            )
        }));
    }

    #[tokio::test]
    async fn move_node_after_emits_floating_changed_for_existing_floating() {
        let (runtime, session_id, floating_id, split_left, split_right) = floating_split_runtime();

        let (response, events) = runtime
            .dispatch_node(NodeRequest::MoveNodeAfter {
                request_id: RequestId(1),
                node_id: split_left,
                sibling_node_id: split_right,
            })
            .await;

        assert!(matches!(response, ServerResponse::Ok(_)));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::FloatingChanged(changed)
                    if changed.session_id == session_id
                        && changed.floating_id == Some(floating_id)
            )
        }));
    }

    #[tokio::test]
    async fn break_node_emits_floating_changed_when_node_stays_in_same_floating() {
        let (runtime, session_id, floating_id, split_right) = hidden_floating_tabs_runtime();

        let (response, events) = runtime
            .dispatch_node(NodeRequest::BreakNode {
                request_id: RequestId(1),
                node_id: split_right,
                destination: NodeBreakDestination::Tab,
            })
            .await;

        assert!(matches!(response, ServerResponse::Ok(_)));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::FloatingChanged(changed)
                    if changed.session_id == session_id
                        && changed.floating_id == Some(floating_id)
            )
        }));
    }
}
