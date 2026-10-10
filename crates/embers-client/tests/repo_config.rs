use std::path::PathBuf;

use embers_client::{
    Action, BufferSpawnSpec, Context, EventInfo, FloatingAnchor, FloatingGeometrySpec,
    FloatingSize, KeyToken, NavigationDirection, PresentationModel, ScriptEngine, TabBarContext,
    TreeSpec,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::{BufferId, Size, SplitDirection};

use crate::support::{ROOT_TABS_ID, SESSION_ID, demo_state};

#[test]
fn repository_config_loads_with_current_public_api() {
    let engine = repository_config_engine();
    assert!(engine.loaded_config().has_tab_bar_formatter());
}

#[test]
fn repository_config_smart_nav_uses_buffer_input_for_nvim() {
    let engine = repository_config_engine();
    let mut state = demo_state();
    state.buffers.get_mut(&BufferId(4)).unwrap().command = vec!["/usr/bin/nvim".to_owned()];
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 24,
        },
    )
    .unwrap();

    assert_eq!(
        engine
            .run_named_action(
                "smart-nav-left",
                Context::from_state(&state, Some(&presentation)),
            )
            .unwrap(),
        vec![Action::SendBytes {
            buffer_id: None,
            bytes: vec![8],
        }]
    );
}

#[test]
fn repository_config_smart_resize_passes_alt_keys_to_nvim() {
    let engine = repository_config_engine();
    let mut state = demo_state();
    // Neovim marks its pane through `embers buffer set-option is-vim 1`.
    state
        .buffers
        .get_mut(&BufferId(4))
        .unwrap()
        .user_options
        .insert("is-vim".to_owned(), "1".to_owned());
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 24,
        },
    )
    .unwrap();

    for (name, key) in [
        ("smart-resize-left", 'h'),
        ("smart-resize-down", 'j'),
        ("smart-resize-up", 'k'),
        ("smart-resize-right", 'l'),
    ] {
        assert_eq!(
            engine
                .run_named_action(name, Context::from_state(&state, Some(&presentation)))
                .unwrap(),
            vec![Action::SendKeys {
                buffer_id: None,
                keys: vec![KeyToken::Alt(key)],
            }],
            "{name}"
        );
    }
}

#[test]
fn repository_config_smart_resize_resizes_other_panes_by_three_cells() {
    let engine = repository_config_engine();

    for (name, direction) in [
        ("smart-resize-left", NavigationDirection::Left),
        ("smart-resize-down", NavigationDirection::Down),
        ("smart-resize-up", NavigationDirection::Up),
        ("smart-resize-right", NavigationDirection::Right),
    ] {
        assert_eq!(
            engine.run_named_action(name, demo_context()).unwrap(),
            vec![Action::ResizePane {
                direction,
                cells: 3
            }],
            "{name}"
        );
    }
}

#[test]
fn repository_config_binds_tmux_prompt_keys() {
    let engine = repository_config_engine();
    let normal = &engine.loaded_config().bindings["normal"];
    let target = |notation: &str| {
        normal
            .iter()
            .find(|binding| binding.notation == notation)
            .map(|binding| binding.target.clone())
            .unwrap_or_else(|| panic!("{notation} is bound"))
    };

    assert!(matches!(
        target("<leader>:").as_slice(),
        [Action::CommandPrompt { template: None, .. }]
    ));
    assert!(matches!(
        target("<leader>,").as_slice(),
        [Action::CommandPrompt { template: Some(template), .. }] if template.starts_with("rename-window")
    ));
    assert!(matches!(
        target("<leader>$").as_slice(),
        [Action::CommandPrompt { template: Some(template), .. }] if template.starts_with("rename-session")
    ));
}

#[test]
fn repository_config_tab_bar_flags_synchronized_windows() {
    let engine = repository_config_engine();
    let mut state = demo_state();
    let presentation = |state: &embers_client::ClientState| {
        PresentationModel::project(
            state,
            SESSION_ID,
            Size {
                width: 80,
                height: 24,
            },
        )
        .unwrap()
    };
    let bar_text = |presentation: &PresentationModel| {
        let bar = engine
            .format_tab_bar(TabBarContext::from_frame(
                presentation.root_tabs.as_ref().unwrap(),
                "normal",
                80,
            ))
            .unwrap()
            .expect("formatter produces a bar");
        bar.left
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>()
    };

    assert!(!bar_text(&presentation(&state)).contains("sync"));

    let root = state.nodes.get_mut(&ROOT_TABS_ID).unwrap();
    root.tabs.as_mut().unwrap().tabs[1].synchronize_panes = true;
    let text = bar_text(&presentation(&state));
    assert!(text.contains(" sync "), "{text:?}");
}

#[test]
fn repository_config_bell_handler_moves_hidden_buffer_to_floating() {
    let engine = repository_config_engine();

    assert_eq!(
        engine
            .dispatch_event(
                "buffer_bell",
                demo_context().with_event(EventInfo {
                    name: "buffer_bell".to_owned(),
                    session_id: Some(SESSION_ID),
                    previous_session_id: None,
                    client_id: None,
                    buffer_id: Some(BufferId(3)),
                    node_id: None,
                    floating_id: None,
                }),
            )
            .unwrap(),
        vec![Action::MoveBufferToFloating {
            buffer_id: BufferId(3),
            geometry: FloatingGeometrySpec {
                width: FloatingSize::Cells(110),
                height: FloatingSize::Cells(32),
                anchor: FloatingAnchor::Center,
                offset_x: 4,
                offset_y: 1,
            },
            title: Some("build".to_owned()),
            focus: true,
            close_on_empty: true,
        }]
    );
}

#[test]
fn repository_config_history_helper_spawns_buffer_with_history_env() {
    let engine = repository_config_engine();
    let actions = engine
        .run_named_action("full-history-tab", demo_context())
        .unwrap();

    let [
        Action::InsertTabAfter {
            tabs_node_id: None,
            title: Some(title),
            child:
                TreeSpec::BufferSpawn(BufferSpawnSpec {
                    title: Some(buffer_title),
                    command,
                    cwd,
                    env,
                }),
        },
    ] = actions.as_slice()
    else {
        panic!("unexpected history action: {actions:?}");
    };

    assert_eq!(title, "full-history");
    assert_eq!(buffer_title, "full-history");
    assert_eq!(
        command,
        &vec![
            "/bin/sh".to_owned(),
            "-lc".to_owned(),
            "printf '%s' \"$EMBERS_HISTORY\" | less -R".to_owned(),
        ]
    );
    assert_eq!(cwd.as_deref(), Some("/tmp"));
    assert!(env["EMBERS_HISTORY"].contains("logs visible"));
    assert!(env["EMBERS_HISTORY"].contains("third row"));
}

#[test]
fn repository_config_split_and_tab_actions_build_expected_shell_trees() {
    let engine = repository_config_engine();

    assert_eq!(
        engine
            .run_named_action("split-below", demo_context())
            .unwrap(),
        vec![Action::SplitCurrent {
            direction: SplitDirection::Horizontal,
            new_child: TreeSpec::BufferSpawn(BufferSpawnSpec {
                title: Some("shell".to_owned()),
                command: vec!["/usr/bin/env".to_owned(), "zsh".to_owned()],
                cwd: Some("/tmp".to_owned()),
                env: Default::default(),
            }),
        }]
    );

    assert_eq!(
        engine
            .run_named_action("new-shell-tab", demo_context())
            .unwrap(),
        vec![Action::InsertTabAfter {
            tabs_node_id: None,
            title: Some("shell".to_owned()),
            child: TreeSpec::BufferSpawn(BufferSpawnSpec {
                title: Some("shell".to_owned()),
                command: vec!["/usr/bin/env".to_owned(), "zsh".to_owned()],
                cwd: Some("/tmp".to_owned()),
                env: Default::default(),
            }),
        }]
    );
}

#[test]
fn repository_config_popup_and_scratchpad_actions_build_floating_layouts() {
    let engine = repository_config_engine();

    assert_eq!(
        engine
            .run_named_action("shell-popup", demo_context())
            .unwrap(),
        vec![Action::OpenFloating {
            spec: embers_client::FloatingSpec {
                tree: TreeSpec::BufferSpawn(BufferSpawnSpec {
                    title: Some("shell".to_owned()),
                    command: vec!["/usr/bin/env".to_owned(), "zsh".to_owned()],
                    cwd: Some("/tmp".to_owned()),
                    env: Default::default(),
                }),
                geometry: FloatingGeometrySpec {
                    width: FloatingSize::Cells(100),
                    height: FloatingSize::Cells(28),
                    anchor: FloatingAnchor::Center,
                    offset_x: 8,
                    offset_y: 2,
                },
                title: Some("shell".to_owned()),
                focus: true,
                close_on_empty: true,
            },
        }]
    );

    let actions = engine
        .run_named_action("scratchpad", demo_context())
        .unwrap();
    let [Action::OpenFloating { spec }] = actions.as_slice() else {
        panic!("unexpected scratchpad action: {actions:?}");
    };

    assert_eq!(spec.title.as_deref(), Some("scratchpad"));
    assert_eq!(
        spec.geometry,
        FloatingGeometrySpec {
            width: FloatingSize::Cells(100),
            height: FloatingSize::Cells(28),
            anchor: FloatingAnchor::Center,
            offset_x: 10,
            offset_y: 3,
        }
    );

    let TreeSpec::Tabs(tabs) = &spec.tree else {
        panic!("scratchpad should build tabs, got {:?}", spec.tree);
    };
    assert_eq!(tabs.active, 0);
    assert_eq!(tabs.tabs.len(), 2);
    assert_eq!(tabs.tabs[0].title, "shell");
    assert_eq!(tabs.tabs[1].title, "tools");
}

fn repository_config_engine() -> ScriptEngine {
    ScriptEngine::load(&LoadedConfigSource {
        origin: ConfigOrigin::Explicit,
        path: Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repository_config.rhai"),
        ),
        source: include_str!("fixtures/repository_config.rhai").to_owned(),
        source_hash: 0,
    })
    .unwrap()
}

fn demo_context() -> Context {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 24,
        },
    )
    .unwrap();
    Context::from_state(&state, Some(&presentation))
}
