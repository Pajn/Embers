use std::process::Output;
use std::time::Duration;

use embers_client::{MuxClient, PresentationModel, Renderer};
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, SessionId, Size, SplitDirection, new_request_id,
};
use embers_protocol::{
    BufferRecord, BufferRequest, BufferResponse, BuffersResponse, ClientMessage, FloatingRequest,
    NodeRequest, ServerResponse, SessionRequest, SessionSnapshot, VisibleSnapshotResponse,
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
    create_buffer_with_command(connection, title, vec!["/bin/sh".to_owned()]).await
}

async fn create_buffer_with_command(
    connection: &mut TestConnection,
    title: &str,
    command: Vec<String>,
) -> embers_protocol::BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some(title.to_owned()),
            command,
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

async fn wait_for_visible_snapshot<F>(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    timeout: Duration,
    mut predicate: F,
) -> VisibleSnapshotResponse
where
    F: FnMut(&VisibleSnapshotResponse) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let snapshot = connection
            .capture_visible_buffer(buffer_id)
            .await
            .expect("visible capture succeeds");
        if predicate(&snapshot) {
            return snapshot;
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for visible snapshot; last snapshot: {snapshot:?}");
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn buffer_record(connection: &mut TestConnection, buffer_id: BufferId) -> BufferRecord {
    match connection
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("get buffer succeeds")
    {
        ServerResponse::Buffer(BufferResponse { buffer, .. }) => buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn wait_for_buffer_activity(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    expected: ActivityState,
    timeout: Duration,
) -> BufferRecord {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let buffer = buffer_record(connection, buffer_id).await;
        if buffer.activity == expected {
            return buffer;
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for buffer {buffer_id} activity {expected:?}; last activity: {:?}",
                buffer.activity
            );
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct HiddenTabFixture {
    nested_tabs_id: NodeId,
    hidden_buffer: BufferRecord,
}

async fn create_hidden_tab_fixture(connection: &mut TestConnection) -> HiddenTabFixture {
    let hidden_buffer = create_buffer(connection, "hidden").await;
    create_hidden_tab_fixture_with_buffer(connection, hidden_buffer).await
}

async fn create_hidden_tab_fixture_with_buffer(
    connection: &mut TestConnection,
    hidden_buffer: BufferRecord,
) -> HiddenTabFixture {
    let session = create_session(connection, "alpha").await;
    let buffer_a = create_buffer(connection, "main").await;
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

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::AddTab {
            request_id: new_request_id(),
            tabs_node_id: nested_tabs_id,
            title: "bg".to_owned(),
            buffer_id: Some(hidden_buffer.id),
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

    HiddenTabFixture {
        nested_tabs_id,
        hidden_buffer,
    }
}

fn fullscreen_fixture_command(
    live_title: &str,
    restored_title: &str,
    sleep_secs: &str,
) -> Vec<String> {
    vec![
        "/bin/sh".to_owned(),
        "-lc".to_owned(),
        format!(
            "printf 'main-before\\n'; \
             printf '\\033]0;{live_title}\\007\\033[?1049h\\033[2J\\033[Hfullscreen-live\\033[3;10Hcursor-target'; \
             sleep {sleep_secs}; \
             printf '\\033]0;{restored_title}\\007\\033[?1049lrestored-after\\n'"
        ),
    ]
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
    run_cli(
        &server,
        &["send-keys", "--enter", "printf", "popup-after-close\\n"],
    );

    let snapshot = session_snapshot_by_name(&mut connection, "alpha").await;
    assert!(snapshot.floating.is_empty());
    connection
        .wait_for_capture_contains(base_buffer, "popup-after-close", Duration::from_secs(3))
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
    assert!(
        buffer_record(&mut connection, buffer_a.id)
            .await
            .attachment_node_id
            .is_none()
    );

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            bytes: b"printf detached-output\\n\r".to_vec(),
        }))
        .await
        .expect("send detached output succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "detached-output", Duration::from_secs(3))
        .await
        .expect("detached buffer captures output");
    wait_for_buffer_activity(
        &mut connection,
        buffer_a.id,
        ActivityState::Activity,
        Duration::from_secs(3),
    )
    .await;

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            bytes: b"printf 'detached-bell\\a\\n'; sleep 0.5\r".to_vec(),
        }))
        .await
        .expect("send detached bell succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "detached-bell", Duration::from_secs(3))
        .await
        .expect("detached buffer captures bell marker");
    wait_for_buffer_activity(
        &mut connection,
        buffer_a.id,
        ActivityState::Bell,
        Duration::from_secs(3),
    )
    .await;

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
    assert_eq!(
        buffer_record(&mut connection, buffer_a.id).await.activity,
        ActivityState::Idle
    );
    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            bytes: b"printf moved-buffer-floating\\n\r".to_vec(),
        }))
        .await
        .expect("send floating marker succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "moved-buffer-floating", Duration::from_secs(3))
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
    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer_a.id,
            bytes: b"printf moved-buffer-reattach\\n\r".to_vec(),
        }))
        .await
        .expect("send reattach marker succeeds");
    connection
        .wait_for_capture_contains(buffer_a.id, "moved-buffer-reattach", Duration::from_secs(3))
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

    let fixture = create_hidden_tab_fixture(&mut connection).await;

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: fixture.hidden_buffer.id,
            bytes: b"printf hidden-activity\\n\r".to_vec(),
        }))
        .await
        .expect("send to hidden buffer succeeds");
    connection
        .wait_for_capture_contains(
            fixture.hidden_buffer.id,
            "hidden-activity",
            Duration::from_secs(3),
        )
        .await
        .expect("hidden buffer captures output");
    wait_for_buffer_activity(
        &mut connection,
        fixture.hidden_buffer.id,
        ActivityState::Activity,
        Duration::from_secs(3),
    )
    .await;

    let mut first_client = MuxClient::connect(server.socket_path())
        .await
        .expect("first client connects");
    let mut saw_hidden_activity = false;
    for _ in 0..10 {
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
            .find(|tabs| tabs.node_id == fixture.nested_tabs_id)
            .expect("nested tabs frame exists");
        if tabs
            .tabs
            .iter()
            .any(|tab| tab.title == "bg" && tab.activity == ActivityState::Activity)
        {
            saw_hidden_activity = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_hidden_activity,
        "hidden tab activity should propagate before reconnect"
    );

    drop(first_client);

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: fixture.nested_tabs_id,
            index: 1,
        }))
        .await
        .expect("select hidden tab succeeds");
    wait_for_buffer_activity(
        &mut connection,
        fixture.hidden_buffer.id,
        ActivityState::Idle,
        Duration::from_secs(3),
    )
    .await;

    let mut second_client = MuxClient::connect(server.socket_path())
        .await
        .expect("second client connects");
    let render = render_session(&mut second_client, "alpha").await;
    assert!(render.contains("hidden-activity"));

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_bell_is_visible_to_clients_until_revealed() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let fixture = create_hidden_tab_fixture(&mut connection).await;

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: fixture.hidden_buffer.id,
            bytes: b"printf 'hidden-bell\\a\\n'; sleep 0.5\r".to_vec(),
        }))
        .await
        .expect("send hidden bell succeeds");
    connection
        .wait_for_capture_contains(
            fixture.hidden_buffer.id,
            "hidden-bell",
            Duration::from_secs(3),
        )
        .await
        .expect("hidden bell marker appears");
    wait_for_buffer_activity(
        &mut connection,
        fixture.hidden_buffer.id,
        ActivityState::Bell,
        Duration::from_secs(3),
    )
    .await;

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    client
        .resync_all_sessions()
        .await
        .expect("client resyncs sessions");
    refresh_all_snapshots(&mut client).await;
    let session_id = session_id_by_name(&client, "alpha");
    let model = PresentationModel::project(
        client.state(),
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
        .find(|tabs| tabs.node_id == fixture.nested_tabs_id)
        .expect("nested tabs frame exists");
    assert!(
        tabs.tabs
            .iter()
            .any(|tab| tab.title == "bg" && tab.activity == ActivityState::Bell)
    );

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: fixture.nested_tabs_id,
            index: 1,
        }))
        .await
        .expect("select hidden tab succeeds");
    wait_for_buffer_activity(
        &mut connection,
        fixture.hidden_buffer.id,
        ActivityState::Idle,
        Duration::from_secs(3),
    )
    .await;

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fullscreen_fixture_enters_alternate_screen_and_restores_primary_screen() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer = create_buffer_with_command(
        &mut connection,
        "fullscreen",
        fullscreen_fixture_command("fullscreen-live-title", "primary-restored-title", "1.0"),
    )
    .await;
    let _ = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "fullscreen".to_owned(),
            buffer_id: Some(buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add fullscreen tab succeeds");

    let live = wait_for_visible_snapshot(
        &mut connection,
        buffer.id,
        Duration::from_secs(3),
        |snapshot| {
            let text = snapshot.lines.join("\n");
            snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-live-title")
                && text.contains("fullscreen-live")
                && text.contains("cursor-target")
        },
    )
    .await;
    let live_text = live.lines.join("\n");
    assert!(!live_text.contains("main-before"));

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let render = render_session(&mut client, "alpha").await;
    assert!(render.contains("fullscreen-live"));
    assert!(render.contains("cursor-target"));
    assert!(!render.contains("main-before"));

    let restored = wait_for_visible_snapshot(
        &mut connection,
        buffer.id,
        Duration::from_secs(4),
        |snapshot| {
            let text = snapshot.lines.join("\n");
            !snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("primary-restored-title")
                && text.contains("main-before")
                && text.contains("restored-after")
        },
    )
    .await;
    let restored_text = restored.lines.join("\n");
    assert!(!restored_text.contains("fullscreen-live"));

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_fullscreen_buffer_reveals_live_alternate_screen_coherently() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let hidden_buffer = create_buffer_with_command(
        &mut connection,
        "fullscreen-hidden",
        fullscreen_fixture_command(
            "fullscreen-hidden-live",
            "fullscreen-hidden-restored",
            "1.2",
        ),
    )
    .await;
    let fixture = create_hidden_tab_fixture_with_buffer(&mut connection, hidden_buffer).await;

    wait_for_visible_snapshot(
        &mut connection,
        fixture.hidden_buffer.id,
        Duration::from_secs(3),
        |snapshot| {
            snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-hidden-live")
                && snapshot.lines.join("\n").contains("fullscreen-live")
        },
    )
    .await;

    let _ = connection
        .request(&ClientMessage::Node(NodeRequest::SelectTab {
            request_id: new_request_id(),
            tabs_node_id: fixture.nested_tabs_id,
            index: 1,
        }))
        .await
        .expect("select fullscreen tab succeeds");

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let live_render = render_session(&mut client, "alpha").await;
    assert!(live_render.contains("fullscreen-live"));
    assert!(live_render.contains("cursor-target"));
    assert!(!live_render.contains("main-before"));

    let restored = wait_for_visible_snapshot(
        &mut connection,
        fixture.hidden_buffer.id,
        Duration::from_secs(4),
        |snapshot| {
            let text = snapshot.lines.join("\n");
            !snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-hidden-restored")
                && text.contains("main-before")
                && text.contains("restored-after")
        },
    )
    .await;
    assert!(!restored.lines.join("\n").contains("fullscreen-live"));

    let restored_render = render_session(&mut client, "alpha").await;
    assert!(restored_render.contains("main-before"));
    assert!(restored_render.contains("restored-after"));
    assert!(!restored_render.contains("fullscreen-live"));

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rapid_terminal_output_renders_latest_visible_snapshot() {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer = create_buffer_with_command(
        &mut connection,
        "burst",
        vec![
            "/bin/sh".to_owned(),
            "-lc".to_owned(),
            "i=1; while [ $i -le 80 ]; do printf 'burst-%02d\\n' \"$i\"; i=$((i+1)); done"
                .to_owned(),
        ],
    )
    .await;
    let _ = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "burst".to_owned(),
            buffer_id: Some(buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add burst tab succeeds");

    connection
        .wait_for_capture_contains(buffer.id, "burst-80", Duration::from_secs(3))
        .await
        .expect("rapid output finishes");
    wait_for_visible_snapshot(
        &mut connection,
        buffer.id,
        Duration::from_secs(3),
        |snapshot| snapshot.total_lines >= 80 && snapshot.lines.join("\n").contains("burst-80"),
    )
    .await;

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let render = render_session(&mut client, "alpha").await;
    let session_id = session_id_by_name(&client, "alpha");
    let presentation = PresentationModel::project(
        client.state(),
        session_id,
        Size {
            width: 80,
            height: 24,
        },
    )
    .expect("projection succeeds");
    let visible_rows = presentation
        .focused_leaf()
        .expect("focused leaf")
        .rect
        .size
        .height
        .saturating_sub(1) as usize;
    let latest_rendered_line = client
        .state()
        .snapshots
        .get(&buffer.id)
        .expect("burst snapshot")
        .lines
        .iter()
        .take(visible_rows)
        .rev()
        .find(|line| line.starts_with("burst-"))
        .expect("latest rendered burst line");
    assert!(render.contains(latest_rendered_line));
    assert!(!render.contains("burst-01"));

    server.shutdown().await.expect("server shuts down");
}
