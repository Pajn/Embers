//! The session/window/pane picker against a real server: its items keep
//! naming the same window while other clients change the session.

use std::fs;
use std::sync::Arc;

use embers_cli::PromptCommandRunner;
use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, KeyEvent, MuxClient, SocketTransport,
};
use embers_core::{SessionId, Size};
use embers_test_support::{TestConnection, TestServer};

use crate::support::{run_cli, session_snapshot_by_name};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

/// The session's window titles in order, and the active one's.
async fn windows(connection: &mut TestConnection) -> (Vec<String>, String) {
    let snapshot = session_snapshot_by_name(connection, "main").await;
    let tabs = snapshot
        .nodes
        .iter()
        .find(|node| node.id == snapshot.session.root_node_id)
        .and_then(|root| root.tabs.as_ref())
        .expect("root is tabs");
    let titles: Vec<String> = tabs.tabs.iter().map(|tab| tab.title.clone()).collect();
    let active = titles[tabs.active as usize].clone();
    (titles, active)
}

fn titles(titles: &[&str]) -> Vec<String> {
    titles.iter().map(|title| (*title).to_owned()).collect()
}

struct Picker {
    configured: ConfiguredClient<SocketTransport>,
    session_id: SessionId,
    _config_dir: tempfile::TempDir,
}

impl Picker {
    async fn connect(server: &TestServer, session_id: SessionId) -> Self {
        let config_dir = tempfile::tempdir().expect("config dir");
        fs::write(
            config_dir.path().join("config.rhai"),
            r#"bind("normal", "<M-w>", action.choose_window());"#,
        )
        .expect("write config");
        let config = ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(config_dir.path()),
        )
        .expect("load config");
        let client = MuxClient::connect(server.socket_path())
            .await
            .expect("connect client");
        let mut configured = ConfiguredClient::new(client, config);
        configured.set_command_runner(Arc::new(PromptCommandRunner::new(
            server.socket_path().to_path_buf(),
        )));
        let mut picker = Self {
            configured,
            session_id,
            _config_dir: config_dir,
        };
        picker.sync().await;
        picker
    }

    /// Catch up with changes made by other clients, as the event loop does.
    async fn sync(&mut self) {
        self.configured
            .client_mut()
            .resync_all_sessions()
            .await
            .expect("resync sessions");
    }

    async fn press(&mut self, key: KeyEvent) {
        self.configured
            .handle_key(self.session_id, VIEWPORT, key)
            .await
            .expect("handle key");
    }
}

fn new_window(server: &TestServer, title: &str) {
    run_cli(
        server,
        [
            "new-window",
            "-t",
            "main",
            "--title",
            title,
            "--",
            "/bin/sh",
        ],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picker_acts_on_its_window_after_an_earlier_window_closes() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "main"]);
    for title in ["win-a", "win-b", "win-c", "win-d", "win-e"] {
        new_window(&server, title);
    }
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let session_id = session_snapshot_by_name(&mut connection, "main")
        .await
        .session
        .id;
    let mut picker = Picker::connect(&server, session_id).await;

    // Enter: open on win-b, then another client closes win-a before it.
    run_cli(&server, ["select-window", "-t", "win-b"]);
    picker.sync().await;
    picker.press(KeyEvent::Alt('w')).await;
    run_cli(&server, ["kill-window", "-t", "win-a"]);
    run_cli(&server, ["select-window", "-t", "win-d"]);
    picker.sync().await;
    picker.press(KeyEvent::Enter).await;
    assert_eq!(
        windows(&mut connection).await,
        (
            titles(&["win-b", "win-c", "win-d", "win-e"]),
            "win-b".to_owned()
        )
    );

    // x then y: open on win-c, then win-b before it closes.
    run_cli(&server, ["select-window", "-t", "win-c"]);
    picker.sync().await;
    picker.press(KeyEvent::Alt('w')).await;
    picker.press(KeyEvent::Char('x')).await;
    run_cli(&server, ["kill-window", "-t", "win-b"]);
    picker.sync().await;
    picker.press(KeyEvent::Char('y')).await;
    assert_eq!(
        windows(&mut connection).await.0,
        titles(&["win-d", "win-e"])
    );
    picker.press(KeyEvent::Char('q')).await;

    // A window that has closed is not replaced by the one after it.
    new_window(&server, "win-f");
    run_cli(&server, ["select-window", "-t", "win-e"]);
    picker.sync().await;
    picker.press(KeyEvent::Alt('w')).await;
    run_cli(&server, ["kill-window", "-t", "win-e"]);
    run_cli(&server, ["select-window", "-t", "win-d"]);
    picker.sync().await;
    picker.press(KeyEvent::Enter).await;
    assert_eq!(
        windows(&mut connection).await,
        (titles(&["win-d", "win-f"]), "win-d".to_owned())
    );
    assert_eq!(
        picker.configured.status_message(),
        Some("window no longer exists")
    );

    new_window(&server, "win-g");
    run_cli(&server, ["select-window", "-t", "win-f"]);
    picker.sync().await;
    picker.press(KeyEvent::Alt('w')).await;
    picker.press(KeyEvent::Char('x')).await;
    run_cli(&server, ["kill-window", "-t", "win-f"]);
    picker.sync().await;
    picker.press(KeyEvent::Char('y')).await;
    assert_eq!(
        windows(&mut connection).await.0,
        titles(&["win-d", "win-g"])
    );
    assert_eq!(
        picker.configured.status_message(),
        Some("window no longer exists")
    );

    server.shutdown().await.expect("shutdown server");
}
