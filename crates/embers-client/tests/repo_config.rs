//! Tests for the example `config.rhai` at the repository root, which
//! translates a typical tmux setup. They load the file itself, so the example
//! cannot drift from the config API.

use std::path::PathBuf;

use embers_client::{
    Action, BufferSpawnSpec, ChooserScope, ClientState, Context, FloatingAnchor,
    FloatingGeometrySpec, FloatingSize, KeyToken, NavigationDirection, PaneBorderColors,
    PresentationModel, ScriptEngine, StatusKeys, StatusLineMode, TabBarContext, TabBarPosition,
    TerminalColor, TreeSpec,
    config::{ConfigOrigin, LoadedConfigSource},
    input::parse_key_sequence,
};
use embers_core::{BufferId, Size, SplitDirection};
use embers_protocol::{BufferHistoryPlacement, BufferHistoryScope};

use crate::support::{FOCUSED_BUFFER_ID, ROOT_TABS_ID, SESSION_ID, demo_state};

#[test]
fn example_config_loads_with_a_c_a_leader_and_settings() {
    let engine = example_config_engine();
    let loaded = engine.loaded_config();
    assert_eq!(loaded.leader, parse_key_sequence("<C-a>").unwrap());
    assert_eq!(loaded.base_index, Some(1));
    assert!(loaded.clipboard);
    assert!(loaded.has_tab_bar_formatter());
    // status-keys vi
    assert_eq!(loaded.status_keys, StatusKeys::Vi);
    // status-position bottom, with messages over the bar
    assert_eq!(loaded.tab_bar_position, TabBarPosition::Bottom);
    assert!(loaded.tab_bar_visible);
    assert_eq!(loaded.status_line, StatusLineMode::Overlay);
    // pane-active-border-style fg=magenta, pane-border-style fg=brightblack
    assert_eq!(
        loaded.theme.pane_borders,
        PaneBorderColors {
            active: Some(TerminalColor::Indexed(5)),
            inactive: Some(TerminalColor::Indexed(8)),
        }
    );
    // Rounded popup frames in the Frappe accent and overlay colors.
    assert_eq!(
        loaded.theme.popup_borders,
        embers_client::PopupBorders {
            lines: embers_client::PopupBorderLines::Rounded,
            active: Some(TerminalColor::Rgb(embers_client::Color {
                red: 0xca,
                green: 0x9e,
                blue: 0xe6,
            })),
            inactive: Some(TerminalColor::Rgb(embers_client::Color {
                red: 0x73,
                green: 0x79,
                blue: 0x94,
            })),
        }
    );
}

#[test]
fn example_config_binds_tmux_keys_to_their_actions() {
    let engine = example_config_engine();
    let named = |name: &str| {
        vec![Action::RunNamedAction {
            name: name.to_owned(),
        }]
    };

    for (mode, notation, expected) in [
        // bind C-a send-prefix
        (
            "normal",
            "<leader><C-a>",
            vec![Action::SendKeys {
                buffer_id: None,
                keys: vec![KeyToken::Ctrl('a')],
            }],
        ),
        ("normal", "<leader>[", vec![Action::EnterCopyMode]),
        (
            "normal",
            "<leader><Space>",
            vec![Action::EnterHints { action: None }],
        ),
        ("normal", "<leader>v", named("split-right")),
        ("normal", "<leader>s", named("split-below")),
        ("normal", "<leader>c", named("new-window")),
        ("normal", "<leader>b", named("file-manager-window")),
        ("normal", "<leader>g", named("lazygit-popup")),
        ("normal", "<leader>l", named("git-log-popup")),
        ("normal", "<leader>d", named("shell-popup")),
        ("normal", "<leader>h", named("visible-history")),
        ("normal", "<leader>H", named("full-history")),
        ("normal", "<leader>'", vec![Action::LastSession]),
        // tmux's default `x` and `&` ask before killing.
        (
            "normal",
            "<leader>x",
            vec![Action::Confirm {
                prompt: "kill-pane #D? (y/n)".to_owned(),
                actions: vec![Action::RunCommand {
                    command: "kill-pane".to_owned(),
                }],
            }],
        ),
        (
            "normal",
            "<leader>&",
            vec![Action::Confirm {
                prompt: "kill-window #W? (y/n)".to_owned(),
                actions: vec![Action::RunCommand {
                    command: "kill-window".to_owned(),
                }],
            }],
        ),
        (
            "normal",
            "<leader>\"",
            vec![Action::ChooseTree {
                scope: ChooserScope::Windows,
            }],
        ),
        (
            "normal",
            "<leader>3",
            vec![Action::SelectWindow { number: 3 }],
        ),
        (
            "normal",
            "<leader>z",
            vec![Action::ToggleZoomNode { node_id: None }],
        ),
        ("normal", "<S-Left>", named("previous-window")),
        ("normal", "<S-Right>", named("next-window")),
        ("normal", "<M-H>", named("previous-window")),
        ("normal", "<M-L>", named("next-window")),
        ("normal", "<C-Tab>", named("next-window")),
        ("normal", "<C-S-Tab>", named("previous-window")),
        ("normal", "<C-h>", named("smart-nav-left")),
        ("normal", "<C-l>", named("smart-nav-right")),
        ("normal", "<M-h>", named("smart-resize-left")),
        ("normal", "<M-l>", named("smart-resize-right")),
        ("normal", "<C-\\>", named("smart-last-pane")),
        (
            "copy",
            "<C-h>",
            vec![Action::FocusDirection {
                direction: NavigationDirection::Left,
            }],
        ),
        (
            "select",
            "<C-j>",
            vec![Action::FocusDirection {
                direction: NavigationDirection::Down,
            }],
        ),
    ] {
        assert_eq!(
            binding(&engine, mode, notation),
            expected,
            "{mode} {notation}"
        );
    }

    // Alt+Shift+h and Alt+h are different keys, as in tmux.
    assert_ne!(
        parse_key_sequence("<M-H>").unwrap(),
        parse_key_sequence("<M-h>").unwrap()
    );
    assert!(matches!(
        binding(&engine, "normal", "<leader>:").as_slice(),
        [Action::CommandPrompt { template: None, .. }]
    ));
    assert!(matches!(
        binding(&engine, "normal", "<leader>,").as_slice(),
        [Action::CommandPrompt { template: Some(template), .. }] if template.starts_with("rename-window")
    ));
}

#[test]
fn example_config_splits_and_windows_start_in_the_pane_directory() {
    let engine = example_config_engine();
    let shell = BufferSpawnSpec {
        title: Some("shell".to_owned()),
        command: vec![std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned())],
        cwd: Some("/tmp".to_owned()),
        env: Default::default(),
    };

    // tmux `split-window -h` is side by side: an Embers "vertical" split.
    assert_eq!(
        run(&engine, "split-right", &demo_state()),
        vec![Action::SplitCurrent {
            direction: SplitDirection::Vertical,
            new_child: TreeSpec::BufferSpawn(shell.clone()),
        }]
    );
    assert_eq!(
        run(&engine, "split-below", &demo_state()),
        vec![Action::SplitCurrent {
            direction: SplitDirection::Horizontal,
            new_child: TreeSpec::BufferSpawn(shell.clone()),
        }]
    );
    assert_eq!(
        run(&engine, "new-window", &demo_state()),
        vec![Action::NewWindow {
            title: Some("shell".to_owned()),
            child: TreeSpec::BufferSpawn(shell),
        }]
    );
    assert_eq!(
        run(&engine, "file-manager-window", &demo_state()),
        vec![Action::NewWindow {
            title: Some("yazi".to_owned()),
            child: TreeSpec::BufferSpawn(BufferSpawnSpec {
                title: Some("yazi".to_owned()),
                command: vec!["yazi".to_owned()],
                cwd: Some("/tmp".to_owned()),
                env: Default::default(),
            }),
        }]
    );
}

#[test]
fn example_config_popups_cover_ninety_percent_in_the_pane_directory() {
    let engine = example_config_engine();
    for (name, title, command) in [
        ("lazygit-popup", "lazygit", vec!["lazygit"]),
        (
            "git-log-popup",
            "git log",
            vec!["sh", "-lc", "git log | nvim -R -"],
        ),
    ] {
        assert_eq!(
            run(&engine, name, &demo_state()),
            vec![Action::OpenFloating {
                spec: embers_client::FloatingSpec {
                    tree: TreeSpec::BufferSpawn(BufferSpawnSpec {
                        title: Some(title.to_owned()),
                        command: command.into_iter().map(str::to_owned).collect(),
                        cwd: Some("/tmp".to_owned()),
                        env: Default::default(),
                    }),
                    geometry: FloatingGeometrySpec {
                        width: FloatingSize::Percent(90),
                        height: FloatingSize::Percent(90),
                        anchor: FloatingAnchor::Center,
                        offset_x: 0,
                        offset_y: 0,
                    },
                    title: Some(title.to_owned()),
                    focus: true,
                    close_on_empty: true,
                    border: true,
                },
            }],
            "{name}"
        );
    }
}

#[test]
fn example_config_switches_windows_of_the_session() {
    let engine = example_config_engine();
    // The focus is in nested tabs; these still switch windows.
    assert_eq!(
        run(&engine, "next-window", &demo_state()),
        vec![Action::NextTab {
            tabs_node_id: Some(ROOT_TABS_ID)
        }]
    );
    assert_eq!(
        run(&engine, "previous-window", &demo_state()),
        vec![Action::PrevTab {
            tabs_node_id: Some(ROOT_TABS_ID)
        }]
    );
}

#[test]
fn example_config_opens_history_of_the_focused_pane() {
    let engine = example_config_engine();
    for (name, scope) in [
        ("visible-history", BufferHistoryScope::Visible),
        ("full-history", BufferHistoryScope::Full),
    ] {
        assert_eq!(
            run(&engine, name, &demo_state()),
            vec![Action::OpenBufferHistory {
                buffer_id: FOCUSED_BUFFER_ID,
                scope,
                placement: BufferHistoryPlacement::Tab,
            }],
            "{name}"
        );
    }
}

#[test]
fn example_config_smart_splits_send_keys_to_neovim() {
    let engine = example_config_engine();
    let vim_state = |configure: &dyn Fn(&mut embers_protocol::BufferRecord)| {
        let mut state = demo_state();
        configure(state.buffers.get_mut(&FOCUSED_BUFFER_ID).unwrap());
        state
    };
    let started_directly = vim_state(&|buffer| buffer.command = vec!["/usr/bin/nvim".to_owned()]);
    let from_a_shell = vim_state(&|buffer| {
        buffer.command = vec!["zsh".to_owned()];
        buffer.foreground_command = Some(vec!["nvim".to_owned(), "notes.txt".to_owned()]);
    });
    let marked = vim_state(&|buffer| {
        buffer
            .user_options
            .insert("is-vim".to_owned(), "1".to_owned());
    });
    for state in [&started_directly, &from_a_shell, &marked] {
        for (name, key) in [
            ("smart-nav-left", KeyToken::Ctrl('h')),
            ("smart-nav-down", KeyToken::Ctrl('j')),
            ("smart-nav-up", KeyToken::Ctrl('k')),
            ("smart-nav-right", KeyToken::Ctrl('l')),
            ("smart-resize-left", KeyToken::Alt('h')),
            ("smart-resize-down", KeyToken::Alt('j')),
            ("smart-resize-up", KeyToken::Alt('k')),
            ("smart-resize-right", KeyToken::Alt('l')),
            ("smart-last-pane", KeyToken::Ctrl('\\')),
        ] {
            assert_eq!(
                run(&engine, name, state),
                vec![Action::SendKeys {
                    buffer_id: None,
                    keys: vec![key],
                }],
                "{name}"
            );
        }
    }

    // The is-vim option overrides what is detected.
    let unmarked = vim_state(&|buffer| {
        buffer.foreground_command = Some(vec!["nvim".to_owned()]);
        buffer
            .user_options
            .insert("is-vim".to_owned(), "0".to_owned());
    });
    assert_eq!(
        run(&engine, "smart-nav-left", &unmarked),
        vec![Action::FocusDirection {
            direction: NavigationDirection::Left
        }]
    );
}

#[test]
fn example_config_smart_splits_move_and_resize_other_panes() {
    let engine = example_config_engine();
    for (name, direction) in [
        ("smart-nav-left", NavigationDirection::Left),
        ("smart-nav-down", NavigationDirection::Down),
        ("smart-nav-up", NavigationDirection::Up),
        ("smart-nav-right", NavigationDirection::Right),
    ] {
        assert_eq!(
            run(&engine, name, &demo_state()),
            vec![Action::FocusDirection { direction }],
            "{name}"
        );
    }
    for (name, direction) in [
        ("smart-resize-left", NavigationDirection::Left),
        ("smart-resize-down", NavigationDirection::Down),
        ("smart-resize-up", NavigationDirection::Up),
        ("smart-resize-right", NavigationDirection::Right),
    ] {
        assert_eq!(
            run(&engine, name, &demo_state()),
            vec![Action::ResizePane {
                direction,
                cells: 3
            }],
            "{name}"
        );
    }
    // tmux `select-pane -l`.
    assert_eq!(
        run(&engine, "smart-last-pane", &demo_state()),
        vec![Action::LastPane]
    );
}

#[test]
fn example_config_labels_windows_from_their_panes() {
    let engine = example_config_engine();
    let mut state = demo_state();
    // Window 1 runs a shell at its prompt; window 2 shows Neovim.
    let shell = state.buffers.get_mut(&BufferId(1)).unwrap();
    shell.command = vec!["-zsh".to_owned()];
    shell.cwd = Some("/home/user/src/embers".to_owned());
    let editor = state.buffers.get_mut(&FOCUSED_BUFFER_ID).unwrap();
    editor.command = vec!["zsh".to_owned()];
    editor.foreground_command = Some(vec!["nvim".to_owned()]);
    editor.cwd = Some("/srv/app".to_owned());

    let (left, right) = tab_bar_text(&engine, &state);
    // The session name comes first, like tmux `#S`.
    assert!(left.starts_with(" demo  "), "{left:?}");
    assert!(left.contains("1 $ src/embers"), "{left:?}");
    assert!(left.contains("2 nvim srv/app"), "{left:?}");
    assert_eq!(right, " /srv/app ");

    // Other programs show the pane title, cut to 24 characters.
    let editor = state.buffers.get_mut(&FOCUSED_BUFFER_ID).unwrap();
    editor.foreground_command = Some(vec!["htop".to_owned()]);
    editor.title = "a pane title that is far too long".to_owned();
    let (left, _) = tab_bar_text(&engine, &state);
    assert!(left.contains("2 a pane title that is fa…"), "{left:?}");

    // A synchronized window is flagged.
    let root = state.nodes.get_mut(&ROOT_TABS_ID).unwrap();
    root.tabs.as_mut().unwrap().tabs[1].synchronize_panes = true;
    let (left, _) = tab_bar_text(&engine, &state);
    assert!(left.contains(" sync"), "{left:?}");
}

fn binding(engine: &ScriptEngine, mode: &str, notation: &str) -> Vec<Action> {
    engine.loaded_config().bindings[mode]
        .iter()
        .find(|binding| binding.notation == notation)
        .map(|binding| binding.target.clone())
        .unwrap_or_else(|| panic!("{mode} {notation} is bound"))
}

fn run(engine: &ScriptEngine, name: &str, state: &ClientState) -> Vec<Action> {
    let presentation = PresentationModel::project(
        state,
        SESSION_ID,
        Size {
            width: 80,
            height: 24,
        },
    )
    .unwrap();
    engine
        .run_named_action(name, Context::from_state(state, Some(&presentation)))
        .unwrap()
}

fn tab_bar_text(engine: &ScriptEngine, state: &ClientState) -> (String, String) {
    let presentation = PresentationModel::project(
        state,
        SESSION_ID,
        Size {
            width: 80,
            height: 24,
        },
    )
    .unwrap();
    let bar = engine
        .format_tab_bar(
            TabBarContext::from_frame(presentation.root_tabs.as_ref().unwrap(), "normal", 80)
                .with_base_index(Some(1))
                .with_session_name("demo"),
        )
        .unwrap()
        .expect("formatter produces a bar");
    let text = |segments: &[embers_client::BarSegment]| {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>()
    };
    (text(&bar.left), text(&bar.right))
}

fn example_config_engine() -> ScriptEngine {
    ScriptEngine::load(&LoadedConfigSource {
        origin: ConfigOrigin::Explicit,
        path: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.rhai")),
        source: include_str!("../../../config.rhai").to_owned(),
        source_hash: 0,
    })
    .unwrap()
}
