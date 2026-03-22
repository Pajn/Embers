use std::convert::TryFrom;
use std::path::PathBuf;

use embers_core::{BufferId, FloatingId, NodeId, Rect, SplitDirection};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};

use crate::input::parse_key_sequence;
use crate::presentation::NavigationDirection;

use super::context::{
    BufferRef, Context, EventInfo, FloatingRef, NodeRef, SessionRef, TabBarContext, TabInfo,
};
use super::model::{
    Action, BufferSpawnSpec, FloatingAnchor, FloatingGeometrySpec, FloatingSize, FloatingSpec,
    NotifyLevel, TabSpec, TabsSpec, TreeSpec,
};
use super::types::{BarSegment, BarSpec, BarTarget, RgbColor, StyleSpec, ThemeSpec};

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

#[derive(Clone, Default)]
struct ActionApi;

#[derive(Clone, Default)]
struct TreeApi;

#[derive(Clone, Default)]
struct UiApi;

#[derive(Clone)]
struct MuxApi {
    context: Context,
}

#[derive(Clone, Default)]
struct SystemApi;

#[derive(Clone)]
struct ThemeRuntimeApi {
    theme: ThemeSpec,
}

impl MuxApi {
    fn new(context: Context) -> Self {
        Self { context }
    }
}

pub fn register_runtime_api(engine: &mut Engine) {
    engine.register_type_with_name::<Action>("Action");
    engine.register_type_with_name::<TreeSpec>("TreeSpec");
    engine.register_type_with_name::<TabSpec>("TabSpec");
    engine.register_type_with_name::<TabsSpec>("TabsSpec");
    engine.register_type_with_name::<Context>("Context");
    engine.register_type_with_name::<EventInfo>("EventInfo");
    engine.register_type_with_name::<SessionRef>("SessionRef");
    engine.register_type_with_name::<BufferRef>("BufferRef");
    engine.register_type_with_name::<NodeRef>("NodeRef");
    engine.register_type_with_name::<FloatingRef>("FloatingRef");
    engine.register_type_with_name::<TabBarContext>("TabBarContext");
    engine.register_type_with_name::<TabInfo>("TabInfo");
    engine.register_type_with_name::<BarSpec>("BarSpec");
    engine.register_type_with_name::<BarSegment>("BarSegment");
    engine.register_type_with_name::<StyleSpec>("StyleSpec");
    engine.register_type_with_name::<RgbColor>("RgbColor");
    engine.register_type_with_name::<ActionApi>("ActionApi");
    engine.register_type_with_name::<TreeApi>("TreeApi");
    engine.register_type_with_name::<UiApi>("UiApi");
    engine.register_type_with_name::<MuxApi>("MuxApi");
    engine.register_type_with_name::<SystemApi>("SystemApi");
    engine.register_type_with_name::<ThemeRuntimeApi>("ThemeRuntimeApi");

    register_context_api(engine);
    register_ref_api(engine);
    register_action_api(engine);
    register_tree_api(engine);
    register_mux_api(engine);
    register_system_api(engine);
    register_ui_api(engine);
    register_theme_runtime_api(engine);
}

pub fn runtime_scope(context: Option<Context>, theme: ThemeSpec) -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push_constant("system", SystemApi);
    scope.push_constant("action", ActionApi);
    scope.push_constant("tree", TreeApi);
    scope.push_constant("ui", UiApi);
    scope.push_constant("theme", ThemeRuntimeApi { theme });
    if let Some(context) = context {
        scope.push_constant("mux", MuxApi::new(context));
    }
    scope
}

pub fn registration_scope() -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push_constant("system", SystemApi);
    scope.push_constant("action", ActionApi);
    scope.push_constant("tree", TreeApi);
    scope.push_constant("ui", UiApi);
    scope
}

pub fn normalize_actions(result: Dynamic) -> Result<Vec<Action>, String> {
    if result.is_unit() {
        return Ok(Vec::new());
    }
    if let Some(action) = result.clone().try_cast::<Action>() {
        return Ok(vec![action]);
    }
    if let Some(actions) = result.try_cast::<Array>() {
        return parse_action_array(actions)
            .map_err(|error| error.to_string())
            .map(|actions| actions);
    }

    Err("script must return Action, [Action], or ()".to_owned())
}

pub fn normalize_bar(result: Dynamic) -> Result<BarSpec, String> {
    result
        .try_cast::<BarSpec>()
        .ok_or_else(|| "tab bar formatter must return a BarSpec".to_owned())
}

fn register_context_api(engine: &mut Engine) {
    engine.register_fn("current_mode", |context: &mut Context| {
        context.current_mode().to_owned()
    });
    engine.register_fn("event", |context: &mut Context| -> Dynamic {
        dynamic_option_custom(context.event())
    });
    engine.register_fn("current_session", |context: &mut Context| -> Dynamic {
        dynamic_option_custom(context.current_session())
    });
    engine.register_fn("current_node", |context: &mut Context| -> Dynamic {
        dynamic_option_custom(context.current_node())
    });
    engine.register_fn("current_buffer", |context: &mut Context| -> Dynamic {
        dynamic_option_custom(context.current_buffer())
    });
    engine.register_fn("current_floating", |context: &mut Context| -> Dynamic {
        dynamic_option_custom(context.current_floating())
    });
    engine.register_fn("sessions", |context: &mut Context| -> Array {
        context.sessions().into_iter().map(Dynamic::from).collect()
    });
    engine.register_fn(
        "find_buffer",
        |context: &mut Context, buffer_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                context.find_buffer(parse_buffer_id(buffer_id)?),
            ))
        },
    );
    engine.register_fn(
        "find_node",
        |context: &mut Context, node_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                context.find_node(parse_node_id(node_id)?),
            ))
        },
    );
    engine.register_fn(
        "find_floating",
        |context: &mut Context, floating_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                context.find_floating(parse_floating_id(floating_id)?),
            ))
        },
    );
    engine.register_fn("detached_buffers", |context: &mut Context| -> Array {
        context
            .detached_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
    engine.register_fn("visible_buffers", |context: &mut Context| -> Array {
        context
            .visible_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
}

fn register_ref_api(engine: &mut Engine) {
    engine.register_fn("name", |event: &mut EventInfo| event.name.clone());
    engine.register_fn("session_id", |event: &mut EventInfo| -> Dynamic {
        event
            .session_id
            .map(|session_id| dynamic_u64(session_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("buffer_id", |event: &mut EventInfo| -> Dynamic {
        event
            .buffer_id
            .map(|buffer_id| dynamic_u64(buffer_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("node_id", |event: &mut EventInfo| -> Dynamic {
        event
            .node_id
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("floating_id", |event: &mut EventInfo| -> Dynamic {
        event
            .floating_id
            .map(|floating_id| dynamic_u64(floating_id.0))
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn("id", |session: &mut SessionRef| dynamic_u64(session.id.0));
    engine.register_fn("name", |session: &mut SessionRef| session.name.clone());
    engine.register_fn("root_node", |session: &mut SessionRef| {
        dynamic_u64(session.root_node_id.0)
    });
    engine.register_fn("floating", |session: &mut SessionRef| -> Array {
        session
            .floating_ids
            .iter()
            .map(|floating_id| dynamic_u64(floating_id.0))
            .collect()
    });

    engine.register_fn("id", |buffer: &mut BufferRef| dynamic_u64(buffer.id.0));
    engine.register_fn("title", |buffer: &mut BufferRef| buffer.title.clone());
    engine.register_fn("command", |buffer: &mut BufferRef| -> Array {
        buffer.command.iter().cloned().map(Dynamic::from).collect()
    });
    engine.register_fn("cwd", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.cwd.clone())
    });
    engine.register_fn("pid", |buffer: &mut BufferRef| -> Dynamic {
        buffer.pid.map(dynamic_u32).unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("process_name", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.process_name())
    });
    engine.register_fn("tty_path", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.tty_path.clone())
    });
    engine.register_fn(
        "env_hint",
        |buffer: &mut BufferRef, key: ImmutableString| -> Dynamic {
            dynamic_option_string(buffer.env_hint(key.as_str()))
        },
    );
    engine.register_fn(
        "snapshot_text",
        |buffer: &mut BufferRef, limit: i64| -> RhaiResult<String> {
            Ok(buffer.snapshot_text(parse_count(limit, "snapshot_text limit")?))
        },
    );
    engine.register_fn("history_text", |buffer: &mut BufferRef| {
        buffer.history_text()
    });
    engine.register_fn("is_attached", |buffer: &mut BufferRef| buffer.is_attached());
    engine.register_fn("is_detached", |buffer: &mut BufferRef| buffer.is_detached());
    engine.register_fn("is_running", |buffer: &mut BufferRef| buffer.is_running());
    engine.register_fn("exit_code", |buffer: &mut BufferRef| -> Dynamic {
        buffer.exit_code.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("is_visible", |buffer: &mut BufferRef| buffer.visible);
    engine.register_fn("session_id", |buffer: &mut BufferRef| -> Dynamic {
        buffer
            .session_id
            .map(|session_id| dynamic_u64(session_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("node_id", |buffer: &mut BufferRef| -> Dynamic {
        buffer
            .node_id()
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("activity", |buffer: &mut BufferRef| {
        activity_name(buffer.activity)
    });

    engine.register_fn("id", |node: &mut NodeRef| dynamic_u64(node.id.0));
    engine.register_fn("kind", |node: &mut NodeRef| node_kind_name(node.kind));
    engine.register_fn("parent", |node: &mut NodeRef| -> Dynamic {
        node.parent_id
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("children", |node: &mut NodeRef| -> Array {
        node.child_ids
            .iter()
            .map(|child_id| dynamic_u64(child_id.0))
            .collect()
    });
    engine.register_fn("session_id", |node: &mut NodeRef| {
        dynamic_u64(node.session_id.0)
    });
    engine.register_fn("geometry", |node: &mut NodeRef| -> Dynamic {
        node.geometry
            .map(rect_map)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("is_root", |node: &mut NodeRef| node.is_root);
    engine.register_fn("is_floating_root", |node: &mut NodeRef| {
        node.is_floating_root
    });
    engine.register_fn("is_visible", |node: &mut NodeRef| node.visible);
    engine.register_fn("is_focused", |node: &mut NodeRef| node.is_focused);
    engine.register_fn("buffer", |node: &mut NodeRef| -> Dynamic {
        node.buffer_id
            .map(|buffer_id| dynamic_u64(buffer_id.0))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("split_direction", |node: &mut NodeRef| -> Dynamic {
        node.split_direction
            .map(split_direction_name)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("split_weights", |node: &mut NodeRef| -> Dynamic {
        node.split_weights
            .as_ref()
            .map(|weights| {
                weights
                    .iter()
                    .copied()
                    .map(|weight| Dynamic::from(i64::from(weight)))
                    .collect::<Array>()
            })
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("active_tab_index", |node: &mut NodeRef| -> Dynamic {
        node.active_tab_index
            .map(dynamic_u32)
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("tab_titles", |node: &mut NodeRef| -> Array {
        node.tab_titles.iter().cloned().map(Dynamic::from).collect()
    });

    engine.register_fn("id", |floating: &mut FloatingRef| {
        dynamic_u64(floating.id.0)
    });
    engine.register_fn("session_id", |floating: &mut FloatingRef| {
        dynamic_u64(floating.session_id.0)
    });
    engine.register_fn("root_node", |floating: &mut FloatingRef| {
        dynamic_u64(floating.root_node_id.0)
    });
    engine.register_fn("title", |floating: &mut FloatingRef| -> Dynamic {
        dynamic_option_string(floating.title.clone())
    });
    engine.register_fn("is_visible", |floating: &mut FloatingRef| floating.visible);
    engine.register_fn("is_focused", |floating: &mut FloatingRef| floating.focused);
    engine.register_fn("geometry", |floating: &mut FloatingRef| -> Map {
        float_geometry_map(floating.geometry)
    });

    engine.register_fn("node_id", |bar: &mut TabBarContext| {
        dynamic_u64(bar.node_id.0)
    });
    engine.register_fn("is_root", |bar: &mut TabBarContext| bar.is_root);
    engine.register_fn("active_index", |bar: &mut TabBarContext| {
        dynamic_usize(bar.active)
    });
    engine.register_fn("mode", |bar: &mut TabBarContext| bar.mode.clone());
    engine.register_fn("viewport_width", |bar: &mut TabBarContext| {
        Dynamic::from(i64::from(bar.viewport_width))
    });
    engine.register_fn("tabs", |bar: &mut TabBarContext| -> Array {
        bar.tabs.iter().cloned().map(Dynamic::from).collect()
    });

    engine.register_fn("index", |tab: &mut TabInfo| dynamic_usize(tab.index));
    engine.register_fn("title", |tab: &mut TabInfo| tab.title.clone());
    engine.register_fn("is_active", |tab: &mut TabInfo| tab.active);
    engine.register_fn("has_activity", |tab: &mut TabInfo| tab.has_activity);
    engine.register_fn("has_bell", |tab: &mut TabInfo| tab.has_bell);
    engine.register_fn("buffer_count", |tab: &mut TabInfo| {
        dynamic_usize(tab.buffer_count)
    });
}

fn register_action_api(engine: &mut Engine) {
    engine.register_fn("noop", |_: &mut ActionApi| Action::Noop);
    engine.register_fn(
        "chain",
        |_: &mut ActionApi, actions: Array| -> RhaiResult<Action> {
            Ok(Action::Chain(parse_action_array(actions)?))
        },
    );
    engine.register_fn("enter_mode", |_: &mut ActionApi, mode: ImmutableString| {
        Action::EnterMode {
            mode: mode.to_string(),
        }
    });
    engine.register_fn("leave_mode", |_: &mut ActionApi| Action::LeaveMode);
    engine.register_fn("toggle_mode", |_: &mut ActionApi, mode: ImmutableString| {
        Action::ToggleMode {
            mode: mode.to_string(),
        }
    });
    engine.register_fn("clear_pending_keys", |_: &mut ActionApi| {
        Action::ClearPendingKeys
    });

    engine.register_fn("focus_left", |_: &mut ActionApi| Action::FocusDirection {
        direction: NavigationDirection::Left,
    });
    engine.register_fn("focus_right", |_: &mut ActionApi| Action::FocusDirection {
        direction: NavigationDirection::Right,
    });
    engine.register_fn("focus_up", |_: &mut ActionApi| Action::FocusDirection {
        direction: NavigationDirection::Up,
    });
    engine.register_fn("focus_down", |_: &mut ActionApi| Action::FocusDirection {
        direction: NavigationDirection::Down,
    });

    engine.register_fn(
        "resize_left",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::ResizeDirection {
                direction: NavigationDirection::Left,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_right",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::ResizeDirection {
                direction: NavigationDirection::Right,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_up",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::ResizeDirection {
                direction: NavigationDirection::Up,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_down",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::ResizeDirection {
                direction: NavigationDirection::Down,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );

    engine.register_fn(
        "select_tab",
        |_: &mut ActionApi, tabs_node_id: i64, index: i64| -> RhaiResult<Action> {
            Ok(Action::SelectTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                index: parse_index(index, "tab index")?,
            })
        },
    );
    engine.register_fn(
        "select_current_tabs",
        |_: &mut ActionApi, index: i64| -> RhaiResult<Action> {
            Ok(Action::SelectTab {
                tabs_node_id: None,
                index: parse_index(index, "tab index")?,
            })
        },
    );
    engine.register_fn(
        "next_tab",
        |_: &mut ActionApi, tabs_node_id: i64| -> RhaiResult<Action> {
            Ok(Action::NextTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
            })
        },
    );
    engine.register_fn("next_current_tabs", |_: &mut ActionApi| Action::NextTab {
        tabs_node_id: None,
    });
    engine.register_fn(
        "prev_tab",
        |_: &mut ActionApi, tabs_node_id: i64| -> RhaiResult<Action> {
            Ok(Action::PrevTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
            })
        },
    );
    engine.register_fn("prev_current_tabs", |_: &mut ActionApi| Action::PrevTab {
        tabs_node_id: None,
    });

    engine.register_fn(
        "focus_buffer",
        |_: &mut ActionApi, buffer_id: i64| -> RhaiResult<Action> {
            Ok(Action::FocusBuffer {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        },
    );
    engine.register_fn(
        "reveal_buffer",
        |_: &mut ActionApi, buffer_id: i64| -> RhaiResult<Action> {
            Ok(Action::RevealBuffer {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        },
    );

    engine.register_fn(
        "split_with",
        |_: &mut ActionApi, direction: ImmutableString, tree: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::SplitCurrent {
                direction: parse_split_direction(direction.as_str())?,
                new_child: tree,
            })
        },
    );

    engine.register_fn(
        "replace_current_with",
        |_: &mut ActionApi, tree: TreeSpec| Action::ReplaceNode {
            node_id: None,
            tree,
        },
    );
    engine.register_fn(
        "replace_node",
        |_: &mut ActionApi, node_id: i64, tree: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::ReplaceNode {
                node_id: Some(parse_node_id(node_id)?),
                tree,
            })
        },
    );

    engine.register_fn(
        "wrap_current_in_split",
        |_: &mut ActionApi, direction: ImmutableString, tree: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::WrapNodeInSplit {
                node_id: None,
                direction: parse_split_direction(direction.as_str())?,
                sibling: tree,
            })
        },
    );
    engine.register_fn(
        "wrap_node_in_split",
        |_: &mut ActionApi,
         node_id: i64,
         direction: ImmutableString,
         tree: TreeSpec|
         -> RhaiResult<Action> {
            Ok(Action::WrapNodeInSplit {
                node_id: Some(parse_node_id(node_id)?),
                direction: parse_split_direction(direction.as_str())?,
                sibling: tree,
            })
        },
    );

    engine.register_fn(
        "wrap_current_in_tabs",
        |_: &mut ActionApi, tabs: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::WrapNodeInTabs {
                node_id: None,
                tabs: parse_tabs_tree(tabs)?,
            })
        },
    );
    engine.register_fn(
        "wrap_node_in_tabs",
        |_: &mut ActionApi, node_id: i64, tabs: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::WrapNodeInTabs {
                node_id: Some(parse_node_id(node_id)?),
                tabs: parse_tabs_tree(tabs)?,
            })
        },
    );

    engine.register_fn(
        "insert_tab_after",
        |_: &mut ActionApi,
         tabs_node_id: i64,
         title: ImmutableString,
         tree: TreeSpec|
         -> RhaiResult<Action> {
            Ok(Action::InsertTabAfter {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                title: Some(title.to_string()),
                child: tree,
            })
        },
    );
    engine.register_fn(
        "insert_tab_after_current",
        |_: &mut ActionApi, title: ImmutableString, tree: TreeSpec| Action::InsertTabAfter {
            tabs_node_id: None,
            title: Some(title.to_string()),
            child: tree,
        },
    );
    engine.register_fn(
        "insert_tab_before",
        |_: &mut ActionApi,
         tabs_node_id: i64,
         title: ImmutableString,
         tree: TreeSpec|
         -> RhaiResult<Action> {
            Ok(Action::InsertTabBefore {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                title: Some(title.to_string()),
                child: tree,
            })
        },
    );
    engine.register_fn(
        "insert_tab_before_current",
        |_: &mut ActionApi, title: ImmutableString, tree: TreeSpec| Action::InsertTabBefore {
            tabs_node_id: None,
            title: Some(title.to_string()),
            child: tree,
        },
    );

    engine.register_fn(
        "open_floating",
        |_: &mut ActionApi, tree: TreeSpec, options: Map| -> RhaiResult<Action> {
            Ok(Action::OpenFloating {
                spec: parse_floating_spec(tree, options)?,
            })
        },
    );
    engine.register_fn(
        "replace_floating_root",
        |_: &mut ActionApi, floating_id: i64, tree: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::ReplaceFloatingRoot {
                floating_id: Some(parse_floating_id(floating_id)?),
                tree,
            })
        },
    );
    engine.register_fn(
        "replace_current_floating_root",
        |_: &mut ActionApi, tree: TreeSpec| Action::ReplaceFloatingRoot {
            floating_id: None,
            tree,
        },
    );
    engine.register_fn("close_floating", |_: &mut ActionApi| {
        Action::CloseFloating { floating_id: None }
    });
    engine.register_fn(
        "close_floating_id",
        |_: &mut ActionApi, floating_id: i64| -> RhaiResult<Action> {
            Ok(Action::CloseFloating {
                floating_id: Some(parse_floating_id(floating_id)?),
            })
        },
    );
    engine.register_fn("close_view", |_: &mut ActionApi| Action::CloseView {
        node_id: None,
    });
    engine.register_fn(
        "close_node",
        |_: &mut ActionApi, node_id: i64| -> RhaiResult<Action> {
            Ok(Action::CloseView {
                node_id: Some(parse_node_id(node_id)?),
            })
        },
    );

    engine.register_fn("kill_buffer", |_: &mut ActionApi| Action::KillBuffer {
        buffer_id: None,
    });
    engine.register_fn(
        "kill_buffer_id",
        |_: &mut ActionApi, buffer_id: i64| -> RhaiResult<Action> {
            Ok(Action::KillBuffer {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
            })
        },
    );
    engine.register_fn("detach_buffer", |_: &mut ActionApi| Action::DetachBuffer {
        buffer_id: None,
    });
    engine.register_fn(
        "detach_buffer_id",
        |_: &mut ActionApi, buffer_id: i64| -> RhaiResult<Action> {
            Ok(Action::DetachBuffer {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
            })
        },
    );

    engine.register_fn(
        "move_buffer_to_node",
        |_: &mut ActionApi, buffer_id: i64, node_id: i64| -> RhaiResult<Action> {
            Ok(Action::MoveBufferToNode {
                buffer_id: parse_buffer_id(buffer_id)?,
                node_id: parse_node_id(node_id)?,
            })
        },
    );
    engine.register_fn(
        "move_buffer_to_floating",
        |_: &mut ActionApi, buffer_id: i64, options: Map| -> RhaiResult<Action> {
            let spec = parse_floating_options(options)?;
            Ok(Action::MoveBufferToFloating {
                buffer_id: parse_buffer_id(buffer_id)?,
                geometry: spec.geometry,
                title: spec.title,
                focus: spec.focus,
            })
        },
    );

    engine.register_fn(
        "send_keys_current",
        |_: &mut ActionApi, notation: ImmutableString| -> RhaiResult<Action> {
            Ok(Action::SendKeys {
                buffer_id: None,
                keys: parse_key_sequence(notation.as_str())
                    .map_err(|error| runtime_error(error.to_string()))?,
            })
        },
    );
    engine.register_fn(
        "send_keys",
        |_: &mut ActionApi, buffer_id: i64, notation: ImmutableString| -> RhaiResult<Action> {
            Ok(Action::SendKeys {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                keys: parse_key_sequence(notation.as_str())
                    .map_err(|error| runtime_error(error.to_string()))?,
            })
        },
    );
    engine.register_fn(
        "send_bytes",
        |_: &mut ActionApi, buffer_id: i64, bytes: ImmutableString| -> RhaiResult<Action> {
            Ok(Action::SendBytes {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                bytes: bytes.as_bytes().to_vec(),
            })
        },
    );
    engine.register_fn(
        "send_bytes",
        |_: &mut ActionApi, buffer_id: i64, bytes: Array| -> RhaiResult<Action> {
            Ok(Action::SendBytes {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                bytes: parse_bytes(bytes)?,
            })
        },
    );
    engine.register_fn(
        "send_bytes_current",
        |_: &mut ActionApi, bytes: ImmutableString| Action::SendBytes {
            buffer_id: None,
            bytes: bytes.as_bytes().to_vec(),
        },
    );
    engine.register_fn(
        "send_bytes_current",
        |_: &mut ActionApi, bytes: Array| -> RhaiResult<Action> {
            Ok(Action::SendBytes {
                buffer_id: None,
                bytes: parse_bytes(bytes)?,
            })
        },
    );

    engine.register_fn("scroll_line_up", |_: &mut ActionApi| Action::ScrollLineUp);
    engine.register_fn("scroll_line_down", |_: &mut ActionApi| {
        Action::ScrollLineDown
    });
    engine.register_fn("scroll_page_up", |_: &mut ActionApi| Action::ScrollPageUp);
    engine.register_fn("scroll_page_down", |_: &mut ActionApi| {
        Action::ScrollPageDown
    });
    engine.register_fn("scroll_to_top", |_: &mut ActionApi| Action::ScrollToTop);
    engine.register_fn("scroll_to_bottom", |_: &mut ActionApi| {
        Action::ScrollToBottom
    });
    engine.register_fn("follow_output", |_: &mut ActionApi| Action::FollowOutput);
    engine.register_fn("enter_search_mode", |_: &mut ActionApi| {
        Action::EnterSearchMode
    });
    engine.register_fn("search_next", |_: &mut ActionApi| Action::SearchNext);
    engine.register_fn("search_prev", |_: &mut ActionApi| Action::SearchPrev);
    engine.register_fn("cancel_search", |_: &mut ActionApi| Action::CancelSearch);
    engine.register_fn("enter_select_char", |_: &mut ActionApi| {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Character,
        }
    });
    engine.register_fn("enter_select_line", |_: &mut ActionApi| {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Line,
        }
    });
    engine.register_fn("enter_select_block", |_: &mut ActionApi| {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Block,
        }
    });
    engine.register_fn("select_move_left", |_: &mut ActionApi| Action::SelectMove {
        direction: NavigationDirection::Left,
    });
    engine.register_fn("select_move_right", |_: &mut ActionApi| {
        Action::SelectMove {
            direction: NavigationDirection::Right,
        }
    });
    engine.register_fn("select_move_up", |_: &mut ActionApi| Action::SelectMove {
        direction: NavigationDirection::Up,
    });
    engine.register_fn("select_move_down", |_: &mut ActionApi| Action::SelectMove {
        direction: NavigationDirection::Down,
    });

    engine.register_fn("yank_selection", |_: &mut ActionApi| Action::CopySelection);
    engine.register_fn("copy_selection", |_: &mut ActionApi| Action::CopySelection);
    engine.register_fn("cancel_selection", |_: &mut ActionApi| {
        Action::CancelSelection
    });
    engine.register_fn(
        "notify",
        |_: &mut ActionApi,
         level: ImmutableString,
         message: ImmutableString|
         -> RhaiResult<Action> {
            Ok(Action::Notify {
                level: parse_notify_level(level.as_str())?,
                message: message.to_string(),
            })
        },
    );
    engine.register_fn(
        "run_named_action",
        |_: &mut ActionApi, name: ImmutableString| Action::RunNamedAction {
            name: name.to_string(),
        },
    );
}

fn register_tree_api(engine: &mut Engine) {
    engine.register_fn("buffer_current", |_: &mut TreeApi| TreeSpec::BufferCurrent);
    engine.register_fn("current_buffer", |_: &mut TreeApi| TreeSpec::BufferCurrent);
    engine.register_fn("current_node", |_: &mut TreeApi| TreeSpec::CurrentNode);
    engine.register_fn("buffer_empty", |_: &mut TreeApi| TreeSpec::BufferEmpty);
    engine.register_fn(
        "buffer_attach",
        |_: &mut TreeApi, buffer_id: i64| -> RhaiResult<TreeSpec> {
            Ok(TreeSpec::BufferAttach {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        },
    );
    engine.register_fn(
        "buffer_spawn",
        |_: &mut TreeApi, command: Array| -> RhaiResult<TreeSpec> {
            Ok(TreeSpec::BufferSpawn(BufferSpawnSpec {
                title: None,
                command: parse_string_array(Dynamic::from(command))?,
                cwd: None,
                env: Default::default(),
            }))
        },
    );
    engine.register_fn(
        "buffer_spawn",
        |_: &mut TreeApi, command: Array, options: Map| -> RhaiResult<TreeSpec> {
            Ok(TreeSpec::BufferSpawn(parse_buffer_spawn(command, options)?))
        },
    );
    engine.register_fn(
        "tab",
        |_: &mut TreeApi, title: ImmutableString, tree: TreeSpec| TabSpec {
            title: title.to_string(),
            tree: Box::new(tree),
        },
    );
    engine.register_fn(
        "tabs",
        |_: &mut TreeApi, tabs: Array| -> RhaiResult<TreeSpec> { build_tabs(tabs, 0) },
    );
    engine.register_fn(
        "tabs_with_active",
        |_: &mut TreeApi, tabs: Array, active: i64| -> RhaiResult<TreeSpec> {
            build_tabs(tabs, parse_index(active, "active tab")?)
        },
    );
    engine.register_fn(
        "split_h",
        |_: &mut TreeApi, children: Array| -> RhaiResult<TreeSpec> {
            build_split(SplitDirection::Horizontal, children, Vec::new())
        },
    );
    engine.register_fn(
        "split_v",
        |_: &mut TreeApi, children: Array| -> RhaiResult<TreeSpec> {
            build_split(SplitDirection::Vertical, children, Vec::new())
        },
    );
    engine.register_fn(
        "split",
        |_: &mut TreeApi, direction: ImmutableString, children: Array| -> RhaiResult<TreeSpec> {
            build_split(
                parse_split_direction(direction.as_str())?,
                children,
                Vec::new(),
            )
        },
    );
    engine.register_fn(
        "split",
        |_: &mut TreeApi,
         direction: ImmutableString,
         children: Array,
         sizes: Array|
         -> RhaiResult<TreeSpec> {
            build_split(
                parse_split_direction(direction.as_str())?,
                children,
                parse_sizes(sizes)?,
            )
        },
    );
}

fn register_mux_api(engine: &mut Engine) {
    engine.register_fn("current_session", |mux: &mut MuxApi| -> Dynamic {
        dynamic_option_custom(mux.context.current_session())
    });
    engine.register_fn("current_node", |mux: &mut MuxApi| -> Dynamic {
        dynamic_option_custom(mux.context.current_node())
    });
    engine.register_fn("current_buffer", |mux: &mut MuxApi| -> Dynamic {
        dynamic_option_custom(mux.context.current_buffer())
    });
    engine.register_fn("current_floating", |mux: &mut MuxApi| -> Dynamic {
        dynamic_option_custom(mux.context.current_floating())
    });
    engine.register_fn("sessions", |mux: &mut MuxApi| -> Array {
        mux.context
            .sessions()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
    engine.register_fn("visible_buffers", |mux: &mut MuxApi| -> Array {
        mux.context
            .visible_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
    engine.register_fn("detached_buffers", |mux: &mut MuxApi| -> Array {
        mux.context
            .detached_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
    engine.register_fn(
        "find_buffer",
        |mux: &mut MuxApi, buffer_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                mux.context.find_buffer(parse_buffer_id(buffer_id)?),
            ))
        },
    );
    engine.register_fn(
        "find_node",
        |mux: &mut MuxApi, node_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                mux.context.find_node(parse_node_id(node_id)?),
            ))
        },
    );
    engine.register_fn(
        "find_floating",
        |mux: &mut MuxApi, floating_id: i64| -> RhaiResult<Dynamic> {
            Ok(dynamic_option_custom(
                mux.context.find_floating(parse_floating_id(floating_id)?),
            ))
        },
    );
}

fn register_system_api(engine: &mut Engine) {
    engine.register_fn(
        "env",
        |_: &mut SystemApi, name: ImmutableString| -> Dynamic {
            std::env::var(name.as_str())
                .ok()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        },
    );
    engine.register_fn(
        "which",
        |_: &mut SystemApi, name: ImmutableString| -> Dynamic {
            which(name.as_str())
                .map(|path| Dynamic::from(path.display().to_string()))
                .unwrap_or(Dynamic::UNIT)
        },
    );
    engine.register_fn("now", |_: &mut SystemApi| -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    });
}

fn register_ui_api(engine: &mut Engine) {
    engine.register_fn("segment", |_: &mut UiApi, text: ImmutableString| {
        BarSegment {
            text: text.to_string(),
            style: StyleSpec::default(),
            target: None,
        }
    });
    engine.register_fn(
        "segment",
        |_: &mut UiApi, text: ImmutableString, options: Map| -> RhaiResult<BarSegment> {
            let (style, target) = parse_segment_options(options)?;
            Ok(BarSegment {
                text: text.to_string(),
                style,
                target,
            })
        },
    );
    engine.register_fn(
        "bar",
        |_: &mut UiApi, left: Array, center: Array, right: Array| -> RhaiResult<BarSpec> {
            Ok(BarSpec {
                left: parse_bar_segments(left)?,
                center: parse_bar_segments(center)?,
                right: parse_bar_segments(right)?,
            })
        },
    );
}

fn register_theme_runtime_api(engine: &mut Engine) {
    engine.register_fn(
        "color",
        |theme: &mut ThemeRuntimeApi, name: ImmutableString| -> Dynamic {
            theme
                .theme
                .palette
                .get(name.as_str())
                .copied()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        },
    );
}

fn build_split(
    direction: SplitDirection,
    children: Array,
    sizes: Vec<u16>,
) -> RhaiResult<TreeSpec> {
    let children = parse_tree_array(children)?;
    if children.is_empty() {
        return Err(runtime_error("split children cannot be empty"));
    }
    if !sizes.is_empty() {
        if sizes.len() != children.len() {
            return Err(runtime_error(
                "split sizes must match the number of children",
            ));
        }
        if sizes.contains(&0) {
            return Err(runtime_error("split sizes must be greater than zero"));
        }
    }
    Ok(TreeSpec::Split {
        direction,
        children,
        sizes,
    })
}

fn build_tabs(tabs: Array, active: usize) -> RhaiResult<TreeSpec> {
    let tabs = parse_tabs(tabs)?;
    if tabs.is_empty() {
        return Err(runtime_error("tabs cannot be empty"));
    }
    if active >= tabs.len() {
        return Err(runtime_error("active tab index is out of bounds"));
    }
    Ok(TreeSpec::Tabs(TabsSpec { tabs, active }))
}

fn parse_tabs_tree(tree: TreeSpec) -> RhaiResult<TabsSpec> {
    match tree {
        TreeSpec::Tabs(tabs) => Ok(tabs),
        _ => Err(runtime_error("expected a tree.tabs(...) spec")),
    }
}

fn parse_tabs(tabs: Array) -> RhaiResult<Vec<TabSpec>> {
    let mut parsed = Vec::with_capacity(tabs.len());
    for tab in tabs {
        let Some(tab) = tab.try_cast::<TabSpec>() else {
            return Err(runtime_error("expected TabSpec values"));
        };
        parsed.push(tab);
    }
    Ok(parsed)
}

fn parse_tree_array(children: Array) -> RhaiResult<Vec<TreeSpec>> {
    let mut parsed = Vec::with_capacity(children.len());
    for child in children {
        let Some(tree) = child.try_cast::<TreeSpec>() else {
            return Err(runtime_error("expected TreeSpec values"));
        };
        parsed.push(tree);
    }
    Ok(parsed)
}

fn parse_action_array(actions: Array) -> RhaiResult<Vec<Action>> {
    let mut parsed = Vec::with_capacity(actions.len());
    for action in actions {
        let Some(action) = action.try_cast::<Action>() else {
            return Err(runtime_error("expected Action values"));
        };
        parsed.push(action);
    }
    Ok(parsed)
}

fn parse_bar_segments(segments: Array) -> RhaiResult<Vec<BarSegment>> {
    let mut parsed = Vec::with_capacity(segments.len());
    for segment in segments {
        let Some(segment) = segment.try_cast::<BarSegment>() else {
            return Err(runtime_error("ui.bar expects BarSegment values"));
        };
        parsed.push(segment);
    }
    Ok(parsed)
}

fn parse_buffer_spawn(command: Array, mut options: Map) -> RhaiResult<BufferSpawnSpec> {
    Ok(BufferSpawnSpec {
        title: parse_optional_string(options.remove("title"))?,
        command: parse_string_array(Dynamic::from(command))?,
        cwd: parse_optional_string(options.remove("cwd"))?,
        env: parse_string_map(options.remove("env"))?,
    })
}

fn parse_floating_spec(tree: TreeSpec, options: Map) -> RhaiResult<FloatingSpec> {
    let options = parse_floating_options(options)?;
    Ok(FloatingSpec {
        tree,
        geometry: options.geometry,
        title: options.title,
        focus: options.focus,
        close_on_empty: options.close_on_empty,
    })
}

struct ParsedFloatingOptions {
    geometry: FloatingGeometrySpec,
    title: Option<String>,
    focus: bool,
    close_on_empty: bool,
}

fn parse_floating_options(mut options: Map) -> RhaiResult<ParsedFloatingOptions> {
    Ok(ParsedFloatingOptions {
        geometry: FloatingGeometrySpec {
            width: parse_floating_size(options.remove("width"))?
                .unwrap_or(FloatingSize::Percent(50)),
            height: parse_floating_size(options.remove("height"))?
                .unwrap_or(FloatingSize::Percent(50)),
            anchor: parse_floating_anchor(options.remove("anchor"))?
                .unwrap_or(FloatingAnchor::Center),
            offset_x: parse_i16_field(options.remove("x"), "x")?.unwrap_or(0),
            offset_y: parse_i16_field(options.remove("y"), "y")?.unwrap_or(0),
        },
        title: parse_optional_string(options.remove("title"))?,
        focus: parse_bool_field(options.remove("focus"))?.unwrap_or(true),
        close_on_empty: parse_bool_field(options.remove("close_on_empty"))?.unwrap_or(true),
    })
}

fn parse_segment_options(mut options: Map) -> RhaiResult<(StyleSpec, Option<BarTarget>)> {
    Ok((
        StyleSpec {
            fg: parse_optional_color(options.remove("fg"))?,
            bg: parse_optional_color(options.remove("bg"))?,
            bold: parse_bool_field(options.remove("bold"))?.unwrap_or(false),
            italic: parse_bool_field(options.remove("italic"))?.unwrap_or(false),
            underline: parse_bool_field(options.remove("underline"))?.unwrap_or(false),
            dim: parse_bool_field(options.remove("dim"))?.unwrap_or(false),
        },
        parse_bar_target(options.remove("target"))?,
    ))
}

fn parse_bar_target(value: Option<Dynamic>) -> RhaiResult<Option<BarTarget>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    let Some(mut target) = value.try_cast::<Map>() else {
        return Err(runtime_error("bar target must be a map"));
    };
    let kind = parse_required_string(&mut target, "kind")?;
    match kind.as_str() {
        "tab" => Ok(Some(BarTarget::Tab {
            tabs_node_id: parse_node_id(parse_required_i64(&mut target, "tabs_node_id")?)?,
            index: parse_index(parse_required_i64(&mut target, "index")?, "target index")?,
        })),
        "floating" => Ok(Some(BarTarget::Floating {
            floating_id: parse_floating_id(parse_required_i64(&mut target, "floating_id")?)?,
        })),
        "buffer" => Ok(Some(BarTarget::Buffer {
            buffer_id: parse_buffer_id(parse_required_i64(&mut target, "buffer_id")?)?,
        })),
        _ => Err(runtime_error(format!("unknown bar target kind '{kind}'"))),
    }
}

fn parse_optional_color(value: Option<Dynamic>) -> RhaiResult<Option<RgbColor>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    value
        .try_cast::<RgbColor>()
        .map(Some)
        .ok_or_else(|| runtime_error("expected a color value"))
}

fn parse_sizes(values: Array) -> RhaiResult<Vec<u16>> {
    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value.try_cast::<i64>() else {
            return Err(runtime_error("split sizes must be integers"));
        };
        parsed.push(parse_amount(value, "split size")?);
    }
    Ok(parsed)
}

fn parse_string_array(value: Dynamic) -> RhaiResult<Vec<String>> {
    let Some(array) = value.try_cast::<Array>() else {
        return Err(runtime_error("expected an array of strings"));
    };
    let mut parsed = Vec::with_capacity(array.len());
    for value in array {
        let Some(value) = value.try_cast::<ImmutableString>() else {
            return Err(runtime_error("expected an array of strings"));
        };
        parsed.push(value.to_string());
    }
    Ok(parsed)
}

fn parse_string_map(
    value: Option<Dynamic>,
) -> RhaiResult<std::collections::BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(Default::default());
    };
    if value.is_unit() {
        return Ok(Default::default());
    }
    let Some(map) = value.try_cast::<Map>() else {
        return Err(runtime_error("expected a string map"));
    };
    let mut parsed = std::collections::BTreeMap::new();
    for (key, value) in map {
        let Some(value) = value.try_cast::<ImmutableString>() else {
            return Err(runtime_error("expected a string map"));
        };
        parsed.insert(key.to_string(), value.to_string());
    }
    Ok(parsed)
}

fn parse_optional_string(value: Option<Dynamic>) -> RhaiResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    let Some(value) = value.try_cast::<ImmutableString>() else {
        return Err(runtime_error("expected a string value"));
    };
    Ok(Some(value.to_string()))
}

fn parse_required_string(options: &mut Map, key: &str) -> RhaiResult<String> {
    parse_optional_string(options.remove(key))?
        .ok_or_else(|| runtime_error(format!("missing '{key}' field")))
}

fn parse_required_i64(options: &mut Map, key: &str) -> RhaiResult<i64> {
    let value = options
        .remove(key)
        .ok_or_else(|| runtime_error(format!("missing '{key}' field")))?;
    value
        .try_cast::<i64>()
        .ok_or_else(|| runtime_error(format!("'{key}' must be an integer")))
}

fn parse_bool_field(value: Option<Dynamic>) -> RhaiResult<Option<bool>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    value
        .try_cast::<bool>()
        .map(Some)
        .ok_or_else(|| runtime_error("expected a boolean value"))
}

fn parse_i16_field(value: Option<Dynamic>, label: &str) -> RhaiResult<Option<i16>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    let Some(value) = value.try_cast::<i64>() else {
        return Err(runtime_error(format!("'{label}' must be an integer")));
    };
    i16::try_from(value)
        .map(Some)
        .map_err(|_| runtime_error(format!("'{label}' is out of range")))
}

fn parse_floating_size(value: Option<Dynamic>) -> RhaiResult<Option<FloatingSize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    if let Some(value) = value.clone().try_cast::<i64>() {
        return Ok(Some(FloatingSize::Cells(parse_amount(
            value,
            "floating size",
        )?)));
    }
    if let Some(value) = value.try_cast::<ImmutableString>() {
        let value = value.to_string();
        if let Some(percent) = value.strip_suffix('%') {
            let percent = percent
                .parse::<u8>()
                .map_err(|_| runtime_error("floating percentages must be between 0 and 100"))?;
            if percent == 0 || percent > 100 {
                return Err(runtime_error(
                    "floating percentages must be between 1 and 100",
                ));
            }
            return Ok(Some(FloatingSize::Percent(percent)));
        }
    }
    Err(runtime_error(
        "floating width/height must be an integer cell count or percentage string like '50%'",
    ))
}

fn parse_floating_anchor(value: Option<Dynamic>) -> RhaiResult<Option<FloatingAnchor>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    let Some(value) = value.try_cast::<ImmutableString>() else {
        return Err(runtime_error("floating anchor must be a string"));
    };
    let anchor = match value.as_str() {
        "center" => FloatingAnchor::Center,
        "top_left" => FloatingAnchor::TopLeft,
        "top_right" => FloatingAnchor::TopRight,
        "bottom_left" => FloatingAnchor::BottomLeft,
        "bottom_right" => FloatingAnchor::BottomRight,
        other => return Err(runtime_error(format!("unknown floating anchor '{other}'"))),
    };
    Ok(Some(anchor))
}

fn parse_bytes(bytes: Array) -> RhaiResult<Vec<u8>> {
    let mut parsed = Vec::with_capacity(bytes.len());
    for byte in bytes {
        let Some(value) = byte.try_cast::<i64>() else {
            return Err(runtime_error("send_bytes expects an array of integers"));
        };
        let value = u8::try_from(value)
            .map_err(|_| runtime_error("send_bytes values must be between 0 and 255"))?;
        parsed.push(value);
    }
    Ok(parsed)
}

fn parse_count(value: i64, label: &str) -> RhaiResult<usize> {
    if value < 0 {
        return Err(runtime_error(format!("{label} must be zero or greater")));
    }
    usize::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_amount(value: i64, label: &str) -> RhaiResult<u16> {
    if value <= 0 {
        return Err(runtime_error(format!("{label} must be greater than zero")));
    }
    u16::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_index(value: i64, label: &str) -> RhaiResult<usize> {
    if value < 0 {
        return Err(runtime_error(format!("{label} must be zero or greater")));
    }
    usize::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_buffer_id(value: i64) -> RhaiResult<BufferId> {
    if value < 0 {
        return Err(runtime_error("buffer id must be zero or greater"));
    }
    Ok(BufferId(value as u64))
}

fn parse_node_id(value: i64) -> RhaiResult<NodeId> {
    if value < 0 {
        return Err(runtime_error("node id must be zero or greater"));
    }
    Ok(NodeId(value as u64))
}

fn parse_floating_id(value: i64) -> RhaiResult<FloatingId> {
    if value < 0 {
        return Err(runtime_error("floating id must be zero or greater"));
    }
    Ok(FloatingId(value as u64))
}

fn parse_notify_level(value: &str) -> RhaiResult<NotifyLevel> {
    match value {
        "info" => Ok(NotifyLevel::Info),
        "warn" => Ok(NotifyLevel::Warn),
        "error" => Ok(NotifyLevel::Error),
        _ => Err(runtime_error(format!("unknown notify level '{value}'"))),
    }
}

fn parse_split_direction(value: &str) -> RhaiResult<SplitDirection> {
    match value.to_ascii_lowercase().as_str() {
        "h" | "horizontal" => Ok(SplitDirection::Horizontal),
        "v" | "vertical" => Ok(SplitDirection::Vertical),
        _ => Err(runtime_error(format!("unknown split direction '{value}'"))),
    }
}

fn dynamic_option_custom<T: Clone + Send + Sync + 'static>(value: Option<T>) -> Dynamic {
    value.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
}

fn dynamic_option_string(value: Option<String>) -> Dynamic {
    value.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
}

fn dynamic_u64(value: u64) -> Dynamic {
    Dynamic::from(i64::try_from(value).unwrap_or(i64::MAX))
}

fn dynamic_u32(value: u32) -> Dynamic {
    Dynamic::from(i64::from(value))
}

fn dynamic_usize(value: usize) -> Dynamic {
    Dynamic::from(i64::try_from(value).unwrap_or(i64::MAX))
}

fn rect_map(rect: Rect) -> Map {
    Map::from_iter([
        ("x".into(), Dynamic::from(i64::from(rect.origin.x))),
        ("y".into(), Dynamic::from(i64::from(rect.origin.y))),
        ("width".into(), Dynamic::from(i64::from(rect.size.width))),
        ("height".into(), Dynamic::from(i64::from(rect.size.height))),
    ])
}

fn float_geometry_map(geometry: embers_core::FloatGeometry) -> Map {
    Map::from_iter([
        ("x".into(), Dynamic::from(i64::from(geometry.x))),
        ("y".into(), Dynamic::from(i64::from(geometry.y))),
        ("width".into(), Dynamic::from(i64::from(geometry.width))),
        ("height".into(), Dynamic::from(i64::from(geometry.height))),
    ])
}

fn activity_name(activity: embers_core::ActivityState) -> String {
    match activity {
        embers_core::ActivityState::Idle => "idle",
        embers_core::ActivityState::Activity => "activity",
        embers_core::ActivityState::Bell => "bell",
    }
    .to_owned()
}

fn node_kind_name(kind: embers_protocol::NodeRecordKind) -> String {
    match kind {
        embers_protocol::NodeRecordKind::BufferView => "buffer_view",
        embers_protocol::NodeRecordKind::Split => "split",
        embers_protocol::NodeRecordKind::Tabs => "tabs",
    }
    .to_owned()
}

fn split_direction_name(direction: embers_core::SplitDirection) -> String {
    match direction {
        embers_core::SplitDirection::Horizontal => "horizontal",
        embers_core::SplitDirection::Vertical => "vertical",
    }
    .to_owned()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), rhai::Position::NONE).into()
}

#[cfg(test)]
mod tests {
    use super::{parse_notify_level, parse_split_direction};

    #[test]
    fn parse_levels_accepts_draft_names() {
        assert!(parse_notify_level("info").is_ok());
        assert!(parse_notify_level("warn").is_ok());
        assert!(parse_notify_level("error").is_ok());
    }

    #[test]
    fn parse_split_direction_accepts_words() {
        assert!(parse_split_direction("horizontal").is_ok());
        assert!(parse_split_direction("vertical").is_ok());
    }
}
