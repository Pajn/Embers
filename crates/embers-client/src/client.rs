use std::collections::BTreeSet;
use std::path::Path;

use embers_core::{BufferId, IdAllocator, MuxError, RequestId, Result, SessionId};
use embers_protocol::{
    BufferRequest, ClientMessage, ScrollbackSliceResponse, ServerEvent, ServerResponse,
    SessionRequest, SnapshotResponse, SubscribeRequest,
};

use crate::socket_transport::SocketTransport;
use crate::state::ClientState;
use crate::transport::Transport;

#[derive(Debug)]
pub struct MuxClient<T> {
    transport: T,
    request_ids: IdAllocator<RequestId>,
    state: ClientState,
}

impl<T> MuxClient<T>
where
    T: Transport,
{
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            request_ids: IdAllocator::new(1),
            state: ClientState::default(),
        }
    }

    pub fn next_request_id(&self) -> RequestId {
        self.request_ids.next()
    }

    pub fn state(&self) -> &ClientState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ClientState {
        &mut self.state
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub async fn request_message(&self, message: ClientMessage) -> Result<ServerResponse> {
        let response = self.transport.request(message).await?;
        expect_response(response)
    }

    pub async fn subscribe(&self, session_id: Option<SessionId>) -> Result<u64> {
        let response = self
            .request_message(ClientMessage::Subscribe(SubscribeRequest {
                request_id: self.next_request_id(),
                session_id,
            }))
            .await?;
        match response {
            ServerResponse::SubscriptionAck(response) => Ok(response.subscription_id),
            other => Err(MuxError::protocol(format!(
                "expected subscription ack response, got {other:?}"
            ))),
        }
    }

    pub async fn process_next_event(&mut self) -> Result<ServerEvent> {
        let event = self.transport.next_event().await?;
        self.state.apply_event(&event);
        self.resync_for_event(&event).await?;
        Ok(event)
    }

    pub async fn resync_session(&mut self, session_id: SessionId) -> Result<()> {
        let response = self
            .transport
            .request(ClientMessage::Session(SessionRequest::Get {
                request_id: self.next_request_id(),
                session_id,
            }))
            .await?;

        match expect_response(response)? {
            ServerResponse::SessionSnapshot(response) => {
                self.state.apply_session_snapshot(response.snapshot);
                Ok(())
            }
            other => Err(MuxError::protocol(format!(
                "expected session snapshot response, got {other:?}"
            ))),
        }
    }

    pub async fn refresh_buffer_snapshot(&mut self, buffer_id: BufferId) -> Result<()> {
        let response = self
            .transport
            .request(ClientMessage::Buffer(BufferRequest::CaptureVisible {
                request_id: self.next_request_id(),
                buffer_id,
            }))
            .await?;

        match expect_response(response)? {
            ServerResponse::VisibleSnapshot(snapshot) => {
                self.state.apply_buffer_snapshot(snapshot);
                Ok(())
            }
            other => Err(MuxError::protocol(format!(
                "expected visible snapshot response, got {other:?}"
            ))),
        }
    }

    pub async fn capture_buffer(&self, buffer_id: BufferId) -> Result<SnapshotResponse> {
        let response = self
            .transport
            .request(ClientMessage::Buffer(BufferRequest::Capture {
                request_id: self.next_request_id(),
                buffer_id,
            }))
            .await?;

        match expect_response(response)? {
            ServerResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(MuxError::protocol(format!(
                "expected snapshot response, got {other:?}"
            ))),
        }
    }

    pub async fn capture_scrollback_slice(
        &self,
        buffer_id: BufferId,
        start_line: u64,
        line_count: u32,
    ) -> Result<ScrollbackSliceResponse> {
        let response = self
            .transport
            .request(ClientMessage::Buffer(BufferRequest::ScrollbackSlice {
                request_id: self.next_request_id(),
                buffer_id,
                start_line,
                line_count,
            }))
            .await?;

        match expect_response(response)? {
            ServerResponse::ScrollbackSlice(snapshot) => Ok(snapshot),
            other => Err(MuxError::protocol(format!(
                "expected scrollback slice response, got {other:?}"
            ))),
        }
    }

    pub async fn resync_dirty_sessions(&mut self) -> Result<()> {
        let session_ids = self
            .state
            .dirty_sessions
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.resync_session(session_id).await?;
        }
        Ok(())
    }

    pub async fn resync_all_sessions(&mut self) -> Result<()> {
        let response = self
            .transport
            .request(ClientMessage::Session(SessionRequest::List {
                request_id: self.next_request_id(),
            }))
            .await?;

        let sessions = match expect_response(response)? {
            ServerResponse::Sessions(response) => response.sessions,
            other => {
                return Err(MuxError::protocol(format!(
                    "expected sessions response, got {other:?}"
                )));
            }
        };

        let live_sessions = sessions
            .iter()
            .map(|session| session.id)
            .collect::<BTreeSet<_>>();
        let known_sessions = self.state.sessions.keys().copied().collect::<Vec<_>>();

        for session_id in known_sessions {
            if !live_sessions.contains(&session_id) {
                self.state.remove_session(session_id);
            }
        }

        for session in sessions {
            self.state.dirty_sessions.insert(session.id);
            self.resync_session(session.id).await?;
        }

        self.resync_detached_buffers().await
    }

    async fn resync_for_event(&mut self, event: &ServerEvent) -> Result<()> {
        match event {
            ServerEvent::SessionCreated(event) => self.resync_session(event.session.id).await,
            ServerEvent::NodeChanged(event) => self.resync_session(event.session_id).await,
            ServerEvent::FloatingChanged(event) => self.resync_session(event.session_id).await,
            ServerEvent::SessionClosed(_)
            | ServerEvent::BufferCreated(_)
            | ServerEvent::BufferDetached(_)
            | ServerEvent::FocusChanged(_)
            | ServerEvent::RenderInvalidated(_) => Ok(()),
        }
    }

    async fn resync_detached_buffers(&mut self) -> Result<()> {
        let response = self
            .transport
            .request(ClientMessage::Buffer(BufferRequest::List {
                request_id: self.next_request_id(),
                session_id: None,
                attached_only: false,
                detached_only: true,
            }))
            .await?;

        match expect_response(response)? {
            ServerResponse::Buffers(response) => {
                self.state.apply_detached_buffers(response.buffers);
                Ok(())
            }
            other => Err(MuxError::protocol(format!(
                "expected buffers response, got {other:?}"
            ))),
        }
    }
}

impl MuxClient<SocketTransport> {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let transport = SocketTransport::connect(path).await?;
        Ok(Self::new(transport))
    }
}

fn expect_response(response: ServerResponse) -> Result<ServerResponse> {
    match response {
        ServerResponse::Error(error) => Err(error.error.into()),
        other => Ok(other),
    }
}
