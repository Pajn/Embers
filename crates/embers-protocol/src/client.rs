use std::path::Path;

use embers_core::RequestId;
use tokio::net::UnixStream;

use crate::codec::{ProtocolError, decode_server_envelope, encode_client_message};
use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
use crate::types::{ClientMessage, ServerEnvelope, ServerResponse};

#[derive(Debug)]
pub struct ProtocolClient {
    stream: UnixStream,
}

impl ProtocolClient {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self { stream })
    }

    pub async fn send(&mut self, message: &ClientMessage) -> Result<(), ProtocolError> {
        let payload = encode_client_message(message)?;
        let frame = RawFrame::new(FrameType::Request, message.request_id(), payload);
        write_frame(&mut self.stream, &frame).await
    }

    pub async fn recv(&mut self) -> Result<Option<ServerEnvelope>, ProtocolError> {
        let Some(frame) = read_frame(&mut self.stream).await? else {
            return Ok(None);
        };

        let envelope = decode_server_envelope(&frame.payload)?;

        match (frame.frame_type, envelope) {
            (FrameType::Response, ServerEnvelope::Response(response)) => {
                let response_id = response.request_id().unwrap_or(RequestId(0));
                if response_id != frame.request_id {
                    return Err(ProtocolError::MismatchedRequestId {
                        expected: frame.request_id,
                        actual: response_id,
                    });
                }
                Ok(Some(ServerEnvelope::Response(response)))
            }
            (FrameType::Event, ServerEnvelope::Event(event)) => {
                if frame.request_id != RequestId(0) {
                    return Err(ProtocolError::MismatchedRequestId {
                        expected: RequestId(0),
                        actual: frame.request_id,
                    });
                }
                Ok(Some(ServerEnvelope::Event(event)))
            }
            (FrameType::Response, ServerEnvelope::Event(_)) => {
                Err(ProtocolError::UnexpectedFrameKind {
                    frame_type: FrameType::Response,
                    envelope_kind: "event",
                })
            }
            (FrameType::Event, ServerEnvelope::Response(_)) => {
                Err(ProtocolError::UnexpectedFrameKind {
                    frame_type: FrameType::Event,
                    envelope_kind: "response",
                })
            }
            (FrameType::Request, _) => Err(ProtocolError::UnexpectedFrameType(FrameType::Request)),
        }
    }

    pub async fn request(
        &mut self,
        message: &ClientMessage,
    ) -> Result<ServerResponse, ProtocolError> {
        let request_id = message.request_id();
        self.send(message).await?;

        loop {
            match self.recv().await? {
                Some(ServerEnvelope::Response(response)) => match response.request_id() {
                    Some(response_id) if response_id != request_id => {
                        return Err(ProtocolError::MismatchedRequestId {
                            expected: request_id,
                            actual: response_id,
                        });
                    }
                    _ => {
                        return Ok(response);
                    }
                },
                Some(ServerEnvelope::Event(_)) => continue,
                None => {
                    return Err(ProtocolError::InvalidMessage(
                        "connection closed before response",
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProtocolClient;
    use embers_core::{ErrorCode, RequestId, WireError};
    use tokio::net::UnixStream;

    use crate::codec::encode_server_envelope;
    use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
    use crate::types::{ClientMessage, ErrorResponse, PingRequest, ServerEnvelope, ServerResponse};

    #[tokio::test]
    async fn request_accepts_unscoped_error_response() {
        let (mut server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient {
            stream: client_stream,
        };

        let request = ClientMessage::Ping(PingRequest {
            request_id: RequestId(7),
            payload: "phase2".to_owned(),
        });

        let server_task = tokio::spawn(async move {
            let frame = read_frame(&mut server)
                .await
                .expect("read request frame")
                .expect("request frame");
            assert_eq!(frame.frame_type, FrameType::Request);
            assert_eq!(frame.request_id, RequestId(7));

            let payload = encode_server_envelope(&ServerEnvelope::Response(ServerResponse::Error(
                ErrorResponse {
                    request_id: None,
                    error: WireError::new(ErrorCode::ProtocolViolation, "bad request"),
                },
            )))
            .expect("encode error response");
            let frame = RawFrame::new(FrameType::Response, RequestId(0), payload);
            write_frame(&mut server, &frame)
                .await
                .expect("write response frame");
        });

        let response = client.request(&request).await.expect("receive response");
        match response {
            ServerResponse::Error(response) => {
                assert_eq!(response.request_id, None);
                assert_eq!(response.error.code, ErrorCode::ProtocolViolation);
                assert_eq!(response.error.message, "bad request");
            }
            other => panic!("expected error response, got {other:?}"),
        }

        server_task.await.expect("server task joins");
    }
}
