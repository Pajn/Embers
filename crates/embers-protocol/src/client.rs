use std::fmt;
use std::path::Path;

use embers_core::RequestId;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::codec::{ProtocolError, decode_server_envelope, encode_client_message};
use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
use crate::types::{ClientMessage, HelloRequest, ServerEnvelope, ServerResponse};
use crate::{PROTOCOL_VERSION, SOFTWARE_VERSION};

type ReaderItem = Result<Option<ServerEnvelope>, ProtocolError>;

/// A connection to a server: over its socket ([`Self::connect`]), or over any
/// byte stream to it, such as the pipes of `embers proxy` run over SSH
/// ([`Self::from_split`]).
pub struct ProtocolClient {
    writer: Box<dyn AsyncWrite + Send + Unpin>,
    reader_rx: mpsc::Receiver<ReaderItem>,
    reader_reached_eof: bool,
    reader_abort_handle: AbortHandle,
}

impl ProtocolClient {
    const READER_CHANNEL_CAPACITY: usize = 64;

    /// Connect to the server at `path` and check that it speaks this
    /// client's protocol (see [`Self::handshake`]).
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let stream = UnixStream::connect(path).await?;
        let mut client = Self::from_stream(stream);
        client.handshake().await?;
        Ok(client)
    }

    /// Tell the server which protocol this client speaks and refuse a server
    /// that speaks another. Answered before any other request is sent, so its
    /// request id cannot collide with theirs.
    pub async fn handshake(&mut self) -> Result<(), ProtocolError> {
        let hello = ClientMessage::Hello(HelloRequest {
            request_id: RequestId(0),
            protocol_version: PROTOCOL_VERSION,
            software_version: SOFTWARE_VERSION.to_owned(),
        });
        match self.request(&hello).await? {
            ServerResponse::Hello(response) if response.protocol_version == PROTOCOL_VERSION => {
                Ok(())
            }
            ServerResponse::Hello(response) => Err(ProtocolError::IncompatibleServer {
                client_version: SOFTWARE_VERSION.to_owned(),
                client_protocol: PROTOCOL_VERSION,
                server_version: response.software_version,
                server_protocol: response.protocol_version,
            }),
            // A server from before the handshake cannot decode the request
            // and answers it with an error.
            ServerResponse::Error(_) => Err(ProtocolError::ServerPredatesHandshake {
                client_version: SOFTWARE_VERSION.to_owned(),
            }),
            _ => Err(ProtocolError::InvalidMessage(
                "server answered the version check with another response",
            )),
        }
    }

    fn from_stream(stream: UnixStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self::from_split(reader, writer)
    }

    /// A client reading the server's frames from `reader` and writing its own
    /// to `writer`. Unlike [`Self::connect`] it does not check the server's
    /// version; call [`Self::handshake`] before any other request.
    pub fn from_split<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (reader_tx, reader_rx) = mpsc::channel(Self::READER_CHANNEL_CAPACITY);
        let reader_task = tokio::spawn(async move {
            Self::run_reader(reader, reader_tx).await;
        });
        let reader_abort_handle = reader_task.abort_handle();

        Self {
            writer: Box::new(writer),
            reader_rx,
            reader_reached_eof: false,
            reader_abort_handle,
        }
    }

    async fn run_reader<R>(mut reader: R, reader_tx: mpsc::Sender<ReaderItem>)
    where
        R: AsyncRead + Unpin,
    {
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

impl fmt::Debug for ProtocolClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtocolClient")
            .field("reader_reached_eof", &self.reader_reached_eof)
            .finish_non_exhaustive()
    }
}

impl Drop for ProtocolClient {
    fn drop(&mut self) {
        self.reader_abort_handle.abort();
    }
}

#[cfg(test)]
impl ProtocolClient {
    fn abort_reader_task(&self) {
        self.reader_abort_handle.abort();
    }

    fn drain_recv_buffer(&mut self) {
        while let Ok(item) = self.reader_rx.try_recv() {
            if matches!(item, Ok(None)) {
                self.reader_reached_eof = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProtocolClient;
    use embers_core::{ErrorCode, RequestId, SessionId, WireError};
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;
    use tokio::time::{Duration, timeout};

    use crate::codec::{ProtocolError, decode_client_message, encode_server_envelope};
    use crate::framing::{FrameType, RawFrame, read_frame, write_frame};
    use crate::types::{
        ClientMessage, ErrorResponse, HelloResponse, PingRequest, ServerEnvelope, ServerEvent,
        ServerResponse, SessionClosedEvent,
    };
    use crate::{PROTOCOL_VERSION, SOFTWARE_VERSION};

    /// Read the client's first request, check that it is the version check,
    /// and answer it with `response`.
    async fn answer_hello(mut server: UnixStream, response: ServerResponse) {
        let frame = read_frame(&mut server)
            .await
            .expect("read request frame")
            .expect("request frame");
        match decode_client_message(&frame.payload).expect("decode request") {
            ClientMessage::Hello(hello) => {
                assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
                assert_eq!(hello.software_version, SOFTWARE_VERSION);
            }
            other => panic!("expected the version check first, got {other:?}"),
        }
        let payload =
            encode_server_envelope(&ServerEnvelope::Response(response)).expect("encode response");
        let frame = RawFrame::new(FrameType::Response, frame.request_id, payload);
        write_frame(&mut server, &frame)
            .await
            .expect("write response frame");
    }

    #[tokio::test]
    async fn handshake_accepts_a_server_speaking_the_same_protocol() {
        let (server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);
        let server_task = tokio::spawn(answer_hello(
            server,
            ServerResponse::Hello(HelloResponse {
                request_id: RequestId(0),
                protocol_version: PROTOCOL_VERSION,
                software_version: "some other release".to_owned(),
            }),
        ));

        client.handshake().await.expect("handshake succeeds");
        server_task.await.expect("server task joins");
    }

    #[tokio::test]
    async fn handshake_refuses_a_server_speaking_another_protocol() {
        let (server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);
        let server_task = tokio::spawn(answer_hello(
            server,
            ServerResponse::Hello(HelloResponse {
                request_id: RequestId(0),
                protocol_version: PROTOCOL_VERSION + 1,
                software_version: "9.9.9".to_owned(),
            }),
        ));

        let error = client.handshake().await.expect_err("handshake fails");
        match &error {
            ProtocolError::IncompatibleServer {
                server_version,
                server_protocol,
                ..
            } => {
                assert_eq!(server_version, "9.9.9");
                assert_eq!(*server_protocol, PROTOCOL_VERSION + 1);
            }
            other => panic!("expected an incompatible server, got {other:?}"),
        }
        let message = error.to_string();
        assert!(message.contains("9.9.9"), "{message}");
        assert!(message.contains(SOFTWARE_VERSION), "{message}");
        server_task.await.expect("server task joins");
    }

    #[tokio::test]
    async fn handshake_refuses_a_server_from_before_the_handshake() {
        let (server, client_stream) = UnixStream::pair().expect("create unix stream pair");
        let mut client = ProtocolClient::from_stream(client_stream);
        // What an older server answers a request kind it cannot decode with.
        let server_task = tokio::spawn(answer_hello(
            server,
            ServerResponse::Error(ErrorResponse {
                request_id: Some(RequestId(0)),
                error: WireError::new(
                    ErrorCode::ProtocolViolation,
                    "invalid message: unexpected client message kind",
                ),
            }),
        ));

        let error = client.handshake().await.expect_err("handshake fails");
        assert!(
            matches!(error, ProtocolError::ServerPredatesHandshake { .. }),
            "{error:?}"
        );
        server_task.await.expect("server task joins");
    }

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

        client.abort_reader_task();
        client.drain_recv_buffer();

        let error = timeout(Duration::from_secs(1), client.recv())
            .await
            .expect("recv returns after reader abort")
            .expect_err("closed reader channel should error");
        assert!(matches!(error, ProtocolError::ReaderTaskExited));
    }
}
