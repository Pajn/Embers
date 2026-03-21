use std::path::{Path, PathBuf};

use mux_core::{ErrorCode, MuxError, Result, WireError, request_span};
use mux_protocol::{
    ClientMessage, ErrorResponse, PingResponse, ProtocolError, ServerEnvelope, ServerResponse,
    decode_client_message, encode_server_envelope, read_frame, write_frame,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::ServerConfig;

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
                        tokio::spawn(async move {
                            if let Err(error) = handle_connection(stream).await {
                                error!(%error, "connection failed");
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

async fn handle_connection(mut stream: UnixStream) -> Result<()> {
    loop {
        let Some(frame) = read_frame(&mut stream)
            .await
            .map_err(protocol_error_to_mux)?
        else {
            debug!("client disconnected");
            return Ok(());
        };

        let request = decode_client_message(&frame).map_err(protocol_error_to_mux)?;
        let span = request_span("handle_request", request.request_id());
        let _entered = span.enter();
        let response = handle_message(request);
        let payload = encode_server_envelope(&response).map_err(protocol_error_to_mux)?;
        write_frame(&mut stream, &payload)
            .await
            .map_err(protocol_error_to_mux)?;
    }
}

fn handle_message(message: ClientMessage) -> ServerEnvelope {
    match message {
        ClientMessage::Ping(request) => {
            ServerEnvelope::Response(ServerResponse::Pong(PingResponse {
                request_id: request.request_id,
                payload: request.payload,
            }))
        }
    }
}

#[allow(dead_code)]
fn protocol_error_response(
    request_id: Option<mux_core::RequestId>,
    error: ProtocolError,
) -> ServerEnvelope {
    ServerEnvelope::Response(ServerResponse::Error(ErrorResponse {
        request_id,
        error: WireError::new(ErrorCode::ProtocolViolation, error.to_string()),
    }))
}

fn protocol_error_to_mux(error: ProtocolError) -> MuxError {
    MuxError::protocol(error.to_string())
}
