use std::path::Path;

use tokio::net::UnixStream;

use crate::codec::{ProtocolError, decode_server_envelope, encode_client_message};
use crate::framing::{read_frame, write_frame};
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
        write_frame(&mut self.stream, &payload).await
    }

    pub async fn recv(&mut self) -> Result<Option<ServerEnvelope>, ProtocolError> {
        let Some(frame) = read_frame(&mut self.stream).await? else {
            return Ok(None);
        };

        Ok(Some(decode_server_envelope(&frame)?))
    }

    pub async fn request(
        &mut self,
        message: &ClientMessage,
    ) -> Result<ServerResponse, ProtocolError> {
        let request_id = message.request_id();
        self.send(message).await?;

        loop {
            match self.recv().await? {
                Some(ServerEnvelope::Response(response)) => {
                    let matches_request = response
                        .request_id()
                        .map(|response_id| response_id == request_id)
                        .unwrap_or(true);
                    if matches_request {
                        return Ok(response);
                    }
                }
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
