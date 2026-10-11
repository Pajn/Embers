//! A session whose last pane closes, because its command exited or it was
//! closed, is destroyed like a tmux session that loses its last window.

use embers_core::{RequestId, SessionId, init_test_tracing};
use embers_protocol::{
    BufferRecord, BufferRequest, ClientMessage, ClientRequest, NodeRequest, ProtocolClient,
    ServerEnvelope, ServerEvent, ServerResponse, SessionRequest, SessionSnapshot,
};
use embers_server::{REMAIN_ON_EXIT_OPTION, Server, ServerConfig, ServerHandle};
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

struct Harness {
    _tempdir: TempDir,
    handle: ServerHandle,
    client: ProtocolClient,
    next_request_id: u64,
}

impl Harness {
    async fn start() -> Self {
        init_test_tracing();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let socket_path = tempdir.path().join("mux.sock");
        let handle = Server::new(ServerConfig::new(socket_path.clone()))
            .start()
            .await
            .expect("start server");
        let client = ProtocolClient::connect(&socket_path)
            .await
            .expect("connect client");
        Self {
            _tempdir: tempdir,
            handle,
            client,
            next_request_id: 1,
        }
    }

    async fn request(
        &mut self,
        message: impl FnOnce(RequestId) -> ClientMessage,
    ) -> ServerResponse {
        let request_id = RequestId(self.next_request_id);
        self.next_request_id += 1;
        self.client
            .request(&message(request_id))
            .await
            .expect("request round-trips")
    }

    async fn create_session(&mut self, name: &str) -> SessionId {
        let name = name.to_owned();
        match self
            .request(|request_id| {
                ClientMessage::Session(SessionRequest::Create { request_id, name })
            })
            .await
        {
            ServerResponse::SessionSnapshot(response) => response.snapshot.session.id,
            other => panic!("expected session snapshot, got {other:?}"),
        }
    }

    async fn create_buffer(&mut self, command: &[&str]) -> BufferRecord {
        let command = command.iter().map(|part| (*part).to_owned()).collect();
        match self
            .request(|request_id| {
                ClientMessage::Buffer(BufferRequest::Create {
                    request_id,
                    title: None,
                    command,
                    cwd: None,
                    env: Default::default(),
                    size: None,
                    session_id: None,
                })
            })
            .await
        {
            ServerResponse::Buffer(response) => response.buffer,
            other => panic!("expected buffer response, got {other:?}"),
        }
    }

    /// Add a window running `command` to the session; returns the snapshot.
    async fn add_window(&mut self, session_id: SessionId, command: &[&str]) -> SessionSnapshot {
        let buffer = self.create_buffer(command).await;
        match self
            .request(|request_id| {
                ClientMessage::Session(SessionRequest::AddRootTab {
                    request_id,
                    session_id,
                    title: "shell".to_owned(),
                    buffer_id: Some(buffer.id),
                    child_node_id: None,
                })
            })
            .await
        {
            ServerResponse::SessionSnapshot(response) => response.snapshot,
            other => panic!("expected session snapshot, got {other:?}"),
        }
    }

    async fn session_ids(&mut self) -> Vec<SessionId> {
        match self
            .request(|request_id| ClientMessage::Session(SessionRequest::List { request_id }))
            .await
        {
            ServerResponse::Sessions(response) => response
                .sessions
                .into_iter()
                .map(|session| session.id)
                .collect(),
            other => panic!("expected sessions, got {other:?}"),
        }
    }

    async fn wait_for_sessions(&mut self, expected: &[SessionId]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sessions = self.session_ids().await;
            if sessions == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "sessions stayed {sessions:?}, expected {expected:?}"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn current_session(&mut self) -> Option<SessionId> {
        match self
            .request(|request_id| {
                ClientMessage::Client(ClientRequest::Get {
                    request_id,
                    client_id: None,
                })
            })
            .await
        {
            ServerResponse::Client(response) => response.client.current_session_id,
            other => panic!("expected client, got {other:?}"),
        }
    }

    async fn switch_to(&mut self, session_id: SessionId) {
        let _ = self
            .request(|request_id| {
                ClientMessage::Client(ClientRequest::Switch {
                    request_id,
                    client_id: None,
                    session_id,
                })
            })
            .await;
    }

    async fn shutdown(self) {
        drop(self.client);
        self.handle.shutdown().await.expect("shutdown server");
    }
}

#[tokio::test]
async fn session_is_destroyed_when_its_last_pane_exits() {
    let mut harness = Harness::start().await;
    let keep = harness.create_session("keep").await;
    harness
        .add_window(keep, &["/bin/sh", "-c", "sleep 30"])
        .await;
    let doomed = harness.create_session("doomed").await;
    harness
        .add_window(doomed, &["/bin/sh", "-c", "sleep 0.3"])
        .await;
    harness.switch_to(doomed).await;

    harness.wait_for_sessions(&[keep]).await;
    // A client on the destroyed session is left without one; the interactive
    // client then moves on according to its detach-on-destroy setting.
    assert_eq!(harness.current_session().await, None);

    harness.shutdown().await;
}

#[tokio::test]
async fn session_is_destroyed_when_its_last_pane_is_closed() {
    let mut harness = Harness::start().await;
    let keep = harness.create_session("keep").await;
    harness
        .add_window(keep, &["/bin/sh", "-c", "sleep 30"])
        .await;
    let doomed = harness.create_session("doomed").await;
    let snapshot = harness
        .add_window(doomed, &["/bin/sh", "-c", "sleep 30"])
        .await;
    let pane = snapshot.session.focused_leaf_id.expect("focused pane");

    match harness
        .request(|request_id| {
            ClientMessage::Node(NodeRequest::Close {
                request_id,
                node_id: pane,
            })
        })
        .await
    {
        ServerResponse::Ok(_) | ServerResponse::SessionSnapshot(_) => {}
        other => panic!("expected close to succeed, got {other:?}"),
    }
    assert_eq!(harness.session_ids().await, vec![keep]);

    harness.shutdown().await;
}

#[tokio::test]
async fn sessions_with_panes_left_or_never_given_any_stay() {
    let mut harness = Harness::start().await;
    // A new session has no pane until a window is added to it.
    let empty = harness.create_session("empty").await;
    let busy = harness.create_session("busy").await;
    harness
        .add_window(busy, &["/bin/sh", "-c", "sleep 30"])
        .await;
    harness
        .add_window(busy, &["/bin/sh", "-c", "sleep 0.2"])
        .await;
    // A dead pane kept by remain-on-exit still counts as a pane.
    let dead = harness.create_session("dead").await;
    let buffer = harness.create_buffer(&["/bin/sh", "-c", "sleep 0.2"]).await;
    let _ = harness
        .request(|request_id| {
            ClientMessage::Buffer(BufferRequest::SetUserOption {
                request_id,
                buffer_id: buffer.id,
                key: REMAIN_ON_EXIT_OPTION.to_owned(),
                value: Some("on".to_owned()),
            })
        })
        .await;
    let _ = harness
        .request(|request_id| {
            ClientMessage::Session(SessionRequest::AddRootTab {
                request_id,
                session_id: dead,
                title: "dead".to_owned(),
                buffer_id: Some(buffer.id),
                child_node_id: None,
            })
        })
        .await;

    sleep(Duration::from_millis(800)).await;
    assert_eq!(harness.session_ids().await, vec![empty, busy, dead]);

    harness.shutdown().await;
}

#[tokio::test]
async fn subscribers_hear_that_the_session_closed() {
    let mut harness = Harness::start().await;
    let doomed = harness.create_session("doomed").await;
    harness
        .add_window(doomed, &["/bin/sh", "-c", "sleep 0.3"])
        .await;
    let _ = harness
        .request(|request_id| {
            ClientMessage::Subscribe(embers_protocol::SubscribeRequest {
                request_id,
                session_id: None,
            })
        })
        .await;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let envelope = tokio::time::timeout(remaining, harness.client.recv())
            .await
            .expect("session closed event before timeout")
            .expect("receive succeeds")
            .expect("server kept connection open");
        let ServerEnvelope::Event(event) = envelope else {
            continue;
        };
        match event {
            ServerEvent::SessionClosed(closed) if closed.session_id == doomed => break,
            // Nothing may ask about the destroyed session after it is gone.
            ServerEvent::NodeChanged(changed) => {
                assert_ne!(
                    changed.session_id, doomed,
                    "layout event for a closing session"
                )
            }
            _ => {}
        }
    }

    harness.shutdown().await;
}
