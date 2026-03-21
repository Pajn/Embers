mod support;

use std::fs;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, KeyEvent, MuxClient,
    ScriptedTransport,
};
use embers_core::{RequestId, SessionId, Size};
use embers_protocol::{
    ClientMessage, FocusChangedEvent, NodeRequest, OkResponse, ServerEvent, ServerResponse,
    SessionRequest, SessionSnapshot, SessionSnapshotResponse,
};
use tempfile::tempdir;

use support::{FOCUSED_LEAF_ID, LEFT_LEAF_ID, SESSION_ID, demo_state, root_focus_state};

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
