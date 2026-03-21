use std::path::Path;
use std::time::Duration;

use mux_core::{MuxError, Result, SessionId, new_request_id};
use mux_protocol::{
    ClientMessage, PingRequest, ProtocolClient, ServerEnvelope, ServerEvent, ServerResponse,
    SubscribeRequest, UnsubscribeRequest,
};

#[derive(Debug)]
pub struct TestConnection {
    client: ProtocolClient,
}

impl TestConnection {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let client = ProtocolClient::connect(path)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?;
        Ok(Self { client })
    }

    pub async fn send(&mut self, message: &ClientMessage) -> Result<()> {
        self.client
            .send(message)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))
    }

    pub async fn recv(&mut self) -> Result<Option<ServerEnvelope>> {
        self.client
            .recv()
            .await
            .map_err(|error| MuxError::transport(error.to_string()))
    }

    pub async fn request(&mut self, message: &ClientMessage) -> Result<ServerResponse> {
        self.client
            .request(message)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))
    }

    pub async fn recv_event(&mut self) -> Result<ServerEvent> {
        match self.recv().await? {
            Some(ServerEnvelope::Event(event)) => Ok(event),
            Some(ServerEnvelope::Response(response)) => Err(MuxError::protocol(format!(
                "expected event, got response: {response:?}"
            ))),
            None => Err(MuxError::transport(
                "connection closed while waiting for an event",
            )),
        }
    }

    pub async fn wait_for_event<F>(
        &mut self,
        timeout: Duration,
        mut predicate: F,
    ) -> Result<ServerEvent>
    where
        F: FnMut(&ServerEvent) -> bool,
    {
        tokio::time::timeout(timeout, async {
            loop {
                let event = self.recv_event().await?;
                if predicate(&event) {
                    return Ok(event);
                }
            }
        })
        .await
        .map_err(|_| MuxError::timeout(format!("timed out waiting for event after {timeout:?}")))?
    }

    pub async fn subscribe(&mut self, session_id: Option<SessionId>) -> Result<u64> {
        let response = self
            .request(&ClientMessage::Subscribe(SubscribeRequest {
                request_id: new_request_id(),
                session_id,
            }))
            .await?;

        match response {
            ServerResponse::SubscriptionAck(ack) => Ok(ack.subscription_id),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to subscribe request: {other:?}"
            ))),
        }
    }

    pub async fn unsubscribe(&mut self, subscription_id: u64) -> Result<()> {
        let response = self
            .request(&ClientMessage::Unsubscribe(UnsubscribeRequest {
                request_id: new_request_id(),
                subscription_id,
            }))
            .await?;

        match response {
            ServerResponse::Ok(_) => Ok(()),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to unsubscribe request: {other:?}"
            ))),
        }
    }

    pub async fn ping(&mut self, payload: impl Into<String>) -> Result<String> {
        let response = self
            .request(&ClientMessage::Ping(PingRequest {
                request_id: new_request_id(),
                payload: payload.into(),
            }))
            .await?;

        match response {
            ServerResponse::Pong(pong) => Ok(pong.payload),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to ping request: {other:?}"
            ))),
        }
    }
}
