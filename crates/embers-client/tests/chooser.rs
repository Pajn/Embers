//! The session/window/pane picker (tmux `choose-tree`).

use std::fs;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use embers_client::{
    Chooser, ChooserItem, ChooserScope, ClientState, CommandContext, CommandRunner,
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, FakeTransport, KeyEvent, MuxClient,
    NORMAL_MODE, TREE_MODE,
};
use embers_core::{BufferId, NodeId, Result, SessionId, Size};
use embers_protocol::{
    ClientMessage, ClientRequest, NodeRequest, SessionRequest, TabRecord, TabsRecord,
};
use tempfile::tempdir;

use crate::support::{
    FOCUSED_LEAF_ID, HIDDEN_NESTED_LEAF_ID, HIDDEN_ROOT_LEAF_ID, LEFT_LEAF_ID, ROOT_SPLIT_ID,
    ROOT_TABS_ID, SESSION_ID, demo_state,
};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};
const OTHER_SESSION: SessionId = SessionId(2);
const OTHER_ROOT: NodeId = NodeId(201);
const OTHER_LEAF: NodeId = NodeId(200);

/// The demo session plus a second session "other" with one window of one
/// pane.
fn two_sessions() -> ClientState {
    let mut state = demo_state();
    let mut session = state.sessions[&SESSION_ID].clone();
    session.id = OTHER_SESSION;
    session.name = "other".to_owned();
    session.root_node_id = OTHER_ROOT;
    session.focused_leaf_id = Some(OTHER_LEAF);
    session.focused_floating_id = None;
    session.floating_ids = Vec::new();
    state.sessions.insert(OTHER_SESSION, session);
    let mut root = state.nodes[&ROOT_TABS_ID].clone();
    root.id = OTHER_ROOT;
    root.session_id = OTHER_SESSION;
    root.tabs = Some(TabsRecord {
        active: 0,
        tabs: vec![TabRecord {
            title: "notes".to_owned(),
            child_id: OTHER_LEAF,
            synchronize_panes: false,
        }],
    });
    state.nodes.insert(OTHER_ROOT, root);
    let mut leaf = state.nodes[&LEFT_LEAF_ID].clone();
    leaf.id = OTHER_LEAF;
    leaf.session_id = OTHER_SESSION;
    leaf.parent_id = Some(OTHER_ROOT);
    leaf.buffer_view.as_mut().unwrap().buffer_id = BufferId(9);
    state.nodes.insert(OTHER_LEAF, leaf);
    let mut buffer = state.buffers[&BufferId(2)].clone();
    buffer.id = BufferId(9);
    buffer.title = "notes".to_owned();
    buffer.attachment_node_id = Some(OTHER_LEAF);
    buffer.foreground_command = Some(vec!["/usr/bin/vim".to_owned()]);
    state.buffers.insert(BufferId(9), buffer);
    state
}

fn labels(chooser: &Chooser, state: &ClientState, base_index: Option<u32>) -> Vec<String> {
    chooser
        .rows(state, Some(SESSION_ID), base_index)
        .into_iter()
        .map(|row| {
            let marker = match (row.has_children, row.expanded) {
                (true, true) => "-",
                (true, false) => "+",
                (false, _) => " ",
            };
            let current = if row.current { " *" } else { "" };
            format!("{}{marker}{}{current}", "  ".repeat(row.depth), row.label)
        })
        .collect()
}

#[test]
fn tree_scope_lists_sessions_windows_and_panes() {
    let state = two_sessions();
    let chooser = Chooser::open(
        ChooserScope::Tree,
        &state,
        Some(SESSION_ID),
        Some(1),
        "normal",
    );

    assert_eq!(
        labels(&chooser, &state, Some(1)),
        vec![
            "-demo: 2 windows *",
            "  -1: shell (1 pane)",
            format!("     {HIDDEN_ROOT_LEAF_ID}: shell [sh]").as_str(),
            "  -2: workspace (3 panes) *",
            format!("     {LEFT_LEAF_ID}: editor [sh]").as_str(),
            format!("     {HIDDEN_NESTED_LEAF_ID}: build [sh]").as_str(),
            format!("     {FOCUSED_LEAF_ID}: logs-long-title [sh] *").as_str(),
            "-other: 1 window",
            "  -1: notes (1 pane)",
            format!("     {OTHER_LEAF}: notes [vim]").as_str(),
        ]
    );
    let rows = chooser.rows(&state, Some(SESSION_ID), Some(1));
    assert_eq!(
        chooser.selected(&rows),
        Some(ChooserItem::Pane(SESSION_ID, FOCUSED_LEAF_ID))
    );
}

#[test]
fn window_and_session_scopes_start_collapsed() {
    let state = two_sessions();
    let windows = Chooser::open(
        ChooserScope::Windows,
        &state,
        Some(SESSION_ID),
        None,
        "normal",
    );
    assert_eq!(
        labels(&windows, &state, None),
        vec![
            "-demo: 2 windows *",
            "  +0: shell (1 pane)",
            "  +1: workspace (3 panes) *",
            "-other: 1 window",
            "  +0: notes (1 pane)",
        ]
    );
    let rows = windows.rows(&state, Some(SESSION_ID), None);
    assert_eq!(
        windows.selected(&rows),
        Some(ChooserItem::Window(SESSION_ID, ROOT_SPLIT_ID))
    );

    let sessions = Chooser::open(
        ChooserScope::Sessions,
        &state,
        Some(SESSION_ID),
        None,
        "normal",
    );
    assert_eq!(
        labels(&sessions, &state, None),
        vec!["+demo: 2 windows *", "+other: 1 window"]
    );
}

#[test]
fn moving_folding_and_filtering() {
    let state = two_sessions();
    let mut chooser = Chooser::open(
        ChooserScope::Sessions,
        &state,
        Some(SESSION_ID),
        None,
        "normal",
    );
    let rows = |chooser: &Chooser| chooser.rows(&state, Some(SESSION_ID), None);

    chooser.expand(&rows(&chooser));
    assert_eq!(rows(&chooser).len(), 4);
    chooser.expand(&rows(&chooser));
    assert_eq!(
        chooser.selected(&rows(&chooser)),
        Some(ChooserItem::Window(SESSION_ID, HIDDEN_ROOT_LEAF_ID))
    );
    chooser.collapse(&rows(&chooser));
    assert_eq!(
        chooser.selected(&rows(&chooser)),
        Some(ChooserItem::Session(SESSION_ID))
    );
    chooser.collapse(&rows(&chooser));
    assert_eq!(rows(&chooser).len(), 2);
    chooser.move_by(&rows(&chooser), 5);
    assert_eq!(
        chooser.selected(&rows(&chooser)),
        Some(ChooserItem::Session(OTHER_SESSION))
    );

    // A filter shows matches anywhere in the tree, with their ancestors.
    chooser.filter = "NOTES".to_owned();
    assert_eq!(
        labels(&chooser, &state, None),
        vec![
            "-other: 1 window",
            "  -0: notes (1 pane)",
            format!("     {OTHER_LEAF}: notes [vim]").as_str(),
        ]
    );
    chooser.filter = "nothing like it".to_owned();
    assert!(rows(&chooser).is_empty());
}

#[derive(Default)]
struct RecordingRunner {
    calls: Mutex<Vec<(Vec<String>, CommandContext)>>,
}

#[async_trait]
impl CommandRunner for RecordingRunner {
    async fn run(&self, argv: Vec<String>, context: CommandContext) -> Result<String> {
        self.calls.lock().unwrap().push((argv, context));
        Ok(String::new())
    }
}

fn configured(
    transport: FakeTransport,
    runner: Arc<RecordingRunner>,
) -> (ConfiguredClient<FakeTransport>, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("config.rhai"),
        r#"
        bind("normal", "<M-t>", action.choose_tree());
        bind("normal", "<M-w>", action.choose_window());
        bind("normal", "<M-s>", action.choose_session());
        bind("normal", "<M-c>", action.run_command("choose-tree -s"));
        "#,
    )
    .unwrap();
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();
    let mut configured = ConfiguredClient::new(MuxClient::new(transport), config);
    *configured.client_mut().state_mut() = two_sessions();
    configured.set_command_runner(runner);
    (configured, tempdir)
}

async fn press(configured: &mut ConfiguredClient<FakeTransport>, keys: &[KeyEvent]) {
    for key in keys {
        // Requests the fake transport cannot answer fail after being
        // recorded, which is all these tests look at.
        let _ = configured
            .handle_key(SESSION_ID, VIEWPORT, key.clone())
            .await;
    }
}

#[tokio::test]
async fn enter_selects_the_chosen_window() {
    let transport = FakeTransport::default();
    let (mut configured, _tempdir) = configured(transport.clone(), Arc::default());

    press(&mut configured, &[KeyEvent::Alt('w')]).await;
    assert_eq!(configured.current_mode(), TREE_MODE);
    press(&mut configured, &[KeyEvent::Char('k'), KeyEvent::Enter]).await;

    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert!(configured.chooser().is_none());
    assert!(transport.requests().iter().any(|request| matches!(
        request,
        ClientMessage::Session(SessionRequest::SelectRootTab {
            session_id: SESSION_ID,
            index: 0,
            ..
        })
    )));
}

#[tokio::test]
async fn enter_on_another_sessions_pane_switches_and_focuses_it() {
    let transport = FakeTransport::default();
    let (mut configured, _tempdir) = configured(transport.clone(), Arc::default());

    press(&mut configured, &[KeyEvent::Alt('t'), KeyEvent::Char('G')]).await;
    press(&mut configured, &[KeyEvent::Enter]).await;

    let requests = transport.requests();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            ClientMessage::Client(ClientRequest::Switch {
                session_id: OTHER_SESSION,
                ..
            })
        )),
        "{requests:?}"
    );
}

#[tokio::test]
async fn enter_on_a_pane_of_the_current_session_focuses_it() {
    let transport = FakeTransport::default();
    let (mut configured, _tempdir) = configured(transport.clone(), Arc::default());
    // Make the demo session current without a switch request.
    press(&mut configured, &[KeyEvent::Alt('t')]).await;
    // From the focused pane, up two rows is the left pane of the same window.
    press(
        &mut configured,
        &[KeyEvent::Up, KeyEvent::Up, KeyEvent::Enter],
    )
    .await;

    assert!(transport.requests().iter().any(|request| matches!(
        request,
        ClientMessage::Node(NodeRequest::Focus {
            session_id: SESSION_ID,
            node_id,
            ..
        }) if *node_id == LEFT_LEAF_ID
    )));
}

#[tokio::test]
async fn x_kills_with_the_cli_commands_after_confirmation() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(FakeTransport::default(), runner.clone());

    // Declined: nothing runs, the picker stays open.
    press(
        &mut configured,
        &[KeyEvent::Alt('t'), KeyEvent::Char('x'), KeyEvent::Char('n')],
    )
    .await;
    assert!(runner.calls.lock().unwrap().is_empty());
    assert_eq!(configured.current_mode(), TREE_MODE);

    // The focused pane.
    press(&mut configured, &[KeyEvent::Char('x')]).await;
    assert!(
        configured
            .render_session(SESSION_ID, VIEWPORT)
            .await
            .unwrap()
            .lines()
            .iter()
            .any(|line| line.contains("kill pane")),
    );
    press(&mut configured, &[KeyEvent::Char('y')]).await;
    // Its window, then the other session.
    press(
        &mut configured,
        &[
            KeyEvent::Char('h'),
            KeyEvent::Char('x'),
            KeyEvent::Char('y'),
            KeyEvent::Char('G'),
            // Pane to window, fold the window, window to session.
            KeyEvent::Char('h'),
            KeyEvent::Char('h'),
            KeyEvent::Char('h'),
            KeyEvent::Char('x'),
            KeyEvent::Char('y'),
        ],
    )
    .await;

    let calls = runner.calls.lock().unwrap().clone();
    let argv: Vec<_> = calls.iter().map(|(argv, _)| argv.join(" ")).collect();
    assert_eq!(
        argv,
        vec![
            format!("kill-pane -t {FOCUSED_LEAF_ID}"),
            "kill-window -t 1".to_owned(),
            "kill-session".to_owned(),
        ]
    );
    assert_eq!(calls[1].1.session_id, Some(SESSION_ID));
    assert_eq!(calls[1].1.base_index, Some(0));
    assert_eq!(calls[2].1.session_id, Some(OTHER_SESSION));
}

#[tokio::test]
async fn choose_commands_open_the_picker_without_the_runner() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(FakeTransport::default(), runner.clone());

    press(&mut configured, &[KeyEvent::Alt('c')]).await;

    assert_eq!(configured.current_mode(), TREE_MODE);
    assert_eq!(
        configured.chooser().map(|chooser| chooser.scope),
        Some(ChooserScope::Sessions)
    );
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn typing_filters_and_q_closes() {
    let transport = FakeTransport::default();
    let (mut configured, _tempdir) = configured(transport.clone(), Arc::default());

    press(&mut configured, &[KeyEvent::Alt('s')]).await;
    press(
        &mut configured,
        &[
            KeyEvent::Char('o'),
            KeyEvent::Char('t'),
            KeyEvent::Char('h'),
        ],
    )
    .await;
    assert_eq!(configured.chooser().unwrap().filter, "oth");
    let grid = configured
        .render_session(SESSION_ID, VIEWPORT)
        .await
        .unwrap();
    let text = grid.lines().join("\n");
    assert!(text.contains("choose-session"), "{text}");
    assert!(text.contains("other: 1 window"), "{text}");
    assert!(!text.contains("demo: 2 windows"), "{text}");
    assert!(text.contains("filter: oth"), "{text}");

    // Backspace and C-h (what a host without the kitty protocol may send for
    // it) both delete a character, as in tmux's prompts.
    press(&mut configured, &[KeyEvent::Backspace, KeyEvent::Ctrl('h')]).await;
    assert_eq!(configured.chooser().unwrap().filter, "o");
    press(&mut configured, &[KeyEvent::Char('t'), KeyEvent::Char('h')]).await;

    // Enter ends filtering; Esc then clears the filter; q closes.
    press(&mut configured, &[KeyEvent::Enter, KeyEvent::Escape]).await;
    assert_eq!(configured.chooser().unwrap().filter, "");
    press(&mut configured, &[KeyEvent::Char('/'), KeyEvent::Char('q')]).await;
    assert_eq!(configured.chooser().unwrap().filter, "q");
    press(&mut configured, &[KeyEvent::Escape, KeyEvent::Char('q')]).await;
    assert!(configured.chooser().is_none());
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert!(transport.requests().is_empty());
}
