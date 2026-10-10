//! The command prompt (tmux `:`): editing, history, templates and handing the
//! line to the command runner.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use embers_client::{
    CommandContext, CommandRunner, ConfigDiscoveryOptions, ConfigManager, ConfiguredClient,
    FakeTransport, KeyEvent, MuxClient, NORMAL_MODE, PROMPT_MODE,
};
use embers_core::{MuxError, Result, Size};
use tempfile::tempdir;

use crate::support::{SESSION_ID, demo_state};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

#[derive(Default)]
struct RecordingRunner {
    calls: Mutex<Vec<(Vec<String>, CommandContext)>>,
    reply: Mutex<Option<Result<String>>>,
}

impl RecordingRunner {
    fn calls(&self) -> Vec<(Vec<String>, CommandContext)> {
        self.calls.lock().unwrap().clone()
    }

    fn argv(&self) -> Vec<Vec<String>> {
        self.calls().into_iter().map(|(argv, _)| argv).collect()
    }
}

#[async_trait]
impl CommandRunner for RecordingRunner {
    async fn run(&self, argv: Vec<String>, context: CommandContext) -> Result<String> {
        self.calls.lock().unwrap().push((argv, context));
        self.reply
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok(String::new()))
    }
}

const BINDINGS: &str = r##"
    set_base_index(1);
    bind("normal", "<M-:>", action.command_prompt());
    bind("normal", "<M-,>", action.rename_window_prompt());
    bind("normal", "<M-$>", action.rename_session_prompt());
    bind("normal", "<M-i>", action.command_prompt_with("select-window -t #I"));
    bind("normal", "<M-n>", action.command_prompt_template("(new window) ", "", "new-window --title %%"));
    bind("normal", "<M-r>", action.run_command("list-windows"));
"##;

fn configured(
    runner: Option<Arc<RecordingRunner>>,
) -> (ConfiguredClient<FakeTransport>, tempfile::TempDir) {
    configured_with(runner, "")
}

fn configured_with(
    runner: Option<Arc<RecordingRunner>>,
    extra: &str,
) -> (ConfiguredClient<FakeTransport>, tempfile::TempDir) {
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("config.rhai"),
        format!("{BINDINGS}\n{extra}"),
    )
    .unwrap();
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();
    let mut configured = ConfiguredClient::new(MuxClient::new(FakeTransport::default()), config);
    *configured.client_mut().state_mut() = demo_state();
    if let Some(runner) = runner {
        configured.set_command_runner(runner);
    }
    (configured, tempdir)
}

async fn press(configured: &mut ConfiguredClient<FakeTransport>, keys: &[KeyEvent]) {
    for key in keys {
        configured
            .handle_key(SESSION_ID, VIEWPORT, key.clone())
            .await
            .unwrap();
    }
}

async fn type_text(configured: &mut ConfiguredClient<FakeTransport>, text: &str) {
    let keys: Vec<_> = text.chars().map(KeyEvent::Char).collect();
    press(configured, &keys).await;
}

fn status(configured: &ConfiguredClient<FakeTransport>) -> String {
    configured.status_line(SESSION_ID, Path::new("embers.sock"))
}

#[tokio::test]
async fn typed_lines_run_with_the_session_as_context() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    assert_eq!(configured.current_mode(), PROMPT_MODE);
    assert_eq!(status(&configured), ":");
    assert_eq!(configured.status_cursor(), Some(1));

    type_text(&mut configured, "resize-pane -L 5").await;
    assert_eq!(status(&configured), ":resize-pane -L 5");
    press(&mut configured, &[KeyEvent::Enter]).await;

    assert_eq!(configured.current_mode(), NORMAL_MODE);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, ["resize-pane", "-L", "5"]);
    assert_eq!(calls[0].1.session_id, Some(SESSION_ID));
    assert_eq!(calls[0].1.base_index, Some(1));
}

#[tokio::test]
async fn vi_status_keys_edit_in_normal_mode_and_escape_twice_cancels() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) =
        configured_with(Some(runner.clone()), r#"set_status_keys("vi");"#);

    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    type_text(&mut configured, "kill-window").await;
    // Esc leaves insert mode: the prompt stays open and keys edit the line.
    press(&mut configured, &[KeyEvent::Escape]).await;
    assert_eq!(configured.current_mode(), PROMPT_MODE);
    type_text(&mut configured, "bcwpane").await;
    assert_eq!(status(&configured), ":kill-pane");
    press(&mut configured, &[KeyEvent::Escape]).await;
    type_text(&mut configured, "0x").await;
    assert_eq!(status(&configured), ":ill-pane");
    // A second Esc, in normal mode, cancels.
    press(&mut configured, &[KeyEvent::Escape]).await;
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert!(runner.calls().is_empty());

    // Enter runs the line from either mode, and rename prompts use vi keys too.
    press(&mut configured, &[KeyEvent::Alt(',')]).await;
    press(&mut configured, &[KeyEvent::Escape]).await;
    type_text(&mut configured, "0Cbuild").await;
    press(&mut configured, &[KeyEvent::Enter]).await;
    assert_eq!(runner.argv(), [["rename-window", "--", "build"]]);
}

#[tokio::test]
async fn confirm_runs_its_action_only_after_y() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured_with(
        Some(runner.clone()),
        r#"bind("normal", "<M-x>", action.confirm("kill-window #W? (y/n)", action.run_command("kill-window")));"#,
    );

    press(&mut configured, &[KeyEvent::Alt('x')]).await;
    assert_eq!(configured.current_mode(), PROMPT_MODE);
    assert_eq!(status(&configured), "kill-window workspace? (y/n)");
    assert_eq!(
        configured.status_cursor(),
        Some("kill-window workspace? (y/n)".len() as u16)
    );
    // Any key other than y cancels, as in tmux.
    press(&mut configured, &[KeyEvent::Char('n')]).await;
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert_eq!(configured.status_cursor(), None);
    press(&mut configured, &[KeyEvent::Alt('x'), KeyEvent::Escape]).await;
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert!(runner.calls().is_empty());

    press(&mut configured, &[KeyEvent::Alt('x'), KeyEvent::Char('y')]).await;
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert_eq!(runner.argv(), [["kill-window"]]);
}

#[tokio::test]
async fn editing_keys_and_escape() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    type_text(&mut configured, "kill-pane").await;
    press(
        &mut configured,
        &[KeyEvent::Home, KeyEvent::Delete, KeyEvent::Char('K')],
    )
    .await;
    assert_eq!(status(&configured), ":Kill-pane");
    assert_eq!(configured.status_cursor(), Some(2));
    press(&mut configured, &[KeyEvent::End, KeyEvent::Ctrl('w')]).await;
    assert_eq!(status(&configured), ":");

    press(&mut configured, &[KeyEvent::Escape]).await;
    assert_eq!(configured.current_mode(), NORMAL_MODE);
    assert!(configured.command_prompt().is_none());
    assert!(runner.calls().is_empty());
}

#[tokio::test]
async fn up_and_down_recall_earlier_lines() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    for line in ["new-window", "split-window --horizontal"] {
        press(&mut configured, &[KeyEvent::Alt(':')]).await;
        type_text(&mut configured, line).await;
        press(&mut configured, &[KeyEvent::Enter]).await;
    }

    press(&mut configured, &[KeyEvent::Alt(':'), KeyEvent::Up]).await;
    assert_eq!(status(&configured), ":split-window --horizontal");
    press(&mut configured, &[KeyEvent::Up]).await;
    assert_eq!(status(&configured), ":new-window");
    press(&mut configured, &[KeyEvent::Down, KeyEvent::Down]).await;
    assert_eq!(status(&configured), ":");
    press(&mut configured, &[KeyEvent::Up, KeyEvent::Enter]).await;
    assert_eq!(
        runner.argv().last().unwrap(),
        &["split-window", "--horizontal"]
    );
}

#[tokio::test]
async fn rename_prompts_start_from_the_current_names() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    press(&mut configured, &[KeyEvent::Alt(',')]).await;
    assert_eq!(status(&configured), "(rename-window) workspace");
    press(&mut configured, &[KeyEvent::Ctrl('u')]).await;
    type_text(&mut configured, "it's logs").await;
    press(&mut configured, &[KeyEvent::Enter]).await;

    press(&mut configured, &[KeyEvent::Alt('$')]).await;
    assert_eq!(status(&configured), "(rename-session) demo");
    type_text(&mut configured, "-2").await;
    press(&mut configured, &[KeyEvent::Enter]).await;

    assert_eq!(
        runner.argv(),
        vec![
            vec!["rename-window", "--", "it's logs"],
            vec!["rename-session", "--", "demo-2"],
        ]
    );

    // Template lines are not recalled as commands.
    press(&mut configured, &[KeyEvent::Alt(':'), KeyEvent::Up]).await;
    assert_eq!(status(&configured), ":");
}

#[tokio::test]
async fn prefilled_and_templated_prompts_expand_formats() {
    let runner = Arc::new(RecordingRunner::default());
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    press(&mut configured, &[KeyEvent::Alt('i')]).await;
    assert_eq!(status(&configured), ":select-window -t 2");
    press(&mut configured, &[KeyEvent::Enter]).await;

    press(&mut configured, &[KeyEvent::Alt('n')]).await;
    assert_eq!(status(&configured), "(new window) ");
    type_text(&mut configured, "build logs").await;
    press(&mut configured, &[KeyEvent::Enter]).await;

    assert_eq!(
        runner.argv(),
        vec![
            vec!["select-window", "-t", "2"],
            vec!["new-window", "--title", "build logs"],
        ]
    );
}

#[tokio::test]
async fn output_and_errors_are_shown_in_the_status_line() {
    let runner = Arc::new(RecordingRunner::default());
    *runner.reply.lock().unwrap() = Some(Ok("1\t1\tshell\n2\t0\tlogs".to_owned()));
    let (mut configured, _tempdir) = configured(Some(runner.clone()));

    press(&mut configured, &[KeyEvent::Alt('r')]).await;
    assert_eq!(runner.argv(), vec![vec!["list-windows"]]);
    assert_eq!(configured.status_message(), Some("1 1 shell; 2 0 logs"));

    *runner.reply.lock().unwrap() = Some(Err(MuxError::invalid_input(
        "unrecognized subcommand 'bogus'",
    )));
    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    type_text(&mut configured, "bogus").await;
    press(&mut configured, &[KeyEvent::Enter]).await;
    assert!(
        configured
            .status_message()
            .is_some_and(|message| message.contains("unrecognized subcommand 'bogus'")),
        "{:?}",
        configured.status_message()
    );

    // Unbalanced quotes never reach the runner.
    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    type_text(&mut configured, "rename-window 'oops").await;
    press(&mut configured, &[KeyEvent::Enter]).await;
    assert_eq!(runner.calls().len(), 2);
}

#[tokio::test]
async fn without_a_runner_commands_are_reported_unavailable() {
    let (mut configured, _tempdir) = configured(None);

    press(&mut configured, &[KeyEvent::Alt(':')]).await;
    type_text(&mut configured, "new-window").await;
    press(&mut configured, &[KeyEvent::Enter]).await;

    assert_eq!(
        configured.status_message(),
        Some("commands are not available in this client")
    );
}
