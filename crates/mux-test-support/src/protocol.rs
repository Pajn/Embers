use std::path::Path;

use mux_core::{MuxError, Result, new_request_id};
use mux_protocol::{ClientMessage, PingRequest, ProtocolClient, ServerResponse};

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

    pub async fn ping(&mut self, payload: impl Into<String>) -> Result<String> {
        let response = self
            .client
            .request(&ClientMessage::Ping(PingRequest {
                request_id: new_request_id(),
                payload: payload.into(),
            }))
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?;

        match response {
            ServerResponse::Pong(pong) => Ok(pong.payload),
            ServerResponse::Error(error) => Err(error.error.into()),
        }
    }
}
