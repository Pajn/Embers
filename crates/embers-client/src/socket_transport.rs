use std::collections::VecDeque;
use std::path::Path;

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
}

impl SocketTransport {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let client = ProtocolClient::connect(path)
            .await
            .map_err(protocol_error_to_mux)?;
        Ok(Self {
            client: Mutex::new(client),
            queued_events: Mutex::new(VecDeque::new()),
        })
    }
}

#[async_trait]
impl Transport for SocketTransport {
    async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        let request_id = message.request_id();
        let mut drained_events = Vec::new();

        let response = {
            let mut client = self.client.lock().await;
            client.send(&message).await.map_err(protocol_error_to_mux)?;

            loop {
                match client.recv().await.map_err(protocol_error_to_mux)? {
                    Some(ServerEnvelope::Event(event)) => drained_events.push(event),
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
        match client.recv().await.map_err(protocol_error_to_mux)? {
            Some(ServerEnvelope::Event(event)) => Ok(event),
            Some(ServerEnvelope::Response(response)) => Err(MuxError::protocol(format!(
                "received response without pending request: {response:?}"
            ))),
            None => Err(MuxError::transport(
                "connection closed while waiting for an event",
            )),
        }
    }
}

fn protocol_error_to_mux(error: ProtocolError) -> MuxError {
    match error {
        ProtocolError::Io(error) => error.into(),
        other => MuxError::transport(other.to_string()),
    }
}
