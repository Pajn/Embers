use std::path::Path;

use embers_client::input::KeyParseError;
use embers_client::{
    Action, InputResolution, KeyToken, PassthroughMode, PresentationModel, ScriptEngine,
    ScriptHarness, StatusLineMode, TabBarContext,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::Size;

use crate::support::{SESSION_ID, demo_state};

#[test]
fn loaded_config_debug_snapshot_is_stable() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("snapshot-config.rhai".into()),
        source: r##"
            fn split_workspace(ctx) { () }
            fn on_created(ctx) { () }
            fn format_tabs(ctx) { ui.bar([], [], []) }

            set_leader("<C-a>");
            define_mode("locked");
            define_action("workspace-split", split_workspace);
            bind("normal", "<leader>ws", "workspace-split");
            on("session_created", on_created);
            tabbar.set_formatter(format_tabs);
            theme.set_palette(#{ active: "#00ff00", inactive: "#333333" });
        "##
        .trim()
        .to_owned(),
        source_hash: 0,
    };

    let engine = ScriptEngine::load(&source).unwrap();
    let loaded = engine.loaded_config();
    let debug_output = format!("{loaded:#?}");

    assert_eq!(
        loaded.source_path.as_deref(),
        Some(Path::new("snapshot-config.rhai"))
    );
    assert_eq!(loaded.source_hash, 0);
    assert_eq!(loaded.leader, vec![KeyToken::Ctrl('a')]);
    assert!(loaded.modes.contains_key("locked"));
    assert_eq!(loaded.bindings["normal"][0].notation, "<leader>ws");
    assert_eq!(
        loaded.named_actions["workspace-split"].name,
        "split_workspace"
    );
    assert_eq!(
        loaded.event_handlers["session_created"][0].name,
        "on_created"
    );
    assert_eq!(
        loaded
            .tab_bar_formatter
            .as_ref()
            .map(|formatter| formatter.name.as_str()),
        Some("format_tabs")
    );
    assert_eq!(loaded.theme.palette["active"].green, 255);
    assert!(debug_output.contains("source_path: Some("));
    assert!(debug_output.contains("ast: \"<ast>\""));
}

#[test]
fn harness_resolves_leader_binding_to_exact_match() {
    let mut harness = ScriptHarness::load(
        r#"
            fn split_workspace(ctx) { () }
            define_action("workspace-split", split_workspace);
            set_leader("<C-a>");
            bind("normal", "<leader>ws", "workspace-split");
        "#,
    )
    .unwrap();

    assert_eq!(
        harness.resolve_notation("normal", "<C-a>w").unwrap(),
        InputResolution::PrefixMatch
    );
    assert_eq!(
        harness.resolve_notation("normal", "s").unwrap(),
        InputResolution::ExactMatch(embers_client::BindingMatch {
            mode: "normal".to_owned(),
            sequence: vec![
                KeyToken::Ctrl('a'),
                KeyToken::Char('w'),
                KeyToken::Char('s'),
            ],
            target: vec![Action::RunNamedAction {
                name: "workspace-split".to_owned(),
            }],
        })
    );
}

#[test]
fn same_sequence_can_resolve_differently_by_mode() {
    let mut harness = ScriptHarness::load(
        r#"
            fn normal_action(ctx) { () }
            fn copy_action(ctx) { () }
            define_action("normal-a", normal_action);
            define_action("copy-a", copy_action);
            bind("normal", "a", "normal-a");
            bind("copy", "a", "copy-a");
        "#,
    )
    .unwrap();

    assert_eq!(
        harness.resolve_notation("normal", "a").unwrap(),
        InputResolution::ExactMatch(embers_client::BindingMatch {
            mode: "normal".to_owned(),
            sequence: vec![KeyToken::Char('a')],
            target: vec![Action::RunNamedAction {
                name: "normal-a".to_owned(),
            }],
        })
    );
    assert_eq!(
        harness.resolve_notation("copy", "a").unwrap(),
        InputResolution::ExactMatch(embers_client::BindingMatch {
            mode: "copy".to_owned(),
            sequence: vec![KeyToken::Char('a')],
            target: vec![Action::RunNamedAction {
                name: "copy-a".to_owned(),
            }],
        })
    );
}

#[test]
fn define_mode_rejects_unknown_options() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("bad-mode-options.rhai".into()),
        source: r#"
            fn enter(ctx) { () }
            define_mode("locked", #{
                on_enter: enter,
                on_entter: enter
            });
        "#
        .trim()
        .to_owned(),
        source_hash: 0,
    };

    let error = match ScriptEngine::load(&source) {
        Ok(_) => panic!("unknown mode options should fail"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("unknown mode option(s): on_entter")
    );
}

#[test]
fn formatter_functions_build_bar_specs_from_runtime_context() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("formatters.rhai".into()),
        source: r##"
            fn format_tabs(ctx) {
                let tabs = ctx.tabs();
                let active = tabs[ctx.active_index()];
                if ctx.is_root() {
                    ui.bar([
                        ui.segment("ROOT ", #{
                            fg: theme.color("active"),
                            bg: theme.color("inactive")
                        }),
                        ui.segment(active.title())
                    ], [], [])
                } else {
                    ui.bar([
                        ui.segment("NESTED "),
                        ui.segment(active.title(), #{ fg: theme.color("active") })
                    ], [], [])
                }
            }

            tabbar.set_formatter(format_tabs);
            theme.set_palette(#{ active: "#00ff00", inactive: "#102030" });
        "##
        .trim()
        .to_owned(),
        source_hash: 0,
    };
    let engine = ScriptEngine::load(&source).unwrap();
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 20,
        },
    )
    .unwrap();

    let root = engine
        .format_tab_bar(TabBarContext::from_frame(
            presentation.root_tabs.as_ref().unwrap(),
            "normal",
            80,
        ))
        .unwrap()
        .unwrap();
    let nested = engine
        .format_tab_bar(TabBarContext::from_frame(
            presentation.focused_tabs().unwrap(),
            "normal",
            80,
        ))
        .unwrap()
        .unwrap();

    assert_eq!(root.left.len(), 2);
    assert!(root.center.is_empty());
    assert!(root.right.is_empty());
    assert_eq!(root.left[0].text, "ROOT ");
    assert_eq!(root.left[1].text, "workspace");
    assert_eq!(root.left[0].style.fg.unwrap().green, 255);
    assert_eq!(root.left[0].style.bg.unwrap().blue, 48);

    assert_eq!(nested.left.len(), 2);
    assert!(nested.center.is_empty());
    assert!(nested.right.is_empty());
    assert_eq!(nested.left[0].text, "NESTED ");
    assert_eq!(nested.left[1].text, "logs-long-title");
    assert_eq!(nested.left[1].style.fg.unwrap().green, 255);
}

#[test]
fn tab_bar_formatters_read_the_pane_each_tab_shows() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: None,
        source: r#"
            fn format_tabs(ctx) {
                let left = [];
                for tab in ctx.tabs() {
                    let cwd = tab.cwd();
                    let cwd = if cwd == () { "-" } else { cwd };
                    left.push(ui.segment(`${tab.process_name()}@${cwd}:${tab.pane_title()}|`));
                }
                ui.bar(left, [], [])
            }
            tabbar.set_formatter(format_tabs);
        "#
        .to_owned(),
        source_hash: 0,
    };
    let engine = ScriptEngine::load(&source).unwrap();
    let mut state = demo_state();
    state
        .buffers
        .get_mut(&embers_core::BufferId(1))
        .unwrap()
        .cwd = None;
    state
        .buffers
        .get_mut(&embers_core::BufferId(4))
        .unwrap()
        .foreground_command = Some(vec!["nvim".to_owned()]);
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 20,
        },
    )
    .unwrap();

    let bar = engine
        .format_tab_bar(TabBarContext::from_frame(
            presentation.root_tabs.as_ref().unwrap(),
            "normal",
            80,
        ))
        .unwrap()
        .unwrap();
    let text: String = bar
        .left
        .iter()
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(text, "sh@-:shell|nvim@/tmp:logs-long-title|");
}

#[test]
fn theme_sets_pane_border_colors() {
    use embers_client::{Color, PaneBorderColors, TerminalColor};

    let harness = ScriptHarness::load(
        r##"
            theme.set_pane_border_color("brightblack");
            theme.set_pane_active_border_color("#ca9ee6");
        "##,
    )
    .unwrap();
    assert_eq!(
        harness.engine().loaded_config().theme.pane_borders,
        PaneBorderColors {
            inactive: Some(TerminalColor::Indexed(8)),
            active: Some(TerminalColor::Rgb(Color {
                red: 0xca,
                green: 0x9e,
                blue: 0xe6,
            })),
        }
    );

    // Both default to unset, which keeps the dim dividers.
    let plain = ScriptHarness::load("").unwrap();
    assert_eq!(
        plain.engine().loaded_config().theme.pane_borders,
        PaneBorderColors::default()
    );

    for bad in [
        r#"theme.set_pane_border_color("mauve");"#,
        r##"theme.set_pane_active_border_color("#12");"##,
    ] {
        assert!(ScriptHarness::load(bad).is_err(), "{bad}");
    }
}

#[test]
fn status_keys_choose_the_prompt_key_table() {
    use embers_client::StatusKeys;

    let keys = |source: &str| {
        ScriptHarness::load(source).map(|harness| harness.engine().loaded_config().status_keys)
    };
    assert_eq!(keys("").unwrap(), StatusKeys::Emacs);
    assert_eq!(keys(r#"set_status_keys("vi");"#).unwrap(), StatusKeys::Vi);
    assert_eq!(
        keys(r#"set_status_keys("emacs");"#).unwrap(),
        StatusKeys::Emacs
    );
    assert!(keys(r#"set_status_keys("vim");"#).is_err());
}

#[test]
fn harness_rejects_empty_notation_without_panicking() {
    let mut harness = ScriptHarness::load("").unwrap();

    assert_eq!(
        harness.resolve_notation("normal", "").unwrap_err(),
        KeyParseError::EmptySequence
    );
}

#[test]
fn passthrough_is_off_unless_the_config_turns_it_on() {
    let load = |source: &str| {
        ScriptEngine::load(&LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: None,
            source: source.to_owned(),
            source_hash: 0,
        })
        .unwrap()
        .loaded_config()
        .passthrough
    };

    // Like tmux's allow-passthrough: a program in a pane cannot write to the
    // host terminal unless the user allows it.
    assert_eq!(load(""), PassthroughMode::Off);
    assert_eq!(load(r#"set_allow_passthrough("on");"#), PassthroughMode::On);
    assert_eq!(
        load(r#"set_allow_passthrough("all");"#),
        PassthroughMode::All
    );
}

#[test]
fn the_root_tab_bar_can_be_hidden() {
    let load = |source: &str| {
        ScriptEngine::load(&LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: None,
            source: source.to_owned(),
            source_hash: 0,
        })
        .map(|engine| engine.loaded_config().tab_bar_visible)
    };

    assert!(load("").unwrap());
    assert!(!load("tabbar.set_visible(false);").unwrap());
    assert!(load("tabbar.set_visible(true);").unwrap());
    assert!(load(r#"tabbar.set_visible("no");"#).is_err());
}

#[test]
fn the_status_line_can_keep_a_row_of_its_own() {
    let load = |source: &str| {
        ScriptEngine::load(&LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: None,
            source: source.to_owned(),
            source_hash: 0,
        })
        .map(|engine| engine.loaded_config().status_line)
    };

    assert_eq!(load("").unwrap(), StatusLineMode::Overlay);
    assert_eq!(
        load(r#"set_status_line("separate");"#).unwrap(),
        StatusLineMode::Separate
    );
    assert_eq!(
        load(r#"set_status_line("overlay");"#).unwrap(),
        StatusLineMode::Overlay
    );
    assert!(load(r#"set_status_line("off");"#).is_err());
}
