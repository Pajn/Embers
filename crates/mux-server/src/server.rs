use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mux_core::{ErrorCode, MuxError, RequestId, Result, WireError, request_span};
use mux_protocol::{
    BufferCreatedEvent, BufferDetachedEvent, BufferRequest, BufferResponse, BuffersResponse,
    ClientMessage, ErrorResponse, FloatingChangedEvent, FloatingRequest, FloatingResponse,
    FocusChangedEvent, FrameType, NodeChangedEvent, OkResponse, PingResponse, ProtocolError,
    RawFrame, ServerEnvelope, ServerEvent, ServerResponse, SessionClosedEvent, SessionCreatedEvent,
    SessionRequest, SessionSnapshotResponse, SessionsResponse, SubscriptionAckResponse,
    decode_client_message, encode_server_envelope, read_frame, write_frame,
};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::protocol::{buffer_record, floating_record, session_record, session_snapshot};
use crate::{BufferAttachment, ServerConfig, ServerState};

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

        let listener = UnixListener::bind(&self.config.socket_path)?;
        let socket_path = self.config.socket_path.clone();
        let runtime = Arc::new(Runtime::default());
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

                        tokio::spawn(write_loop(writer, outbound_rx));

                        let runtime = runtime.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_connection(runtime, connection_id, reader, outbound_tx).await {
                                error!(%error, connection_id, "connection failed");
                            }
                        });
                    }
                }
            }

            Ok(())
        });

        Ok(ServerHandle {
            socket_path: self.config.socket_path,
            shutdown: Some(shutdown_tx),
            join,
        })
    }
}

#[derive(Debug)]
pub struct ServerHandle {
    socket_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    join: JoinHandle<Result<()>>,
}

impl ServerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn shutdown(mut self) -> Result<()> {
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

#[derive(Debug)]
struct Subscription {
    connection_id: u64,
    session_id: Option<mux_core::SessionId>,
    sender: mpsc::UnboundedSender<ServerEnvelope>,
}

#[derive(Debug)]
struct Runtime {
    state: Mutex<ServerState>,
    subscriptions: Mutex<BTreeMap<u64, Subscription>>,
    next_connection_id: AtomicU64,
    next_subscription_id: AtomicU64,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            state: Mutex::new(ServerState::new()),
            subscriptions: Mutex::new(BTreeMap::new()),
            next_connection_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
        }
    }
}

impl Runtime {
    async fn dispatch_request(
        &self,
        connection_id: u64,
        outbound: &mpsc::UnboundedSender<ServerEnvelope>,
        request: ClientMessage,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        match request {
            ClientMessage::Ping(request) => (
                ServerResponse::Pong(PingResponse {
                    request_id: request.request_id,
                    payload: request.payload,
                }),
                Vec::new(),
            ),
            ClientMessage::Session(request) => self.dispatch_session(request).await,
            ClientMessage::Buffer(request) => self.dispatch_buffer(request).await,
            ClientMessage::Node(request) => self.dispatch_node(request).await,
            ClientMessage::Floating(request) => self.dispatch_floating(request).await,
            ClientMessage::Input(request) => (
                unsupported_response(request.request_id(), "input requests are not available yet"),
                Vec::new(),
            ),
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
                    ),
                    None => (
                        error_response(
                            Some(request.request_id),
                            ErrorCode::NotFound,
                            format!("unknown subscription {}", request.subscription_id),
                        ),
                        Vec::new(),
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
            } => match state.close_session(session_id) {
                Ok(()) => (
                    ServerResponse::Ok(OkResponse { request_id }),
                    vec![ServerEvent::SessionClosed(SessionClosedEvent {
                        session_id,
                    })],
                ),
                Err(error) => (mux_error_response(Some(request_id), error), Vec::new()),
            },
        }
    }

    async fn dispatch_buffer(&self, request: BufferRequest) -> (ServerResponse, Vec<ServerEvent>) {
        let mut state = self.state.lock().await;

        match request {
            BufferRequest::Create {
                request_id,
                title,
                command,
                cwd,
            } => {
                let buffer_id = state.create_buffer(
                    title.unwrap_or_else(|| "buffer".to_owned()),
                    command,
                    cwd.map(Into::into),
                );
                let record = state
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
            } => match state.buffer(buffer_id) {
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
            BufferRequest::Kill { request_id, .. } => (
                unsupported_response(request_id, "kill-buffer is not available yet"),
                Vec::new(),
            ),
            BufferRequest::Capture { request_id, .. } => (
                unsupported_response(request_id, "capture-buffer is not available yet"),
                Vec::new(),
            ),
        }
    }

    async fn dispatch_node(
        &self,
        request: mux_protocol::NodeRequest,
    ) -> (ServerResponse, Vec<ServerEvent>) {
        let mut state = self.state.lock().await;

        match request {
            mux_protocol::NodeRequest::GetTree {
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
            mux_protocol::NodeRequest::Split {
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
            mux_protocol::NodeRequest::WrapInTabs { request_id, .. } => (
                unsupported_response(request_id, "wrap-node-in-tabs is not available yet"),
                Vec::new(),
            ),
            mux_protocol::NodeRequest::AddTab {
                request_id,
                tabs_node_id,
                title,
                child_node_id,
            } => {
                let session_id = match state.node(tabs_node_id) {
                    Ok(node) => node.session_id(),
                    Err(error) => return (mux_error_response(Some(request_id), error), Vec::new()),
                };
                if let Err(error) = state.add_tab_sibling(tabs_node_id, title, child_node_id) {
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
            mux_protocol::NodeRequest::SelectTab {
                request_id,
                tabs_node_id,
                index,
            } => {
                let session_id = match state.node(tabs_node_id) {
                    Ok(node) => node.session_id(),
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
            mux_protocol::NodeRequest::Focus {
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
            mux_protocol::NodeRequest::Close {
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
            mux_protocol::NodeRequest::MoveBufferToNode { request_id, .. } => (
                unsupported_response(request_id, "move-buffer-to-node is not available yet"),
                Vec::new(),
            ),
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
                geometry,
                title,
            } => match state.create_floating_window(session_id, root_node_id, geometry, title) {
                Ok(floating_id) => {
                    if let Err(error) = state.focus_floating(floating_id) {
                        return (mux_error_response(Some(request_id), error), Vec::new());
                    }
                    match state.floating_window(floating_id) {
                        Ok(floating) => {
                            let mut events =
                                vec![ServerEvent::FloatingChanged(FloatingChangedEvent {
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
                    }
                }
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

    async fn broadcast(&self, events: Vec<ServerEvent>) {
        if events.is_empty() {
            return;
        }

        let mut subscriptions = self.subscriptions.lock().await;
        subscriptions.retain(|_, subscription| {
            for event in &events {
                let event_matches = event.session_id().is_none()
                    || subscription.session_id.is_none()
                    || subscription.session_id == event.session_id();

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

    async fn cleanup_connection(&self, connection_id: u64) {
        self.subscriptions
            .lock()
            .await
            .retain(|_, subscription| subscription.connection_id != connection_id);
    }
}

async fn handle_connection(
    runtime: Arc<Runtime>,
    connection_id: u64,
    mut reader: OwnedReadHalf,
    outbound: mpsc::UnboundedSender<ServerEnvelope>,
) -> Result<()> {
    loop {
        let Some(frame) = read_frame(&mut reader)
            .await
            .map_err(protocol_error_to_mux)?
        else {
            debug!(connection_id, "client disconnected");
            runtime.cleanup_connection(connection_id).await;
            return Ok(());
        };

        if frame.frame_type != FrameType::Request {
            let _ = outbound.send(ServerEnvelope::Response(protocol_error_response(
                Some(frame.request_id),
                ProtocolError::UnexpectedFrameType(frame.frame_type),
            )));
            continue;
        }

        let request = match decode_client_message(&frame.payload) {
            Ok(request) => {
                if request.request_id() != frame.request_id {
                    let _ = outbound.send(ServerEnvelope::Response(protocol_error_response(
                        Some(frame.request_id),
                        ProtocolError::MismatchedRequestId {
                            expected: frame.request_id,
                            actual: request.request_id(),
                        },
                    )));
                    continue;
                }
                request
            }
            Err(error) => {
                let _ = outbound.send(ServerEnvelope::Response(protocol_error_response(
                    Some(frame.request_id),
                    error,
                )));
                continue;
            }
        };

        let span = request_span("handle_request", request.request_id());
        let _entered = span.enter();
        let (response, events) = runtime
            .dispatch_request(connection_id, &outbound, request)
            .await;

        if outbound.send(ServerEnvelope::Response(response)).is_err() {
            runtime.cleanup_connection(connection_id).await;
            return Ok(());
        }
        runtime.broadcast(events).await;
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
        write_frame(&mut writer, &frame)
            .await
            .map_err(protocol_error_to_mux)?;
    }

    Ok(())
}

fn focus_changed_event(
    state: &ServerState,
    session_id: mux_core::SessionId,
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

fn unsupported_response(request_id: RequestId, message: impl Into<String>) -> ServerResponse {
    error_response(Some(request_id), ErrorCode::Unsupported, message)
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
