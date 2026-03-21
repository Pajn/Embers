mod support;

use embers_client::{
    Action, BufferSpawnSpec, BufferTarget, Context, FloatingOptions, NodeTarget, PresentationModel,
    ScriptEngine, TabSpec, TreeSpec, WeightedTreeSpec,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::{BufferId, FloatGeometry, NodeId, Size, SplitDirection};

use support::{SESSION_ID, demo_state};

#[test]
fn action_helpers_roundtrip_to_typed_actions() {
    let engine = load_engine(
        r#"
            fn enter_copy_action() { action.enter_mode("copy") }
            fn focus_left_action() { action.focus_left() }
            fn resize_right_action() { action.resize_right(2) }
            fn select_tab_action() { action.select_tab(2) }
            fn split_tree_action() { action.split_h(tree.buffer_current()) }
            fn replace_current_action() { action.replace_current_with(tree.buffer_attach(9)) }
            fn replace_node_action() { action.replace_node(7, tree.buffer_current()) }
            fn wrap_split_action() { action.wrap_current_in_split_v(tree.buffer_current()) }
            fn wrap_tabs_action() {
                action.wrap_current_in_tabs([
                    tree.tab("main", tree.current_node()),
                    tree.tab("scratch", tree.buffer_empty())
                ], 1)
            }
            fn insert_tab_action() { action.insert_tab_after_current("logs", tree.buffer_current()) }
            fn open_popup_action() {
                action.open_floating(
                    tree.buffer_current(),
                    #{ x: 1, y: 2, width: 30, height: 10, title: "popup" }
                )
            }
            fn detach_buffer_action() { action.detach_current_buffer() }
            fn kill_buffer_action() { action.kill_current_buffer() }
            fn send_keys_action() { action.send_keys("abc") }
            fn send_bytes_action() { action.send_bytes([65, 66]) }
            fn notify_user_action() { action.notify("hello") }
            fn reload_cfg_action() { action.reload_config() }

            define_action("enter-copy", enter_copy_action);
            define_action("focus-left", focus_left_action);
            define_action("resize-right", resize_right_action);
            define_action("select-tab", select_tab_action);
            define_action("split-tree", split_tree_action);
            define_action("replace-current", replace_current_action);
            define_action("replace-node", replace_node_action);
            define_action("wrap-split", wrap_split_action);
            define_action("wrap-tabs", wrap_tabs_action);
            define_action("insert-tab", insert_tab_action);
            define_action("open-popup", open_popup_action);
            define_action("detach-buffer", detach_buffer_action);
            define_action("kill-buffer", kill_buffer_action);
            define_action("send-keys", send_keys_action);
            define_action("send-bytes", send_bytes_action);
            define_action("notify-user", notify_user_action);
            define_action("reload-cfg", reload_cfg_action);
        "#,
    );
    let context = demo_context();

    assert_eq!(
        engine.run_named_action("enter-copy", context.clone()).unwrap(),
        vec![Action::EnterMode {
            mode: "copy".to_owned(),
        }]
    );
    assert_eq!(
        engine.run_named_action("focus-left", context.clone()).unwrap(),
        vec![Action::Focus {
            direction: embers_client::NavigationDirection::Left,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("resize-right", context.clone())
            .unwrap(),
        vec![Action::Resize {
            direction: embers_client::NavigationDirection::Right,
            amount: 2,
        }]
    );
    assert_eq!(
        engine.run_named_action("select-tab", context.clone()).unwrap(),
        vec![Action::SelectTab { index: 2 }]
    );
    assert_eq!(
        engine.run_named_action("split-tree", context.clone()).unwrap(),
        vec![Action::Split {
            direction: SplitDirection::Horizontal,
            tree: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("replace-current", context.clone())
            .unwrap(),
        vec![Action::ReplaceCurrentWith {
            tree: TreeSpec::BufferAttach {
                buffer_id: BufferId(9),
            },
        }]
    );
    assert_eq!(
        engine.run_named_action("replace-node", context.clone()).unwrap(),
        vec![Action::ReplaceNode {
            target: NodeTarget::Node(NodeId(7)),
            tree: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine.run_named_action("wrap-split", context.clone()).unwrap(),
        vec![Action::WrapCurrentInSplit {
            direction: SplitDirection::Vertical,
            tree: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine.run_named_action("wrap-tabs", context.clone()).unwrap(),
        vec![Action::WrapCurrentInTabs {
            tabs: vec![
                TabSpec {
                    title: "main".to_owned(),
                    tree: Box::new(TreeSpec::CurrentNode),
                },
                TabSpec {
                    title: "scratch".to_owned(),
                    tree: Box::new(TreeSpec::BufferEmpty),
                },
            ],
            active: 1,
        }]
    );
    assert_eq!(
        engine.run_named_action("insert-tab", context.clone()).unwrap(),
        vec![Action::InsertTabAfterCurrent {
            title: "logs".to_owned(),
            tree: TreeSpec::BufferCurrent,
        }]
    );
    assert_eq!(
        engine.run_named_action("open-popup", context.clone()).unwrap(),
        vec![Action::OpenFloating {
            tree: TreeSpec::BufferCurrent,
            options: FloatingOptions {
                geometry: FloatGeometry::new(1, 2, 30, 10),
                title: Some("popup".to_owned()),
            },
        }]
    );
    assert_eq!(
        engine
            .run_named_action("detach-buffer", context.clone())
            .unwrap(),
        vec![Action::DetachBuffer {
            target: BufferTarget::Current,
        }]
    );
    assert_eq!(
        engine
            .run_named_action("kill-buffer", context.clone())
            .unwrap(),
        vec![Action::KillBuffer {
            target: BufferTarget::Current,
            force: false,
        }]
    );
    assert_eq!(
        engine.run_named_action("send-keys", context.clone()).unwrap(),
        vec![Action::SendBytes {
            target: BufferTarget::Current,
            bytes: b"abc".to_vec(),
        }]
    );
    assert_eq!(
        engine.run_named_action("send-bytes", context.clone()).unwrap(),
        vec![Action::SendBytes {
            target: BufferTarget::Current,
            bytes: vec![65, 66],
        }]
    );
    assert_eq!(
        engine.run_named_action("notify-user", context.clone()).unwrap(),
        vec![Action::Notify {
            message: "hello".to_owned(),
        }]
    );
    assert_eq!(
        engine.run_named_action("reload-cfg", context).unwrap(),
        vec![Action::ReloadConfig]
    );
}

#[test]
fn action_arrays_preserve_order_and_unit_is_noop() {
    let engine = load_engine(
        r#"
            fn chained() {
                action.chain([action.focus_left(), action.focus_right(), action.focus_up()])
            }
            fn noop() { () }
            define_action("chained", chained);
            define_action("noop", noop);
        "#,
    );
    let context = demo_context();

    assert_eq!(
        engine.run_named_action("chained", context.clone()).unwrap(),
        vec![
            Action::Focus {
                direction: embers_client::NavigationDirection::Left,
            },
            Action::Focus {
                direction: embers_client::NavigationDirection::Right,
            },
            Action::Focus {
                direction: embers_client::NavigationDirection::Up,
            },
        ]
    );
    assert!(engine.run_named_action("noop", context).unwrap().is_empty());
}

#[test]
fn invalid_action_shapes_fail_cleanly() {
    let engine = load_engine(
        r#"
            fn bad_bytes() { action.send_bytes(["x"]) }
            define_action("bad-bytes", bad_bytes);
        "#,
    );

    let error = engine
        .run_named_action("bad-bytes", demo_context())
        .expect_err("invalid action arguments should fail");

    assert!(error.to_string().contains("send_bytes expects an array of integers"));
}

#[test]
fn query_api_supports_smart_nav_style_scripts() {
    let engine = load_engine(
        r#"
            fn smart_nav_left() {
                let buffer = mux.current_buffer();
                let node = mux.current_node();
                if buffer.is_visible()
                    && !buffer.is_detached()
                    && system.process_name(buffer) == "sh"
                    && buffer.command()[0] == "/bin/sh"
                    && node.kind() == "buffer_view"
                {
                    action.send_keys("h")
                } else {
                    action.focus_left()
                }
            }
            define_action("smart-nav-left", smart_nav_left);
        "#,
    );

    assert_eq!(
        engine.run_named_action("smart-nav-left", demo_context()).unwrap(),
        vec![Action::SendBytes {
            target: BufferTarget::Current,
            bytes: b"h".to_vec(),
        }]
    );
}

#[test]
fn event_handlers_can_inspect_visibility_and_session_relationships() {
    let engine = load_engine(
        r#"
            fn bell_handler() {
                let session = mux.current_session();
                if session.name() == "demo" && mux.visible_floating().len > 0 {
                    action.notify("floating-visible")
                } else {
                    ()
                }
            }
            on("bell", bell_handler);
        "#,
    );

    assert_eq!(
        engine.dispatch_event("bell", demo_context()).unwrap(),
        vec![Action::Notify {
            message: "floating-visible".to_owned(),
        }]
    );
}

#[test]
fn missing_optional_values_surface_as_unit() {
    let engine = load_engine(
        r#"
            fn check_missing() {
                if mux.current_buffer() == () && mux.current_node() == () {
                    action.notify("missing")
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
            message: "missing".to_owned(),
        }]
    );
}

#[test]
fn tree_builders_roundtrip_nested_specs() {
    let engine = load_engine(
        r#"
            fn build_tree() {
                action.replace_current_with(
                    tree.tabs_with_active([
                        tree.tab("main", tree.split_h([
                            tree.buffer_current(),
                            tree.weight(2, tree.buffer_attach(9))
                        ])),
                        tree.tab("scratch", tree.buffer_spawn(
                            #{ title: "scratch", command: ["/bin/sh"], cwd: "/tmp" }
                        ))
                    ], 1)
                )
            }
            define_action("build-tree", build_tree);
        "#,
    );

    assert_eq!(
        engine.run_named_action("build-tree", demo_context()).unwrap(),
        vec![Action::ReplaceCurrentWith {
            tree: TreeSpec::Tabs {
                tabs: vec![
                    TabSpec {
                        title: "main".to_owned(),
                        tree: Box::new(TreeSpec::Split {
                            direction: SplitDirection::Horizontal,
                            children: vec![
                                WeightedTreeSpec {
                                    weight: 1,
                                    tree: Box::new(TreeSpec::BufferCurrent),
                                },
                                WeightedTreeSpec {
                                    weight: 2,
                                    tree: Box::new(TreeSpec::BufferAttach {
                                        buffer_id: BufferId(9),
                                    }),
                                },
                            ],
                        }),
                    },
                    TabSpec {
                        title: "scratch".to_owned(),
                        tree: Box::new(TreeSpec::BufferSpawn(BufferSpawnSpec {
                            title: Some("scratch".to_owned()),
                            command: vec!["/bin/sh".to_owned()],
                            cwd: Some("/tmp".to_owned()),
                        })),
                    },
                ],
                active: 1,
            },
        }]
    );
}

#[test]
fn invalid_tree_specs_are_rejected() {
    let empty_split = load_engine(
        r#"
            fn bad_split() { action.replace_current_with(tree.split_h([])) }
            define_action("bad-split", bad_split);
        "#,
    );
    let empty_tabs = load_engine(
        r#"
            fn bad_tabs() { action.replace_current_with(tree.tabs([])) }
            define_action("bad-tabs", bad_tabs);
        "#,
    );
    let bad_active = load_engine(
        r#"
            fn bad_active() {
                action.replace_current_with(
                    tree.tabs_with_active([tree.tab("main", tree.buffer_current())], 2)
                )
            }
            define_action("bad-active", bad_active);
        "#,
    );
    let bad_weight = load_engine(
        r#"
            fn bad_weight() {
                action.replace_current_with(
                    tree.split_h([tree.weight(0, tree.buffer_current())])
                )
            }
            define_action("bad-weight", bad_weight);
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
        bad_weight
            .run_named_action("bad-weight", demo_context())
            .unwrap_err()
            .to_string()
            .contains("weight must be greater than zero")
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
