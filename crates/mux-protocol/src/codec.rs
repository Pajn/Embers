use flatbuffers::FlatBufferBuilder;
use mux_core::{ErrorCode, RequestId, WireError};
use thiserror::Error;

use crate::generated::mux::protocol as fb;
use crate::types::{
    ClientMessage, ErrorResponse, HeartbeatEvent, PingRequest, PingResponse, ServerEnvelope,
    ServerEvent, ServerResponse,
};

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("flatbuffer decode error: {0}")]
    InvalidFlatbuffer(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("invalid message: {0}")]
    InvalidMessage(&'static str),
    #[error("invalid message: {0}")]
    InvalidMessageOwned(String),
    #[error("frame exceeds max length: {0}")]
    FrameTooLarge(usize),
}

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>, ProtocolError> {
    let mut builder = FlatBufferBuilder::new();

    let envelope = match message {
        ClientMessage::Ping(request) => {
            let payload = builder.create_string(&request.payload);
            let ping_request = fb::PingRequest::create(
                &mut builder,
                &fb::PingRequestArgs {
                    payload: Some(payload),
                },
            );
            fb::Envelope::create(
                &mut builder,
                &fb::EnvelopeArgs {
                    request_id: u64::from(request.request_id),
                    kind: fb::MessageKind::PingRequest,
                    ping_request: Some(ping_request),
                    ping_response: None,
                    error_response: None,
                    heartbeat_event: None,
                },
            )
        }
    };

    builder.finish(envelope, Some("EMBR"));
    Ok(builder.finished_data().to_vec())
}

pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let envelope = fb::root_as_envelope(bytes)?;
    match envelope.kind() {
        fb::MessageKind::PingRequest => {
            let ping = required(envelope.ping_request(), "ping_request")?;
            let payload = required(ping.payload(), "ping_request.payload")?;
            Ok(ClientMessage::Ping(PingRequest {
                request_id: RequestId(envelope.request_id()),
                payload: payload.to_owned(),
            }))
        }
        other => Err(ProtocolError::InvalidMessageOwned(format!(
            "unexpected client message kind: {other:?}"
        ))),
    }
}

pub fn encode_server_envelope(message: &ServerEnvelope) -> Result<Vec<u8>, ProtocolError> {
    let mut builder = FlatBufferBuilder::new();

    let envelope = match message {
        ServerEnvelope::Response(ServerResponse::Pong(response)) => {
            let payload = builder.create_string(&response.payload);
            let pong = fb::PingResponse::create(
                &mut builder,
                &fb::PingResponseArgs {
                    payload: Some(payload),
                },
            );
            fb::Envelope::create(
                &mut builder,
                &fb::EnvelopeArgs {
                    request_id: u64::from(response.request_id),
                    kind: fb::MessageKind::PingResponse,
                    ping_request: None,
                    ping_response: Some(pong),
                    error_response: None,
                    heartbeat_event: None,
                },
            )
        }
        ServerEnvelope::Response(ServerResponse::Error(response)) => {
            let message = builder.create_string(&response.error.message);
            let error_response = fb::ErrorResponse::create(
                &mut builder,
                &fb::ErrorResponseArgs {
                    code: encode_error_code(response.error.code),
                    message: Some(message),
                },
            );
            fb::Envelope::create(
                &mut builder,
                &fb::EnvelopeArgs {
                    request_id: response.request_id.map_or(0, u64::from),
                    kind: fb::MessageKind::ErrorResponse,
                    ping_request: None,
                    ping_response: None,
                    error_response: Some(error_response),
                    heartbeat_event: None,
                },
            )
        }
        ServerEnvelope::Event(ServerEvent::Heartbeat(event)) => {
            let message = builder.create_string(&event.message);
            let heartbeat = fb::HeartbeatEvent::create(
                &mut builder,
                &fb::HeartbeatEventArgs {
                    message: Some(message),
                },
            );
            fb::Envelope::create(
                &mut builder,
                &fb::EnvelopeArgs {
                    request_id: 0,
                    kind: fb::MessageKind::HeartbeatEvent,
                    ping_request: None,
                    ping_response: None,
                    error_response: None,
                    heartbeat_event: Some(heartbeat),
                },
            )
        }
    };

    builder.finish(envelope, Some("EMBR"));
    Ok(builder.finished_data().to_vec())
}

pub fn decode_server_envelope(bytes: &[u8]) -> Result<ServerEnvelope, ProtocolError> {
    let envelope = fb::root_as_envelope(bytes)?;
    match envelope.kind() {
        fb::MessageKind::PingResponse => {
            let pong = required(envelope.ping_response(), "ping_response")?;
            let payload = required(pong.payload(), "ping_response.payload")?;
            Ok(ServerEnvelope::Response(ServerResponse::Pong(
                PingResponse {
                    request_id: RequestId(envelope.request_id()),
                    payload: payload.to_owned(),
                },
            )))
        }
        fb::MessageKind::ErrorResponse => {
            let error_response = required(envelope.error_response(), "error_response")?;
            let message = required(error_response.message(), "error_response.message")?;
            let request_id = match envelope.request_id() {
                0 => None,
                value => Some(RequestId(value)),
            };
            Ok(ServerEnvelope::Response(ServerResponse::Error(
                ErrorResponse {
                    request_id,
                    error: WireError::new(decode_error_code(error_response.code()), message),
                },
            )))
        }
        fb::MessageKind::HeartbeatEvent => {
            let heartbeat = required(envelope.heartbeat_event(), "heartbeat_event")?;
            let message = required(heartbeat.message(), "heartbeat_event.message")?;
            Ok(ServerEnvelope::Event(ServerEvent::Heartbeat(
                HeartbeatEvent {
                    message: message.to_owned(),
                },
            )))
        }
        other => Err(ProtocolError::InvalidMessageOwned(format!(
            "unexpected server message kind: {other:?}"
        ))),
    }
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, ProtocolError> {
    value.ok_or(ProtocolError::InvalidMessage(field))
}

fn encode_error_code(code: ErrorCode) -> fb::ErrorCodeWire {
    match code {
        ErrorCode::Unknown => fb::ErrorCodeWire::Unknown,
        ErrorCode::InvalidRequest => fb::ErrorCodeWire::InvalidRequest,
        ErrorCode::ProtocolViolation => fb::ErrorCodeWire::ProtocolViolation,
        ErrorCode::Transport => fb::ErrorCodeWire::Transport,
        ErrorCode::NotFound => fb::ErrorCodeWire::NotFound,
        ErrorCode::Conflict => fb::ErrorCodeWire::Conflict,
        ErrorCode::Unsupported => fb::ErrorCodeWire::Unsupported,
        ErrorCode::Timeout => fb::ErrorCodeWire::Timeout,
        ErrorCode::Internal => fb::ErrorCodeWire::Internal,
    }
}

fn decode_error_code(code: fb::ErrorCodeWire) -> ErrorCode {
    match code {
        fb::ErrorCodeWire::Unknown => ErrorCode::Unknown,
        fb::ErrorCodeWire::InvalidRequest => ErrorCode::InvalidRequest,
        fb::ErrorCodeWire::ProtocolViolation => ErrorCode::ProtocolViolation,
        fb::ErrorCodeWire::Transport => ErrorCode::Transport,
        fb::ErrorCodeWire::NotFound => ErrorCode::NotFound,
        fb::ErrorCodeWire::Conflict => ErrorCode::Conflict,
        fb::ErrorCodeWire::Unsupported => ErrorCode::Unsupported,
        fb::ErrorCodeWire::Timeout => ErrorCode::Timeout,
        fb::ErrorCodeWire::Internal => ErrorCode::Internal,
        _ => ErrorCode::Unknown,
    }
}
