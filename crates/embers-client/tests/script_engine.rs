mod support;

use std::path::Path;

use embers_client::{
    Context, InputResolution, KeyToken, PresentationModel, ScriptEngine, ScriptHarness,
    TabBarContext,
    config::{ConfigOrigin, LoadedConfigSource},
};
use embers_core::Size;

use support::{SESSION_ID, demo_state};

#[test]
fn loaded_config_debug_snapshot_is_stable() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("snapshot-config.rhai".into()),
        source: r##"
            fn split_workspace() { () }
            fn on_created() { () }
            fn root_tabs() { () }

            set_leader("<C-a>");
            define_mode("locked");
            define_action("workspace-split", split_workspace);
            bind("normal", "<leader>ws", "workspace-split");
            on("session-created", on_created);
            tabbar.set_root_formatter(root_tabs);
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
        loaded.event_handlers["session-created"][0].name,
        "on_created"
    );
    assert_eq!(
        loaded
            .root_tab_formatter
            .as_ref()
            .map(|formatter| formatter.name.as_str()),
        Some("root_tabs")
    );
    assert!(loaded.nested_tab_formatter.is_none());
    assert_eq!(loaded.theme.palette["active"].green, 255);
    assert!(debug_output.contains("source_path: Some("));
    assert!(debug_output.contains("ast: \"<ast>\""));
}

#[test]
fn harness_resolves_leader_binding_to_exact_match() {
    let mut harness = ScriptHarness::load(
        r#"
            fn split_workspace() { () }
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
            target: "workspace-split".to_owned(),
        })
    );
}

#[test]
fn same_sequence_can_resolve_differently_by_mode() {
    let mut harness = ScriptHarness::load(
        r#"
            fn normal_action() { () }
            fn copy_action() { () }
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
            target: "normal-a".to_owned(),
        })
    );
    assert_eq!(
        harness.resolve_notation("copy", "a").unwrap(),
        InputResolution::ExactMatch(embers_client::BindingMatch {
            mode: "copy".to_owned(),
            sequence: vec![KeyToken::Char('a')],
            target: "copy-a".to_owned(),
        })
    );
}

#[test]
fn formatter_functions_build_bar_specs_from_runtime_context() {
    let source = LoadedConfigSource {
        origin: ConfigOrigin::BuiltIn,
        path: Some("formatters.rhai".into()),
        source: r##"
            fn root_bar() {
                let tabs = bar.tabs();
                let active = tabs[bar.active_index()];
                ui.bar([
                    ui.segment("ROOT ", theme.color("active"), theme.color("inactive")),
                    ui.segment(active.title())
                ])
            }

            fn nested_bar() {
                let tabs = bar.tabs();
                let active = tabs[bar.active_index()];
                ui.bar([
                    ui.segment("NESTED "),
                    ui.segment(active.title(), theme.color("active"))
                ])
            }

            tabbar.set_root_formatter(root_bar);
            tabbar.set_nested_formatter(nested_bar);
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
    let context = Context::from_state(&state, Some(&presentation));

    let root = engine
        .format_root_tabbar(
            context.clone(),
            TabBarContext::from_frame(presentation.root_tabs.as_ref().unwrap()),
        )
        .unwrap()
        .unwrap();
    let nested = engine
        .format_nested_tabbar(
            context,
            TabBarContext::from_frame(presentation.focused_tabs().unwrap()),
        )
        .unwrap()
        .unwrap();

    assert_eq!(root.segments.len(), 2);
    assert_eq!(root.segments[0].text, "ROOT ");
    assert_eq!(root.segments[1].text, "workspace");
    assert_eq!(root.segments[0].foreground.unwrap().green, 255);
    assert_eq!(root.segments[0].background.unwrap().blue, 48);

    assert_eq!(nested.segments.len(), 2);
    assert_eq!(nested.segments[0].text, "NESTED ");
    assert_eq!(nested.segments[1].text, "logs-long-title");
    assert_eq!(nested.segments[1].foreground.unwrap().green, 255);
}
