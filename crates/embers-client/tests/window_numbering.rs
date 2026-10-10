//! `set_base_index` (tmux `base-index`): the window numbers tab bar
//! formatters see and `action.select_window` uses.

use std::fs;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, KeyEvent, MuxClient,
    PresentationModel, ScriptedTransport, TabBarContext,
};
use embers_core::{RequestId, Size};
use embers_protocol::{
    ClientMessage, OkResponse, ServerResponse, SessionRequest, SessionSnapshot,
    SessionSnapshotResponse,
};
use tempfile::tempdir;

use crate::support::{SESSION_ID, demo_state};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

fn manager_from_source(source: &str) -> (ConfigManager, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    fs::write(tempdir.path().join("config.rhai"), source).unwrap();
    (
        ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
        )
        .unwrap(),
        tempdir,
    )
}

fn demo_snapshot() -> SessionSnapshot {
    let state = demo_state();
    SessionSnapshot {
        session: state.sessions.get(&SESSION_ID).unwrap().clone(),
        nodes: state.nodes.values().cloned().collect(),
        buffers: state.buffers.values().cloned().collect(),
        floating: state.floating.values().cloned().collect(),
    }
}

#[test]
fn set_base_index_is_recorded_in_the_loaded_config() {
    let (manager, _tempdir) = manager_from_source("set_base_index(1);");
    assert_eq!(manager.active_script().loaded_config().base_index, Some(1));

    let (manager, _tempdir) = manager_from_source("");
    assert_eq!(manager.active_script().loaded_config().base_index, None);
}

#[test]
fn set_base_index_rejects_negative_numbers() {
    let tempdir = tempdir().unwrap();
    fs::write(tempdir.path().join("config.rhai"), "set_base_index(-1);").unwrap();
    let error = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .err()
    .expect("negative base index is rejected");
    assert!(error.to_string().contains("base index"), "{error}");
}

#[test]
fn tab_numbers_start_at_the_base_index() {
    let state = demo_state();
    let presentation = PresentationModel::project(&state, SESSION_ID, VIEWPORT).unwrap();
    let root = presentation.root_tabs.as_ref().unwrap();

    let context = TabBarContext::from_frame(root, "normal", 80);
    let numbers: Vec<_> = context.tabs.iter().map(|tab| tab.number).collect();
    assert_eq!(numbers, vec![0, 1]);

    let context = TabBarContext::from_frame(root, "normal", 80).with_base_index(Some(1));
    let numbers: Vec<_> = context.tabs.iter().map(|tab| tab.number).collect();
    let indexes: Vec<_> = context.tabs.iter().map(|tab| tab.index).collect();
    assert_eq!(numbers, vec![1, 2]);
    assert_eq!(indexes, vec![0, 1], "index() stays zero-based");
}

#[tokio::test]
async fn formatters_see_window_numbers_from_the_config_base_index() {
    let transport = ScriptedTransport::default();
    let (config, _tempdir) = manager_from_source(
        r#"
        set_base_index(1);
        fn format(ctx) {
            let labels = [];
            for tab in ctx.tabs() {
                labels.push(ui.segment(`${tab.number()}:${tab.title()} `));
            }
            ui.bar(labels, [], [])
        }
        tabbar.set_formatter(format);
        "#,
    );
    let mut configured = ConfiguredClient::new(MuxClient::new(transport), config);
    *configured.client_mut().state_mut() = demo_state();
    assert_eq!(configured.base_index(), Some(1));

    let grid = configured
        .render_session(SESSION_ID, VIEWPORT)
        .await
        .unwrap();
    let first_row = grid.lines()[0].clone();
    assert!(
        first_row.starts_with("1:shell 2:workspace"),
        "tab bar was {first_row:?}"
    );
}

#[tokio::test]
async fn select_window_selects_by_window_number() {
    let transport = ScriptedTransport::default();
    transport.push_exchange(
        ClientMessage::Session(SessionRequest::SelectRootTab {
            request_id: RequestId(1),
            session_id: SESSION_ID,
            index: 0,
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
            snapshot: demo_snapshot(),
        }),
    );
    let (config, _tempdir) = manager_from_source(
        r#"
        set_base_index(1);
        bind("normal", "<M-1>", action.select_window(1));
        bind("normal", "<M-3>", action.select_window(3));
        "#,
    );
    let mut configured = ConfiguredClient::new(MuxClient::new(transport.clone()), config);
    *configured.client_mut().state_mut() = demo_state();

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('1'))
        .await
        .unwrap();
    assert!(configured.notifications().is_empty());
    transport.assert_exhausted().unwrap();

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('3'))
        .await
        .unwrap();
    assert!(
        configured
            .notifications()
            .last()
            .is_some_and(|message| message.contains("no window 3")),
        "{:?}",
        configured.notifications()
    );
}
