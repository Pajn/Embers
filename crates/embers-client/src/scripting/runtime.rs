use std::convert::TryFrom;

use embers_core::{BufferId, FloatGeometry, NodeId, Rect, SplitDirection};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};

use crate::presentation::NavigationDirection;

use super::context::{
    BufferRef, Context, FloatingRef, NodeRef, SessionRef, TabBarContext, TabStateRef,
};
use super::model::{
    Action, BufferSpawnSpec, BufferTarget, FloatingOptions, NodeTarget, TabSpec, TreeSpec,
    WeightedTreeSpec,
};
use super::types::{BarSpec, RgbColor, SegmentSpec, ThemeSpec};

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
    engine.register_type_with_name::<WeightedTreeSpec>("WeightedTreeSpec");
    engine.register_type_with_name::<TabSpec>("TabSpec");
    engine.register_type_with_name::<Context>("Context");
    engine.register_type_with_name::<SessionRef>("SessionRef");
    engine.register_type_with_name::<BufferRef>("BufferRef");
    engine.register_type_with_name::<NodeRef>("NodeRef");
    engine.register_type_with_name::<FloatingRef>("FloatingRef");
    engine.register_type_with_name::<TabBarContext>("TabBarContext");
    engine.register_type_with_name::<TabStateRef>("TabStateRef");
    engine.register_type_with_name::<BarSpec>("BarSpec");
    engine.register_type_with_name::<SegmentSpec>("SegmentSpec");
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

pub fn runtime_scope(
    context: Context,
    theme: ThemeSpec,
    bar_context: Option<TabBarContext>,
) -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push_constant("ctx", context.clone());
    scope.push_constant("mux", MuxApi::new(context));
    scope.push_constant("system", SystemApi);
    scope.push_constant("action", ActionApi);
    scope.push_constant("tree", TreeApi);
    scope.push_constant("ui", UiApi);
    scope.push_constant("theme", ThemeRuntimeApi { theme });
    if let Some(bar_context) = bar_context {
        scope.push_constant("bar", bar_context);
    }
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
        let mut normalized = Vec::with_capacity(actions.len());
        for action in actions {
            let Some(action) = action.try_cast::<Action>() else {
                return Err("script returned a non-Action item in an action array".to_owned());
            };
            normalized.push(action);
        }
        return Ok(normalized);
    }

    Err("script must return Action, [Action], or ()".to_owned())
}

pub fn normalize_bar(result: Dynamic) -> Result<BarSpec, String> {
    result
        .try_cast::<BarSpec>()
        .ok_or_else(|| "script formatter must return a BarSpec".to_owned())
}

fn register_context_api(engine: &mut Engine) {
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
    engine.register_fn("visible_floating", |context: &mut Context| -> Array {
        context
            .visible_floating()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
}

fn register_ref_api(engine: &mut Engine) {
    engine.register_fn("id", |session: &mut SessionRef| session.id.0 as i64);
    engine.register_fn("name", |session: &mut SessionRef| session.name.clone());

    engine.register_fn("id", |buffer: &mut BufferRef| buffer.id.0 as i64);
    engine.register_fn("title", |buffer: &mut BufferRef| buffer.title.clone());
    engine.register_fn("command", |buffer: &mut BufferRef| -> Array {
        buffer.command.iter().cloned().map(Dynamic::from).collect()
    });
    engine.register_fn("cwd", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.cwd.clone())
    });
    engine.register_fn("process_name", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.process_name())
    });
    engine.register_fn("tty_path", |buffer: &mut BufferRef| -> Dynamic {
        dynamic_option_string(buffer.tty_path.clone())
    });
    engine.register_fn("is_visible", |buffer: &mut BufferRef| buffer.visible);
    engine.register_fn("is_detached", |buffer: &mut BufferRef| buffer.detached);
    engine.register_fn("activity", |buffer: &mut BufferRef| {
        activity_name(buffer.activity)
    });
    engine.register_fn("state", |buffer: &mut BufferRef| {
        buffer_state_name(buffer.state)
    });

    engine.register_fn("id", |node: &mut NodeRef| node.id.0 as i64);
    engine.register_fn("kind", |node: &mut NodeRef| node_kind_name(node.kind));
    engine.register_fn("children", |node: &mut NodeRef| -> Array {
        node.child_ids
            .iter()
            .map(|child_id| Dynamic::from(child_id.0 as i64))
            .collect()
    });
    engine.register_fn("geometry", |node: &mut NodeRef| -> Dynamic {
        node.geometry
            .map(rect_map)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("tab_titles", |node: &mut NodeRef| -> Array {
        node.tab_titles.iter().cloned().map(Dynamic::from).collect()
    });
    engine.register_fn("active_tab", |node: &mut NodeRef| -> Dynamic {
        node.active_tab
            .map(|index| Dynamic::from(index as i64))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("buffer_id", |node: &mut NodeRef| -> Dynamic {
        node.buffer_id
            .map(|buffer_id| Dynamic::from(buffer_id.0 as i64))
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("is_visible", |node: &mut NodeRef| node.visible);

    engine.register_fn("id", |window: &mut FloatingRef| window.id.0 as i64);
    engine.register_fn("title", |window: &mut FloatingRef| -> Dynamic {
        dynamic_option_string(window.title.clone())
    });
    engine.register_fn("is_visible", |window: &mut FloatingRef| window.visible);
    engine.register_fn("is_focused", |window: &mut FloatingRef| window.focused);
    engine.register_fn("geometry", |window: &mut FloatingRef| -> Map {
        float_geometry_map(window.geometry)
    });

    engine.register_fn("is_root", |bar: &mut TabBarContext| bar.is_root);
    engine.register_fn("active_index", |bar: &mut TabBarContext| bar.active as i64);
    engine.register_fn("tabs", |bar: &mut TabBarContext| -> Array {
        bar.tabs.iter().cloned().map(Dynamic::from).collect()
    });

    engine.register_fn("title", |tab: &mut TabStateRef| tab.title.clone());
    engine.register_fn("is_active", |tab: &mut TabStateRef| tab.active);
    engine.register_fn("activity", |tab: &mut TabStateRef| {
        activity_name(tab.activity)
    });
}

fn register_action_api(engine: &mut Engine) {
    engine.register_fn("enter_mode", |_: &mut ActionApi, mode: ImmutableString| {
        Action::EnterMode {
            mode: mode.to_string(),
        }
    });
    engine.register_fn("focus_left", |_: &mut ActionApi| Action::Focus {
        direction: NavigationDirection::Left,
    });
    engine.register_fn("focus_right", |_: &mut ActionApi| Action::Focus {
        direction: NavigationDirection::Right,
    });
    engine.register_fn("focus_up", |_: &mut ActionApi| Action::Focus {
        direction: NavigationDirection::Up,
    });
    engine.register_fn("focus_down", |_: &mut ActionApi| Action::Focus {
        direction: NavigationDirection::Down,
    });
    engine.register_fn(
        "resize_left",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::Resize {
                direction: NavigationDirection::Left,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_right",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::Resize {
                direction: NavigationDirection::Right,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_up",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::Resize {
                direction: NavigationDirection::Up,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "resize_down",
        |_: &mut ActionApi, amount: i64| -> RhaiResult<Action> {
            Ok(Action::Resize {
                direction: NavigationDirection::Down,
                amount: parse_amount(amount, "resize amount")?,
            })
        },
    );
    engine.register_fn(
        "select_tab",
        |_: &mut ActionApi, index: i64| -> RhaiResult<Action> {
            Ok(Action::SelectTab {
                index: parse_index(index, "tab index")?,
            })
        },
    );
    engine.register_fn("split_h", |_: &mut ActionApi, tree: TreeSpec| {
        Action::Split {
            direction: SplitDirection::Horizontal,
            tree,
        }
    });
    engine.register_fn("split_v", |_: &mut ActionApi, tree: TreeSpec| {
        Action::Split {
            direction: SplitDirection::Vertical,
            tree,
        }
    });
    engine.register_fn(
        "replace_current_with",
        |_: &mut ActionApi, tree: TreeSpec| Action::ReplaceCurrentWith { tree },
    );
    engine.register_fn(
        "replace_node",
        |_: &mut ActionApi, node_id: i64, tree: TreeSpec| -> RhaiResult<Action> {
            Ok(Action::ReplaceNode {
                target: NodeTarget::Node(parse_node_id(node_id)?),
                tree,
            })
        },
    );
    engine.register_fn(
        "wrap_current_in_split_h",
        |_: &mut ActionApi, tree: TreeSpec| Action::WrapCurrentInSplit {
            direction: SplitDirection::Horizontal,
            tree,
        },
    );
    engine.register_fn(
        "wrap_current_in_split_v",
        |_: &mut ActionApi, tree: TreeSpec| Action::WrapCurrentInSplit {
            direction: SplitDirection::Vertical,
            tree,
        },
    );
    engine.register_fn(
        "wrap_current_in_tabs",
        |_: &mut ActionApi, tabs: Array, active: i64| -> RhaiResult<Action> {
            let tabs = parse_tabs(tabs)?;
            let active = parse_index(active, "active tab")?;
            if tabs.is_empty() {
                return Err(runtime_error("tabs cannot be empty"));
            }
            if active >= tabs.len() {
                return Err(runtime_error("active tab index is out of bounds"));
            }
            Ok(Action::WrapCurrentInTabs { tabs, active })
        },
    );
    engine.register_fn(
        "insert_tab_after_current",
        |_: &mut ActionApi, title: ImmutableString, tree: TreeSpec| Action::InsertTabAfterCurrent {
            title: title.to_string(),
            tree,
        },
    );
    engine.register_fn(
        "open_floating",
        |_: &mut ActionApi, tree: TreeSpec, options: Map| -> RhaiResult<Action> {
            Ok(Action::OpenFloating {
                tree,
                options: parse_floating_options(options)?,
            })
        },
    );
    engine.register_fn("detach_current_buffer", |_: &mut ActionApi| {
        Action::DetachBuffer {
            target: BufferTarget::Current,
        }
    });
    engine.register_fn("kill_current_buffer", |_: &mut ActionApi| {
        Action::KillBuffer {
            target: BufferTarget::Current,
            force: false,
        }
    });
    engine.register_fn("send_keys", |_: &mut ActionApi, text: ImmutableString| {
        Action::SendBytes {
            target: BufferTarget::Current,
            bytes: text.as_bytes().to_vec(),
        }
    });
    engine.register_fn(
        "send_bytes",
        |_: &mut ActionApi, bytes: Array| -> RhaiResult<Action> {
            Ok(Action::SendBytes {
                target: BufferTarget::Current,
                bytes: parse_bytes(bytes)?,
            })
        },
    );
    engine.register_fn("notify", |_: &mut ActionApi, message: ImmutableString| {
        Action::Notify {
            message: message.to_string(),
        }
    });
    engine.register_fn("reload_config", |_: &mut ActionApi| Action::ReloadConfig);
    engine.register_fn(
        "chain",
        |_: &mut ActionApi, actions: Array| -> RhaiResult<Array> {
            for action in &actions {
                if action.clone().try_cast::<Action>().is_none() {
                    return Err(runtime_error(
                        "action.chain expects an array of Action values",
                    ));
                }
            }
            Ok(actions)
        },
    );
}

fn register_tree_api(engine: &mut Engine) {
    engine.register_fn("buffer_current", |_: &mut TreeApi| TreeSpec::BufferCurrent);
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
        |_: &mut TreeApi, options: Map| -> RhaiResult<TreeSpec> {
            Ok(TreeSpec::BufferSpawn(parse_buffer_spawn(options)?))
        },
    );
    engine.register_fn(
        "weight",
        |_: &mut TreeApi, weight: i64, tree: TreeSpec| -> RhaiResult<WeightedTreeSpec> {
            let weight = parse_amount(weight, "weight")?;
            if weight == 0 {
                return Err(runtime_error("weights must be greater than zero"));
            }
            Ok(WeightedTreeSpec {
                weight,
                tree: Box::new(tree),
            })
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
            build_split(SplitDirection::Horizontal, children)
        },
    );
    engine.register_fn(
        "split_v",
        |_: &mut TreeApi, children: Array| -> RhaiResult<TreeSpec> {
            build_split(SplitDirection::Vertical, children)
        },
    );
    engine.register_fn(
        "split",
        |_: &mut TreeApi, direction: ImmutableString, children: Array| -> RhaiResult<TreeSpec> {
            build_split(parse_split_direction(direction.as_str())?, children)
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
    engine.register_fn("detached_buffers", |mux: &mut MuxApi| -> Array {
        mux.context
            .detached_buffers()
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
    engine.register_fn("visible_floating", |mux: &mut MuxApi| -> Array {
        mux.context
            .visible_floating()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });
}

fn register_system_api(engine: &mut Engine) {
    engine.register_fn(
        "process_name",
        |_: &mut SystemApi, buffer: BufferRef| -> Dynamic {
            dynamic_option_string(buffer.process_name())
        },
    );
    engine.register_fn(
        "tty_path",
        |_: &mut SystemApi, buffer: BufferRef| -> Dynamic {
            dynamic_option_string(buffer.tty_path)
        },
    );
    engine.register_fn("command", |_: &mut SystemApi, buffer: BufferRef| -> Array {
        buffer.command.into_iter().map(Dynamic::from).collect()
    });
}

fn register_ui_api(engine: &mut Engine) {
    engine.register_fn("segment", |_: &mut UiApi, text: ImmutableString| {
        SegmentSpec {
            text: text.to_string(),
            foreground: None,
            background: None,
        }
    });
    engine.register_fn(
        "segment",
        |_: &mut UiApi, text: ImmutableString, foreground: RgbColor| SegmentSpec {
            text: text.to_string(),
            foreground: Some(foreground),
            background: None,
        },
    );
    engine.register_fn(
        "segment",
        |_: &mut UiApi, text: ImmutableString, foreground: RgbColor, background: RgbColor| {
            SegmentSpec {
                text: text.to_string(),
                foreground: Some(foreground),
                background: Some(background),
            }
        },
    );
    engine.register_fn(
        "bar",
        |_: &mut UiApi, segments: Array| -> RhaiResult<BarSpec> {
            let mut parsed = Vec::with_capacity(segments.len());
            for segment in segments {
                let Some(segment) = segment.try_cast::<SegmentSpec>() else {
                    return Err(runtime_error("ui.bar expects SegmentSpec values"));
                };
                parsed.push(segment);
            }
            Ok(BarSpec { segments: parsed })
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

fn build_split(direction: SplitDirection, children: Array) -> RhaiResult<TreeSpec> {
    let children = parse_weighted_children(children)?;
    if children.is_empty() {
        return Err(runtime_error("split children cannot be empty"));
    }
    Ok(TreeSpec::Split {
        direction,
        children,
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
    Ok(TreeSpec::Tabs { tabs, active })
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

fn parse_weighted_children(children: Array) -> RhaiResult<Vec<WeightedTreeSpec>> {
    let mut parsed = Vec::with_capacity(children.len());
    for child in children {
        if let Some(weighted) = child.clone().try_cast::<WeightedTreeSpec>() {
            parsed.push(weighted);
            continue;
        }
        let Some(tree) = child.try_cast::<TreeSpec>() else {
            return Err(runtime_error(
                "split children must be TreeSpec or WeightedTreeSpec",
            ));
        };
        parsed.push(WeightedTreeSpec {
            weight: 1,
            tree: Box::new(tree),
        });
    }
    Ok(parsed)
}

fn parse_buffer_spawn(mut options: Map) -> RhaiResult<BufferSpawnSpec> {
    let command = options
        .remove("command")
        .ok_or_else(|| runtime_error("buffer_spawn requires a 'command' array"))?;
    Ok(BufferSpawnSpec {
        title: parse_optional_string(options.remove("title"))?,
        command: parse_string_array(command)?,
        cwd: parse_optional_string(options.remove("cwd"))?,
    })
}

fn parse_floating_options(mut options: Map) -> RhaiResult<FloatingOptions> {
    let x = parse_u16_field(&mut options, "x")?;
    let y = parse_u16_field(&mut options, "y")?;
    let width = parse_u16_field(&mut options, "width")?;
    let height = parse_u16_field(&mut options, "height")?;
    Ok(FloatingOptions {
        geometry: FloatGeometry::new(x, y, width, height),
        title: parse_optional_string(options.remove("title"))?,
    })
}

fn parse_u16_field(options: &mut Map, key: &str) -> RhaiResult<u16> {
    let value = options
        .remove(key)
        .ok_or_else(|| runtime_error(format!("missing '{key}' field")))?;
    let Some(value) = value.try_cast::<i64>() else {
        return Err(runtime_error(format!("'{key}' must be an integer")));
    };
    parse_amount(value, key)
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

fn rect_map(rect: Rect) -> Map {
    [
        ("x".into(), Dynamic::from(rect.origin.x as i64)),
        ("y".into(), Dynamic::from(rect.origin.y as i64)),
        ("width".into(), Dynamic::from(rect.size.width as i64)),
        ("height".into(), Dynamic::from(rect.size.height as i64)),
    ]
    .into_iter()
    .collect()
}

fn float_geometry_map(geometry: FloatGeometry) -> Map {
    [
        ("x".into(), Dynamic::from(geometry.x as i64)),
        ("y".into(), Dynamic::from(geometry.y as i64)),
        ("width".into(), Dynamic::from(geometry.width as i64)),
        ("height".into(), Dynamic::from(geometry.height as i64)),
    ]
    .into_iter()
    .collect()
}

fn activity_name(activity: embers_core::ActivityState) -> String {
    match activity {
        embers_core::ActivityState::Idle => "idle",
        embers_core::ActivityState::Activity => "activity",
        embers_core::ActivityState::Bell => "bell",
    }
    .to_owned()
}

fn buffer_state_name(state: embers_protocol::BufferRecordState) -> String {
    match state {
        embers_protocol::BufferRecordState::Created => "created",
        embers_protocol::BufferRecordState::Running => "running",
        embers_protocol::BufferRecordState::Exited => "exited",
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

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), rhai::Position::NONE).into()
}
