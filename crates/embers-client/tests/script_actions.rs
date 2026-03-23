use std::collections::BTreeMap;

use embers_client::{
    Action, BufferSpawnSpec, Context, EventInfo, FloatingAnchor, FloatingGeometrySpec,
    FloatingSize, FloatingSpec, KeyToken, NavigationDirection, NotifyLevel, PresentationModel,
    ScriptEngine, SelectionKind, TabSpec, TabsSpec, TreeSpec,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::{BufferId, FloatingId, NodeId, Size, SplitDirection};

use crate::support::{SESSION_ID, demo_state};

#[test]
fn action_helpers_roundtrip_to_typed_actions() {
    let engine = load_engine(
        r#"
            fn enter_copy_action(ctx) { action.enter_mode("copy") }
            fn focus_left_action(ctx) { action.focus_left() }
            fn select_tab_action(ctx) { action.select_current_tabs(2) }
            fn split_tree_action(ctx) { action.split_with("horizontal", tree.buffer_current()) }
            fn replace_current_action(ctx) { action.replace_current_with(tree.buffer_attach(9)) }
            fn replace_node_action(ctx) { action.replace_node(7, tree.buffer_current()) }
            fn insert_tab_action(ctx) {
                action.insert_tab_after_current("logs", tree.buffer_current())
            }
            fn open_popup_action(ctx) {
                action.open_floating(
                    tree.buffer_current(),
                    #{ x: 1, y: 2, width: 30, height: 10, title: "popup" }
                )
            }
            fn detach_buffer_action(ctx) { action.detach_buffer() }
            fn kill_buffer_action(ctx) { action.kill_buffer() }
            fn send_keys_action(ctx) { action.send_keys_current("abc") }
            fn send_bytes_action(ctx) { action.send_bytes_current([65, 66]) }
            fn scroll_page_action(ctx) { action.scroll_page_up() }
            fn search_action(ctx) { action.enter_search_mode() }
            fn search_next_action(ctx) { action.search_next() }
            fn select_char_action(ctx) { action.enter_select_char() }
            fn select_move_action(ctx) { action.select_move_left() }
            fn yank_action(ctx) { action.yank_selection() }
            fn notify_user_action(ctx) { action.notify("info", "hello") }

            define_action("enter-copy", enter_copy_action);
            define_action("focus-left", focus_left_action);
            define_action("select-tab", select_tab_action);
            define_action("split-tree", split_tree_action);
            define_action("replace-current", replace_current_action);
            define_action("replace-node", replace_node_action);
            define_action("insert-tab", insert_tab_action);
            define_action("open-popup", open_popup_action);
            define_action("detach-buffer", detach_buffer_action);
            define_action("kill-buffer", kill_buffer_action);
            define_action("send-keys", send_keys_action);
            define_action("send-bytes", send_bytes_action);
            define_action("scroll-page", scroll_page_action);
            define_action("search", search_action);
            define_action("search-next", search_next_action);
            define_action("select-char", select_char_action);
            define_action("select-move", select_move_action);
            define_action("yank", yank_action);
            define_action("notify-user", notify_user_action);
        "#,
    );
    let context = demo_context();

    assert_eq!(
        engine
            .run_named_action("enter-copy", context.clone())
            .unwrap(),
        vec![Action::EnterMode {
            mode: "copy".to_owned(),
        }]
    );
    assert_eq!(
        engine
            .run_named_action("focus-left", context.clone())
            .unwrap(),
        vec![Action::FocusDirection {
            direction: NavigationDirection::Left,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("select-tab", context.clone())
            .unwrap(),
        vec![Action::SelectTab {
            tabs_node_id: None,
            index: 2,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("split-tree", context.clone())
            .unwrap(),
        vec![Action::SplitCurrent {
            direction: SplitDirection::Horizontal,
            new_child: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("replace-current", context.clone())
            .unwrap(),
        vec![Action::ReplaceNode {
            node_id: None,
            tree: TreeSpec::BufferAttach {
                buffer_id: BufferId(9),
            },
        }]
    );
    assert_eq!(
        engine
            .run_named_action("replace-node", context.clone())
            .unwrap(),
        vec![Action::ReplaceNode {
            node_id: Some(NodeId(7)),
            tree: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("insert-tab", context.clone())
            .unwrap(),
        vec![Action::InsertTabAfter {
            tabs_node_id: None,
            title: Some("logs".to_owned()),
            child: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("open-popup", context.clone())
            .unwrap(),
        vec![Action::OpenFloating {
            spec: FloatingSpec {
                tree: TreeSpec::BufferCurrent,
                geometry: FloatingGeometrySpec {
                    width: FloatingSize::Cells(30),
                    height: FloatingSize::Cells(10),
                    anchor: FloatingAnchor::Center,
                    offset_x: 1,
                    offset_y: 2,
                },
                title: Some("popup".to_owned()),
                focus: true,
                close_on_empty: true,
            },
        }]
    );
    assert_eq!(
        engine
            .run_named_action("detach-buffer", context.clone())
            .unwrap(),
        vec![Action::DetachBuffer { buffer_id: None }]
    );
    assert_eq!(
        engine
            .run_named_action("kill-buffer", context.clone())
            .unwrap(),
        vec![Action::KillBuffer { buffer_id: None }]
    );
    assert_eq!(
        engine
            .run_named_action("send-keys", context.clone())
            .unwrap(),
        vec![Action::SendKeys {
            buffer_id: None,
            keys: vec![
                KeyToken::Char('a'),
                KeyToken::Char('b'),
                KeyToken::Char('c'),
            ],
        }]
    );
    assert_eq!(
        engine
            .run_named_action("send-bytes", context.clone())
            .unwrap(),
        vec![Action::SendBytes {
            buffer_id: None,
            bytes: vec![65, 66],
        }]
    );
    assert_eq!(
        engine
            .run_named_action("scroll-page", context.clone())
            .unwrap(),
        vec![Action::ScrollPageUp]
    );
    assert_eq!(
        engine.run_named_action("search", context.clone()).unwrap(),
        vec![Action::EnterSearchMode]
    );
    assert_eq!(
        engine
            .run_named_action("search-next", context.clone())
            .unwrap(),
        vec![Action::SearchNext]
    );
    assert_eq!(
        engine
            .run_named_action("select-char", context.clone())
            .unwrap(),
        vec![Action::EnterSelect {
            kind: SelectionKind::Character,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("select-move", context.clone())
            .unwrap(),
        vec![Action::SelectMove {
            direction: NavigationDirection::Left,
        }]
    );
    assert_eq!(
        engine.run_named_action("yank", context.clone()).unwrap(),
        vec![Action::CopySelection]
    );
    assert_eq!(
        engine.run_named_action("notify-user", context).unwrap(),
        vec![Action::Notify {
            level: NotifyLevel::Info,
            message: "hello".to_owned(),
        }]
    );
}

#[test]
fn action_arrays_preserve_order_and_unit_is_noop() {
    let engine = load_engine(
        r#"
            fn chained(ctx) {
                [action.focus_left(), action.focus_right(), action.focus_up()]
            }
            fn noop(ctx) { () }
            define_action("chained", chained);
            define_action("noop", noop);
        "#,
    );
    let context = demo_context();

    assert_eq!(
        engine.run_named_action("chained", context.clone()).unwrap(),
        vec![
            Action::FocusDirection {
                direction: NavigationDirection::Left,
            },
            Action::FocusDirection {
                direction: NavigationDirection::Right,
            },
            Action::FocusDirection {
                direction: NavigationDirection::Up,
            },
        ]
    );
    assert!(engine.run_named_action("noop", context).unwrap().is_empty());
}

#[test]
fn invalid_action_shapes_fail_cleanly() {
    let engine = load_engine(
        r#"
            fn bad_bytes(ctx) { action.send_bytes_current(["x"]) }
            define_action("bad-bytes", bad_bytes);
        "#,
    );

    let error = engine
        .run_named_action("bad-bytes", demo_context())
        .expect_err("invalid action arguments should fail");

    assert!(
        error
            .to_string()
            .contains("send_bytes expects an array of integers")
    );
}

#[test]
fn query_api_supports_smart_nav_style_scripts() {
    let engine = load_engine(
        r#"
            fn smart_nav_left(ctx) {
                let buffer = ctx.current_buffer();
                let node = ctx.current_node();
                if buffer.is_visible()
                    && !buffer.is_detached()
                    && buffer.process_name() == "sh"
                    && buffer.command()[0] == "/bin/sh"
                    && node.kind() == "buffer_view"
                {
                    action.send_keys_current("h")
                } else {
                    action.focus_left()
                }
            }
            define_action("smart-nav-left", smart_nav_left);
        "#,
    );

    assert_eq!(
        engine
            .run_named_action("smart-nav-left", demo_context())
            .unwrap(),
        vec![Action::SendKeys {
            buffer_id: None,
            keys: vec![KeyToken::Char('h')],
        }]
    );
}

#[test]
fn event_handlers_can_inspect_visibility_and_session_relationships() {
    let engine = load_engine(
        r#"
            fn bell_handler(ctx) {
                let session = ctx.current_session();
                let event = ctx.event();
                if session.name() == "demo"
                    && session.floating().len > 0
                    && event.name() == "buffer_bell"
                {
                    action.notify("info", "floating-visible")
                } else {
                    ()
                }
            }
            on("buffer_bell", bell_handler);
        "#,
    );

    assert_eq!(
        engine
            .dispatch_event(
                "buffer_bell",
                demo_context().with_event(EventInfo {
                    name: "buffer_bell".to_owned(),
                    session_id: Some(SESSION_ID),
                    buffer_id: Some(BufferId(4)),
                    node_id: None,
                    floating_id: Some(FloatingId(90)),
                }),
            )
            .unwrap(),
        vec![Action::Notify {
            level: NotifyLevel::Info,
            message: "floating-visible".to_owned(),
        }]
    );
}

#[test]
fn missing_optional_values_surface_as_unit() {
    let engine = load_engine(
        r#"
            fn check_missing(ctx) {
                if ctx.current_buffer() == () && ctx.current_node() == () {
                    action.notify("warn", "missing")
                } else {
                    action.focus_left()
                }
            }
            define_action("check-missing", check_missing);
        "#,
    );

    assert_eq!(
        engine
            .run_named_action("check-missing", Context::default())
            .unwrap(),
        vec![Action::Notify {
            level: NotifyLevel::Warn,
            message: "missing".to_owned(),
        }]
    );
}

#[test]
fn tree_builders_roundtrip_nested_specs() {
    let engine = load_engine(
        r#"
            fn build_tree(ctx) {
                action.replace_current_with(
                    tree.tabs_with_active([
                        tree.tab("main", tree.split("horizontal", [
                            tree.buffer_current(),
                            tree.buffer_attach(9)
                        ], [1, 2])),
                        tree.tab("scratch", tree.buffer_spawn(
                            ["/bin/sh"],
                            #{ title: "scratch", cwd: "/tmp" }
                        ))
                    ], 1)
                )
            }
            define_action("build-tree", build_tree);
        "#,
    );

    assert_eq!(
        engine
            .run_named_action("build-tree", demo_context())
            .unwrap(),
        vec![Action::ReplaceNode {
            node_id: None,
            tree: TreeSpec::Tabs(TabsSpec {
                tabs: vec![
                    TabSpec {
                        title: "main".to_owned(),
                        tree: Box::new(TreeSpec::Split {
                            direction: SplitDirection::Horizontal,
                            children: vec![
                                TreeSpec::BufferCurrent,
                                TreeSpec::BufferAttach {
                                    buffer_id: BufferId(9),
                                },
                            ],
                            sizes: vec![1, 2],
                        }),
                    },
                    TabSpec {
                        title: "scratch".to_owned(),
                        tree: Box::new(TreeSpec::BufferSpawn(BufferSpawnSpec {
                            title: Some("scratch".to_owned()),
                            command: vec!["/bin/sh".to_owned()],
                            cwd: Some("/tmp".to_owned()),
                            env: BTreeMap::new(),
                        })),
                    },
                ],
                active: 1,
            }),
        }]
    );
}

#[test]
fn invalid_tree_specs_are_rejected() {
    let empty_split = load_engine(
        r#"
            fn bad_split(ctx) { action.replace_current_with(tree.split_h([])) }
            define_action("bad-split", bad_split);
        "#,
    );
    let empty_tabs = load_engine(
        r#"
            fn bad_tabs(ctx) { action.replace_current_with(tree.tabs([])) }
            define_action("bad-tabs", bad_tabs);
        "#,
    );
    let bad_active = load_engine(
        r#"
            fn bad_active(ctx) {
                action.replace_current_with(
                    tree.tabs_with_active([tree.tab("main", tree.buffer_current())], 2)
                )
            }
            define_action("bad-active", bad_active);
        "#,
    );
    let bad_sizes = load_engine(
        r#"
            fn bad_sizes(ctx) {
                action.replace_current_with(
                    tree.split("horizontal", [tree.buffer_current()], [0])
                )
            }
            define_action("bad-sizes", bad_sizes);
        "#,
    );

    assert!(
        empty_split
            .run_named_action("bad-split", demo_context())
            .unwrap_err()
            .to_string()
            .contains("split children cannot be empty")
    );
    assert!(
        empty_tabs
            .run_named_action("bad-tabs", demo_context())
            .unwrap_err()
            .to_string()
            .contains("tabs cannot be empty")
    );
    assert!(
        bad_active
            .run_named_action("bad-active", demo_context())
            .unwrap_err()
            .to_string()
            .contains("active tab index is out of bounds")
    );
    assert!(
        bad_sizes
            .run_named_action("bad-sizes", demo_context())
            .unwrap_err()
            .to_string()
            .contains("split size must be greater than zero")
    );
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

fn load_engine(source: &str) -> ScriptEngine {
    ScriptEngine::load(&LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("script-actions.rhai".into()),
        source: source.trim().to_owned(),
        source_hash: 0,
    })
    .unwrap()
}
