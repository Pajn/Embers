use std::process::Output;
use std::time::Duration;

use embers_client::{MuxClient, PresentationModel, Renderer};
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, SessionId, Size, SplitDirection, new_request_id,
};
use embers_protocol::{
    BufferRequest, BufferResponse, BuffersResponse, ClientMessage, FloatingRequest, NodeRequest,
    ServerResponse, SessionRequest, SessionSnapshot,
};
use embers_test_support::{TestConnection, TestServer, cargo_bin};

fn run_cli(server: &TestServer, args: &[&str]) -> Output {
    let output = cargo_bin("embers")
        .arg("--socket")
        .arg(server.socket_path())
        .args(args)
        .output()
        .expect("cli command runs");
    assert!(
        output.status.success(),
        "cli failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

async fn create_session(connection: &mut TestConnection, name: &str) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::Create {
            request_id: new_request_id(),
            name: name.to_owned(),
        }))
        .await
        .expect("create session succeeds");
    match response {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    }
}

async fn create_buffer(
    connection: &mut TestConnection,
    title: &str,
) -> embers_protocol::BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some(title.to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
        }))
        .await
        .expect("create buffer succeeds");
    match response {
        ServerResponse::Buffer(BufferResponse { buffer, .. }) => buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn session_snapshot_by_name(connection: &mut TestConnection, name: &str) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::List {
            request_id: new_request_id(),
        }))
        .await
        .expect("list sessions succeeds");
    let session_id = match response {
        ServerResponse::Sessions(response) => {
            response
                .sessions
                .into_iter()
                .find(|session| session.name == name)
                .expect("session exists")
                .id
        }
        other => panic!("expected sessions response, got {other:?}"),
    };
    connection
        .session_snapshot(session_id)
        .await
        .expect("session snapshot succeeds")
}

fn node(snapshot: &SessionSnapshot, node_id: NodeId) -> &embers_protocol::NodeRecord {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .unwrap_or_else(|| panic!("node {node_id} missing from snapshot"))
}

fn buffer_for_leaf(snapshot: &SessionSnapshot, leaf_id: NodeId) -> BufferId {
    node(snapshot, leaf_id)
        .buffer_view
        .as_ref()
        .unwrap_or_else(|| panic!("node {leaf_id} is not a leaf"))
        .buffer_id
}

fn session_id_by_name(client: &MuxClient<embers_client::SocketTransport>, name: &str) -> SessionId {
    client
        .state()
        .sessions
        .values()
        .find(|session| session.name == name)
        .unwrap_or_else(|| panic!("session {name} missing from client state"))
        .id
}

async fn refresh_all_snapshots(client: &mut MuxClient<embers_client::SocketTransport>) {
    let buffer_ids = client.state().buffers.keys().copied().collect::<Vec<_>>();
    for buffer_id in buffer_ids {
        client
            .refresh_buffer_snapshot(buffer_id)
            .await
            .unwrap_or_else(|error| panic!("refreshing snapshot for {buffer_id} failed: {error}"));
    }
}

async fn render_session(
    client: &mut MuxClient<embers_client::SocketTransport>,
    session_name: &str,
) -> String {
    client.resync_all_sessions().await.expect("resync succeeds");
    refresh_all_snapshots(client).await;
    let session_id = session_id_by_name(client, session_name);
    let model = PresentationModel::project(
        client.state(),
        session_id,
        Size {
            width: 80,
            height: 24,
        },
    )
    .expect("projection succeeds");
    Renderer.render(client.state(), &model).render()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basic_cli_workflow_renders_split_output() {
    let server = TestServer::start().await.expect("server starts");

    run_cli(&server, &["new-session", "alpha"]);
    run_cli(
        &server,
        &[
            "new-window",
            "-t",
            "alpha",
            "--title",
            "work",
            "--",
            "/bin/sh",
        ],
    );
    let split = run_cli(&server, &["split-window", "--", "/bin/sh"]);
    let pane_id = stdout(&split)
        .trim()
        .parse::<u64>()
        .expect("split-window returns pane id");

    run_cli(
        &server,
        &[
            "send-keys",
            "-t",
            &pane_id.to_string(),
            "--enter",
            "printf",
            "e2e-basic\\n",
        ],
    );

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let buffer_id = buffer_for_leaf(&snapshot, NodeId(pane_id));
    connection
        .wait_for_capture_contains(buffer_id, "e2e-basic", Duration::from_secs(3))
        .await
        .expect("pane output arrives");

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let render = render_session(&mut client, "alpha").await;
    assert!(render.contains("e2e-basic"));

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_tabs_switch_visible_output() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer_a = create_buffer(&mut connection, "root").await;
    let session = match connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "work".to_owned(),
            buffer_id: Some(buffer_a.id),
            child_node_id: None,
        }))
        .await
        .expect("add root tab succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let root_leaf = session
        .session
        .focused_leaf_id
        .expect("root leaf is focused");

    let buffer_b = create_buffer(&mut connection, "nested-one").await;
    let session = match connection
        .request(&ClientMessage::Node(NodeRequest::Split {
            request_id: new_request_id(),
            leaf_node_id: root_leaf,
            direction: SplitDirection::Vertical,
            new_buffer_id: buffer_b.id,
        }))
        .await
        .expect("split succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let right_leaf = session
        .session
        .focused_leaf_id
        .expect("new split leaf is focused");

    let session = match connection
        .request(&ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: new_request_id(),
            node_id: right_leaf,
            title: "one".to_owned(),
        }))
        .await
        .expect("wrap in tabs succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let tabs_node_id = node(&session, right_leaf)
        .parent_id
        .expect("wrapped leaf has tabs parent");

    let buffer_c = create_buffer(&mut connection, "nested-two").await;
    let session = match connection
        .request(&ClientMessage::Node(NodeRequest::AddTab {
            request_id: new_request_id(),
            tabs_node_id,
            title: "two".to_owned(),
            buffer_id: Some(buffer_c.id),
            child_node_id: None,
            index: 1,
        }))
        .await
        .expect("add nested tab succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let active_index = node(&session, tabs_node_id)
        .tabs
        .as_ref()
        .expect("tabs payload")
        .active;
    if active_index != 1 {
        let _ = connection
            .request(&ClientMessage::Node(NodeRequest::SelectTab {
                request_id: new_request_id(),
                tabs_node_id,
                index: 1,
            }))
            .await
            .expect("select nested tab succeeds");
    }

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_c.id,
            bytes: b"printf nested-two\\n\r".to_vec(),
        }))
        .await
        .expect("send to nested tab succeeds");
    connection
        .wait_for_capture_contains(buffer_c.id, "nested-two", Duration::from_secs(3))
        .await
        .expect("second nested tab outputs");

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let render = render_session(&mut client, "alpha").await;
    assert!(render.contains("nested-two"));

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id,
            index: 0,
        }))
        .await
        .expect("select first nested tab succeeds");
    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_b.id,
            bytes: b"printf nested-one\\n\r".to_vec(),
        }))
        .await
        .expect("send to first nested tab succeeds");
    connection
        .wait_for_capture_contains(buffer_b.id, "nested-one", Duration::from_secs(3))
        .await
        .expect("first nested tab outputs");

    let render = render_session(&mut client, "alpha").await;
    assert!(render.contains("nested-one"));
    assert!(!render.contains("nested-two"));

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn popup_close_preserves_underlying_buffer() {
    let server = TestServer::start().await.expect("server starts");

    run_cli(&server, &["new-session", "alpha"]);
    run_cli(
        &server,
        &[
            "new-window",
            "-t",
            "alpha",
            "--title",
            "work",
            "--",
            "/bin/sh",
        ],
    );

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    let base_leaf = snapshot
        .session
        .focused_leaf_id
        .expect("focused leaf exists");
    let base_buffer = buffer_for_leaf(&snapshot, base_leaf);

    run_cli(
        &server,
        &["send-keys", "--enter", "printf", "popup-base\\n"],
    );
    connection
        .wait_for_capture_contains(base_buffer, "popup-base", Duration::from_secs(3))
        .await
        .expect("base pane captures output");

    let created = run_cli(
        &server,
        &[
            "display-popup",
            "-t",
            "alpha",
            "--title",
            "scratch",
            "--x",
            "2",
            "--y",
            "1",
            "--width",
            "20",
            "--height",
            "6",
            "--",
            "/bin/sh",
        ],
    );
    let popup_id = stdout(&created)
        .trim()
        .parse::<u64>()
        .expect("display-popup returns popup id");

    run_cli(&server, &["kill-popup", "-t", &popup_id.to_string()]);

    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert!(snapshot.floating.is_empty());
    connection
        .wait_for_capture_contains(base_buffer, "popup-base", Duration::from_secs(3))
        .await
        .expect("base pane survives popup lifecycle");

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_and_detach_workflows_preserve_running_buffers() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer_a = create_buffer(&mut connection, "main").await;
    let session = match connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "main".to_owned(),
            buffer_id: Some(buffer_a.id),
            child_node_id: None,
        }))
        .await
        .expect("add root tab succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let main_leaf = session.session.focused_leaf_id.expect("main leaf exists");

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            bytes: b"printf moved-buffer\\n\r".to_vec(),
        }))
        .await
        .expect("send input succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "moved-buffer", Duration::from_secs(3))
        .await
        .expect("buffer output arrives");

    let _ = connection
        .request(&ClientMessage::Buffer(BufferRequest::Detach {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
        }))
        .await
        .expect("detach succeeds");

    let popup = match connection
        .request(&ClientMessage::Floating(FloatingRequest::Create {
            request_id: new_request_id(),
            session_id: session.session.id,
            root_node_id: None,
            buffer_id: Some(buffer_a.id),
            geometry: FloatGeometry::new(4, 2, 24, 8),
            title: Some("moved".to_owned()),
            focus: true,
            close_on_empty: true,
        }))
        .await
        .expect("create floating from detached buffer succeeds")
    {
        ServerResponse::Floating(response) => response.floating,
        other => panic!("expected floating response, got {other:?}"),
    };
    connection
        .wait_for_capture_contains(buffer_a.id, "moved-buffer", Duration::from_secs(3))
        .await
        .expect("buffer survives floating move");

    let _ = connection
        .request(&ClientMessage::Floating(FloatingRequest::Close {
            request_id: new_request_id(),
            floating_id: popup.id,
        }))
        .await
        .expect("close floating succeeds");

    let buffer_b = create_buffer(&mut connection, "target").await;
    let session = match connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "target".to_owned(),
            buffer_id: Some(buffer_b.id),
            child_node_id: None,
        }))
        .await
        .expect("add target window succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let target_leaf = session.session.focused_leaf_id.expect("target leaf exists");

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::MoveBufferToNode {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            target_leaf_node_id: target_leaf,
        }))
        .await
        .expect("reattach detached buffer succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "moved-buffer", Duration::from_secs(3))
        .await
        .expect("buffer survives reattach");

    let detached = match connection
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: new_request_id(),
            session_id: None,
            attached_only: false,
            detached_only: true,
        }))
        .await
        .expect("list detached buffers succeeds")
    {
        ServerResponse::Buffers(BuffersResponse { buffers, .. }) => buffers,
        other => panic!("expected buffers response, got {other:?}"),
    };
    assert!(detached.iter().any(|buffer| buffer.id == buffer_b.id));
    assert_ne!(main_leaf, target_leaf);

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_activity_is_visible_and_reconnect_rehydrates_state() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer_a = create_buffer(&mut connection, "main").await;
    let session = match connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "main".to_owned(),
            buffer_id: Some(buffer_a.id),
            child_node_id: None,
        }))
        .await
        .expect("add root tab succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let main_leaf = session.session.focused_leaf_id.expect("main leaf exists");

    let session = match connection
        .request(&ClientMessage::Node(NodeRequest::WrapInTabs {
            request_id: new_request_id(),
            node_id: main_leaf,
            title: "main".to_owned(),
        }))
        .await
        .expect("wrap main leaf in tabs succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot response, got {other:?}"),
    };
    let nested_tabs_id = node(&session, main_leaf)
        .parent_id
        .expect("wrapped main leaf has tabs parent");

    let buffer_b = create_buffer(&mut connection, "hidden").await;
    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::AddTab {
            request_id: new_request_id(),
            tabs_node_id: nested_tabs_id,
            title: "bg".to_owned(),
            buffer_id: Some(buffer_b.id),
            child_node_id: None,
            index: 1,
        }))
        .await
        .expect("add hidden tab succeeds");
    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: nested_tabs_id,
            index: 0,
        }))
        .await
        .expect("select visible tab succeeds");

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_b.id,
            bytes: b"printf hidden-activity\\n\r".to_vec(),
        }))
        .await
        .expect("send to hidden buffer succeeds");
    connection
        .wait_for_capture_contains(buffer_b.id, "hidden-activity", Duration::from_secs(3))
        .await
        .expect("hidden buffer captures output");

    let mut first_client = MuxClient::connect(server.socket_path())
        .await
        .expect("first client connects");
    first_client
        .resync_all_sessions()
        .await
        .expect("first client resyncs");
    refresh_all_snapshots(&mut first_client).await;
    let session_id = session_id_by_name(&first_client, "alpha");
    let model = PresentationModel::project(
        first_client.state(),
        session_id,
        Size {
            width: 80,
            height: 24,
        },
    )
    .expect("projection succeeds");
    let tabs = model
        .tab_bars
        .iter()
        .find(|tabs| tabs.node_id == nested_tabs_id)
        .expect("nested tabs frame exists");
    assert!(
        tabs.tabs
            .iter()
            .any(|tab| tab.title == "bg" && tab.activity != ActivityState::Idle)
    );

    drop(first_client);

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: nested_tabs_id,
            index: 1,
        }))
        .await
        .expect("select hidden tab succeeds");

    let mut second_client = MuxClient::connect(server.socket_path())
        .await
        .expect("second client connects");
    let render = render_session(&mut second_client, "alpha").await;
    assert!(render.contains("hidden-activity"));

    server.shutdown().await.expect("server shuts down");
}
