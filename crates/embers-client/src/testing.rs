use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use embers_core::{MuxError, Result};
use embers_protocol::{ClientMessage, ServerEvent, ServerResponse};

use crate::transport::Transport;

#[derive(Clone, Debug, Default)]
pub struct FakeTransport {
    requests: Arc<Mutex<Vec<ClientMessage>>>,
    responses: Arc<Mutex<VecDeque<ServerResponse>>>,
    events: Arc<Mutex<VecDeque<ServerEvent>>>,
}

impl FakeTransport {
    pub fn push_response(&self, response: ServerResponse) {
        self.responses
            .lock()
            .expect("responses lock")
            .push_back(response);
    }

    pub fn push_event(&self, event: ServerEvent) {
        self.events.lock().expect("events lock").push_back(event);
    }

    pub fn requests(&self) -> Vec<ClientMessage> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        self.requests.lock().expect("requests lock").push(message);
        self.responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .ok_or_else(|| MuxError::transport("fake transport has no queued response"))
    }

    async fn next_event(&self) -> Result<ServerEvent> {
        self.events
            .lock()
            .expect("events lock")
            .pop_front()
            .ok_or_else(|| MuxError::transport("fake transport has no queued event"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    pub request: ClientMessage,
    pub response: ServerResponse,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptedTransport {
    exchanges: Arc<Mutex<VecDeque<Exchange>>>,
    events: Arc<Mutex<VecDeque<ServerEvent>>>,
}

impl ScriptedTransport {
    pub fn push_exchange(&self, request: ClientMessage, response: ServerResponse) {
        self.exchanges
            .lock()
            .expect("exchanges lock")
            .push_back(Exchange { request, response });
    }

    pub fn push_event(&self, event: ServerEvent) {
        self.events.lock().expect("events lock").push_back(event);
    }

    pub fn assert_exhausted(&self) -> Result<()> {
        let remaining = self.exchanges.lock().expect("exchanges lock").len();
        if remaining == 0 {
            Ok(())
        } else {
            Err(MuxError::transport(format!(
                "scripted transport still has {remaining} pending exchange(s)"
            )))
        }
    }
}

#[async_trait]
impl Transport for ScriptedTransport {
    async fn request(&self, message: ClientMessage) -> Result<ServerResponse> {
        let expected = self
            .exchanges
            .lock()
            .expect("exchanges lock")
            .pop_front()
            .ok_or_else(|| MuxError::transport("scripted transport has no queued exchange"))?;

        if expected.request != message {
            return Err(MuxError::transport(format!(
                "unexpected request: expected {:?}, got {:?}",
                expected.request, message
            )));
        }

        Ok(expected.response)
    }

    async fn next_event(&self) -> Result<ServerEvent> {
        self.events
            .lock()
            .expect("events lock")
            .pop_front()
            .ok_or_else(|| MuxError::transport("scripted transport has no queued event"))
    }
}

pub type TestGrid = crate::grid::RenderGrid;

#[cfg(test)]
mod tests {
    use embers_core::RequestId;
    use embers_protocol::{ClientMessage, PingRequest, PingResponse, ServerResponse};

    use super::{FakeTransport, ScriptedTransport, TestGrid};
    use crate::Transport;

    #[tokio::test]
    async fn fake_transport_records_requests() {
        let transport = FakeTransport::default();
        let request = ClientMessage::Ping(PingRequest {
            request_id: RequestId(3),
            payload: "phase0".to_owned(),
        });
        transport.push_response(ServerResponse::Pong(PingResponse {
            request_id: RequestId(3),
            payload: "phase0".to_owned(),
        }));

        let response = transport.request(request.clone()).await.expect("response");
        assert_eq!(transport.requests(), vec![request]);
        assert_eq!(
            response,
            ServerResponse::Pong(PingResponse {
                request_id: RequestId(3),
                payload: "phase0".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn scripted_transport_rejects_mismatched_requests() {
        let transport = ScriptedTransport::default();
        transport.push_exchange(
            ClientMessage::Ping(PingRequest {
                request_id: RequestId(4),
                payload: "expected".to_owned(),
            }),
            ServerResponse::Pong(PingResponse {
                request_id: RequestId(4),
                payload: "expected".to_owned(),
            }),
        );

        let error = transport
            .request(ClientMessage::Ping(PingRequest {
                request_id: RequestId(4),
                payload: "different".to_owned(),
            }))
            .await
            .expect_err("request must mismatch");
        assert!(error.to_string().contains("unexpected request"));
    }

    #[test]
    fn test_grid_renders_rows() {
        let mut grid = TestGrid::new(6, 2);
        grid.put_str(1, 0, "embers");
        grid.put_str(0, 1, "ok");

        assert_eq!(grid.render(), " ember\nok    ");
    }
}
