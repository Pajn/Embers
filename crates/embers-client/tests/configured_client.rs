mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, KeyEvent, MouseEvent,
    MouseEventKind, MouseModifiers, MuxClient, PresentationModel, ScriptedTransport,
};
use embers_core::{ActivityState, BufferId, NodeId, PtySize, RequestId, SessionId, Size};
use embers_protocol::{
    BufferCreatedEvent, BufferRecord, BufferRecordState, BufferViewRecord, ClientMessage,
    FocusChangedEvent, InputRequest, NodeRecord, NodeRecordKind, NodeRequest, OkResponse,
    RenderInvalidatedEvent, ScrollbackSliceResponse, ServerEvent, ServerResponse, SessionRecord,
    SessionRequest, SessionSnapshot, SessionSnapshotResponse, SnapshotResponse,
    VisibleSnapshotResponse,
};
use tempfile::tempdir;

use support::{FOCUSED_LEAF_ID, LEFT_LEAF_ID, SESSION_ID, demo_state, root_focus_state};

const SECOND_SESSION_ID: SessionId = SessionId(2);
const SECOND_ROOT_ID: NodeId = NodeId(200);
const SECOND_BUFFER_ID: BufferId = BufferId(70);

fn manager_from_source(source: &str) -> (ConfigManager, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(&config_path, source).unwrap();
    (
        ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
        )
        .unwrap(),
        tempdir,
    )
}

fn session_snapshot_from_state(
    state: &embers_client::ClientState,
    session_id: SessionId,
) -> SessionSnapshot {
    let session = state.sessions.get(&session_id).unwrap().clone();
    let nodes = state.nodes.values().cloned().collect::<Vec<_>>();
    let buffers = state.buffers.values().cloned().collect::<Vec<_>>();
    let floating = state.floating.values().cloned().collect::<Vec<_>>();
    SessionSnapshot {
        session,
        nodes,
        buffers,
        floating,
    }
}

fn visible_snapshot_from_state(
    state: &embers_client::ClientState,
    buffer_id: BufferId,
    request_id: RequestId,
) -> VisibleSnapshotResponse {
    let mut snapshot = state.snapshots.get(&buffer_id).unwrap().clone();
    snapshot.request_id = request_id;
    snapshot
}

fn scrollback_slice_response(
    buffer_id: BufferId,
    request_id: RequestId,
    start_line: u64,
    total_lines: u64,
    lines: &[&str],
) -> ScrollbackSliceResponse {
    ScrollbackSliceResponse {
        request_id,
        buffer_id,
        start_line,
        total_lines,
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
    }
}

fn snapshot_response(
    buffer_id: BufferId,
    request_id: RequestId,
    lines: &[&str],
) -> SnapshotResponse {
    SnapshotResponse {
        request_id,
        buffer_id,
        sequence: 1,
        size: embers_core::PtySize::new(80, 24),
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        title: None,
        cwd: None,
    }
}

fn second_session_state() -> embers_client::ClientState {
    let mut state = demo_state();
    state.sessions.insert(
        SECOND_SESSION_ID,
        SessionRecord {
            id: SECOND_SESSION_ID,
            name: "other".to_owned(),
            root_node_id: SECOND_ROOT_ID,
            floating_ids: Vec::new(),
            focused_leaf_id: Some(SECOND_ROOT_ID),
            focused_floating_id: None,
        },
    );
    state.nodes.insert(
        SECOND_ROOT_ID,
        NodeRecord {
            id: SECOND_ROOT_ID,
            session_id: SECOND_SESSION_ID,
            parent_id: None,
            kind: NodeRecordKind::BufferView,
            buffer_view: Some(BufferViewRecord {
                buffer_id: SECOND_BUFFER_ID,
                focused: true,
                zoomed: false,
                follow_output: true,
                last_render_size: PtySize::new(80, 20),
            }),
            split: None,
            tabs: None,
        },
    );
    state.buffers.insert(
        SECOND_BUFFER_ID,
        BufferRecord {
            id: SECOND_BUFFER_ID,
            title: "other pane".to_owned(),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            state: BufferRecordState::Running,
            pid: None,
            attachment_node_id: Some(SECOND_ROOT_ID),
            pty_size: PtySize::new(80, 20),
            activity: ActivityState::Idle,
            last_snapshot_seq: 1,
            exit_code: None,
            env: BTreeMap::new(),
        },
    );
    state.snapshots.insert(
        SECOND_BUFFER_ID,
        VisibleSnapshotResponse {
            request_id: RequestId(0),
            buffer_id: SECOND_BUFFER_ID,
            sequence: 1,
            size: PtySize::new(80, 20),
            lines: vec!["other pane".to_owned()],
            title: Some("other pane".to_owned()),
            cwd: None,
            viewport_top_line: 0,
            total_lines: 1,
            alternate_screen: false,
            mouse_reporting: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor: None,
        },
    );
    state
}

#[tokio::test]
async fn configured_keybinding_executes_live_focus_action() {
    let transport = ScriptedTransport::default();
    transport.push_exchange(
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(1),
            session_id: SESSION_ID,
            node_id: LEFT_LEAF_ID,
        }),
        ServerResponse::Ok(OkResponse {
            request_id: RequestId(1),
        }),
    );
    let focused_state = root_focus_state();
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(2),
            session_id: SESSION_ID,
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(2),
            snapshot: session_snapshot_from_state(&focused_state, SESSION_ID),
        }),
    );

    let mut client = MuxClient::new(transport.clone());
    *client.state_mut() = demo_state();
    let (config, _tempdir) = manager_from_source(
        r#"
            fn move_left(ctx) { action.focus_left() }
            define_action("move-left", move_left);
            bind("normal", "<C-h>", "move-left");
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Ctrl('h'),
        )
        .await
        .unwrap();

    assert_eq!(
        configured
            .client()
            .state()
            .sessions
            .get(&SESSION_ID)
            .and_then(|session| session.focused_leaf_id),
        Some(LEFT_LEAF_ID)
    );
    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn configured_render_uses_scripted_tab_bars() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source(
        r##"
            fn format_tabs(ctx) {
                let tabs = ctx.tabs();
                let active = tabs[ctx.active_index()];
                if ctx.is_root() {
                    ui.bar([ui.segment("ROOT " + active.title())], [], [])
                } else {
                    ui.bar([ui.segment("NESTED " + active.title())], [], [])
                }
            }

            tabbar.set_formatter(format_tabs);
        "##,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    let grid = configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();
    let rendered = grid.render();

    assert!(rendered.contains("ROOT workspace"));
    assert!(rendered.contains("NESTED logs-long-title"));
}

#[tokio::test]
async fn reload_updates_live_bindings() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(
        &config_path,
        r#"
            fn notify_left(ctx) { action.notify("info", "left") }
            define_action("notify-left", notify_left);
            bind("normal", "<C-h>", "notify-left");
        "#,
    )
    .unwrap();
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();
    let client = MuxClient::new(FakeTransport::default());
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    fs::write(
        &config_path,
        r#"
            fn notify_right(ctx) { action.notify("info", "right") }
            define_action("notify-right", notify_right);
            bind("normal", "<C-h>", "notify-right");
        "#,
    )
    .unwrap();
    configured.reload_config().unwrap();
    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Ctrl('h'),
        )
        .await
        .unwrap();

    assert_eq!(configured.notifications(), ["right"]);
}

#[tokio::test]
async fn paste_events_wrap_bytes_for_bracketed_paste_buffers() {
    let transport = FakeTransport::default();
    transport.push_response(ServerResponse::Ok(OkResponse {
        request_id: RequestId(1),
    }));

    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&BufferId(4))
        .unwrap()
        .bracketed_paste = true;

    transport.push_response(ServerResponse::VisibleSnapshot(
        visible_snapshot_from_state(&state, BufferId(4), RequestId(2)),
    ));
    transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(3),
        snapshot: session_snapshot_from_state(&state, SESSION_ID),
    }));

    let client = MuxClient::new(transport.clone());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = state;

    configured
        .handle_paste(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            b"hello world".to_vec(),
        )
        .await
        .unwrap();

    assert_eq!(
        transport.requests()[0],
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(1),
            buffer_id: BufferId(4),
            bytes: b"\x1b[200~hello world\x1b[201~".to_vec(),
        })
    );
}

#[tokio::test]
async fn focus_events_forward_when_program_requested_them() {
    let transport = FakeTransport::default();
    transport.push_response(ServerResponse::Ok(OkResponse {
        request_id: RequestId(1),
    }));

    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&BufferId(4))
        .unwrap()
        .focus_reporting = true;

    transport.push_response(ServerResponse::VisibleSnapshot(
        visible_snapshot_from_state(&state, BufferId(4), RequestId(2)),
    ));
    transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(3),
        snapshot: session_snapshot_from_state(&state, SESSION_ID),
    }));

    let client = MuxClient::new(transport.clone());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = state;

    configured
        .handle_focus_event(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(
        transport.requests()[0],
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(1),
            buffer_id: BufferId(4),
            bytes: b"\x1b[I".to_vec(),
        })
    );
}

#[tokio::test]
async fn focus_events_are_ignored_when_program_did_not_request_them() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    configured
        .handle_focus_event(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            true,
        )
        .await
        .unwrap();

    assert!(configured.client().transport().requests().is_empty());
}

#[tokio::test]
async fn page_up_scrolls_locally_with_scrollback_slices() {
    let transport = FakeTransport::default();
    transport.push_response(ServerResponse::ScrollbackSlice(scrollback_slice_response(
        BufferId(4),
        RequestId(1),
        12,
        60,
        &["history line", "match line"],
    )));

    let mut state = demo_state();
    let snapshot = state.snapshots.get_mut(&BufferId(4)).unwrap();
    snapshot.total_lines = 60;
    snapshot.viewport_top_line = 36;
    snapshot.lines = vec!["tail one".to_owned(), "tail two".to_owned()];
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.total_line_count = 60;
    view.scroll_top_line = 36;
    view.follow_output = true;

    let client = MuxClient::new(transport.clone());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = state;

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::PageUp,
        )
        .await
        .unwrap();

    let view = configured
        .client()
        .state()
        .view_state(FOCUSED_LEAF_ID)
        .expect("focused view state");
    assert_eq!(view.scroll_top_line, 12);
    assert!(!view.follow_output);
    assert_eq!(view.visible_lines[0], "history line");
    assert!(matches!(
        transport.requests()[0],
        ClientMessage::Buffer(embers_protocol::BufferRequest::ScrollbackSlice {
            buffer_id: BufferId(4),
            start_line: 12,
            ..
        })
    ));
}

#[tokio::test]
async fn search_prompt_commits_matches_and_navigates_locally() {
    let transport = FakeTransport::default();
    transport.push_response(ServerResponse::Snapshot(snapshot_response(
        BufferId(4),
        RequestId(1),
        &["alpha", "needle here", "tail needle"],
    )));

    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();
    configured
        .client_mut()
        .state_mut()
        .view_state_mut(FOCUSED_LEAF_ID)
        .unwrap()
        .follow_output = false;

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('/'),
        )
        .await
        .unwrap();
    for ch in "needle".chars() {
        configured
            .handle_key(
                SESSION_ID,
                Size {
                    width: 80,
                    height: 20,
                },
                KeyEvent::Char(ch),
            )
            .await
            .unwrap();
    }
    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Enter,
        )
        .await
        .unwrap();

    let view = configured
        .client()
        .state()
        .view_state(FOCUSED_LEAF_ID)
        .expect("focused view state");
    let search = view.search_state.as_ref().expect("search state");
    assert_eq!(search.query, "needle");
    assert_eq!(search.matches.len(), 2);
    assert_eq!(search.active_match_index, Some(0));

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('n'),
        )
        .await
        .unwrap();
    assert_eq!(
        configured
            .client()
            .state()
            .view_state(FOCUSED_LEAF_ID)
            .and_then(|view| view.search_state.as_ref())
            .and_then(|search| search.active_match_index),
        Some(1)
    );

    let grid = configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();
    assert!(
        grid.ansi_lines()
            .iter()
            .any(|line| line.contains("\x1b[4m"))
    );
}

#[tokio::test]
async fn pasted_text_updates_search_prompt_without_forwarding_input() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();
    configured
        .client_mut()
        .state_mut()
        .view_state_mut(FOCUSED_LEAF_ID)
        .unwrap()
        .follow_output = false;

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('/'),
        )
        .await
        .unwrap();
    configured
        .handle_paste(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            b"needle".to_vec(),
        )
        .await
        .unwrap();

    assert!(configured.client().transport().requests().is_empty());
    assert_eq!(
        configured.status_line(SESSION_ID, Path::new("/tmp/embers.sock")),
        "[demo] /needle"
    );
}

#[tokio::test]
async fn select_mode_yanks_selection_to_osc52() {
    let transport = FakeTransport::default();
    transport.push_response(ServerResponse::Snapshot(snapshot_response(
        BufferId(4),
        RequestId(1),
        &["logs visible", "second row"],
    )));

    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();
    configured
        .client_mut()
        .state_mut()
        .view_state_mut(FOCUSED_LEAF_ID)
        .unwrap()
        .follow_output = false;

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('v'),
        )
        .await
        .unwrap();
    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('l'),
        )
        .await
        .unwrap();
    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('y'),
        )
        .await
        .unwrap();

    let output = configured.drain_terminal_output();
    assert_eq!(output.len(), 1);
    let osc52 = String::from_utf8(output[0].clone()).unwrap();
    assert!(osc52.starts_with("\x1b]52;c;"));
    assert!(osc52.contains("bG8="));
    assert!(
        configured
            .client()
            .state()
            .view_state(FOCUSED_LEAF_ID)
            .and_then(|view| view.selection_state.as_ref())
            .is_none()
    );
}

#[tokio::test]
async fn wheel_mouse_events_scroll_locally_or_forward_to_program() {
    let mut initial_state = demo_state();
    let presentation = PresentationModel::project(
        &initial_state,
        SESSION_ID,
        Size {
            width: 80,
            height: 20,
        },
    )
    .unwrap();
    let focused = presentation.focused_leaf().unwrap().clone();

    let local_transport = FakeTransport::default();
    local_transport.push_response(ServerResponse::ScrollbackSlice(scrollback_slice_response(
        BufferId(4),
        RequestId(1),
        33,
        60,
        &["older output"],
    )));
    initial_state
        .snapshots
        .get_mut(&BufferId(4))
        .unwrap()
        .total_lines = 60;
    initial_state
        .snapshots
        .get_mut(&BufferId(4))
        .unwrap()
        .viewport_top_line = 36;
    let view = initial_state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.total_line_count = 60;
    view.scroll_top_line = 36;
    view.follow_output = true;
    let client = MuxClient::new(local_transport.clone());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = initial_state.clone();
    configured
        .handle_mouse(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            MouseEvent {
                row: focused.rect.origin.y as u16,
                column: focused.rect.origin.x as u16,
                modifiers: MouseModifiers::default(),
                kind: MouseEventKind::WheelUp,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        local_transport.requests()[0],
        ClientMessage::Buffer(embers_protocol::BufferRequest::ScrollbackSlice {
            start_line: 33,
            ..
        })
    ));

    let forward_transport = FakeTransport::default();
    forward_transport.push_response(ServerResponse::Ok(OkResponse {
        request_id: RequestId(1),
    }));
    forward_transport.push_response(ServerResponse::VisibleSnapshot(
        visible_snapshot_from_state(&initial_state, BufferId(4), RequestId(2)),
    ));
    forward_transport.push_response(ServerResponse::SessionSnapshot(SessionSnapshotResponse {
        request_id: RequestId(3),
        snapshot: session_snapshot_from_state(&initial_state, SESSION_ID),
    }));
    let client = MuxClient::new(forward_transport.clone());
    let (config, _tempdir) = manager_from_source("");
    let mut configured = ConfiguredClient::new(client, config);
    let mut state = initial_state;
    state
        .snapshots
        .get_mut(&BufferId(4))
        .unwrap()
        .mouse_reporting = true;
    *configured.client_mut().state_mut() = state;
    configured
        .handle_mouse(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            MouseEvent {
                row: focused.rect.origin.y as u16,
                column: focused.rect.origin.x as u16,
                modifiers: MouseModifiers::default(),
                kind: MouseEventKind::WheelUp,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        forward_transport.requests()[0],
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(1),
            buffer_id: BufferId(4),
            bytes: b"\x1b[<64;1;1M".to_vec(),
        })
    );
}

#[tokio::test]
async fn render_invalidated_events_use_their_buffer_session_context() {
    let state = second_session_state();
    let transport = FakeTransport::default();
    transport.push_event(ServerEvent::RenderInvalidated(RenderInvalidatedEvent {
        buffer_id: SECOND_BUFFER_ID,
    }));
    transport.push_response(ServerResponse::VisibleSnapshot(
        visible_snapshot_from_state(&state, SECOND_BUFFER_ID, RequestId(1)),
    ));
    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source(
        r#"
            fn on_render(ctx) { action.notify("info", ctx.current_session().name()) }
            on("render_invalidated", on_render);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = state;
    configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();

    let event = configured.process_next_event().await.unwrap();

    assert!(matches!(event, ServerEvent::RenderInvalidated(_)));
    assert_eq!(configured.notifications(), ["other"]);
}

#[tokio::test]
async fn detached_buffer_events_do_not_fall_back_to_the_active_session() {
    let transport = FakeTransport::default();
    transport.push_event(ServerEvent::BufferCreated(BufferCreatedEvent {
        buffer: BufferRecord {
            id: BufferId(71),
            title: "detached".to_owned(),
            command: vec!["/bin/sh".to_owned()],
            cwd: None,
            state: BufferRecordState::Running,
            pid: None,
            attachment_node_id: None,
            pty_size: PtySize::new(80, 20),
            activity: ActivityState::Idle,
            last_snapshot_seq: 0,
            exit_code: None,
            env: BTreeMap::new(),
        },
    }));
    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source(
        r#"
            fn on_buffer(ctx) {
                if ctx.current_session() == () {
                    action.notify("info", "none")
                } else {
                    action.notify("info", ctx.current_session().name())
                }
            }
            on("buffer_created", on_buffer);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();
    configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();

    let event = configured.process_next_event().await.unwrap();

    assert!(matches!(event, ServerEvent::BufferCreated(_)));
    assert_eq!(configured.notifications(), ["none"]);
}

#[tokio::test]
async fn disabling_wheel_scroll_in_config_suppresses_local_mouse_scrolling() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 20,
        },
    )
    .unwrap();
    let focused = presentation.focused_leaf().unwrap().clone();
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source("mouse.set_wheel_scroll(false);");
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = state;

    configured
        .handle_mouse(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            MouseEvent {
                row: (focused.rect.origin.y + 1) as u16,
                column: focused.rect.origin.x as u16,
                modifiers: MouseModifiers::default(),
                kind: MouseEventKind::WheelUp,
            },
        )
        .await
        .unwrap();

    assert!(configured.client().transport().requests().is_empty());
}

#[tokio::test]
async fn event_hook_executes_real_actions() {
    let transport = ScriptedTransport::default();
    let focused_state = root_focus_state();
    transport.push_event(ServerEvent::FocusChanged(FocusChangedEvent {
        session_id: SESSION_ID,
        focused_leaf_id: Some(FOCUSED_LEAF_ID),
        focused_floating_id: None,
    }));
    transport.push_exchange(
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(1),
            session_id: SESSION_ID,
            node_id: LEFT_LEAF_ID,
        }),
        ServerResponse::Ok(OkResponse {
            request_id: RequestId(1),
        }),
    );
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::Get {
            request_id: RequestId(2),
            session_id: SESSION_ID,
        }),
        ServerResponse::SessionSnapshot(SessionSnapshotResponse {
            request_id: RequestId(2),
            snapshot: session_snapshot_from_state(&focused_state, SESSION_ID),
        }),
    );

    let mut client = MuxClient::new(transport.clone());
    *client.state_mut() = demo_state();
    let (config, _tempdir) = manager_from_source(
        r#"
            fn on_focus(ctx) { action.focus_left() }
            on("focus_changed", on_focus);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();

    configured.process_next_event().await.unwrap();

    assert_eq!(
        configured
            .client()
            .state()
            .sessions
            .get(&SESSION_ID)
            .and_then(|session| session.focused_leaf_id),
        Some(LEFT_LEAF_ID)
    );
    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();
}

#[tokio::test]
async fn keybinding_runtime_errors_become_notifications() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source(
        r#"
            fn broken(ctx) {
                let xs = [];
                xs[1]
            }

            define_action("broken", broken);
            bind("normal", "<C-h>", "broken");
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Ctrl('h'),
        )
        .await
        .unwrap();

    assert_eq!(configured.notifications().len(), 1);
}

#[tokio::test]
async fn recursive_named_actions_stop_at_expansion_limit() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source(
        r#"
            fn alpha(ctx) { action.run_named_action("beta") }
            fn beta(ctx) { action.run_named_action("alpha") }

            define_action("alpha", alpha);
            define_action("beta", beta);
            bind("normal", "a", "alpha");
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    configured
        .handle_key(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
            KeyEvent::Char('a'),
        )
        .await
        .unwrap();

    assert_eq!(
        configured.notifications(),
        ["invalid input: action expansion limit reached"]
    );
}

#[tokio::test]
async fn event_handler_runtime_errors_do_not_crash_client() {
    let transport = FakeTransport::default();
    transport.push_event(ServerEvent::FocusChanged(FocusChangedEvent {
        session_id: SESSION_ID,
        focused_leaf_id: Some(FOCUSED_LEAF_ID),
        focused_floating_id: None,
    }));
    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source(
        r#"
            fn broken(ctx) {
                let xs = [];
                xs[1]
            }

            on("focus_changed", broken);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    let event = configured.process_next_event().await.unwrap();

    assert!(matches!(event, ServerEvent::FocusChanged(_)));
    assert_eq!(configured.notifications().len(), 1);
}

#[tokio::test]
async fn formatter_failures_fall_back_to_default_rendering() {
    let client = MuxClient::new(FakeTransport::default());
    let (config, _tempdir) = manager_from_source(
        r#"
            fn broken_bar(ctx) { 1 }
            tabbar.set_formatter(broken_bar);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    let grid = configured
        .render_session(
            SESSION_ID,
            Size {
                width: 80,
                height: 20,
            },
        )
        .await
        .unwrap();
    let rendered = grid.render();

    assert!(rendered.contains("workspace"));
    assert_eq!(configured.notifications().len(), 1);
}

#[tokio::test]
async fn event_hooks_can_notify_without_an_active_view() {
    let transport = FakeTransport::default();
    transport.push_event(ServerEvent::FocusChanged(FocusChangedEvent {
        session_id: SESSION_ID,
        focused_leaf_id: Some(FOCUSED_LEAF_ID),
        focused_floating_id: None,
    }));
    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source(
        r#"
            fn on_focus(ctx) { action.notify("info", "focus hook") }
            on("focus_changed", on_focus);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    let event = configured.process_next_event().await.unwrap();

    assert!(matches!(event, ServerEvent::FocusChanged(_)));
    assert_eq!(configured.notifications(), ["focus hook"]);
}

#[tokio::test]
async fn event_context_keeps_session_without_an_active_view() {
    let transport = FakeTransport::default();
    transport.push_event(ServerEvent::FocusChanged(FocusChangedEvent {
        session_id: SESSION_ID,
        focused_leaf_id: Some(FOCUSED_LEAF_ID),
        focused_floating_id: None,
    }));
    let client = MuxClient::new(transport);
    let (config, _tempdir) = manager_from_source(
        r#"
            fn on_focus(ctx) {
                let session = ctx.current_session();
                if session == () {
                    action.notify("error", "missing")
                } else {
                    action.notify("info", session.name())
                }
            }

            on("focus_changed", on_focus);
        "#,
    );
    let mut configured = ConfiguredClient::new(client, config);
    *configured.client_mut().state_mut() = demo_state();

    let event = configured.process_next_event().await.unwrap();

    assert!(matches!(event, ServerEvent::FocusChanged(_)));
    assert_eq!(configured.notifications(), ["demo"]);
}
