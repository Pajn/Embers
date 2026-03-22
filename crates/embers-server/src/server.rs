use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use embers_core::{
    BufferId, ErrorCode, MuxError, PtySize, RequestId, Result, WireError, request_span,
};
use embers_protocol::{
    BufferCreatedEvent, BufferDetachedEvent, BufferRequest, BufferResponse, BuffersResponse,
    ClientChangedEvent, ClientMessage, ClientRecord, ClientRequest, ClientResponse,
    ClientsResponse, ErrorResponse, FloatingChangedEvent, FloatingRequest, FloatingResponse,
    FocusChangedEvent, FrameType, InputRequest, NodeChangedEvent, OkResponse, PingResponse,
    ProtocolError, RawFrame, RenderInvalidatedEvent, ScrollbackSliceResponse, ServerEnvelope,
    ServerEvent, ServerResponse, SessionClosedEvent, SessionCreatedEvent, SessionRenamedEvent,
    SessionRequest, SessionSnapshotResponse, SessionsResponse, SnapshotResponse,
    SubscriptionAckResponse, VisibleSnapshotResponse, decode_client_message,
    encode_server_envelope, read_frame, write_frame_no_flush,
};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::persist::{load_workspace, save_workspace};
use crate::protocol::{buffer_record, floating_record, session_record, session_snapshot};
use crate::{
    BufferAttachment, BufferRuntimeCallbacks, BufferRuntimeHandle, BufferRuntimeStatus,
    BufferRuntimeUpdate, BufferState, ServerConfig, ServerState, TabEntry,
};

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

        let restored_state = load_workspace(&self.config.workspace_path)?;
        let socket_path = self.config.socket_path.clone();
        let runtime = Arc::new(Runtime::new(
            restored_state.unwrap_or_default(),
            self.config.socket_path.clone(),
            self.config.workspace_path.clone(),
            self.config.runtime_dir.clone(),
            self.config.buffer_env.clone(),
        ));
        runtime.restore_buffer_runtimes().await?;
        let listener = UnixListener::bind(&self.config.socket_path)?;
        set_socket_permissions(&self.config.socket_path)?;
        let shutdown_signal = runtime.shutdown.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            let _cleanup = SocketCleanup::new(socket_path.clone());
            info!(socket_path = %socket_path.display(), "mux server listening");

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        debug!("server shutdown requested");
                        break;
                    }
                    result = listener.accept() => {
                        let (stream, _) = result?;
                        let connection_id = runtime.next_connection_id.fetch_add(1, Ordering::Relaxed);
                        let (reader, writer) = stream.into_split();
                        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
                        let (shutdown_tx, shutdown_rx) = oneshot::channel();
                        let (stopped_tx, stopped_rx) = oneshot::channel();
                        runtime
                            .register_client(connection_id, shutdown_tx, stopped_rx)
                            .await;

                        let write_runtime = runtime.clone();
                        let write_handle = tokio::spawn(write_loop(writer, outbound_rx));
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

                            match exit {
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
            runtime.shutdown_runtimes().await;
            runtime.quiesce_state_tasks().await;
            if let Err(error) = runtime.persist_workspace().await {
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
    sender: mpsc::UnboundedSender<ServerEnvelope>,
}

struct ClientConnection {
    current_session_id: Option<embers_core::SessionId>,
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
    buffer_runtimes: Mutex<BTreeMap<BufferId, BufferRuntimeHandle>>,
    buffer_shutdown_intents: StdMutex<BTreeSet<BufferId>>,
    socket_path: PathBuf,
    workspace_path: PathBuf,
    runtime_dir: PathBuf,
    buffer_env: BTreeMap<String, OsString>,
    subscriptions: Mutex<BTreeMap<u64, Subscription>>,
    clients: Mutex<BTreeMap<u64, ClientConnection>>,
    next_connection_id: AtomicU64,
    next_subscription_id: AtomicU64,
    shutdown: ShutdownSignal,
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
    ) -> Self {
        Self {
            state: Mutex::new(state),
            buffer_runtimes: Mutex::new(BTreeMap::new()),
            buffer_shutdown_intents: StdMutex::new(BTreeSet::new()),
            socket_path,
            workspace_path,
            runtime_dir,
            buffer_env,
            subscriptions: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
            next_connection_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
            shutdown: ShutdownSignal::new(),
            connection_tasks: TaskCounter::default(),
            state_tasks: TaskCounter::default(),
        }
    }
}

impl Runtime {
    async fn persist_workspace(&self) -> Result<()> {
        let state = self.state.lock().await;
        save_workspace(&self.workspace_path, &state)
    }

    async fn quiesce_state_tasks(&self) {
        self.connection_tasks.wait_for_idle().await;
        self.state_tasks.wait_for_idle().await;
    }

    fn take_buffer_shutdown_intent(&self, buffer_id: BufferId) -> bool {
        self.buffer_shutdown_intents
            .lock()
            .expect("buffer shutdown intent lock")
            .remove(&buffer_id)
    }

    fn runtime_socket_path(&self, buffer_id: BufferId) -> Result<PathBuf> {
        let path = self
            .runtime_dir
            .join(format!("buffer-{}.sock", buffer_id.0));
        validate_keeper_socket_path(&self.socket_path, &path)?;
        Ok(path)
    }

    fn buffer_runtime_callbacks(self: &Arc<Self>) -> BufferRuntimeCallbacks {
        let output_handle = tokio::runtime::Handle::current();
        let exit_handle = output_handle.clone();
        let output_runtime = self.clone();
        let exit_runtime = self.clone();
        let output_tasks = self.state_tasks.clone();
        let exit_tasks = self.state_tasks.clone();

        BufferRuntimeCallbacks {
            on_output: Arc::new(move |buffer_id, update| {
                let runtime = output_runtime.clone();
                let task = output_tasks.enter();
                std::mem::drop(output_handle.spawn(async move {
                    let _task = task;
                    runtime.record_buffer_update(buffer_id, update).await;
                }));
            }),
            on_exit: Arc::new(move |buffer_id, exit_code| {
                let runtime = exit_runtime.clone();
                let task = exit_tasks.enter();
                std::mem::drop(exit_handle.spawn(async move {
                    let _task = task;
                    runtime.record_buffer_exit(buffer_id, exit_code).await;
                }));
            }),
        }
    }

    async fn restore_buffer_runtimes(self: &Arc<Self>) -> Result<()> {
        let buffers = {
            let state = self.state.lock().await;
            state.buffers.values().cloned().collect::<Vec<_>>()
        };

        for buffer in buffers {
            let Some(socket_path) = buffer.runtime_socket_path().cloned() else {
                if matches!(buffer.state, BufferState::Running(_) | BufferState::Created) {
                    let mut state = self.state.lock().await;
                    let _ =
                        state.mark_buffer_interrupted(buffer.id, buffer_pid_hint(&buffer.state));
                }
                continue;
            };
            if !socket_path.exists() {
                debug!(
                    %buffer.id,
                    socket_path = %socket_path.display(),
                    "skipping runtime restore because keeper socket is missing"
                );
                let mut state = self.state.lock().await;
                let _ = state.set_buffer_runtime_socket_path(buffer.id, None);
                let _ = state.mark_buffer_interrupted(buffer.id, buffer_pid_hint(&buffer.state));
                continue;
            }

            match self
                .attach_buffer_runtime(buffer.id, socket_path.clone())
                .await
            {
                Ok((runtime, status)) => {
                    let mut state = self.state.lock().await;
                    let _ =
                        state.set_buffer_runtime_socket_path(buffer.id, Some(socket_path.clone()));
                    apply_runtime_status(&mut state, buffer.id, &status);
                    drop(state);
                    self.buffer_runtimes.lock().await.insert(buffer.id, runtime);
                }
                Err(error) => {
                    debug!(
                        %buffer.id,
                        socket_path = %socket_path.display(),
                        %error,
                        "failed to restore buffer runtime"
                    );
                    let mut state = self.state.lock().await;
                    let _ = state.set_buffer_runtime_socket_path(buffer.id, None);
                    let _ =
                        state.mark_buffer_interrupted(buffer.id, buffer_pid_hint(&buffer.state));
                }
            }
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
                shutdown: Some(shutdown),
                stopped: Some(stopped),
            },
        );
    }

    async fn dispatch_request(
        self: &Arc<Self>,
        connection_id: u64,
        outbound: &mpsc::UnboundedSender<ServerEnvelope>,
        request: ClientMessage,
    ) -> (
        ServerResponse,
        Vec<ServerEvent>,
        Option<oneshot::Sender<()>>,
    ) {
        match request {
            ClientMessage::Ping(request) => (
                ServerResponse::Pong(PingResponse {
                    request_id: request.request_id,
                    payload: request.payload,
                }),
                Vec::new(),
                None,
            ),
            ClientMessage::Session(request) => {
                let (resp, events) = self.dispatch_session(request).await;
                (resp, events, None)
            }
            ClientMessage::Buffer(request) => {
                let (resp, events) = self.dispatch_buffer(request).await;
                (resp, events, None)
            }
            ClientMessage::Node(request) => {
                let (resp, events) = self.dispatch_node(request).await;
                (resp, events, None)
            }
            ClientMessage::Floating(request) => {
                let (resp, events) = self.dispatch_floating(request).await;
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
            ClientRequest::Switch {
                request_id,
                client_id,
                session_id,
            } => {
                let target_id = client_id.map(|id| id.get()).unwrap_or(connection_id);
                match self.set_client_session(target_id, Some(session_id)).await {
                    Ok((client, event)) => (
                        ServerResponse::Client(ClientResponse { request_id, client }),
                        vec![event],
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
        }
    }

    async fn dispatch_buffer(
        self: &Arc<Self>,
        request: BufferRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        match request {
            BufferRequest::Create {
                request_id,
                title,
                command,
                cwd,
                env,
            } => {
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
                    state.create_buffer_with_env(
                        title.unwrap_or_else(|| "buffer".to_owned()),
                        command,
                        cwd.map(Into::into),
                        env,
                    )
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
                let buffers = state
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
                    .map(buffer_record)
                    .collect();

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
            } => match self.buffer_runtime(buffer_id).await {
                Ok(runtime) => match runtime.kill().await {
                    Ok(()) => (ServerResponse::Ok(OkResponse { request_id }), Vec::new()),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                },
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
            } => match self.buffer_runtime(buffer_id).await {
                Ok(runtime) => match runtime.write(bytes).await {
                    Ok(()) => (ServerResponse::Ok(OkResponse { request_id }), Vec::new()),
                    Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
                },
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
            InputRequest::Resize {
                request_id,
                buffer_id,
                cols,
                rows,
            } => {
                let runtime = match self.buffer_runtime(buffer_id).await {
                    Ok(runtime) => runtime,
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                let size = {
                    let state = self.state.lock().await;
                    match state.buffer(buffer_id) {
                        Ok(buffer) => PtySize {
                            cols,
                            rows,
                            pixel_width: buffer.pty_size.pixel_width,
                            pixel_height: buffer.pty_size.pixel_height,
                        },
                        Err(error) => {
                            return (mux_error_response(Some(request_id), error), Vec::new());
                        }
                    }
                };

                if let Err(error) = runtime.resize(size).await {
                    return (mux_error_response(Some(request_id), error), Vec::new());
                }

                {
                    let mut state = self.state.lock().await;
                    if let Err(error) = state.set_buffer_size(buffer_id, size) {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                }

                (
                    ServerResponse::Ok(OkResponse { request_id }),
                    vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                        buffer_id,
                    })],
                )
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
            } {
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
        let (command, cwd, size, env_hints) = {
            let state = self.state.lock().await;
            let buffer = state.buffer(buffer_id)?.clone();
            (buffer.command, buffer.cwd, buffer.pty_size, buffer.env)
        };

        let mut buffer_env = self.buffer_env.clone();
        for (key, value) in env_hints {
            buffer_env.insert(key, OsString::from(value));
        }
        let runtime = BufferRuntimeHandle::spawn(
            buffer_id,
            self.runtime_socket_path(buffer_id)?,
            &command,
            cwd.as_deref(),
            &buffer_env,
            size,
            self.buffer_runtime_callbacks(),
        )
        .await?;
        let status = runtime.status().await?;

        {
            let mut state = self.state.lock().await;
            if let Err(error) = state.mark_buffer_running(buffer_id, status.pid) {
                let _ = runtime.kill().await;
                let _ = runtime.join_threads().await;
                return Err(error);
            }
            state.set_buffer_runtime_socket_path(
                buffer_id,
                Some(runtime.socket_path().to_path_buf()),
            )?;
            apply_runtime_status(&mut state, buffer_id, &status);
        }

        self.buffer_runtimes.lock().await.insert(buffer_id, runtime);
        Ok(())
    }

    async fn attach_buffer_runtime(
        self: &Arc<Self>,
        buffer_id: BufferId,
        socket_path: PathBuf,
    ) -> Result<(BufferRuntimeHandle, BufferRuntimeStatus)> {
        let runtime =
            BufferRuntimeHandle::attach(buffer_id, socket_path, self.buffer_runtime_callbacks())
                .await?;
        let status = runtime.status().await?;
        Ok((runtime, status))
    }

    async fn buffer_runtime(&self, buffer_id: BufferId) -> Result<BufferRuntimeHandle> {
        if let Some(runtime) = self.buffer_runtimes.lock().await.get(&buffer_id).cloned() {
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

    async fn capture_snapshot(
        &self,
        request_id: RequestId,
        buffer_id: BufferId,
    ) -> Result<SnapshotResponse> {
        let buffer = {
            let state = self.state.lock().await;
            state.buffer(buffer_id)?.clone()
        };
        let runtime = self.buffer_runtime(buffer_id).await?;
        let snapshot = runtime.capture_snapshot(buffer.cwd.clone()).await?;
        self.sync_buffer_runtime_status(buffer_id, &runtime).await?;

        Ok(SnapshotResponse {
            request_id,
            buffer_id,
            sequence: snapshot.sequence,
            size: snapshot.size,
            lines: snapshot.lines,
            title: snapshot.title.or(Some(buffer.title)),
            cwd: buffer.cwd.map(|path| path.display().to_string()),
        })
    }

    async fn capture_visible_snapshot(
        &self,
        request_id: RequestId,
        buffer_id: BufferId,
    ) -> Result<VisibleSnapshotResponse> {
        let buffer = {
            let state = self.state.lock().await;
            state.buffer(buffer_id)?.clone()
        };
        let runtime = self.buffer_runtime(buffer_id).await?;
        let snapshot = runtime.capture_visible_snapshot(buffer.cwd.clone()).await?;
        self.sync_buffer_runtime_status(buffer_id, &runtime).await?;

        Ok(VisibleSnapshotResponse {
            request_id,
            buffer_id,
            sequence: snapshot.sequence,
            size: snapshot.size,
            lines: snapshot.lines.into_iter().map(|line| line.text).collect(),
            title: snapshot.title,
            cwd: snapshot.cwd.map(|path| path.display().to_string()),
            viewport_top_line: snapshot.viewport_top_line,
            total_lines: snapshot.total_lines,
            alternate_screen: snapshot.modes.alternate_screen,
            mouse_reporting: snapshot.modes.mouse_reporting,
            focus_reporting: snapshot.modes.focus_reporting,
            bracketed_paste: snapshot.modes.bracketed_paste,
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
            lines: slice.lines,
        })
    }

    async fn record_buffer_update(&self, buffer_id: BufferId, update: BufferRuntimeUpdate) {
        let updated = {
            let mut state = self.state.lock().await;
            let Some(buffer) = state.buffers.get_mut(&buffer_id) else {
                return;
            };
            if update.sequence <= buffer.last_snapshot_seq {
                false
            } else {
                buffer.last_snapshot_seq = update.sequence;
                buffer.activity = update.activity;
                if let Some(title) = update.title {
                    match title {
                        Some(title) => buffer.title = title,
                        None => buffer.title.clear(),
                    }
                }
                true
            }
        };

        if updated {
            self.broadcast(
                vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                    buffer_id,
                })],
                &[],
            )
            .await;
        }
    }

    async fn record_buffer_exit(&self, buffer_id: BufferId, exit_code: Option<i32>) {
        let should_interrupt = self.take_buffer_shutdown_intent(buffer_id);
        if should_interrupt {
            let runtime = self.buffer_runtimes.lock().await.remove(&buffer_id);
            drop(runtime);
        }
        let updated = {
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
                Ok(()) => true,
                Err(error) => {
                    debug!(%buffer_id, %error, "buffer exited after state cleanup");
                    false
                }
            }
        };

        if updated {
            self.broadcast(
                vec![ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
                    buffer_id,
                })],
                &[],
            )
            .await;
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
            },
        )
        .await;
        if !status.running {
            self.record_buffer_exit(buffer_id, status.exit_code).await;
        }
        Ok(())
    }

    async fn shutdown_runtimes(&self) {
        let runtimes: Vec<_> = {
            let runtimes = self.buffer_runtimes.lock().await;
            let mut shutdown_intents = self
                .buffer_shutdown_intents
                .lock()
                .expect("buffer shutdown intent lock");
            runtimes
                .iter()
                .map(|(&buffer_id, runtime)| {
                    shutdown_intents.insert(buffer_id);
                    runtime.clone()
                })
                .collect()
        };
        for runtime in runtimes {
            if let Err(error) = runtime.join_threads().await {
                debug!(%error, "failed to join buffer runtime threads during shutdown");
            }
        }
        self.buffer_runtimes.lock().await.clear();
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

                    if event_matches
                        && subscription
                            .sender
                            .send(ServerEnvelope::Event(event.clone()))
                            .is_err()
                    {
                        return false;
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
    ) -> Result<(ClientRecord, ServerEvent)> {
        let previous_session_id = {
            let state = self.state.lock().await;
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
            previous
        };
        let record = self
            .client_record(client_id)
            .await
            .ok_or_else(|| MuxError::not_found(format!("client {client_id} was not found")))?;
        let event = ServerEvent::ClientChanged(ClientChangedEvent {
            client: record.clone(),
            previous_session_id,
        });
        Ok((record, event))
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
        self.clients.lock().await.remove(&connection_id);
        self.subscriptions
            .lock()
            .await
            .retain(|_, subscription| subscription.connection_id != connection_id);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionExit {
    Closed,
    SelfDetached,
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
    outbound: mpsc::UnboundedSender<ServerEnvelope>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<ConnectionExit> {
    let mut server_shutdown = runtime.shutdown.subscribe();
    loop {
        let frame = tokio::select! {
            _ = wait_for_shutdown(&mut server_shutdown) => {
                return Ok(ConnectionExit::Closed);
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
                    .is_err()
                {
                    return Err(MuxError::transport("connection writer closed"));
                }
                continue;
            }
        };

        let span = request_span("handle_request", request.request_id());
        let _entered = span.enter();
        let (response, events, deferred_shutdown) = runtime
            .dispatch_request(connection_id, &outbound, request)
            .await;

        if outbound.send(ServerEnvelope::Response(response)).is_err() {
            return Err(MuxError::transport("connection writer closed"));
        }
        let retired_session_ids = closed_session_ids(&events);
        runtime.broadcast(events, &retired_session_ids).await;

        // Handle self-detach: trigger shutdown after response is sent
        if let Some(shutdown) = deferred_shutdown {
            let _ = shutdown.send(());
            return Ok(ConnectionExit::SelfDetached);
        }
    }
}

async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut outbound: mpsc::UnboundedReceiver<ServerEnvelope>,
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

/// Maximum Unix-domain socket path length in bytes for runtime keeper sockets.
/// These values come from `sockaddr_un.sun_path`: macOS exposes 104 bytes per
/// `unix(4)`, while other Unix/Linux platforms expose 108 bytes per `unix(7)`.
/// `validate_keeper_socket_path` uses this limit to validate keeper socket
/// paths derived from the server socket path before binding.
#[cfg(target_os = "macos")]
const UNIX_SOCKET_PATH_LIMIT: usize = 104;
/// Maximum Unix-domain socket path length in bytes for runtime keeper sockets.
/// These values come from `sockaddr_un.sun_path`: macOS exposes 104 bytes per
/// `unix(4)`, while other Unix/Linux platforms expose 108 bytes per `unix(7)`.
/// `validate_keeper_socket_path` uses this limit to validate keeper socket
/// paths derived from the server socket path before binding.
#[cfg(all(unix, not(target_os = "macos")))]
const UNIX_SOCKET_PATH_LIMIT: usize = 108;

fn validate_keeper_socket_path(server_socket_path: &Path, keeper_socket_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let len = keeper_socket_path.as_os_str().as_bytes().len();
        if len > UNIX_SOCKET_PATH_LIMIT {
            return Err(MuxError::invalid_input(format!(
                "runtime keeper socket path is too long ({len} bytes, max {UNIX_SOCKET_PATH_LIMIT}): {} (runtime_dir derived from server socket {}). Use a shorter server socket path.",
                keeper_socket_path.display(),
                server_socket_path.display(),
            )));
        }
    }
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

fn layout_snapshot_response(
    state: &ServerState,
    request_id: RequestId,
    session_id: embers_core::SessionId,
) -> (ServerResponse, Vec<ServerEvent>) {
    match session_snapshot(state, session_id) {
        Ok(snapshot) => {
            let mut events = vec![ServerEvent::NodeChanged(NodeChangedEvent { session_id })];
            if let Some(focus_event) = focus_changed_event(state, session_id) {
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

fn apply_runtime_status(
    state: &mut ServerState,
    buffer_id: BufferId,
    status: &BufferRuntimeStatus,
) {
    if let Some(buffer) = state.buffers.get_mut(&buffer_id) {
        buffer.last_snapshot_seq = status.sequence;
    }
    if let Some(title) = &status.title {
        let _ = state.set_buffer_title(buffer_id, title.clone());
    }
    let _ = state.set_buffer_activity(buffer_id, status.activity);
    if status.running {
        let _ = state.mark_buffer_running(buffer_id, status.pid);
    } else {
        let _ = state.mark_buffer_exited(buffer_id, status.exit_code);
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
    #[cfg(unix)]
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::PathBuf;
    use std::sync::Arc;

    use embers_core::ActivityState;
    use embers_protocol::{ServerEnvelope, ServerEvent};
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::{Runtime, ShutdownSignal, Subscription, wait_for_shutdown};
    use crate::{BufferRuntimeUpdate, BufferState, ServerState};

    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn shutdown_signal_is_latched_for_new_receivers() {
        let signal = ShutdownSignal::new();
        signal.trigger();
        let mut shutdown = signal.subscribe();

        timeout(Duration::from_millis(50), wait_for_shutdown(&mut shutdown))
            .await
            .expect("latched shutdown should resolve immediately");
    }

    #[test]
    fn buffer_shutdown_intents_are_consumed_per_buffer() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
        );
        runtime
            .buffer_shutdown_intents
            .lock()
            .expect("buffer shutdown intent lock")
            .insert(embers_core::BufferId(1));

        assert!(runtime.take_buffer_shutdown_intent(embers_core::BufferId(1)));
        assert!(!runtime.take_buffer_shutdown_intent(embers_core::BufferId(1)));
        assert!(!runtime.take_buffer_shutdown_intent(embers_core::BufferId(2)));
    }

    #[tokio::test]
    async fn record_buffer_update_ignores_stale_sequences() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
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
        let (sender, mut receiver) = mpsc::unbounded_channel();
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
    async fn record_buffer_update_clears_title() {
        let runtime = Runtime::new(
            ServerState::new(),
            PathBuf::from("server.sock"),
            PathBuf::from("workspace"),
            PathBuf::from("runtime"),
            BTreeMap::new(),
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
    async fn restore_buffer_runtimes_clears_missing_socket_paths() {
        let tempdir = tempdir().expect("tempdir");
        let mut state = ServerState::new();
        let buffer_id = state.create_buffer("buffer", vec!["/bin/sh".to_owned()], None);
        state
            .mark_buffer_running(buffer_id, Some(42))
            .expect("mark running");
        state
            .set_buffer_runtime_socket_path(
                buffer_id,
                Some(tempdir.path().join("missing-runtime.sock")),
            )
            .expect("set runtime socket path");

        let runtime = Arc::new(Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
        ));

        runtime
            .restore_buffer_runtimes()
            .await
            .expect("restore succeeds");

        let state = runtime.state.lock().await;
        let buffer = state.buffer(buffer_id).expect("buffer exists");
        assert!(matches!(buffer.state, BufferState::Interrupted(_)));
        assert_eq!(buffer.runtime_socket_path(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_buffer_runtimes_clears_unreachable_socket_paths() {
        let tempdir = tempdir().expect("tempdir");
        let socket_path = tempdir.path().join("stale-runtime.sock");
        let listener = StdUnixListener::bind(&socket_path).expect("bind stale socket");
        drop(listener);

        let mut state = ServerState::new();
        let buffer_id = state.create_buffer("buffer", vec!["/bin/sh".to_owned()], None);
        state
            .mark_buffer_running(buffer_id, Some(42))
            .expect("mark running");
        state
            .set_buffer_runtime_socket_path(buffer_id, Some(socket_path.clone()))
            .expect("set runtime socket path");

        let runtime = Arc::new(Runtime::new(
            state,
            tempdir.path().join("server.sock"),
            tempdir.path().join("workspace.json"),
            tempdir.path().join("runtime"),
            BTreeMap::new(),
        ));

        runtime
            .restore_buffer_runtimes()
            .await
            .expect("restore succeeds");

        let state = runtime.state.lock().await;
        let buffer = state.buffer(buffer_id).expect("buffer exists");
        assert!(matches!(buffer.state, BufferState::Interrupted(_)));
        assert_eq!(buffer.runtime_socket_path(), None);
    }
}
