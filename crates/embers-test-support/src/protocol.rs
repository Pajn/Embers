use std::path::Path;
use std::time::Duration;

use embers_core::{BufferId, MuxError, Result, SessionId, new_request_id};
use embers_protocol::{
    BufferRequest, ClientMessage, PingRequest, ProtocolClient, ScrollbackSliceResponse,
    ServerEnvelope, ServerEvent, ServerResponse, SessionRequest, SessionSnapshot, SnapshotResponse,
    SubscribeRequest, UnsubscribeRequest, VisibleSnapshotResponse,
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

    pub async fn session_snapshot(&mut self, session_id: SessionId) -> Result<SessionSnapshot> {
        let response = self
            .request(&ClientMessage::Session(SessionRequest::Get {
                request_id: new_request_id(),
                session_id,
            }))
            .await?;

        match response {
            ServerResponse::SessionSnapshot(response) => Ok(response.snapshot),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to session snapshot request: {other:?}"
            ))),
        }
    }

    pub async fn capture_buffer(&mut self, buffer_id: BufferId) -> Result<SnapshotResponse> {
        let response = self
            .request(&ClientMessage::Buffer(BufferRequest::Capture {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await?;

        match response {
            ServerResponse::Snapshot(snapshot) => Ok(snapshot),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to capture request: {other:?}"
            ))),
        }
    }

    pub async fn capture_visible_buffer(
        &mut self,
        buffer_id: BufferId,
    ) -> Result<VisibleSnapshotResponse> {
        let response = self
            .request(&ClientMessage::Buffer(BufferRequest::CaptureVisible {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await?;

        match response {
            ServerResponse::VisibleSnapshot(snapshot) => Ok(snapshot),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to visible capture request: {other:?}"
            ))),
        }
    }

    pub async fn capture_scrollback_slice(
        &mut self,
        buffer_id: BufferId,
        start_line: u64,
        line_count: u32,
    ) -> Result<ScrollbackSliceResponse> {
        let response = self
            .request(&ClientMessage::Buffer(BufferRequest::ScrollbackSlice {
                request_id: new_request_id(),
                buffer_id,
                start_line,
                line_count,
            }))
            .await?;

        match response {
            ServerResponse::ScrollbackSlice(snapshot) => Ok(snapshot),
            ServerResponse::Error(error) => Err(error.error.into()),
            other => Err(MuxError::protocol(format!(
                "unexpected response to scrollback slice request: {other:?}"
            ))),
        }
    }

    pub async fn wait_for_capture_contains(
        &mut self,
        buffer_id: BufferId,
        needle: &str,
        timeout: Duration,
    ) -> Result<SnapshotResponse> {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let snapshot = self.capture_buffer(buffer_id).await?;
            let capture = snapshot.lines.join("\n");
            if capture.contains(needle) {
                return Ok(snapshot);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(MuxError::timeout(format!(
                    "timed out waiting for buffer {buffer_id} to contain {needle:?}; last capture: {:?}",
                    capture
                )));
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
