use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use embers_core::{MuxError, Result};
use embers_protocol::{
    ClientMessage, ProtocolClient, ProtocolError, ServerEnvelope, ServerEvent, ServerResponse,
};
use tokio::sync::Mutex;

use crate::transport::Transport;

#[derive(Debug)]
pub struct SocketTransport {
    client: Mutex<ProtocolClient>,
    queued_events: Mutex<VecDeque<ServerEvent>>,
    server_shutdown: Arc<AtomicBool>,
}

impl SocketTransport {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let client = ProtocolClient::connect(path)
            .await
            .map_err(protocol_error_to_mux)?;
        Ok(Self {
            client: Mutex::new(client),
            queued_events: Mutex::new(VecDeque::new()),
            server_shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Set once the server said it is stopping on purpose
    /// ([`ServerEvent::ServerShutdown`]), whichever call read that event. A
    /// connection that closes without it means the server went away
    /// unexpectedly. Shared, so it can still be read after the transport is
    /// gone.
    pub fn server_shutdown_flag(&self) -> Arc<AtomicBool> {
        self.server_shutdown.clone()
    }

    fn note_event(&self, event: &ServerEvent) {
        if matches!(event, ServerEvent::ServerShutdown) {
            self.server_shutdown.store(true, Ordering::Release);
        }
    }

    /// After a write to the server failed, queue the events it had already
    /// sent, so a [`ServerEvent::ServerShutdown`] that arrived just before the
    /// connection closed is still seen.
    async fn read_final_events(&self, client: &mut ProtocolClient) {
        let mut events = Vec::new();
        while let Ok(Ok(Some(ServerEnvelope::Event(event)))) =
            tokio::time::timeout(FINAL_EVENTS_TIMEOUT, client.recv()).await
        {
            self.note_event(&event);
            events.push(event);
        }
        self.queued_events.lock().await.extend(events);
    }
}

/// How long [`SocketTransport::read_final_events`] waits for each event the
/// server sent before it closed the connection. They are already buffered,
/// so this only bounds the wait when nothing is.
const FINAL_EVENTS_TIMEOUT: Duration = Duration::from_millis(100);

#[async_trait]
impl Transport for SocketTransport {
    async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        let request_id = message.request_id();
        let mut drained_events = Vec::new();

        let response = {
            let mut client = self.client.lock().await;
            if let Err(error) = client.send(&message).await {
                // The server may have closed the connection after telling us
                // why; read what it sent before reporting the failure.
                self.read_final_events(&mut client).await;
                return Err(protocol_error_to_mux(error));
            }

            loop {
                match client.recv().await.map_err(protocol_error_to_mux)? {
                    Some(ServerEnvelope::Event(event)) => {
                        self.note_event(&event);
                        drained_events.push(event);
                    }
                    Some(ServerEnvelope::Response(response)) => {
                        if let Some(response_id) = response.request_id()
                            && response_id != request_id
                        {
                            break Err(MuxError::protocol(format!(
                                "mismatched response id: expected {request_id}, got {response_id}"
                            )));
                        }
                        break Ok(response);
                    }
                    None => break Err(MuxError::transport("connection closed before response")),
                }
            }
        };

        if !drained_events.is_empty() {
            self.queued_events.lock().await.extend(drained_events);
        }

        response
    }

    async fn next_event(&self) -> Result<ServerEvent> {
        if let Some(event) = self.queued_events.lock().await.pop_front() {
            return Ok(event);
        }

        let mut client = self.client.lock().await;
        let event = expect_event(client.recv().await.map_err(protocol_error_to_mux)?)?;
        self.note_event(&event);
        Ok(event)
    }

    async fn ready_event(&self) -> Result<Option<ServerEvent>> {
        if let Some(event) = self.queued_events.lock().await.pop_front() {
            return Ok(Some(event));
        }

        let mut client = self.client.lock().await;
        // `recv` is cancel-safe, so a zero timeout takes only what has already
        // arrived and loses nothing.
        match tokio::time::timeout(Duration::ZERO, client.recv()).await {
            Ok(envelope) => {
                let event = expect_event(envelope.map_err(protocol_error_to_mux)?)?;
                self.note_event(&event);
                Ok(Some(event))
            }
            Err(_) => Ok(None),
        }
    }
}

fn expect_event(envelope: Option<ServerEnvelope>) -> Result<ServerEvent> {
    match envelope {
        Some(ServerEnvelope::Event(event)) => Ok(event),
        Some(ServerEnvelope::Response(response)) => Err(MuxError::protocol(format!(
            "received response without pending request: {response:?}"
        ))),
        None => Err(MuxError::transport(
            "connection closed while waiting for an event",
        )),
    }
}

fn protocol_error_to_mux(error: ProtocolError) -> MuxError {
    match error {
        ProtocolError::Io(error) => error.into(),
        other => MuxError::transport(other.to_string()),
    }
}
