//! Buffers whose programs run on another host, as detached buffers of that
//! host's server. Here the "host" is a second test server, which the local
//! server reaches through `embers proxy` where it would run SSH.

use embers_core::{ActivityState, BufferId, new_request_id};
use embers_protocol::{
    BufferRecord, BufferRecordState, BufferRequest, ClientMessage, InputRequest, ServerResponse,
};
use embers_server::{Server, ServerConfig};
use embers_test_support::{TestConnection, TestServer, cargo_bin_path};

use crate::support::{run_cli, session_snapshot_by_name, stdout};
use tokio::time::{Duration, Instant, sleep};

const HOST: &str = "devbox";

fn proxy_command(remote: &TestServer) -> Vec<String> {
    vec![
        cargo_bin_path("embers").display().to_string(),
        "--socket".to_owned(),
        remote.socket_path().display().to_string(),
        "proxy".to_owned(),
    ]
}

/// A remote server, and a local server that runs buffers for [`HOST`] on it.
async fn start_servers() -> (TestServer, TestServer) {
    let remote = TestServer::start().await.expect("start remote server");
    let command = proxy_command(&remote);
    let local = TestServer::start_with(|config| config.remote_command = command)
        .await
        .expect("start local server");
    (remote, local)
}

async fn connect(server: &TestServer) -> TestConnection {
    TestConnection::connect(server.socket_path())
        .await
        .expect("connect")
}

async fn create_buffer(
    connection: &mut TestConnection,
    command: &[&str],
    host: Option<&str>,
) -> BufferRecord {
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: Some("remote".to_owned()),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd: None,
            env: Default::default(),
            size: None,
            session_id: None,
            host: host.map(str::to_owned),
        }))
        .await
        .expect("create buffer");
    match response {
        ServerResponse::Buffer(response) => response.buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn get_buffer(connection: &mut TestConnection, buffer_id: BufferId) -> BufferRecord {
    match connection
        .request(&ClientMessage::Buffer(BufferRequest::Get {
            request_id: new_request_id(),
            buffer_id,
        }))
        .await
        .expect("get buffer")
    {
        ServerResponse::Buffer(response) => response.buffer,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn detached_buffers(connection: &mut TestConnection) -> Vec<BufferRecord> {
    match connection
        .request(&ClientMessage::Buffer(BufferRequest::List {
            request_id: new_request_id(),
            session_id: None,
            attached_only: false,
            detached_only: true,
        }))
        .await
        .expect("list buffers")
    {
        ServerResponse::Buffers(response) => response.buffers,
        other => panic!("expected buffers response, got {other:?}"),
    }
}

async fn send(connection: &mut TestConnection, buffer_id: BufferId, input: &str) {
    connection
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: new_request_id(),
            buffer_id,
            bytes: input.as_bytes().to_vec(),
        }))
        .await
        .expect("send input");
}

async fn wait_for_state(
    connection: &mut TestConnection,
    buffer_id: BufferId,
    state: BufferRecordState,
) -> BufferRecord {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let record = get_buffer(connection, buffer_id).await;
        if record.state == state {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "buffer {buffer_id} stayed {:?}, expected {state:?}",
            record.state
        );
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_output(connection: &mut TestConnection, buffer_id: BufferId, needle: &str) {
    connection
        .wait_for_capture_contains(buffer_id, needle, Duration::from_secs(10))
        .await
        .expect("output arrives");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_buffer_runs_as_a_detached_buffer_of_the_hosts_server() {
    let (remote, local) = start_servers().await;
    let mut local_connection = connect(&local).await;
    let mut remote_connection = connect(&remote).await;

    let buffer = create_buffer(
        &mut local_connection,
        &["/bin/sh", "-c", "echo on-the-''host; sleep 30"],
        Some(HOST),
    )
    .await;
    assert_eq!(buffer.host.as_deref(), Some(HOST));
    // The program's pid is on the other host.
    assert_eq!(buffer.pid, None);

    let on_host = detached_buffers(&mut remote_connection).await;
    assert_eq!(on_host.len(), 1, "{on_host:?}");
    assert_eq!(on_host[0].command[2], "echo on-the-''host; sleep 30");
    assert_eq!(on_host[0].host, None);

    wait_for_output(&mut local_connection, buffer.id, "on-the-host").await;
    let visible = local_connection
        .capture_visible_buffer(buffer.id)
        .await
        .expect("capture visible");
    assert!(
        visible
            .lines
            .iter()
            .any(|line| line.text.contains("on-the-host")),
        "{:?}",
        visible.lines
    );

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_and_resizes_reach_the_remote_program() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let buffer = create_buffer(&mut connection, &["/bin/sh"], Some(HOST)).await;

    send(&mut connection, buffer.id, "echo $((6 * 7))-done\n").await;
    wait_for_output(&mut connection, buffer.id, "42-done").await;

    connection
        .request(&ClientMessage::Input(InputRequest::Resize {
            request_id: new_request_id(),
            buffer_id: buffer.id,
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        }))
        .await
        .expect("resize");
    send(&mut connection, buffer.id, "stty size\n").await;
    wait_for_output(&mut connection, buffer.id, "30 100").await;

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_program_exiting_or_killed_exits_the_buffer() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;

    let exits = create_buffer(
        &mut connection,
        &["/bin/sh", "-c", "sleep 0.3; exit 3"],
        Some(HOST),
    )
    .await;
    let exited = wait_for_state(&mut connection, exits.id, BufferRecordState::Exited).await;
    assert_eq!(exited.exit_code, Some(3));

    let runs = create_buffer(&mut connection, &["sleep", "30"], Some(HOST)).await;
    connection
        .request(&ClientMessage::Buffer(BufferRequest::Kill {
            request_id: new_request_id(),
            buffer_id: runs.id,
            force: false,
        }))
        .await
        .expect("kill");
    wait_for_state(&mut connection, runs.id, BufferRecordState::Exited).await;

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bell_on_the_host_flags_the_buffer() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let buffer = create_buffer(
        &mut connection,
        &["/bin/sh", "-c", "sleep 0.3; printf '\\a'; sleep 30"],
        Some(HOST),
    )
    .await;

    let deadline = Instant::now() + Duration::from_secs(10);
    while get_buffer(&mut connection, buffer.id).await.activity != ActivityState::Bell {
        assert!(Instant::now() < deadline, "the bell was not recorded");
        sleep(Duration::from_millis(50)).await;
    }

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_the_host_interrupts_its_buffers_rather_than_exiting_them() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let buffer = create_buffer(&mut connection, &["sleep", "30"], Some(HOST)).await;
    let local_buffer = create_buffer(&mut connection, &["sleep", "30"], None).await;

    remote.shutdown().await.expect("shutdown remote server");

    wait_for_state(&mut connection, buffer.id, BufferRecordState::Interrupted).await;
    // Buffers on this machine are not affected.
    assert_eq!(
        get_buffer(&mut connection, local_buffer.id).await.state,
        BufferRecordState::Running
    );

    local.shutdown().await.expect("shutdown local server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffers_come_back_once_the_host_can_be_reached_again() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let buffer = create_buffer(&mut connection, &["/bin/sh"], Some(HOST)).await;
    send(&mut connection, buffer.id, "echo before-''drop\n").await;
    wait_for_output(&mut connection, buffer.id, "before-drop").await;

    // The connection drops, as when the network goes away, while the
    // program keeps running on the host.
    let proxy = format!("{} proxy", remote.socket_path().display());
    let killed = std::process::Command::new("pkill")
        .args(["-f", &proxy])
        .status()
        .expect("run pkill");
    assert!(killed.success(), "no proxy process to kill");
    wait_for_state(&mut connection, buffer.id, BufferRecordState::Interrupted).await;

    wait_for_state(&mut connection, buffer.id, BufferRecordState::Running).await;
    wait_for_output(&mut connection, buffer.id, "before-drop").await;
    send(&mut connection, buffer.id, "echo after-''drop\n").await;
    wait_for_output(&mut connection, buffer.id, "after-drop").await;

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_server_ends_remote_programs_too() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let mut remote_connection = connect(&remote).await;
    let buffer = create_buffer(&mut connection, &["sleep", "30"], Some(HOST)).await;
    let remote_id = detached_buffers(&mut remote_connection).await[0].id;
    assert_eq!(
        get_buffer(&mut connection, buffer.id).await.host.as_deref(),
        Some(HOST)
    );

    let _ = connection
        .request(&ClientMessage::KillServer(
            embers_protocol::KillServerRequest {
                request_id: new_request_id(),
            },
        ))
        .await;

    wait_for_state(&mut remote_connection, remote_id, BufferRecordState::Exited).await;

    let _ = local.shutdown().await;
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_server_takes_up_its_remote_buffers_again() {
    let remote = TestServer::start().await.expect("start remote server");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut config = ServerConfig::new(tempdir.path().join("local.sock"));
    config.remote_command = proxy_command(&remote);

    let first = Server::new(config.clone())
        .start()
        .await
        .expect("start local server");
    let mut connection = TestConnection::connect(&config.socket_path)
        .await
        .expect("connect");
    let buffer = create_buffer(&mut connection, &["/bin/sh"], Some(HOST)).await;
    send(&mut connection, buffer.id, "echo before-''restart\n").await;
    wait_for_output(&mut connection, buffer.id, "before-restart").await;
    drop(connection);
    first.shutdown().await.expect("shutdown local server");

    let second = Server::new(config.clone())
        .start()
        .await
        .expect("restart local server");
    let mut connection = TestConnection::connect(&config.socket_path)
        .await
        .expect("connect");
    // Still the same program on the host, with what it printed before.
    wait_for_state(&mut connection, buffer.id, BufferRecordState::Running).await;
    wait_for_output(&mut connection, buffer.id, "before-restart").await;
    send(&mut connection, buffer.id, "echo after-''restart\n").await;
    wait_for_output(&mut connection, buffer.id, "after-restart").await;

    drop(connection);
    second.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_cannot_be_reached_is_reported() {
    let remote = TestServer::start().await.expect("start remote server");
    let local = TestServer::start_with(|config| {
        config.remote_command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "echo \"$0: no route to host\" >&2; exit 255".to_owned(),
            "{host}".to_owned(),
        ];
    })
    .await
    .expect("start local server");
    let mut connection = connect(&local).await;

    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: None,
            command: vec!["sleep".to_owned(), "30".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
            session_id: None,
            host: Some(HOST.to_owned()),
        }))
        .await
        .expect("request answered");
    match response {
        ServerResponse::Error(response) => {
            assert!(
                response.error.message.contains("devbox: no route to host"),
                "{}",
                response.error.message
            );
        }
        other => panic!("expected an error, got {other:?}"),
    }
    // A host name SSH would read as an option is refused before anything runs.
    let response = connection
        .request(&ClientMessage::Buffer(BufferRequest::Create {
            request_id: new_request_id(),
            title: None,
            command: vec!["sleep".to_owned(), "30".to_owned()],
            cwd: None,
            env: Default::default(),
            size: None,
            session_id: None,
            host: Some("-oProxyCommand=sh".to_owned()),
        }))
        .await
        .expect("request answered");
    assert!(matches!(response, ServerResponse::Error(_)), "{response:?}");

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

/// Run `embers` against `server` and return what it printed, trimmed.
fn cli(server: &TestServer, args: &[&str]) -> String {
    stdout(&run_cli(server, args)).trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panes_created_from_a_remote_pane_run_on_its_host_unless_local() {
    let (remote, local) = start_servers().await;
    cli(&local, &["new-session", "-d", "-s", "work"]);
    let remote_pane = cli(
        &local,
        &[
            "new-window",
            "-t",
            "work",
            "--host",
            HOST,
            "-P",
            "-F",
            "%#{buffer_id}",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(
        cli(
            &local,
            &["display-message", "-p", "-t", &remote_pane, "#{pane_host}"]
        ),
        HOST
    );
    cli(
        &local,
        &[
            "send-keys",
            "-t",
            &remote_pane,
            "--enter",
            "echo hi-from-''host",
        ],
    );
    let mut connection = connect(&local).await;
    let buffer_id = BufferId(remote_pane[1..].parse().expect("buffer id"));
    wait_for_output(&mut connection, buffer_id, "hi-from-host").await;

    let split = cli(
        &local,
        &[
            "split-window",
            "-t",
            &remote_pane,
            "-P",
            "-F",
            "#{pane_host}",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(split, HOST);
    let split = cli(
        &local,
        &[
            "split-window",
            "-t",
            &remote_pane,
            "--local",
            "-P",
            "-F",
            "[#{pane_host}]",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(split, "[]");
    // A window from the session's first, local pane stays local.
    let window = cli(
        &local,
        &[
            "new-window",
            "-t",
            "work:0",
            "-P",
            "-F",
            "[#{pane_host}]",
            "--",
            "/bin/sh",
        ],
    );
    assert_eq!(window, "[]");

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_split_bound_to_a_key_runs_on_the_focused_panes_host() {
    use embers_client::{
        ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, KeyEvent, MuxClient,
    };
    use embers_core::Size;

    let (remote, local) = start_servers().await;
    cli(
        &local,
        &[
            "new-session",
            "-d",
            "-s",
            "work",
            "--host",
            HOST,
            "--",
            "/bin/sh",
        ],
    );

    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tempdir.path().join("config.rhai"),
        r#"
            fn split(ctx) { action.split_with("vertical", tree.buffer_spawn(["/bin/sh"])) }
            define_action("split", split);
            bind("normal", "1", "split");
        "#,
    )
    .expect("write config");
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .expect("load config");
    let client = MuxClient::connect(local.socket_path())
        .await
        .expect("client connects");
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .client_mut()
        .resync_all_sessions()
        .await
        .expect("resync");
    let session_id = configured
        .client()
        .state()
        .sessions
        .values()
        .find(|session| session.name == "work")
        .expect("session cached")
        .id;

    configured
        .handle_key(
            session_id,
            Size {
                width: 80,
                height: 24,
            },
            KeyEvent::Char('1'),
        )
        .await
        .expect("action runs");
    assert!(
        configured.notifications().is_empty(),
        "{:?}",
        configured.notifications()
    );

    let hosts = cli(
        &local,
        &["list-panes", "-t", "work", "-F", "[#{pane_host}]"],
    );
    assert_eq!(hosts, format!("[{HOST}]\n[{HOST}]"));

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clipboard_writes_and_passthrough_from_a_remote_pane_reach_its_clients() {
    let (remote, local) = start_servers().await;
    // Once told to, the program writes the clipboard (OSC 52) and sends a
    // notification for the outer terminal (tmux passthrough).
    let script = r"read _; printf '\033]52;c;%s\a' $(printf from-the-host | base64); printf '\033Ptmux;\033\033]9;hello-''from-host\a\033\\'; sleep 30";
    cli(
        &local,
        &[
            "new-session",
            "-d",
            "-s",
            "work",
            "--host",
            HOST,
            "--",
            "/bin/sh",
            "-c",
            script,
        ],
    );

    // A client showing the session gets what its pane sends.
    let mut client = connect(&local).await;
    client.subscribe(None).await.expect("subscribe");
    let snapshot = session_snapshot_by_name(&mut client, "work").await;
    client
        .request(&ClientMessage::Client(
            embers_protocol::ClientRequest::Switch {
                request_id: new_request_id(),
                client_id: None,
                session_id: snapshot.session.id,
                node_id: None,
            },
        ))
        .await
        .expect("switch to the session");
    let buffer_id = snapshot.buffers[0].id;
    send(&mut client, buffer_id, "\r").await;

    let mut clipboard = None;
    let mut passthrough = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while clipboard.is_none() || !String::from_utf8_lossy(&passthrough).contains("hello-from-host")
    {
        assert!(
            Instant::now() < deadline,
            "missing clipboard {clipboard:?} or passthrough {:?}",
            String::from_utf8_lossy(&passthrough)
        );
        let event = client
            .wait_for_event(Duration::from_secs(10), |event| {
                matches!(
                    event,
                    embers_protocol::ServerEvent::Clipboard(_)
                        | embers_protocol::ServerEvent::Passthrough(_)
                )
            })
            .await
            .expect("event arrives");
        match event {
            embers_protocol::ServerEvent::Clipboard(event) => {
                assert_eq!(event.buffer_id, buffer_id);
                clipboard = Some(event.text);
            }
            embers_protocol::ServerEvent::Passthrough(event) => {
                assert_eq!(event.buffer_id, buffer_id);
                for write in event.writes {
                    passthrough.extend(write.bytes);
                }
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(clipboard.as_deref(), Some("from-the-host"));

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_to_one_host_each_get_their_own_answer() {
    let (remote, local) = start_servers().await;
    let mut connection = connect(&local).await;
    let mut buffers = Vec::new();
    for marker in ["first-''buffer", "second-''buffer"] {
        let script = format!("echo {marker}; sleep 30");
        let buffer = create_buffer(&mut connection, &["/bin/sh", "-c", &script], Some(HOST)).await;
        wait_for_output(&mut connection, buffer.id, &marker.replace("''", "")).await;
        buffers.push((buffer.id, marker.replace("''", "")));
    }

    // Requests from several clients are in flight to the host at once.
    let mut tasks = tokio::task::JoinSet::new();
    for client in 0..4 {
        let socket = local.socket_path().to_owned();
        let buffers = buffers.clone();
        tasks.spawn(async move {
            let mut connection = TestConnection::connect(&socket).await.expect("connect");
            for round in 0..10 {
                let (buffer_id, marker) = &buffers[(client + round) % buffers.len()];
                let capture = connection
                    .capture_buffer(*buffer_id)
                    .await
                    .expect("capture");
                assert!(
                    capture
                        .lines
                        .iter()
                        .any(|line| line.contains(marker.as_str())),
                    "buffer {buffer_id} answered with {:?}",
                    capture.lines
                );
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("client task");
    }

    local.shutdown().await.expect("shutdown local server");
    remote.shutdown().await.expect("shutdown remote server");
}
