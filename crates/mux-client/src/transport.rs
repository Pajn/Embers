use async_trait::async_trait;
use mux_core::Result;
use mux_protocol::{ClientMessage, ServerEvent, ServerResponse};

#[async_trait]
pub trait Transport: Send + Sync {
    async fn request(&self, message: ClientMessage) -> Result<ServerResponse>;
    async fn next_event(&self) -> Result<ServerEvent>;
}
