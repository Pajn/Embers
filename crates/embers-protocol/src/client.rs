use std::path::Path;

use embers_core::RequestId;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::codec::{ProtocolError, decode_server_envelope, encode_client_message};
use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
use crate::types::{ClientMessage, ServerEnvelope, ServerResponse};

type ReaderItem = Result<Option<ServerEnvelope>, ProtocolError>;

#[derive(Debug)]
pub struct ProtocolClient {
    writer: OwnedWriteHalf,
    reader_rx: mpsc::Receiver<ReaderItem>,
    reader_reached_eof: bool,
    reader_task: JoinHandle<()>,
}

impl ProtocolClient {
    const READER_CHANNEL_CAPACITY: usize = 64;

    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self::from_stream(stream))
    }

    fn from_stream(stream: UnixStream) -> Self {
        let (reader, writer) = stream.into_split();
        let (reader_tx, reader_rx) = mpsc::channel(Self::READER_CHANNEL_CAPACITY);
        let reader_task = tokio::spawn(async move {
            Self::run_reader(reader, reader_tx).await;
        });

        Self {
            writer,
            reader_rx,
            reader_reached_eof: false,
            reader_task,
        }
    }

    async fn run_reader(mut reader: OwnedReadHalf, reader_tx: mpsc::Sender<ReaderItem>) {
        loop {
            let next = match read_frame(&mut reader).await {
                Ok(Some(frame)) => Self::decode_frame(frame).map(Some),
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            let terminal = !matches!(next, Ok(Some(_)));
            if reader_tx.send(next).await.is_err() || terminal {
                break;
            }
        }
    }

    fn decode_frame(frame: RawFrame) -> Result<ServerEnvelope, ProtocolError> {
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
                Ok(ServerEnvelope::Response(response))
            }
            (FrameType::Event, ServerEnvelope::Event(event)) => {
                if frame.request_id != RequestId(0) {
                    return Err(ProtocolError::MismatchedRequestId {
                        expected: RequestId(0),
                        actual: frame.request_id,
                    });
                }
                Ok(ServerEnvelope::Event(event))
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

    pub async fn send(&mut self, message: &ClientMessage) -> Result<(), ProtocolError> {
        let payload = encode_client_message(message)?;
        let frame = RawFrame::new(FrameType::Request, message.request_id(), payload);
        write_frame(&mut self.writer, &frame).await
    }

    pub async fn recv(&mut self) -> Result<Option<ServerEnvelope>, ProtocolError> {
        match self.reader_rx.recv().await {
            Some(Ok(None)) => {
                self.reader_reached_eof = true;
                Ok(None)
            }
            Some(result) => result,
            None if self.reader_reached_eof => Ok(None),
            None => Err(ProtocolError::ReaderTaskExited),
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

impl Drop for ProtocolClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::ProtocolClient;
    use embers_core::{ErrorCode, RequestId, SessionId, WireError};
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;
    use tokio::task::yield_now;
    use tokio::time::{Duration, timeout};

    use crate::codec::{ProtocolError, encode_server_envelope};
    use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
    use crate::types::{
        ClientMessage, ErrorResponse, PingRequest, ServerEnvelope, ServerEvent, ServerResponse,
        SessionClosedEvent,
    };

    #[tokio::test]
    async fn request_accepts_unscoped_error_response() {
        let (mut server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);

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

    #[tokio::test]
    async fn recv_timeout_does_not_cancel_in_progress_frame_read() {
        let (mut server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);

        let payload = encode_server_envelope(&ServerEnvelope::Event(ServerEvent::SessionClosed(
            SessionClosedEvent {
                session_id: SessionId(9),
            },
        )))
        .expect("encode event");
        let mut frame_bytes = Vec::with_capacity(13 + payload.len());
        frame_bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame_bytes.push(FrameType::Event as u8);
        frame_bytes.extend_from_slice(&0_u64.to_le_bytes());
        frame_bytes.extend_from_slice(&payload);

        server
            .write_all(&frame_bytes[..5])
            .await
            .expect("write partial frame");

        let timed_out = timeout(Duration::from_millis(20), client.recv()).await;
        assert!(timed_out.is_err(), "partial frame should keep recv pending");

        server
            .write_all(&frame_bytes[5..])
            .await
            .expect("write remainder");

        let envelope = timeout(Duration::from_secs(1), client.recv())
            .await
            .expect("recv finishes after remainder arrives")
            .expect("recv succeeds")
            .expect("connection remains open");
        assert!(matches!(
            envelope,
            ServerEnvelope::Event(ServerEvent::SessionClosed(SessionClosedEvent {
                session_id
            })) if session_id == SessionId(9)
        ));
    }

    #[tokio::test]
    async fn recv_reports_reader_task_exit_when_channel_closes_without_eof() {
        let (_server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);

        client.reader_task.abort();
        yield_now().await;

        let error = timeout(Duration::from_secs(1), client.recv())
            .await
            .expect("recv returns after reader abort")
            .expect_err("closed reader channel should error");
        assert!(matches!(error, ProtocolError::ReaderTaskExited));
    }
}
