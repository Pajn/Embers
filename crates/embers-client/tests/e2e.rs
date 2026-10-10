use std::process::Output;

use embers_client::{MuxClient, PresentationModel, Renderer};
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, SessionId, Size, SplitDirection, new_request_id,
};
use embers_protocol::{
    BufferRecord, BufferRequest, BufferResponse, BuffersResponse, ClientMessage, FloatingRequest,
    NodeRequest, ServerResponse, SessionRequest, SessionSnapshot, VisibleSnapshotResponse,
};
use embers_test_support::{TestConnection, TestServer, cargo_bin};
use tokio::time::{Duration, Instant};

/// Join styled snapshot lines into newline-delimited plain text for assertions.
fn lines_text(lines: &[embers_core::SnapshotLine]) -> String {
    lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

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

/// Create two sessions ("alpha", "beta"), each with a single shell window.
fn two_sessions_with_shells(server: &TestServer) {
    run_cli(server, &["new-session", "alpha"]);
    run_cli(server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);
    run_cli(server, &["new-session", "beta"]);
    run_cli(server, &["new-window", "-t", "beta", "--", "/bin/sh"]);
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
            size: None,
            session_id: None,
        }))
        .await
        .expect("create buffer succeeds");
    match response {
        ServerResponse::Buffer(BufferResponse { buffer, .. }) => buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

/// Keep `buffer_id` in its pane after its command exits (`remain-on-exit on`),
/// so a test can still render the output of a command that has finished.
async fn remain_on_exit(connection: &mut TestConnection, buffer_id: BufferId) {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::SetUserOption {
            request_id: new_request_id(),
            buffer_id,
            key: "remain-on-exit".to_owned(),
            value: Some("on".to_owned()),
        }))
        .await
        .expect("set remain-on-exit succeeds");
    assert!(
        matches!(response, ServerResponse::Buffer(_)),
        "expected buffer response, got {response:?}"
    );
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

async fn refresh_all_snapshots(
    client: &mut MuxClient<embers_client::SocketTransport>,
) -> embers_core::Result<()> {
    let buffer_ids = client.state().buffers.keys().copied().collect::<Vec<_>>();
    for buffer_id in buffer_ids {
        let _ = client.refresh_buffer_snapshot(buffer_id).await;
    }
    Ok(())
}

async fn render_session(
    client: &mut MuxClient<embers_client::SocketTransport>,
    session_name: &str,
) -> String {
    client.resync_all_sessions().await.expect("resync succeeds");
    refresh_all_snapshots(client)
        .await
        .expect("refresh snapshots succeeds");
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

async fn poll_for_tab_activity(
    client: &mut MuxClient<embers_client::SocketTransport>,
    session_name: &str,
    tabs_node_id: NodeId,
    target_title: &str,
    expected_activity: ActivityState,
    timeout: Duration,
    poll_interval: Duration,
) -> embers_core::Result<bool> {
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        match tokio::time::timeout(remaining, client.resync_all_sessions()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => return Ok(false),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        match tokio::time::timeout(remaining, refresh_all_snapshots(client)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => return Ok(false),
        }
        let maybe_session_id = client
            .state()
            .sessions
            .values()
            .find(|session| session.name == session_name)
            .map(|session| session.id);
        if let Some(session_id) = maybe_session_id
            && let Ok(model) = PresentationModel::project(
                client.state(),
                session_id,
                Size {
                    width: 80,
                    height: 24,
                },
            )
            && let Some(tabs) = model
                .tab_bars
                .iter()
                .find(|tabs| tabs.node_id == tabs_node_id)
            && tabs
                .tabs
                .iter()
                .any(|tab| tab.title == target_title && tab.activity == expected_activity)
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(poll_interval).await;
    }
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
    let deadline = Instant::now() + timeout;
    let mut last_snapshot = "<none>".to_owned();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let snapshot =
            tokio::time::timeout(remaining, connection.capture_visible_buffer(buffer_id))
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out waiting for visible snapshot; last snapshot: {last_snapshot}")
                })
                .expect("visible capture succeeds");
        if predicate(&snapshot) {
            return snapshot;
        }
        last_snapshot = format!("{snapshot:?}");

        if Instant::now() >= deadline {
            panic!("timed out waiting for visible snapshot; last snapshot: {last_snapshot}");
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
    let deadline = Instant::now() + timeout;
    let mut last_activity = "<none>".to_owned();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let buffer = tokio::time::timeout(remaining, buffer_record(connection, buffer_id))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for buffer {buffer_id} activity {expected:?}; last activity: {last_activity}"
                )
            });
        if buffer.activity == expected {
            return buffer;
        }
        last_activity = format!("{:?}", buffer.activity);

        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for buffer {buffer_id} activity {expected:?}; last activity: {last_activity}"
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
            bytes: b"printf 'detached-bell'; printf '\\a'; sleep 0.5\r".to_vec(),
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
    assert_eq!(
        buffer_record(&mut connection, buffer_a.id)
            .await
            .attachment_node_id,
        Some(target_leaf),
        "buffer should be reattached to the requested target leaf"
    );
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
    let saw_hidden_activity = poll_for_tab_activity(
        &mut first_client,
        "alpha",
        fixture.nested_tabs_id,
        "bg",
        ActivityState::Activity,
        Duration::from_secs(2),
        Duration::from_millis(50),
    )
    .await
    .expect("poll hidden activity succeeds");
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
            bytes: b"printf 'hidden-bell'; printf '\\a'; sleep 0.5\r".to_vec(),
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
    refresh_all_snapshots(&mut client)
        .await
        .expect("refresh snapshots succeeds");
    let saw_hidden_bell = poll_for_tab_activity(
        &mut client,
        "alpha",
        fixture.nested_tabs_id,
        "bg",
        ActivityState::Bell,
        Duration::from_secs(2),
        Duration::from_millis(50),
    )
    .await
    .expect("poll hidden bell succeeds");
    assert!(
        saw_hidden_bell,
        "timed out waiting for bell activity after reconnect"
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
    let cleared_hidden_bell = poll_for_tab_activity(
        &mut client,
        "alpha",
        fixture.nested_tabs_id,
        "bg",
        ActivityState::Idle,
        Duration::from_secs(2),
        Duration::from_millis(50),
    )
    .await
    .expect("poll cleared hidden bell succeeds");
    assert!(
        cleared_hidden_bell,
        "hidden tab kept Bell activity after being revealed"
    );

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
        fullscreen_fixture_command("fullscreen-live-title", "primary-restored-title", "3.0"),
    )
    .await;
    remain_on_exit(&mut connection, buffer.id).await;
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
            let text = lines_text(&snapshot.lines);
            snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-live-title")
                && text.contains("fullscreen-live")
                && text.contains("cursor-target")
        },
    )
    .await;
    let live_text = lines_text(&live.lines);
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
            let text = lines_text(&snapshot.lines);
            !snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("primary-restored-title")
                && text.contains("main-before")
                && text.contains("restored-after")
        },
    )
    .await;
    let restored_text = lines_text(&restored.lines);
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
            "4.0",
        ),
    )
    .await;
    remain_on_exit(&mut connection, hidden_buffer.id).await;
    let fixture = create_hidden_tab_fixture_with_buffer(&mut connection, hidden_buffer).await;

    wait_for_visible_snapshot(
        &mut connection,
        fixture.hidden_buffer.id,
        Duration::from_secs(3),
        |snapshot| {
            snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-hidden-live")
                && lines_text(&snapshot.lines).contains("fullscreen-live")
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
        Duration::from_secs(6),
        |snapshot| {
            let text = lines_text(&snapshot.lines);
            !snapshot.alternate_screen
                && snapshot.title.as_deref() == Some("fullscreen-hidden-restored")
                && text.contains("main-before")
                && text.contains("restored-after")
        },
    )
    .await;
    assert!(!lines_text(&restored.lines).contains("fullscreen-live"));

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
    remain_on_exit(&mut connection, buffer.id).await;
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
    let visible_snapshot = wait_for_visible_snapshot(
        &mut connection,
        buffer.id,
        Duration::from_secs(3),
        |snapshot| snapshot.total_lines >= 80 && lines_text(&snapshot.lines).contains("burst-80"),
    )
    .await;

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let render = render_session(&mut client, "alpha").await;
    let latest_rendered_line = visible_snapshot
        .lines
        .iter()
        .rev()
        .find(|line| line.text.starts_with("burst-"))
        .expect("latest rendered burst line");
    assert!(render.contains(&latest_rendered_line.text));
    assert!(!render.contains("burst-01"));

    server.shutdown().await.expect("server shuts down");
}

/// Run `script` under `/bin/sh` in a fresh pane, wait until `marker` shows in its
/// visible snapshot, then render the session through a separate client the way
/// the interactive client does. Returns the rendered ANSI and plain text.
async fn render_pane_output(script: &str, marker: &str) -> (String, String) {
    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");

    let session = create_session(&mut connection, "alpha").await;
    let buffer = create_buffer_with_command(
        &mut connection,
        "styled",
        vec!["/bin/sh".to_owned(), "-lc".to_owned(), script.to_owned()],
    )
    .await;
    // The script may finish before the pane is rendered.
    remain_on_exit(&mut connection, buffer.id).await;
    let _ = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "styled".to_owned(),
            buffer_id: Some(buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add styled tab succeeds");

    connection
        .wait_for_capture_contains(buffer.id, marker, Duration::from_secs(3))
        .await
        .expect("styled output renders");
    let _ = wait_for_visible_snapshot(
        &mut connection,
        buffer.id,
        Duration::from_secs(3),
        |snapshot| lines_text(&snapshot.lines).contains(marker),
    )
    .await;

    let mut client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    client.resync_all_sessions().await.expect("resync succeeds");
    refresh_all_snapshots(&mut client)
        .await
        .expect("refresh snapshots succeeds");
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
    let grid = Renderer.render(client.state(), &model);
    let rendered = (grid.ansi_lines().join("\n"), grid.render());

    server.shutdown().await.expect("server shuts down");
    rendered
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn styled_pane_output_reaches_client_ansi_lines() {
    // Emit red text via SGR, then idle so the pane persists while we render.
    let (ansi, plain) =
        render_pane_output("printf '\\033[31mRED-TEXT\\033[0m\\n'; sleep 2", "RED-TEXT").await;

    // ANSI red maps to indexed color 1; the client re-emits `38;5;1m` so the
    // outer terminal's palette resolves it (semantic colors are not baked to RGB).
    assert!(
        ansi.contains("\x1b[38;5;1m"),
        "expected indexed red SGR in client output:\n{ansi:?}"
    );
    assert!(plain.contains("RED-TEXT"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undercurl_and_underline_color_reach_client_ansi_lines() {
    // What Neovim emits for a diagnostic: a curly underline in true-color red.
    let (ansi, plain) = render_pane_output(
        "printf '\\033[4:3m\\033[58:2::255:0:0mDIAG\\033[0m\\n'; sleep 2",
        "DIAG",
    )
    .await;

    assert!(
        ansi.contains("\x1b[4:3m\x1b[58:2::255:0:0mDIAG"),
        "expected curly underline and its color in client output:\n{ansi:?}"
    );
    assert!(plain.contains("DIAG"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn osc8_hyperlink_reaches_client_ansi_lines() {
    let (ansi, plain) = render_pane_output(
        "printf '\\033]8;id=doc;https://example.com/doc\\033\\\\DOCLINK\\033]8;;\\033\\\\ after\\n'; sleep 2",
        "DOCLINK",
    )
    .await;

    assert!(
        ansi.contains("\x1b]8;id=doc;https://example.com/doc\x1b\\DOCLINK\x1b]8;;\x1b\\ after"),
        "expected an OSC 8 span around the link text in client output:\n{ansi:?}"
    );
    assert!(plain.contains("DOCLINK after"));
}

/// Attach a configured client to a session whose pane copies "hi" with OSC 52
/// once it reads a line, trigger the copy, and return the client's host
/// terminal output collected until the clipboard event was handled.
async fn pane_osc52_output(config_source: &str) -> Vec<u8> {
    use embers_client::ConfiguredClient;

    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let session = create_session(&mut connection, "alpha").await;
    // Wait for a line first so the copy happens after the client is attached.
    let buffer = create_buffer_with_command(
        &mut connection,
        "yank",
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "read _; printf '\\033]52;c;%s\\a' $(printf hi | base64); sleep 5".to_owned(),
        ],
    )
    .await;
    let _ = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "yank".to_owned(),
            buffer_id: Some(buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add yank tab succeeds");

    let config_dir = tempfile::tempdir().expect("config tempdir");
    std::fs::write(config_dir.path().join("config.rhai"), config_source).expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(config_dir.path()),
    )
    .expect("load config");
    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .subscribe(None)
        .await
        .expect("subscribe");
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    configured
        .client_mut()
        .switch_current_session(session.session.id)
        .await
        .expect("switch to alpha");

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer.id,
            bytes: b"\r".to_vec(),
        }))
        .await
        .expect("trigger copy");

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "clipboard event never arrived");
        let event = configured
            .process_next_event_timeout(Duration::from_millis(100))
            .await
            .expect("process event");
        output.extend(configured.drain_terminal_output().concat());
        if matches!(event, Some(embers_protocol::ServerEvent::Clipboard(_))) {
            break;
        }
    }
    server.shutdown().await.expect("server shuts down");
    output
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn osc52_from_pane_reaches_client_terminal_output() {
    let output = pane_osc52_output("").await;
    assert!(
        contains_subslice(&output, b"\x1b]52;c;aGk=\x07"),
        "expected OSC 52 for \"hi\" in {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn osc52_from_pane_is_dropped_when_clipboard_is_off() {
    let output = pane_osc52_output("set_clipboard(false);").await;
    assert!(
        !contains_subslice(&output, b"\x1b]52;"),
        "expected no OSC 52 in {:?}",
        String::from_utf8_lossy(&output)
    );
}

/// What a client wrote to its host terminal after drawing a pane whose program
/// sent a tmux passthrough sequence.
struct PassthroughOutput {
    /// Bytes written after the frame.
    frame_output: Vec<u8>,
    /// The 1-based host position of the pane's cell (row 3, column 5), or
    /// `None` when the pane was not on screen.
    pane_cursor: Option<(i32, i32)>,
}

/// Attach a configured client to a session whose pane runs `script` once it
/// reads a line, trigger it, wait for the client to handle the passthrough
/// event that marks the end of the script's output (`ESC ] 9 ; done BEL`),
/// then draw a frame. With `hidden`, the pane is in a background window.
async fn pane_passthrough_output(
    config_source: &str,
    script: &str,
    hidden: bool,
) -> PassthroughOutput {
    use embers_client::ConfiguredClient;

    let server = TestServer::start().await.expect("server starts");
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let session = create_session(&mut connection, "alpha").await;
    let buffer = create_buffer_with_command(
        &mut connection,
        "images",
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!(
                "read _; {script}; printf '\\033Ptmux;\\033\\033]9;done\\007\\033\\\\'; sleep 5"
            ),
        ],
    )
    .await;
    let _ = connection
        .request(&ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: new_request_id(),
            session_id: session.session.id,
            title: "images".to_owned(),
            buffer_id: Some(buffer.id),
            child_node_id: None,
        }))
        .await
        .expect("add images tab succeeds");
    if hidden {
        let front = create_buffer(&mut connection, "front").await;
        let _ = connection
            .request(&ClientMessage::Session(SessionRequest::AddRootTab {
                request_id: new_request_id(),
                session_id: session.session.id,
                title: "front".to_owned(),
                buffer_id: Some(front.id),
                child_node_id: None,
            }))
            .await
            .expect("add front tab succeeds");
        let _ = connection
            .request(&ClientMessage::Session(SessionRequest::SelectRootTab {
                request_id: new_request_id(),
                session_id: session.session.id,
                index: 1,
            }))
            .await
            .expect("select front tab succeeds");
    }

    let config_dir = tempfile::tempdir().expect("config tempdir");
    std::fs::write(config_dir.path().join("config.rhai"), config_source).expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(config_dir.path()),
    )
    .expect("load config");
    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .subscribe(None)
        .await
        .expect("subscribe");
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    configured
        .client_mut()
        .switch_current_session(session.session.id)
        .await
        .expect("switch to alpha");
    let viewport = Size {
        width: 80,
        height: 24,
    };
    // Draw once so the client knows its layout before the program writes.
    let _ = configured
        .render_session(session.session.id, viewport)
        .await
        .expect("first frame");
    let _ = configured.drain_frame_output();

    let _ = connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id: buffer.id,
            bytes: b"\r".to_vec(),
        }))
        .await
        .expect("start the script");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "passthrough event never arrived");
        let event = configured
            .process_next_event_timeout(Duration::from_millis(100))
            .await
            .expect("process event");
        if let Some(embers_protocol::ServerEvent::Passthrough(event)) = event
            && event
                .writes
                .iter()
                .any(|write| write.bytes == b"\x1b]9;done\x07")
        {
            break;
        }
    }
    let _ = configured.drain_terminal_output();
    let _ = configured
        .render_session(session.session.id, viewport)
        .await
        .expect("frame after passthrough");
    let frame_output = configured.drain_frame_output();

    let model =
        PresentationModel::project(configured.client().state(), session.session.id, viewport)
            .expect("projection succeeds");
    let pane_cursor = model
        .leaves
        .iter()
        .find(|leaf| leaf.buffer_id == buffer.id)
        .map(|leaf| (leaf.rect.origin.y + 1 + 2 + 1, leaf.rect.origin.x + 4 + 1));
    assert_eq!(pane_cursor.is_none(), hidden, "pane visibility");
    server.shutdown().await.expect("server shuts down");
    PassthroughOutput {
        frame_output,
        pane_cursor,
    }
}

/// A kitty graphics command wrapped for tmux, as image.nvim sends it under tmux:
/// every ESC in the payload doubled.
const WRAPPED_KITTY_IMAGE: &str =
    "printf '\\033[3;5H\\033Ptmux;\\033\\033_Gf=100,a=T;iVBORw0KGgo=\\033\\033\\\\\\033\\\\'";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tmux_passthrough_reaches_the_host_at_the_pane_cursor() {
    let output = pane_passthrough_output("", WRAPPED_KITTY_IMAGE, false).await;
    let (row, col) = output.pane_cursor.expect("pane on screen");
    let expected = format!("\x1b[{row};{col}H\x1b_Gf=100,a=T;iVBORw0KGgo=\x1b\\");
    assert!(
        contains_subslice(&output.frame_output, expected.as_bytes()),
        "expected {expected:?} in {:?}",
        String::from_utf8_lossy(&output.frame_output)
    );
    // Unwrapped: no tmux introducer or doubled ESC reaches the host.
    assert!(!contains_subslice(&output.frame_output, b"tmux;"));
    assert!(!contains_subslice(&output.frame_output, b"\x1b\x1b"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_kitty_graphics_reach_the_host_at_the_pane_cursor() {
    // Sent without the tmux wrapper, as a program that does not know it runs
    // in a multiplexer would.
    let script = "printf '\\033[3;5H\\033_Gf=100,a=T;iVBORw0KGgo=\\033\\\\'";
    let output = pane_passthrough_output("", script, false).await;
    let (row, col) = output.pane_cursor.expect("pane on screen");
    let expected = format!("\x1b[{row};{col}H\x1b_Gf=100,a=T;iVBORw0KGgo=\x1b\\");
    assert!(
        contains_subslice(&output.frame_output, expected.as_bytes()),
        "expected {expected:?} in {:?}",
        String::from_utf8_lossy(&output.frame_output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tmux_passthrough_is_dropped_when_off() {
    let output = pane_passthrough_output(
        "set_allow_passthrough(\"off\");",
        WRAPPED_KITTY_IMAGE,
        false,
    )
    .await;
    assert!(
        output.frame_output.is_empty(),
        "expected no output, got {:?}",
        String::from_utf8_lossy(&output.frame_output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tmux_passthrough_from_a_hidden_pane_needs_all() {
    let output = pane_passthrough_output("", WRAPPED_KITTY_IMAGE, true).await;
    assert!(
        output.frame_output.is_empty(),
        "`on` forwards only from panes on screen, got {:?}",
        String::from_utf8_lossy(&output.frame_output)
    );

    let output =
        pane_passthrough_output("set_allow_passthrough(\"all\");", WRAPPED_KITTY_IMAGE, true).await;
    // Written as it is, in order, without moving the cursor: there is no pane
    // on screen to put it in.
    assert!(
        contains_subslice(
            &output.frame_output,
            b"\x1b_Gf=100,a=T;iVBORw0KGgo=\x1b\\\x1b]9;done\x07"
        ),
        "expected both writes in {:?}",
        String::from_utf8_lossy(&output.frame_output)
    );
    assert!(!contains_subslice(&output.frame_output, b"H\x1b_G"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_tmux_passthrough_is_dropped() {
    // One byte over the 4 MiB cap, then a small sequence that must still pass.
    let script = format!(
        "printf '\\033Ptmux;'; head -c {} /dev/zero | tr '\\0' x; printf '\\033\\\\'",
        embers_core::MAX_PASSTHROUGH_BYTES + 1
    );
    let output = pane_passthrough_output("", &script, false).await;
    assert!(
        !contains_subslice(&output.frame_output, b"xxxx"),
        "oversized payload was forwarded"
    );
    assert!(contains_subslice(&output.frame_output, b"\x1b]9;done\x07"));
}

fn session_switch_config() -> (embers_client::ConfigManager, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tempdir.path().join("config.rhai"),
        r#"
            fn go_alpha(ctx) { action.switch_session("alpha") }
            fn go_beta(ctx) { action.switch_session("beta") }
            fn go_last(ctx) { action.last_session() }
            fn go_next(ctx) { action.next_session() }
            fn go_ghost(ctx) { action.switch_session("ghost") }
            define_action("go-alpha", go_alpha);
            define_action("go-beta", go_beta);
            define_action("go-last", go_last);
            define_action("go-next", go_next);
            define_action("go-ghost", go_ghost);
            bind("normal", "p", "go-alpha");
            bind("normal", "o", "go-beta");
            bind("normal", "l", "go-last");
            bind("normal", "c", "go-next");
            bind("normal", "x", "go-ghost");
        "#,
    )
    .expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .expect("load config");
    (config, tempdir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_shell_spawns_with_socket_env_and_reports_failures() {
    use embers_client::{ConfiguredClient, KeyEvent};

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(&server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let out_dir = tempfile::tempdir().expect("tempdir");
    let socket_out = out_dir.path().join("socket.txt");
    let config_dir = tempfile::tempdir().expect("config tempdir");
    std::fs::write(
        config_dir.path().join("config.rhai"),
        format!(
            r#"
                fn write_socket(ctx) {{ action.run_shell("printf %s \"$EMBERS_SOCKET\" > {out}") }}
                fn fail(ctx) {{ action.run_shell("exit 3") }}
                fn nop(ctx) {{ action.noop() }}
                define_action("write-socket", write_socket);
                define_action("fail", fail);
                define_action("nop", nop);
                bind("normal", "w", "write-socket");
                bind("normal", "f", "fail");
                bind("normal", "z", "nop");
            "#,
            out = socket_out.display()
        ),
    )
    .expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(config_dir.path()),
    )
    .expect("load config");

    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let mut configured = ConfiguredClient::new(client, config);
    configured.set_socket_path(server.socket_path().to_path_buf());
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = configured
        .client()
        .state()
        .sessions
        .values()
        .find(|session| session.name == "alpha")
        .map(|session| session.id)
        .expect("alpha session");
    let size = Size {
        width: 80,
        height: 24,
    };

    // run_shell spawns with $EMBERS_SOCKET injected (proves spawn + env).
    configured
        .handle_key(alpha, size, KeyEvent::Char('w'))
        .await
        .expect("run-shell dispatch returns immediately");
    let deadline = Instant::now() + Duration::from_secs(3);
    let socket_string = server.socket_path().to_string_lossy().into_owned();
    loop {
        if let Ok(contents) = std::fs::read_to_string(&socket_out)
            && contents == socket_string
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "run-shell did not write socket path"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // A non-zero exit surfaces a warning, drained on the next tick.
    configured
        .handle_key(alpha, size, KeyEvent::Char('f'))
        .await
        .expect("failing run-shell dispatch returns immediately");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // The benign key triggers a drain of background notifications.
        configured
            .handle_key(alpha, size, KeyEvent::Char('z'))
            .await
            .expect("nop key");
        if configured
            .notifications()
            .iter()
            .any(|note| note.contains("run-shell") && note.contains("exited"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected a run-shell failure notification; got {:?}",
            configured.notifications()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hints_copy_and_callback_over_a_url() {
    use base64::Engine as _;
    use embers_client::{ConfiguredClient, KeyEvent};

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(
        &server,
        &[
            "new-window",
            "-t",
            "alpha",
            "--",
            "/bin/sh",
            "-lc",
            "printf 'X https://example.com/x Y\\n'; cat",
        ],
    );

    let config_dir = tempfile::tempdir().expect("config tempdir");
    std::fs::write(
        config_dir.path().join("config.rhai"),
        r#"
            fn go_hints(ctx) { action.enter_hints() }
            fn go_hints_cb(ctx) { action.enter_hints_with("capture") }
            fn capture(ctx) { action.notify("info", ctx.hint_selection()) }
            define_action("go-hints", go_hints);
            define_action("go-hints-cb", go_hints_cb);
            define_action("capture", capture);
            bind("normal", "h", "go-hints");
            bind("normal", "j", "go-hints-cb");
        "#,
    )
    .expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(config_dir.path()),
    )
    .expect("load config");

    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .subscribe(None)
        .await
        .expect("subscribe");
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = session_id_by_name(configured.client(), "alpha");
    let size = Size {
        width: 80,
        height: 24,
    };

    // Fetch the pane's visible snapshot (it rendered before we attached, so no
    // render event is pending) and retry until entering hints finds the URL.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let buffer_ids: Vec<_> = configured
            .client()
            .state()
            .buffers
            .keys()
            .copied()
            .collect();
        for buffer_id in buffer_ids {
            let _ = configured
                .client_mut()
                .refresh_buffer_snapshot(buffer_id)
                .await;
        }
        configured
            .handle_key(alpha, size, KeyEvent::Char('h'))
            .await
            .expect("enter hints");
        if configured.current_mode() == "hints" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "hints never found the URL; notes {:?}",
            configured.notifications()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = configured.drain_terminal_output();

    // The single match is labelled "a"; typing it copies the URL via OSC 52.
    configured
        .handle_key(alpha, size, KeyEvent::Char('a'))
        .await
        .expect("select hint");
    assert_eq!(configured.current_mode(), "normal");
    let out = configured.drain_terminal_output().concat();
    let encoded = base64::engine::general_purpose::STANDARD.encode("https://example.com/x");
    let expected = format!("\x1b]52;c;{encoded}\x07");
    assert!(
        contains_subslice(&out, expected.as_bytes()),
        "expected OSC 52 clipboard for the URL in {:?}",
        String::from_utf8_lossy(&out)
    );

    // The callback variant invokes the named action with ctx.hint_selection().
    let before = configured.notifications().len();
    configured
        .handle_key(alpha, size, KeyEvent::Char('j'))
        .await
        .expect("enter hints with callback");
    assert_eq!(configured.current_mode(), "hints");
    configured
        .handle_key(alpha, size, KeyEvent::Char('a'))
        .await
        .expect("select hint for callback");
    assert!(
        configured
            .notifications()
            .iter()
            .skip(before)
            .any(|note| note.contains("https://example.com/x")),
        "callback should notify with the selected URL; got {:?}",
        configured.notifications()
    );

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_title_follows_session_on_attach_switch_and_rename() {
    use embers_client::{ConfiguredClient, KeyEvent};

    async fn drain_until_contains(
        configured: &mut ConfiguredClient<embers_client::SocketTransport>,
        needle: &[u8],
    ) {
        let mut accumulated: Vec<u8> = configured.drain_terminal_output().concat();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !contains_subslice(&accumulated, needle) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for terminal output {:?}; got {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(&accumulated)
            );
            let _ = configured
                .process_next_event_timeout(Duration::from_millis(100))
                .await;
            accumulated.extend(configured.drain_terminal_output().concat());
        }
    }

    let server = TestServer::start().await.expect("server starts");
    two_sessions_with_shells(&server);

    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let (config, _tempdir) = session_switch_config();
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .subscribe(None)
        .await
        .expect("subscribe");
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = session_id_by_name(configured.client(), "alpha");
    let beta = session_id_by_name(configured.client(), "beta");
    let size = Size {
        width: 80,
        height: 24,
    };

    // Attach-time title emission: `interactive::run` emits the title at startup by
    // calling `emit_terminal_title` directly, which we exercise here — the full run
    // loop needs a real TTY and input threads, so this covers the direct emission.
    configured.emit_terminal_title(alpha);
    let drained = configured.drain_terminal_output().concat();
    assert!(
        contains_subslice(&drained, b"\x1b]2;alpha\x07"),
        "attach title missing in {:?}",
        String::from_utf8_lossy(&drained)
    );

    // Switch: the own-client ClientChanged updates the title.
    configured
        .handle_key(alpha, size, KeyEvent::Char('o'))
        .await
        .expect("switch to beta");
    assert_eq!(configured.active_session_id(), Some(beta));
    drain_until_contains(&mut configured, b"\x1b]2;beta\x07").await;

    // Rename of the active session updates the title.
    run_cli(&server, &["rename-session", "-t", "beta", "gamma"]);
    drain_until_contains(&mut configured, b"\x1b]2;gamma\x07").await;

    server.shutdown().await.expect("server shuts down");
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_switch_actions_move_between_sessions() {
    use embers_client::{ConfiguredClient, KeyEvent};

    let server = TestServer::start().await.expect("server starts");
    // Two sessions, each with a live shell pane.
    two_sessions_with_shells(&server);

    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let (config, _tempdir) = session_switch_config();
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync sessions");

    let session_id = |configured: &ConfiguredClient<_>, name: &str| {
        configured
            .client()
            .state()
            .sessions
            .values()
            .find(|session| session.name == name)
            .map(|session| session.id)
            .unwrap_or_else(|| panic!("session {name} exists"))
    };
    let alpha = session_id(&configured, "alpha");
    let beta = session_id(&configured, "beta");
    let size = Size {
        width: 80,
        height: 24,
    };

    // Switch by name: alpha -> beta.
    configured
        .handle_key(alpha, size, KeyEvent::Char('o'))
        .await
        .expect("switch to beta");
    assert_eq!(configured.active_session_id(), Some(beta));

    // Last-session toggles back to alpha, then forward to beta again.
    configured
        .handle_key(beta, size, KeyEvent::Char('l'))
        .await
        .expect("last session");
    assert_eq!(configured.active_session_id(), Some(alpha));
    configured
        .handle_key(alpha, size, KeyEvent::Char('l'))
        .await
        .expect("last session again");
    assert_eq!(configured.active_session_id(), Some(beta));

    // Cycle wraps across the two sessions.
    configured
        .handle_key(beta, size, KeyEvent::Char('c'))
        .await
        .expect("cycle next");
    assert_eq!(configured.active_session_id(), Some(alpha));

    // Unknown session name notifies without changing the active session.
    let before = configured.notifications().len();
    configured
        .handle_key(alpha, size, KeyEvent::Char('x'))
        .await
        .expect("unknown session name is handled");
    assert_eq!(configured.active_session_id(), Some(alpha));
    let unknown_notice = &configured.notifications()[before..];
    assert_eq!(
        unknown_notice.len(),
        1,
        "expected exactly one notification for the unknown session name, got {unknown_notice:?}"
    );
    assert!(
        unknown_notice[0].contains("no session named 'ghost'"),
        "expected an unknown-session notification naming 'ghost', got {:?}",
        unknown_notice[0]
    );

    // last-session gracefully handles a previous session that has since closed:
    // active is alpha with previous=beta, so kill beta and press last.
    run_cli(&server, &["kill-session", "-t", "beta"]);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync after kill");
    let before = configured.notifications().len();
    configured
        .handle_key(alpha, size, KeyEvent::Char('l'))
        .await
        .expect("last-session with a closed previous session is handled gracefully");
    assert_eq!(configured.active_session_id(), Some(alpha));
    let missing_notice = &configured.notifications()[before..];
    assert_eq!(
        missing_notice.len(),
        1,
        "expected exactly one notification for the closed previous session, got {missing_notice:?}"
    );
    assert!(
        missing_notice[0].contains("no previous session"),
        "expected a 'no previous session' notification, not an error switch, got {:?}",
        missing_notice[0]
    );

    server.shutdown().await.expect("server shuts down");
}

/// `(leaf, buffer)` pairs of `session_name`, ordered by leaf node id
/// (creation order).
async fn leaf_buffers(
    connection: &mut TestConnection,
    session_name: &str,
) -> Vec<(NodeId, BufferId)> {
    let snapshot = session_snapshot_by_name(connection, session_name).await;
    let mut leaves = snapshot
        .nodes
        .iter()
        .filter_map(|node| {
            node.buffer_view
                .as_ref()
                .map(|view| (node.id, view.buffer_id))
        })
        .collect::<Vec<_>>();
    leaves.sort();
    leaves
}

/// Ask the shell in `buffer_id` for its PTY size and wait for the `rows cols`
/// answer, so the assertion covers the real PTY rather than server bookkeeping.
async fn assert_stty_size(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    cols: u16,
    rows: u16,
) {
    assert_eq!(
        buffer_record(connection, buffer_id).await.pty_size,
        embers_core::PtySize::new(cols, rows),
        "server-side pty size of buffer {buffer_id}"
    );
    connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id,
            bytes: b"stty size\n".to_vec(),
        }))
        .await
        .expect("send stty succeeds");
    connection
        .wait_for_capture_contains(buffer_id, &format!("{rows} {cols}"), Duration::from_secs(3))
        .await
        .expect("shell reports the resized pty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pane_pty_size_follows_the_drawn_pane_size() {
    use embers_client::ConfiguredClient;

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(&server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let (config, _tempdir) = session_switch_config();
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = session_id_by_name(configured.client(), "alpha");
    let initial = Size {
        width: 101,
        height: 31,
    };

    // First attach: the root tab bar takes one row and the pane title another,
    // so a 101x31 viewport leaves 101x29 for the shell.
    configured
        .sync_pty_sizes(alpha, initial)
        .await
        .expect("sync");
    let leaves = leaf_buffers(&mut connection, "alpha").await;
    assert_eq!(leaves.len(), 1);
    assert_stty_size(&mut connection, leaves[0].1, 101, 29).await;

    // A vertical split shares the width, minus one divider column.
    run_cli(&server, &["split-window", "--", "/bin/sh"]);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync after split");
    configured
        .sync_pty_sizes(alpha, initial)
        .await
        .expect("sync after split");
    let leaves = leaf_buffers(&mut connection, "alpha").await;
    assert_eq!(leaves.len(), 2);
    assert_stty_size(&mut connection, leaves[0].1, 50, 29).await;
    assert_stty_size(&mut connection, leaves[1].1, 50, 29).await;

    // Resizing the outer terminal resizes every visible pane.
    let resized = Size {
        width: 121,
        height: 41,
    };
    configured
        .sync_pty_sizes(alpha, resized)
        .await
        .expect("sync after outer resize");
    assert_stty_size(&mut connection, leaves[0].1, 60, 39).await;
    assert_stty_size(&mut connection, leaves[1].1, 60, 39).await;

    // A zoomed pane fills the whole viewport (no tab bar); the hidden pane
    // keeps its last size.
    connection
        .request(&ClientMessage::Node(NodeRequest::Zoom {
            request_id: new_request_id(),
            node_id: leaves[0].0,
        }))
        .await
        .expect("zoom succeeds");
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync after zoom");
    configured
        .sync_pty_sizes(alpha, resized)
        .await
        .expect("sync after zoom");
    assert_stty_size(&mut connection, leaves[0].1, 121, 40).await;
    assert_stty_size(&mut connection, leaves[1].1, 60, 39).await;

    server.shutdown().await.expect("server shuts down");
}

/// Shell command whose first output is the PTY size it started with.
const REPORT_STARTUP_SIZE: &str = "stty size; exec /bin/sh";

/// The newest buffer shown in `session_name`: the one a create command just
/// spawned.
async fn newest_buffer(connection: &mut TestConnection, session_name: &str) -> BufferId {
    leaf_buffers(connection, session_name)
        .await
        .into_iter()
        .map(|(_, buffer_id)| buffer_id)
        .max()
        .expect("session shows a buffer")
}

/// The first `rows cols` line `buffer_id` printed, as `(cols, rows)`.
async fn first_reported_size(connection: &mut TestConnection, buffer_id: BufferId) -> (u16, u16) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let capture = connection
            .capture_buffer(buffer_id)
            .await
            .expect("capture succeeds");
        let size = capture.lines.iter().find_map(|line| {
            let (rows, cols) = line.trim().split_once(' ')?;
            Some((cols.parse().ok()?, rows.parse().ok()?))
        });
        if let Some(size) = size {
            return size;
        }
        assert!(
            Instant::now() < deadline,
            "buffer {buffer_id} never reported its size; last capture: {:?}",
            capture.lines
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The PTY size `configured` draws `buffer_id` at in `session_id`.
async fn drawn_size(
    configured: &mut embers_client::ConfiguredClient<embers_client::SocketTransport>,
    session_id: SessionId,
    viewport: Size,
    buffer_id: BufferId,
) -> (u16, u16) {
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let presentation =
        PresentationModel::project(configured.client().state(), session_id, viewport)
            .expect("projection succeeds");
    let size = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.buffer_id == buffer_id)
        .and_then(|leaf| leaf.pty_size())
        .unwrap_or_else(|| panic!("buffer {buffer_id} is not drawn"));
    (size.cols, size.rows)
}

/// The startup size `buffer_id` reported must be the size it is drawn at,
/// without the client having resized it.
async fn assert_spawned_at_drawn_size(
    connection: &mut TestConnection,
    configured: &mut embers_client::ConfiguredClient<embers_client::SocketTransport>,
    session_id: SessionId,
    viewport: Size,
    buffer_id: BufferId,
    expected: (u16, u16),
) {
    let reported = first_reported_size(connection, buffer_id).await;
    let drawn = drawn_size(configured, session_id, viewport, buffer_id).await;
    assert_eq!(drawn, expected, "drawn size of buffer {buffer_id}");
    assert_eq!(reported, drawn, "startup size of buffer {buffer_id}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_created_panes_start_at_their_drawn_size() {
    use embers_client::ConfiguredClient;

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(&server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let (config, _tempdir) = session_switch_config();
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = session_id_by_name(configured.client(), "alpha");
    let viewport = Size {
        width: 101,
        height: 31,
    };
    // The attached client draws the session once; nothing below syncs again,
    // so every size checked is the one the pane was spawned with.
    configured
        .sync_pty_sizes(alpha, viewport)
        .await
        .expect("sync");

    // A horizontal split stacks the panes: 30 rows below the tab bar, minus
    // one divider row, shared 14/15, each minus its title row.
    run_cli(
        &server,
        &[
            "split-window",
            "--horizontal",
            "--",
            "/bin/sh",
            "-c",
            REPORT_STARTUP_SIZE,
        ],
    );
    let buffer_id = newest_buffer(&mut connection, "alpha").await;
    assert_spawned_at_drawn_size(
        &mut connection,
        &mut configured,
        alpha,
        viewport,
        buffer_id,
        (101, 14),
    )
    .await;

    // A vertical split of that pane shares its width, minus one divider column.
    run_cli(
        &server,
        &["split-window", "--", "/bin/sh", "-c", REPORT_STARTUP_SIZE],
    );
    let buffer_id = newest_buffer(&mut connection, "alpha").await;
    assert_spawned_at_drawn_size(
        &mut connection,
        &mut configured,
        alpha,
        viewport,
        buffer_id,
        (50, 14),
    )
    .await;

    // A new window gets the whole area below the tab bar.
    run_cli(
        &server,
        &[
            "new-window",
            "-t",
            "alpha",
            "--",
            "/bin/sh",
            "-c",
            REPORT_STARTUP_SIZE,
        ],
    );
    let buffer_id = newest_buffer(&mut connection, "alpha").await;
    assert_spawned_at_drawn_size(
        &mut connection,
        &mut configured,
        alpha,
        viewport,
        buffer_id,
        (101, 29),
    )
    .await;

    // A popup loses its border and title row.
    run_cli(
        &server,
        &[
            "display-popup",
            "-t",
            "alpha",
            "--width",
            "40",
            "--height",
            "12",
            "--",
            "/bin/sh",
            "-c",
            REPORT_STARTUP_SIZE,
        ],
    );
    let buffer_id = newest_buffer(&mut connection, "alpha").await;
    assert_spawned_at_drawn_size(
        &mut connection,
        &mut configured,
        alpha,
        viewport,
        buffer_id,
        (38, 9),
    )
    .await;

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn action_created_panes_start_at_their_drawn_size() {
    use embers_client::{ConfiguredClient, KeyEvent};

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(&server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let client = MuxClient::connect(server.socket_path())
        .await
        .expect("client connects");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let script = r#"
            fn split_h(ctx) { action.split_with("horizontal", tree.buffer_spawn(REPORTER)) }
            fn split_v(ctx) { action.split_with("vertical", tree.buffer_spawn(REPORTER)) }
            fn new_tab(ctx) {
                action.insert_tab_after_current("reporter", tree.buffer_spawn(REPORTER))
            }
            fn popup(ctx) {
                action.open_floating(
                    tree.buffer_spawn(REPORTER),
                    #{ x: 2, y: 2, width: 30, height: 10 }
                )
            }
            define_action("split-h", split_h);
            define_action("split-v", split_v);
            define_action("new-tab", new_tab);
            define_action("popup", popup);
            bind("normal", "1", "split-h");
            bind("normal", "2", "split-v");
            bind("normal", "3", "new-tab");
            bind("normal", "4", "popup");
        "#
    .replace(
        "REPORTER",
        &format!(r#"["/bin/sh", "-c", "{REPORT_STARTUP_SIZE}"]"#),
    );
    std::fs::write(tempdir.path().join("config.rhai"), script).expect("write config");
    let config = embers_client::ConfigManager::load(
        embers_client::ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .expect("load config");
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let alpha = session_id_by_name(configured.client(), "alpha");
    let viewport = Size {
        width: 101,
        height: 31,
    };

    for (key, expected) in [
        ('1', (101, 14)),
        ('2', (50, 14)),
        ('3', (101, 29)),
        ('4', (28, 7)),
    ] {
        configured
            .handle_key(alpha, viewport, KeyEvent::Char(key))
            .await
            .expect("action runs");
        assert!(
            configured.notifications().is_empty(),
            "{:?}",
            configured.notifications()
        );
        let buffer_id = newest_buffer(&mut connection, "alpha").await;
        assert_spawned_at_drawn_size(
            &mut connection,
            &mut configured,
            alpha,
            viewport,
            buffer_id,
            expected,
        )
        .await;
    }

    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn most_recently_active_client_owns_the_pane_pty_size() {
    use embers_client::{ConfiguredClient, KeyEvent};

    let server = TestServer::start().await.expect("server starts");
    run_cli(&server, &["new-session", "alpha"]);
    run_cli(&server, &["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("protocol connection");
    let mut clients = Vec::new();
    let mut tempdirs = Vec::new();
    for _ in 0..2 {
        let client = MuxClient::connect(server.socket_path())
            .await
            .expect("client connects");
        let (config, tempdir) = session_switch_config();
        let mut configured = ConfiguredClient::new(client, config);
        configured
            .client_mut()
            .resync_all_sessions()
            .await
            .expect("resync");
        clients.push(configured);
        tempdirs.push(tempdir);
    }
    let alpha = session_id_by_name(clients[0].client(), "alpha");
    let large = Size {
        width: 101,
        height: 31,
    };
    let small = Size {
        width: 81,
        height: 21,
    };
    let buffer_id = leaf_buffers(&mut connection, "alpha").await[0].1;

    clients[0].sync_pty_sizes(alpha, large).await.expect("sync");
    assert_stty_size(&mut connection, buffer_id, 101, 29).await;

    // A client attaching later takes over with its own size.
    clients[1].sync_pty_sizes(alpha, small).await.expect("sync");
    assert_stty_size(&mut connection, buffer_id, 81, 19).await;

    // Redrawing an idle client at an unchanged size does not fight back, even
    // once it has seen the other client's size.
    clients[0]
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    clients[0].sync_pty_sizes(alpha, large).await.expect("sync");
    assert_stty_size(&mut connection, buffer_id, 81, 19).await;

    // Typing in the first client makes it the active one again. Ctrl-U then
    // clears the typed character so the shell reads a clean `stty size`.
    clients[0]
        .handle_key(alpha, large, KeyEvent::Char('z'))
        .await
        .expect("key passes through");
    clients[0].sync_pty_sizes(alpha, large).await.expect("sync");
    connection
        .request(&ClientMessage::Input(embers_protocol::InputRequest::Send {
            request_id: new_request_id(),
            buffer_id,
            bytes: b"\x15".to_vec(),
        }))
        .await
        .expect("clear typed key");
    assert_stty_size(&mut connection, buffer_id, 101, 29).await;

    server.shutdown().await.expect("server shuts down");
}
