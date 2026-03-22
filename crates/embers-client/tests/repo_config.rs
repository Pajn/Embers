mod support;

use std::fs;
use std::path::PathBuf;

use embers_client::{
    Action, BufferSpawnSpec, Context, EventInfo, FloatingAnchor, FloatingGeometrySpec,
    FloatingSize, PresentationModel, ScriptEngine, TreeSpec,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::{BufferId, Size, SplitDirection};

use support::{SESSION_ID, demo_state};

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
fn repository_config_bell_handler_moves_hidden_buffer_to_floating() {
    let engine = repository_config_engine();

    assert_eq!(
        engine
            .dispatch_event(
                "buffer_bell",
                demo_context().with_event(EventInfo {
                    name: "buffer_bell".to_owned(),
                    session_id: Some(SESSION_ID),
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
                command: vec!["/bin/zsh".to_owned()],
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
                command: vec!["/bin/zsh".to_owned()],
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
                    command: vec!["/bin/zsh".to_owned()],
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config.rhai")
        .canonicalize()
        .unwrap();
    let source = fs::read_to_string(&path).unwrap();

    ScriptEngine::load(&LoadedConfigSource {
        origin: ConfigOrigin::Explicit,
        path: Some(path),
        source,
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
