use embers_core::{BufferId, RequestId, init_test_tracing};
use embers_protocol::{
    BufferRecordState, BufferRequest, BufferResponse, BuffersResponse, ClientMessage, InputRequest,
    ProtocolClient, ServerResponse, SessionRequest, SessionSnapshotResponse, SnapshotResponse,
};
use embers_server::{Server, ServerConfig};
use tempfile::tempdir;
use tokio::time::{Duration, Instant, sleep};

async fn request_session_snapshot(
    client: &mut ProtocolClient,
    request: SessionRequest,
) -> SessionSnapshotResponse {
    match client
        .request(&ClientMessage::Session(request))
        .await
        .expect("session request succeeds")
    {
        ServerResponse::SessionSnapshot(response) => response,
        other => panic!("expected session snapshot, got {other:?}"),
    }
}

async fn request_buffer(client: &mut ProtocolClient, request: BufferRequest) -> BufferResponse {
    match client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer request succeeds")
    {
        ServerResponse::Buffer(response) => response,
        other => panic!("expected buffer response, got {other:?}"),
    }
}

async fn request_buffers(client: &mut ProtocolClient, request: BufferRequest) -> BuffersResponse {
    match client
        .request(&ClientMessage::Buffer(request))
        .await
        .expect("buffer list succeeds")
    {
        ServerResponse::Buffers(response) => response,
        other => panic!("expected buffers response, got {other:?}"),
    }
}

async fn wait_for_snapshot_line(
    client: &mut ProtocolClient,
    request_id: RequestId,
    buffer_id: BufferId,
    expected: &str,
) -> SnapshotResponse {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(ServerResponse::Snapshot(snapshot)) = client
            .request(&ClientMessage::Buffer(BufferRequest::Capture {
                request_id,
                buffer_id,
            }))
            .await
            && snapshot.lines.iter().any(|line| line.contains(expected))
        {
            return snapshot;
        }

        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    panic!(
        "capture for buffer {buffer_id} did not contain expected line '{expected}' before timeout"
    );
}

#[tokio::test]
async fn clean_restart_restores_workspace_and_keeps_live_buffers_running() {
    init_test_tracing();

    let tempdir = tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mux.sock");
    let config = ServerConfig::new(socket_path.clone());
    let workspace_path = config.workspace_path.clone();

    let handle = Server::new(config.clone())
        .start()
        .await
        .expect("start server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("connect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Create {
            request_id: RequestId(1),
            name: "main".to_owned(),
        },
    )
    .await;
    let session_id = session.snapshot.session.id;

    let attached = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(2),
            title: Some("attached".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
        },
    )
    .await
    .buffer;

    let detached = request_buffer(
        &mut client,
        BufferRequest::Create {
            request_id: RequestId(3),
            title: Some("detached".to_owned()),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            env: Default::default(),
        },
    )
    .await
    .buffer;

    let attached_id = attached.id;
    let detached_id = detached.id;

    let restored_layout = request_session_snapshot(
        &mut client,
        SessionRequest::AddRootTab {
            request_id: RequestId(4),
            session_id,
            title: "shell".to_owned(),
            buffer_id: Some(attached_id),
            child_node_id: None,
        },
    )
    .await;
    assert_eq!(restored_layout.snapshot.session.id, session_id);

    let deadline = Instant::now() + Duration::from_secs(2);
    let attached_running = loop {
        let response = request_buffer(
            &mut client,
            BufferRequest::Get {
                request_id: RequestId(40),
                buffer_id: attached_id,
            },
        )
        .await;
        if response.buffer.state == BufferRecordState::Running {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(25)).await;
    };
    assert!(
        attached_running,
        "attached buffer {attached_id} did not reach Running before shutdown"
    );

    handle.shutdown().await.expect("shutdown server");
    assert!(workspace_path.exists());

    let handle = Server::new(config).start().await.expect("restart server");
    let mut client = ProtocolClient::connect(&socket_path)
        .await
        .expect("reconnect client");

    let session = request_session_snapshot(
        &mut client,
        SessionRequest::Get {
            request_id: RequestId(5),
            session_id,
        },
    )
    .await;
    assert_eq!(session.snapshot.session.name, "main");
    let attached_buffer = session
        .snapshot
        .buffers
        .iter()
        .find(|buffer| buffer.id == attached_id)
        .expect("attached buffer restored");
    assert_eq!(attached_buffer.state, BufferRecordState::Running);
    assert!(attached_buffer.attachment_node_id.is_some());

    let buffers = request_buffers(
        &mut client,
        BufferRequest::List {
            request_id: RequestId(6),
            session_id: None,
            attached_only: false,
            detached_only: false,
        },
    )
    .await;
    let detached_buffer = buffers
        .buffers
        .iter()
        .find(|buffer| buffer.id == detached_id)
        .expect("detached buffer restored");
    assert_eq!(detached_buffer.state, BufferRecordState::Running);
    assert_eq!(detached_buffer.attachment_node_id, None);

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(7),
            buffer_id: attached_id,
            bytes: b"printf restarted-attached\\n\r".to_vec(),
        }))
        .await
        .expect("send to attached buffer succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }

    match client
        .request(&ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(8),
            buffer_id: detached_id,
            bytes: b"printf restarted-detached\\n\r".to_vec(),
        }))
        .await
        .expect("send to detached buffer succeeds")
    {
        ServerResponse::Ok(_) => {}
        other => panic!("expected ok response, got {other:?}"),
    }

    let _attached_capture =
        wait_for_snapshot_line(&mut client, RequestId(9), attached_id, "restarted-attached").await;

    let _detached_capture = wait_for_snapshot_line(
        &mut client,
        RequestId(10),
        detached_id,
        "restarted-detached",
    )
    .await;

    handle.shutdown().await.expect("shutdown restarted server");
}
