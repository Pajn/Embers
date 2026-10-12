//! `display-message` in the client: the status line text, its expiry, and
//! format expansion for the focused pane.

use std::fs;
use std::path::Path;
use std::time::Duration;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, FormatContext,
    KeyEvent, MuxClient, expand_format,
};
use embers_core::Size;
use embers_protocol::{DisplayMessageEvent, ServerEvent};
use tempfile::tempdir;

use crate::support::{
    FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID, HIDDEN_ROOT_LEAF_ID, SESSION_ID, demo_state,
};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

fn configured(source: &str) -> (ConfiguredClient<FakeTransport>, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    fs::write(tempdir.path().join("config.rhai"), source).unwrap();
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();
    let mut configured = ConfiguredClient::new(MuxClient::new(FakeTransport::default()), config);
    *configured.client_mut().state_mut() = demo_state();
    (configured, tempdir)
}

#[test]
fn format_context_describes_the_pane_and_its_window() {
    let state = demo_state();
    let focused = FormatContext::from_state(&state, SESSION_ID, None, None).unwrap();
    assert_eq!(focused.session_name, "demo");
    assert_eq!(focused.window_index, 1);
    assert_eq!(focused.window_name, "workspace");
    assert_eq!(focused.pane_id, Some(FOCUSED_LEAF_ID));
    assert_eq!(
        expand_format("#S:#I:#W #{node_id}", &focused),
        format!("demo:1:workspace {FOCUSED_LEAF_ID}")
    );

    // A pane in another window reports that window.
    let hidden =
        FormatContext::from_state(&state, SESSION_ID, Some(HIDDEN_ROOT_LEAF_ID), Some(1)).unwrap();
    assert_eq!(hidden.window_index, 1);
    assert_eq!(hidden.window_name, "shell");

    // A popup pane is in no window, so it reports the active one.
    let popup =
        FormatContext::from_state(&state, SESSION_ID, Some(FLOATING_TOP_LEAF_ID), None).unwrap();
    assert_eq!(popup.window_name, "workspace");
}

#[tokio::test]
async fn display_message_action_expands_formats_in_the_status_line() {
    let (mut configured, _tempdir) = configured(
        r##"
        set_base_index(1);
        bind("normal", "<M-m>", action.display_message("#S:#I:#W ##"));
        "##,
    );

    configured
        .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt('m'))
        .await
        .unwrap();

    assert_eq!(configured.status_message(), Some("demo:2:workspace #"));
    assert_eq!(
        configured.status_line(SESSION_ID, Path::new("embers.sock")),
        "demo:2:workspace #"
    );
}

#[tokio::test]
async fn display_message_events_show_until_their_time_is_up() {
    let (mut configured, _tempdir) = configured("");

    configured
        .handle_event(&ServerEvent::DisplayMessage(DisplayMessageEvent {
            message: "build \x1b[31mdone\x07".to_owned(),
            duration_ms: 30,
        }))
        .await
        .unwrap();
    // Control characters never reach the host terminal.
    assert_eq!(configured.status_message(), Some("build  [31mdone "));
    assert!(!configured.expire_status_message());

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(configured.status_message(), None);
    assert!(configured.expire_status_message());
    assert!(
        configured
            .status_line(SESSION_ID, Path::new("embers.sock"))
            .starts_with("[demo]")
    );
}
