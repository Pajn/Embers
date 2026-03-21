use mux_core::{RequestId, WireError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub request_id: RequestId,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingResponse {
    pub request_id: RequestId,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorResponse {
    pub request_id: Option<RequestId>,
    pub error: WireError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeartbeatEvent {
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    Ping(PingRequest),
}

impl ClientMessage {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Ping(request) => request.request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerResponse {
    Pong(PingResponse),
    Error(ErrorResponse),
}

impl ServerResponse {
    pub fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::Pong(response) => Some(response.request_id),
            Self::Error(response) => response.request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEvent {
    Heartbeat(HeartbeatEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEnvelope {
    Response(ServerResponse),
    Event(ServerEvent),
}
