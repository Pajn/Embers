//! A pane whose command exits closes, and its buffer record goes away, like
//! tmux with `remain-on-exit off`; `remain-on-exit` keeps it instead.

use embers_core::{
    BufferId, ErrorCode, NodeId, RequestId, SessionId, SplitDirection, init_test_tracing,
};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferRequest, ClientMessage, NodeRequest, ProtocolClient,
    ServerResponse, SessionRequest, SessionSnapshot,
};
use embers_server::{REMAIN_ON_EXIT_OPTION, RemainOnExit, Server, ServerConfig, ServerHandle};
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
        Self::start_with(|_| {}).await
    }

    async fn start_with(configure: impl FnOnce(&mut ServerConfig)) -> Self {
        init_test_tracing();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let socket_path = tempdir.path().join("mux.sock");
        let mut config = ServerConfig::new(socket_path.clone());
        configure(&mut config);
        let handle = Server::new(config).start().await.expect("start server");
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
                    host: None,
                })
            })
            .await
        {
            ServerResponse::Buffer(response) => response.buffer,
            other => panic!("expected buffer response, got {other:?}"),
        }
    }

    async fn get_buffer(&mut self, buffer_id: BufferId) -> Option<BufferRecord> {
        match self
            .request(|request_id| {
                ClientMessage::Buffer(BufferRequest::Get {
                    request_id,
                    buffer_id,
                })
            })
            .await
        {
            ServerResponse::Buffer(response) => Some(response.buffer),
            ServerResponse::Error(error) if error.error.code == ErrorCode::NotFound => None,
            other => panic!("expected buffer response, got {other:?}"),
        }
    }

    async fn session(&mut self, session_id: SessionId) -> SessionSnapshot {
        match self
            .request(|request_id| {
                ClientMessage::Session(SessionRequest::Get {
                    request_id,
                    session_id,
                })
            })
            .await
        {
            ServerResponse::SessionSnapshot(response) => response.snapshot,
            other => panic!("expected session snapshot, got {other:?}"),
        }
    }

    /// A session with a long-running shell, split to show `command` next to
    /// it. Returns the session and the split-off pane's buffer and view.
    async fn split_with(&mut self, command: &[&str], remain_on_exit: Option<&str>) -> Pane {
        let session_id = match self
            .request(|request_id| {
                ClientMessage::Session(SessionRequest::Create {
                    request_id,
                    name: "main".to_owned(),
                })
            })
            .await
        {
            ServerResponse::SessionSnapshot(response) => response.snapshot.session.id,
            other => panic!("expected session snapshot, got {other:?}"),
        };
        let shell = self.create_buffer(&["/bin/sh", "-c", "sleep 30"]).await;
        let snapshot = match self
            .request(|request_id| {
                ClientMessage::Session(SessionRequest::AddRootTab {
                    request_id,
                    session_id,
                    title: "main".to_owned(),
                    buffer_id: Some(shell.id),
                    child_node_id: None,
                })
            })
            .await
        {
            ServerResponse::SessionSnapshot(response) => response.snapshot,
            other => panic!("expected session snapshot, got {other:?}"),
        };
        let shell_view = snapshot.session.focused_leaf_id.expect("focused leaf");

        let buffer = self.create_buffer(command).await;
        if let Some(value) = remain_on_exit {
            let value = value.to_owned();
            match self
                .request(|request_id| {
                    ClientMessage::Buffer(BufferRequest::SetUserOption {
                        request_id,
                        buffer_id: buffer.id,
                        key: REMAIN_ON_EXIT_OPTION.to_owned(),
                        value: Some(value),
                    })
                })
                .await
            {
                ServerResponse::Buffer(_) => {}
                other => panic!("expected buffer response, got {other:?}"),
            }
        }
        let view_id = match self
            .request(|request_id| {
                ClientMessage::Node(NodeRequest::Split {
                    request_id,
                    leaf_node_id: shell_view,
                    direction: SplitDirection::Vertical,
                    new_buffer_id: buffer.id,
                })
            })
            .await
        {
            // Read the new view from the split's own snapshot: a command that
            // has already exited is cleaned up as soon as the split attaches
            // it, so its record may be gone by the next request.
            ServerResponse::SessionSnapshot(response) => response
                .snapshot
                .nodes
                .iter()
                .find(|node| {
                    node.buffer_view
                        .as_ref()
                        .is_some_and(|view| view.buffer_id == buffer.id)
                })
                .map(|node| node.id)
                .expect("split pane shows the buffer"),
            other => panic!("expected session snapshot, got {other:?}"),
        };
        Pane {
            session_id,
            buffer_id: buffer.id,
            view_id,
        }
    }

    async fn wait_for_buffer_state(&mut self, buffer_id: BufferId, state: BufferRecordState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let record = self.get_buffer(buffer_id).await.expect("buffer exists");
            if record.state == state {
                return;
            }
            assert!(Instant::now() < deadline, "buffer did not reach {state:?}");
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn wait_for_buffer_removed(&mut self, buffer_id: BufferId) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.get_buffer(buffer_id).await.is_some() {
            assert!(
                Instant::now() < deadline,
                "buffer {buffer_id} was not removed"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn shutdown(self) {
        drop(self.client);
        self.handle.shutdown().await.expect("shutdown server");
    }
}

struct Pane {
    session_id: SessionId,
    buffer_id: BufferId,
    view_id: NodeId,
}

fn has_node(snapshot: &SessionSnapshot, node_id: NodeId) -> bool {
    snapshot.nodes.iter().any(|node| node.id == node_id)
}

#[tokio::test]
async fn pane_closes_and_buffer_is_removed_when_its_command_exits() {
    let mut harness = Harness::start().await;
    let pane = harness
        .split_with(&["/bin/sh", "-c", "sleep 0.3"], None)
        .await;

    harness.wait_for_buffer_removed(pane.buffer_id).await;
    let snapshot = harness.session(pane.session_id).await;
    assert!(
        !has_node(&snapshot, pane.view_id),
        "the exited pane's view should be closed"
    );
    assert_eq!(
        snapshot
            .nodes
            .iter()
            .filter(|node| node.buffer_view.is_some())
            .count(),
        1,
        "the other pane stays"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn pane_whose_command_exits_before_it_is_shown_still_closes() {
    let mut harness = Harness::start().await;
    // `exit` ends the command before the split request attaches it.
    let pane = harness.split_with(&["/bin/sh", "-c", "exit 3"], None).await;

    harness.wait_for_buffer_removed(pane.buffer_id).await;
    let snapshot = harness.session(pane.session_id).await;
    assert!(!has_node(&snapshot, pane.view_id));

    harness.shutdown().await;
}

#[tokio::test]
async fn remain_on_exit_keeps_a_capturable_dead_pane_until_it_is_closed() {
    let mut harness = Harness::start().await;
    let pane = harness
        .split_with(
            &["/bin/sh", "-c", "printf 'last words\\n'; sleep 0.3"],
            Some("on"),
        )
        .await;

    harness
        .wait_for_buffer_state(pane.buffer_id, BufferRecordState::Exited)
        .await;
    // Give a disposal that should not happen the chance to run.
    sleep(Duration::from_millis(200)).await;
    let record = harness
        .get_buffer(pane.buffer_id)
        .await
        .expect("dead pane's buffer is kept");
    assert_eq!(record.attachment_node_id, Some(pane.view_id));
    let buffer_id = pane.buffer_id;
    match harness
        .request(|request_id| {
            ClientMessage::Buffer(BufferRequest::Capture {
                request_id,
                buffer_id,
            })
        })
        .await
    {
        ServerResponse::Snapshot(snapshot) => assert!(
            snapshot
                .lines
                .iter()
                .any(|line| line.contains("last words")),
            "dead pane still captures its output: {:?}",
            snapshot.lines
        ),
        other => panic!("expected snapshot, got {other:?}"),
    }

    // Closing the dead pane cleans up its buffer.
    let view_id = pane.view_id;
    match harness
        .request(|request_id| {
            ClientMessage::Node(NodeRequest::Close {
                request_id,
                node_id: view_id,
            })
        })
        .await
    {
        ServerResponse::SessionSnapshot(_) => {}
        other => panic!("expected session snapshot, got {other:?}"),
    }
    assert!(harness.get_buffer(pane.buffer_id).await.is_none());

    harness.shutdown().await;
}

#[tokio::test]
async fn server_wide_remain_on_exit_keeps_dead_panes_unless_a_pane_turns_it_off() {
    let mut harness = Harness::start_with(|config| config.remain_on_exit = RemainOnExit::On).await;
    let kept = harness
        .split_with(&["/bin/sh", "-c", "sleep 0.3"], None)
        .await;
    harness
        .wait_for_buffer_state(kept.buffer_id, BufferRecordState::Exited)
        .await;
    sleep(Duration::from_millis(200)).await;
    let record = harness
        .get_buffer(kept.buffer_id)
        .await
        .expect("the server default keeps the dead pane");
    assert_eq!(record.attachment_node_id, Some(kept.view_id));

    // The pane option still wins over the server default.
    let closed = harness
        .split_with(&["/bin/sh", "-c", "sleep 0.3"], Some("off"))
        .await;
    harness.wait_for_buffer_removed(closed.buffer_id).await;

    harness.shutdown().await;
}
