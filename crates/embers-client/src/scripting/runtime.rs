use std::convert::TryFrom;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use embers_core::{BufferId, FloatingId, NodeId, Rect, SplitDirection};
use embers_protocol::{
    BufferHistoryPlacement, BufferHistoryScope, NodeBreakDestination, NodeJoinPlacement,
};
use rhai::plugin::*;
use rhai::{
    Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, NativeCallContext, Position, Scope,
};

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
use super::{RhaiResultOf, ScriptResult};

#[derive(Clone, Default)]
pub(crate) struct ActionApi;

#[derive(Clone, Default)]
pub(crate) struct TreeApi;

#[derive(Clone, Default)]
pub(crate) struct UiApi;

#[derive(Clone)]
pub(crate) struct MuxApi {
    context: Context,
}

#[derive(Clone, Default)]
pub(crate) struct SystemApi;

#[derive(Clone)]
pub(crate) struct ThemeRuntimeApi {
    theme: ThemeSpec,
}

impl MuxApi {
    fn new(context: Context) -> Self {
        Self { context }
    }
}

pub fn register_runtime_api(engine: &mut Engine) {
    register_documented_runtime_modules(engine);
}

// Used by `documentation.rs` and the live runtime to register the shared exported API modules.
#[allow(dead_code)]
pub(crate) fn register_documented_runtime_api(engine: &mut Engine) {
    register_documented_runtime_modules(engine);
}

fn register_documented_runtime_modules(engine: &mut Engine) {
    register_runtime_types(engine);
    engine.register_global_module(rhai::exported_module!(documented_context_api).into());
    engine.register_global_module(rhai::exported_module!(documented_ref_api).into());
    engine.register_global_module(rhai::exported_module!(documented_action_api).into());
    engine.register_global_module(rhai::exported_module!(documented_tree_api).into());
    engine.register_global_module(rhai::exported_module!(documented_mux_api).into());
    engine.register_global_module(rhai::exported_module!(documented_system_api).into());
    engine.register_global_module(rhai::exported_module!(documented_ui_api).into());
    engine.register_global_module(rhai::exported_module!(documented_theme_runtime_api).into());
}

pub(crate) fn register_documented_registration_runtime_api(engine: &mut Engine) {
    register_runtime_types(engine);
    engine.register_global_module(rhai::exported_module!(documented_action_api).into());
    engine.register_global_module(rhai::exported_module!(documented_tree_api).into());
    engine.register_global_module(rhai::exported_module!(documented_system_api).into());
    engine.register_global_module(rhai::exported_module!(documented_ui_api).into());
}

fn register_runtime_types(engine: &mut Engine) {
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
}

pub fn runtime_scope(context: Option<Context>, theme: ThemeSpec) -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push("system", SystemApi);
    scope.push("action", ActionApi);
    scope.push("tree", TreeApi);
    scope.push("ui", UiApi);
    scope.push("theme", ThemeRuntimeApi { theme });
    if let Some(context) = context {
        scope.push("mux", MuxApi::new(context));
    }
    scope
}

pub fn registration_scope() -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push("system", SystemApi);
    scope.push("action", ActionApi);
    scope.push("tree", TreeApi);
    scope.push("ui", UiApi);
    scope
}

pub fn normalize_actions(result: Dynamic) -> Result<Vec<Action>, String> {
    let actions = if result.is_unit() {
        Vec::new()
    } else if let Some(action) = result.clone().try_cast::<Action>() {
        vec![action]
    } else if let Some(actions) = result.try_cast::<Array>() {
        parse_action_array(actions).map_err(|error| error.to_string())?
    } else {
        return Err("script must return Action, [Action], or ()".to_owned());
    };

    validate_live_actions(&actions)?;
    Ok(actions)
}

fn validate_live_actions(actions: &[Action]) -> Result<(), String> {
    for action in actions {
        if let Action::Chain(inner) = action {
            validate_live_actions(inner)?
        }
    }

    Ok(())
}

pub fn normalize_bar(result: Dynamic) -> Result<BarSpec, String> {
    result
        .try_cast::<BarSpec>()
        .ok_or_else(|| "tab bar formatter must return a BarSpec".to_owned())
}

#[allow(dead_code)]
#[export_module]
mod documented_context_api {
    use super::{
        Array, Context, Dynamic, NativeCallContext, dynamic_option_custom, parse_buffer_id,
        parse_floating_id, parse_node_id, with_call_position,
    };

    /// Return the active input mode name.
    #[rhai_fn(name = "current_mode")]
    pub fn current_mode(context: &mut Context) -> String {
        context.current_mode().to_owned()
    }

    /// Return the current event payload, if any.
    ///
    /// ReturnType: `EventInfo | ()`
    #[rhai_fn(name = "event")]
    pub fn event(context: &mut Context) -> Dynamic {
        dynamic_option_custom(context.event())
    }

    /// Return the current session reference, if any.
    ///
    /// ReturnType: `SessionRef | ()`
    #[rhai_fn(name = "current_session")]
    pub fn current_session(context: &mut Context) -> Dynamic {
        dynamic_option_custom(context.current_session())
    }

    /// Return the currently focused node, if any.
    ///
    /// ReturnType: `NodeRef | ()`
    #[rhai_fn(name = "current_node")]
    pub fn current_node(context: &mut Context) -> Dynamic {
        dynamic_option_custom(context.current_node())
    }

    /// Return the currently focused buffer, if any.
    ///
    /// ReturnType: `BufferRef | ()`
    ///
    /// # Example
    ///
    /// ```rhai
    /// let buffer = ctx.current_buffer();
    /// if buffer != () {
    ///     print(buffer.title());
    /// }
    /// ```
    #[rhai_fn(name = "current_buffer")]
    pub fn current_buffer(context: &mut Context) -> Dynamic {
        dynamic_option_custom(context.current_buffer())
    }

    /// Return the currently focused floating window, if any.
    ///
    /// ReturnType: `FloatingRef | ()`
    #[rhai_fn(name = "current_floating")]
    pub fn current_floating(context: &mut Context) -> Dynamic {
        dynamic_option_custom(context.current_floating())
    }

    /// Return every visible session.
    #[rhai_fn(name = "sessions")]
    pub fn sessions(context: &mut Context) -> Array {
        context.sessions().into_iter().map(Dynamic::from).collect()
    }

    /// Find a buffer by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `BufferRef | ()`
    #[rhai_fn(return_raw, name = "find_buffer")]
    pub fn find_buffer(
        ctx: NativeCallContext,
        context: &mut Context,
        buffer_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                context.find_buffer(parse_buffer_id(buffer_id)?),
            ))
        })
    }

    /// Find a node by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `NodeRef | ()`
    #[rhai_fn(return_raw, name = "find_node")]
    pub fn find_node(
        ctx: NativeCallContext,
        context: &mut Context,
        node_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                context.find_node(parse_node_id(node_id)?),
            ))
        })
    }

    /// Find a floating window by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `FloatingRef | ()`
    #[rhai_fn(return_raw, name = "find_floating")]
    pub fn find_floating(
        ctx: NativeCallContext,
        context: &mut Context,
        floating_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                context.find_floating(parse_floating_id(floating_id)?),
            ))
        })
    }

    /// Return detached buffers in the current model snapshot.
    #[rhai_fn(name = "detached_buffers")]
    pub fn detached_buffers(context: &mut Context) -> Array {
        context
            .detached_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    }

    /// Return visible buffers in the current model snapshot.
    #[rhai_fn(name = "visible_buffers")]
    pub fn visible_buffers(context: &mut Context) -> Array {
        context
            .visible_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_ref_api {
    use super::{
        Array, BufferRef, Dynamic, EventInfo, FloatingRef, Map, NativeCallContext, NodeRef,
        SessionRef, TabBarContext, TabInfo, activity_name, dynamic_option_string, dynamic_u32,
        dynamic_u64, float_geometry_map, node_kind_name, parse_count, rect_map,
        split_direction_name, with_call_position,
    };

    /// Return the session id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "session_id")]
    pub fn event_session_id(event: &mut EventInfo) -> Dynamic {
        event
            .session_id
            .map(|session_id| dynamic_u64(session_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the buffer id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "buffer_id")]
    pub fn event_buffer_id(event: &mut EventInfo) -> Dynamic {
        event
            .buffer_id
            .map(|buffer_id| dynamic_u64(buffer_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the previous session id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "previous_session_id")]
    pub fn event_previous_session_id(event: &mut EventInfo) -> Dynamic {
        event
            .previous_session_id
            .map(|session_id| dynamic_u64(session_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the client id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "client_id")]
    pub fn event_client_id(event: &mut EventInfo) -> Dynamic {
        event.client_id.map(dynamic_u64).unwrap_or(Dynamic::UNIT)
    }

    /// Return the event name.
    #[rhai_fn(name = "name")]
    pub fn event_name(event: &mut EventInfo) -> String {
        event.name.clone()
    }

    /// Return the node id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "node_id")]
    pub fn event_node_id(event: &mut EventInfo) -> Dynamic {
        event
            .node_id
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the floating id attached to an event, or `()`.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "floating_id")]
    pub fn event_floating_id(event: &mut EventInfo) -> Dynamic {
        event
            .floating_id
            .map(|floating_id| dynamic_u64(floating_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the numeric session id.
    #[rhai_fn(name = "id")]
    pub fn session_id(session: &mut SessionRef) -> i64 {
        i64::try_from(session.id.0).unwrap_or(i64::MAX)
    }

    /// Return the session name.
    #[rhai_fn(name = "name")]
    pub fn session_name(session: &mut SessionRef) -> String {
        session.name.clone()
    }

    /// Return the root tabs node for the session.
    #[rhai_fn(name = "root_node")]
    pub fn session_root_node(session: &mut SessionRef) -> i64 {
        i64::try_from(session.root_node_id.0).unwrap_or(i64::MAX)
    }

    /// Return floating window ids attached to the session.
    #[rhai_fn(name = "floating")]
    pub fn session_floating(session: &mut SessionRef) -> Array {
        session
            .floating_ids
            .iter()
            .map(|floating_id| dynamic_u64(floating_id.0))
            .collect()
    }

    /// Return the buffer title.
    #[rhai_fn(name = "title")]
    pub fn buffer_title(buffer: &mut BufferRef) -> String {
        buffer.title.clone()
    }

    /// Return the numeric buffer id.
    #[rhai_fn(name = "id")]
    pub fn buffer_id(buffer: &mut BufferRef) -> i64 {
        i64::try_from(buffer.id.0).unwrap_or(i64::MAX)
    }

    /// Return the attached session id, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "session_id")]
    pub fn buffer_session_id(buffer: &mut BufferRef) -> Dynamic {
        buffer
            .session_id
            .map(|session_id| dynamic_u64(session_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the attached node id, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "node_id")]
    pub fn buffer_node_id(buffer: &mut BufferRef) -> Dynamic {
        buffer
            .node_id()
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the original command vector.
    #[rhai_fn(name = "command")]
    pub fn buffer_command(buffer: &mut BufferRef) -> Array {
        buffer.command.iter().cloned().map(Dynamic::from).collect()
    }

    /// Return the working directory, if any.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "cwd")]
    pub fn buffer_cwd(buffer: &mut BufferRef) -> Dynamic {
        dynamic_option_string(buffer.cwd.clone())
    }

    /// Return the process id, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "pid")]
    pub fn buffer_pid(buffer: &mut BufferRef) -> Dynamic {
        buffer.pid.map(dynamic_u32).unwrap_or(Dynamic::UNIT)
    }

    /// Return the detected process name, if any.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "process_name")]
    pub fn buffer_process_name(buffer: &mut BufferRef) -> Dynamic {
        dynamic_option_string(buffer.process_name())
    }

    /// Return the controlling TTY path, if any.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "tty_path")]
    pub fn buffer_tty_path(buffer: &mut BufferRef) -> Dynamic {
        dynamic_option_string(buffer.tty_path.clone())
    }

    /// Look up a single environment hint captured on the buffer.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "env_hint")]
    pub fn buffer_env_hint(buffer: &mut BufferRef, key: &str) -> Dynamic {
        dynamic_option_string(buffer.env_hint(key))
    }

    /// Return a text snapshot limited to the requested line count.
    #[rhai_fn(return_raw, name = "snapshot_text")]
    pub fn buffer_snapshot_text(
        ctx: NativeCallContext,
        buffer: &mut BufferRef,
        limit: i64,
    ) -> RhaiResultOf<String> {
        with_call_position(ctx, || {
            Ok(buffer.snapshot_text(parse_count(limit, "snapshot_text limit")?))
        })
    }

    /// Return the full captured history text for the buffer.
    ///
    /// # Example
    ///
    /// ```rhai
    /// let buffer = ctx.current_buffer();
    /// if buffer != () {
    ///     let history = buffer.history_text();
    /// }
    /// ```
    #[rhai_fn(name = "history_text")]
    pub fn buffer_history_text(buffer: &mut BufferRef) -> String {
        buffer.history_text()
    }

    /// Return whether the buffer is currently attached to a node.
    #[rhai_fn(name = "is_attached")]
    pub fn buffer_is_attached(buffer: &mut BufferRef) -> bool {
        buffer.is_attached()
    }

    /// Return whether the buffer has been detached.
    #[rhai_fn(name = "is_detached")]
    pub fn buffer_is_detached(buffer: &mut BufferRef) -> bool {
        buffer.is_detached()
    }

    /// Return whether the buffer process is still running.
    #[rhai_fn(name = "is_running")]
    pub fn buffer_is_running(buffer: &mut BufferRef) -> bool {
        buffer.is_running()
    }

    /// Return the process exit code, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "exit_code")]
    pub fn buffer_exit_code(buffer: &mut BufferRef) -> Dynamic {
        buffer.exit_code.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
    }

    /// Return whether the buffer is visible in the current presentation.
    #[rhai_fn(name = "is_visible")]
    pub fn buffer_is_visible(buffer: &mut BufferRef) -> bool {
        buffer.visible
    }

    /// Return the current activity state name.
    #[rhai_fn(name = "activity")]
    pub fn buffer_activity(buffer: &mut BufferRef) -> String {
        activity_name(buffer.activity)
    }

    /// Return the node id.
    #[rhai_fn(name = "id")]
    pub fn node_id(node: &mut NodeRef) -> i64 {
        i64::try_from(node.id.0).unwrap_or(i64::MAX)
    }

    /// Return the owning session id.
    #[rhai_fn(name = "session_id")]
    pub fn node_session_id(node: &mut NodeRef) -> i64 {
        i64::try_from(node.session_id.0).unwrap_or(i64::MAX)
    }

    /// Return the node kind such as `buffer_view`, `split`, or `tabs`.
    #[rhai_fn(name = "kind")]
    pub fn node_kind(node: &mut NodeRef) -> String {
        node_kind_name(node.kind)
    }

    /// Return the parent node id, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "parent")]
    pub fn node_parent(node: &mut NodeRef) -> Dynamic {
        node.parent_id
            .map(|node_id| dynamic_u64(node_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return child node ids.
    #[rhai_fn(name = "children")]
    pub fn node_children(node: &mut NodeRef) -> Array {
        node.child_ids
            .iter()
            .map(|child_id| dynamic_u64(child_id.0))
            .collect()
    }

    /// Return the geometry map, if any.
    ///
    /// ReturnType: `Map | ()`
    #[rhai_fn(name = "geometry")]
    pub fn node_geometry(node: &mut NodeRef) -> Dynamic {
        node.geometry
            .map(rect_map)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return whether the node is the session root.
    #[rhai_fn(name = "is_root")]
    pub fn node_is_root(node: &mut NodeRef) -> bool {
        node.is_root
    }

    /// Return whether the node is the root of a floating window.
    #[rhai_fn(name = "is_floating_root")]
    pub fn node_is_floating_root(node: &mut NodeRef) -> bool {
        node.is_floating_root
    }

    /// Return whether the node is visible in the current presentation.
    #[rhai_fn(name = "is_visible")]
    pub fn node_is_visible(node: &mut NodeRef) -> bool {
        node.visible
    }

    /// Return whether the node is focused.
    #[rhai_fn(name = "is_focused")]
    pub fn node_is_focused(node: &mut NodeRef) -> bool {
        node.is_focused
    }

    /// Return the attached buffer id, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "buffer")]
    pub fn node_buffer(node: &mut NodeRef) -> Dynamic {
        node.buffer_id
            .map(|buffer_id| dynamic_u64(buffer_id.0))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the split direction, if any.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "split_direction")]
    pub fn node_split_direction(node: &mut NodeRef) -> Dynamic {
        node.split_direction
            .map(split_direction_name)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return split weights, if any.
    ///
    /// ReturnType: `Array | ()`
    #[rhai_fn(name = "split_weights")]
    pub fn node_split_weights(node: &mut NodeRef) -> Dynamic {
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
    }

    /// Return the active tab index, if any.
    ///
    /// ReturnType: `int | ()`
    #[rhai_fn(name = "active_tab_index")]
    pub fn node_active_tab_index(node: &mut NodeRef) -> Dynamic {
        node.active_tab_index
            .map(dynamic_u32)
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return tab titles on a tabs node.
    #[rhai_fn(name = "tab_titles")]
    pub fn node_tab_titles(node: &mut NodeRef) -> Array {
        node.tab_titles.iter().cloned().map(Dynamic::from).collect()
    }

    /// Return the floating id.
    #[rhai_fn(name = "id")]
    pub fn floating_id(floating: &mut FloatingRef) -> i64 {
        i64::try_from(floating.id.0).unwrap_or(i64::MAX)
    }

    /// Return the owning session id.
    #[rhai_fn(name = "session_id")]
    pub fn floating_session_id(floating: &mut FloatingRef) -> i64 {
        i64::try_from(floating.session_id.0).unwrap_or(i64::MAX)
    }

    /// Return the root node id.
    #[rhai_fn(name = "root_node")]
    pub fn floating_root_node(floating: &mut FloatingRef) -> i64 {
        i64::try_from(floating.root_node_id.0).unwrap_or(i64::MAX)
    }

    /// Return the floating title, if any.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "title")]
    pub fn floating_title(floating: &mut FloatingRef) -> Dynamic {
        dynamic_option_string(floating.title.clone())
    }

    /// Return the floating geometry map.
    #[rhai_fn(name = "geometry")]
    pub fn floating_geometry(floating: &mut FloatingRef) -> Map {
        float_geometry_map(floating.geometry)
    }

    /// Return whether the floating is visible.
    #[rhai_fn(name = "is_visible")]
    pub fn floating_is_visible(floating: &mut FloatingRef) -> bool {
        floating.visible
    }

    /// Return whether the floating is focused.
    #[rhai_fn(name = "is_focused")]
    pub fn floating_is_focused(floating: &mut FloatingRef) -> bool {
        floating.focused
    }

    /// Return the tabs node id currently being formatted.
    #[rhai_fn(name = "node_id")]
    pub fn bar_node_id(bar: &mut TabBarContext) -> i64 {
        i64::try_from(bar.node_id.0).unwrap_or(i64::MAX)
    }

    /// Return whether the formatted tabs are the root tabs.
    #[rhai_fn(name = "is_root")]
    pub fn bar_is_root(bar: &mut TabBarContext) -> bool {
        bar.is_root
    }

    /// Return the active tab index.
    #[rhai_fn(name = "active_index")]
    pub fn bar_active_index(bar: &mut TabBarContext) -> i64 {
        i64::try_from(bar.active).unwrap_or(i64::MAX)
    }

    /// Return tab metadata used by the formatter.
    #[rhai_fn(name = "tabs")]
    pub fn bar_tabs(bar: &mut TabBarContext) -> Array {
        bar.tabs.iter().cloned().map(Dynamic::from).collect()
    }

    /// Return the formatter mode name.
    #[rhai_fn(name = "mode")]
    pub fn bar_mode(bar: &mut TabBarContext) -> String {
        bar.mode.clone()
    }

    /// Return the tab title.
    #[rhai_fn(name = "title")]
    pub fn tab_title(tab: &mut TabInfo) -> String {
        tab.title.clone()
    }

    /// Return the zero-based tab index.
    #[rhai_fn(name = "index")]
    pub fn tab_index(tab: &mut TabInfo) -> i64 {
        i64::try_from(tab.index).unwrap_or(i64::MAX)
    }

    /// Return whether the tab is active.
    #[rhai_fn(name = "is_active")]
    pub fn tab_is_active(tab: &mut TabInfo) -> bool {
        tab.active
    }

    /// Return whether the tab has activity.
    #[rhai_fn(name = "has_activity")]
    pub fn tab_has_activity(tab: &mut TabInfo) -> bool {
        tab.has_activity
    }

    /// Return whether the tab has a bell marker.
    #[rhai_fn(name = "has_bell")]
    pub fn tab_has_bell(tab: &mut TabInfo) -> bool {
        tab.has_bell
    }

    /// Return how many buffers are attached to the tab.
    #[rhai_fn(name = "buffer_count")]
    pub fn tab_buffer_count(tab: &mut TabInfo) -> i64 {
        i64::try_from(tab.buffer_count).unwrap_or(i64::MAX)
    }

    /// Return the formatter viewport width in cells.
    #[rhai_fn(name = "viewport_width")]
    pub fn bar_viewport_width(bar: &mut TabBarContext) -> i64 {
        i64::from(bar.viewport_width)
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_mux_api {
    use super::{
        Array, Dynamic, MuxApi, NativeCallContext, dynamic_option_custom, parse_buffer_id,
        parse_floating_id, parse_node_id, with_call_position,
    };

    /// Return the current session reference, if any.
    ///
    /// ReturnType: `SessionRef | ()`
    #[rhai_fn(name = "current_session")]
    pub fn current_session(mux: &mut MuxApi) -> Dynamic {
        dynamic_option_custom(mux.context.current_session())
    }

    /// Return the currently focused node, if any.
    ///
    /// ReturnType: `NodeRef | ()`
    #[rhai_fn(name = "current_node")]
    pub fn current_node(mux: &mut MuxApi) -> Dynamic {
        dynamic_option_custom(mux.context.current_node())
    }

    /// Return the currently focused buffer, if any.
    ///
    /// ReturnType: `BufferRef | ()`
    #[rhai_fn(name = "current_buffer")]
    pub fn current_buffer(mux: &mut MuxApi) -> Dynamic {
        dynamic_option_custom(mux.context.current_buffer())
    }

    /// Return the currently focused floating window, if any.
    ///
    /// ReturnType: `FloatingRef | ()`
    #[rhai_fn(name = "current_floating")]
    pub fn current_floating(mux: &mut MuxApi) -> Dynamic {
        dynamic_option_custom(mux.context.current_floating())
    }

    /// Return every visible session.
    #[rhai_fn(name = "sessions")]
    pub fn sessions(mux: &mut MuxApi) -> Array {
        mux.context
            .sessions()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    }

    /// Return visible buffers in the current model snapshot.
    #[rhai_fn(name = "visible_buffers")]
    pub fn visible_buffers(mux: &mut MuxApi) -> Array {
        mux.context
            .visible_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    }

    /// Return detached buffers in the current model snapshot.
    #[rhai_fn(name = "detached_buffers")]
    pub fn detached_buffers(mux: &mut MuxApi) -> Array {
        mux.context
            .detached_buffers()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    }

    /// Find a buffer by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `BufferRef | ()`
    #[rhai_fn(return_raw, name = "find_buffer")]
    pub fn find_buffer(
        ctx: NativeCallContext,
        mux: &mut MuxApi,
        buffer_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                mux.context.find_buffer(parse_buffer_id(buffer_id)?),
            ))
        })
    }

    /// Find a node by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `NodeRef | ()`
    #[rhai_fn(return_raw, name = "find_node")]
    pub fn find_node(
        ctx: NativeCallContext,
        mux: &mut MuxApi,
        node_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                mux.context.find_node(parse_node_id(node_id)?),
            ))
        })
    }

    /// Find a floating window by numeric id. Returns `()` when it does not exist.
    ///
    /// ReturnType: `FloatingRef | ()`
    #[rhai_fn(return_raw, name = "find_floating")]
    pub fn find_floating(
        ctx: NativeCallContext,
        mux: &mut MuxApi,
        floating_id: i64,
    ) -> RhaiResultOf<Dynamic> {
        with_call_position(ctx, || {
            Ok(dynamic_option_custom(
                mux.context.find_floating(parse_floating_id(floating_id)?),
            ))
        })
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_action_api {
    use super::{
        Action, ActionApi, Array, ImmutableString, Map, NativeCallContext, NavigationDirection,
        TreeSpec, parse_action_array, parse_buffer_history_placement, parse_buffer_history_scope,
        parse_buffer_id, parse_bytes, parse_floating_id, parse_floating_options,
        parse_floating_spec, parse_index, parse_key_sequence, parse_node_break_destination,
        parse_node_id, parse_node_join_placement, parse_notify_level, parse_split_direction,
        with_call_position,
    };

    /// Build a no-op action.
    #[rhai_fn(name = "noop")]
    pub fn noop(_: &mut ActionApi) -> Action {
        Action::Noop
    }

    /// Chain multiple actions into one composite action.
    #[rhai_fn(return_raw, name = "chain")]
    pub fn chain(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        actions: Array,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || Ok(Action::Chain(parse_action_array(actions)?)))
    }

    /// Enter a specific input mode by name.
    #[rhai_fn(name = "enter_mode")]
    pub fn enter_mode(_: &mut ActionApi, mode: &str) -> Action {
        Action::EnterMode {
            mode: mode.to_owned(),
        }
    }

    /// Leave the active input mode.
    #[rhai_fn(name = "leave_mode")]
    pub fn leave_mode(_: &mut ActionApi) -> Action {
        Action::LeaveMode
    }

    /// Toggle a named input mode.
    #[rhai_fn(name = "toggle_mode")]
    pub fn toggle_mode(_: &mut ActionApi, mode: &str) -> Action {
        Action::ToggleMode {
            mode: mode.to_owned(),
        }
    }

    /// Clear any partially-entered key sequence.
    #[rhai_fn(name = "clear_pending_keys")]
    pub fn clear_pending_keys(_: &mut ActionApi) -> Action {
        Action::ClearPendingKeys
    }

    /// Focus the view to the left of the current node.
    ///
    /// # Example
    ///
    /// ```rhai
    /// action.focus_left()
    /// ```
    #[rhai_fn(name = "focus_left")]
    pub fn focus_left(_: &mut ActionApi) -> Action {
        Action::FocusDirection {
            direction: NavigationDirection::Left,
        }
    }

    /// Focus the view to the right of the current node.
    #[rhai_fn(name = "focus_right")]
    pub fn focus_right(_: &mut ActionApi) -> Action {
        Action::FocusDirection {
            direction: NavigationDirection::Right,
        }
    }

    /// Focus the view above the current node.
    #[rhai_fn(name = "focus_up")]
    pub fn focus_up(_: &mut ActionApi) -> Action {
        Action::FocusDirection {
            direction: NavigationDirection::Up,
        }
    }

    /// Focus the view below the current node.
    #[rhai_fn(name = "focus_down")]
    pub fn focus_down(_: &mut ActionApi) -> Action {
        Action::FocusDirection {
            direction: NavigationDirection::Down,
        }
    }

    /// Select a tab by index in a specific tabs node.
    #[rhai_fn(return_raw, name = "select_tab")]
    pub fn select_tab(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tabs_node_id: i64,
        index: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SelectTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                index: parse_index(index, "tab index")?,
            })
        })
    }

    /// Select a tab by index in the currently focused tabs node.
    #[rhai_fn(return_raw, name = "select_current_tabs")]
    pub fn select_current_tabs(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        index: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SelectTab {
                tabs_node_id: None,
                index: parse_index(index, "tab index")?,
            })
        })
    }

    /// Select the next tab in a specific tabs node.
    #[rhai_fn(return_raw, name = "next_tab")]
    pub fn next_tab(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tabs_node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::NextTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
            })
        })
    }

    /// Select the next tab in the currently focused tabs node.
    #[rhai_fn(name = "next_current_tabs")]
    pub fn next_current_tabs(_: &mut ActionApi) -> Action {
        Action::NextTab { tabs_node_id: None }
    }

    /// Select the previous tab in a specific tabs node.
    #[rhai_fn(return_raw, name = "prev_tab")]
    pub fn prev_tab(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tabs_node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::PrevTab {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
            })
        })
    }

    /// Select the previous tab in the currently focused tabs node.
    #[rhai_fn(name = "prev_current_tabs")]
    pub fn prev_current_tabs(_: &mut ActionApi) -> Action {
        Action::PrevTab { tabs_node_id: None }
    }

    /// Focus a specific buffer by id.
    #[rhai_fn(return_raw, name = "focus_buffer")]
    pub fn focus_buffer(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::FocusBuffer {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        })
    }

    /// Reveal a specific buffer by id.
    #[rhai_fn(return_raw, name = "reveal_buffer")]
    pub fn reveal_buffer(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::RevealBuffer {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        })
    }

    /// Split the current node and attach the provided tree as the new sibling.
    #[rhai_fn(return_raw, name = "split_with")]
    pub fn split_with(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        direction: &str,
        tree: TreeSpec,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SplitCurrent {
                direction: parse_split_direction(direction)?,
                new_child: tree,
            })
        })
    }

    /// Insert a tab after a specific tabs node.
    #[rhai_fn(return_raw, name = "insert_tab_after")]
    pub fn insert_tab_after(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tabs_node_id: i64,
        title: &str,
        tree: TreeSpec,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::InsertTabAfter {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                title: Some(title.to_owned()),
                child: tree,
            })
        })
    }

    /// Insert a tab after the current tab in the focused tabs node.
    #[rhai_fn(name = "insert_tab_after_current")]
    pub fn insert_tab_after_current(_: &mut ActionApi, title: &str, tree: TreeSpec) -> Action {
        Action::InsertTabAfter {
            tabs_node_id: None,
            title: Some(title.to_owned()),
            child: tree,
        }
    }

    /// Insert a tab before a specific tabs node.
    #[rhai_fn(return_raw, name = "insert_tab_before")]
    pub fn insert_tab_before(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tabs_node_id: i64,
        title: &str,
        tree: TreeSpec,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::InsertTabBefore {
                tabs_node_id: Some(parse_node_id(tabs_node_id)?),
                title: Some(title.to_owned()),
                child: tree,
            })
        })
    }

    /// Insert a tab before the current tab.
    #[rhai_fn(name = "insert_tab_before_current")]
    pub fn insert_tab_before_current(_: &mut ActionApi, title: &str, tree: TreeSpec) -> Action {
        Action::InsertTabBefore {
            tabs_node_id: None,
            title: Some(title.to_owned()),
            child: tree,
        }
    }

    /// Replace the focused node with a new tree.
    #[rhai_fn(name = "replace_current_with")]
    pub fn replace_current_with(_: &mut ActionApi, tree: TreeSpec) -> Action {
        Action::ReplaceNode {
            node_id: None,
            tree,
        }
    }

    /// Replace a specific node by id with a new tree.
    #[rhai_fn(return_raw, name = "replace_node")]
    pub fn replace_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        node_id: i64,
        tree: TreeSpec,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::ReplaceNode {
                node_id: Some(parse_node_id(node_id)?),
                tree,
            })
        })
    }

    /// Open a floating view around the provided tree.
    #[rhai_fn(return_raw, name = "open_floating")]
    pub fn open_floating(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        tree: TreeSpec,
        options: Map,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::OpenFloating {
                spec: parse_floating_spec(tree, options)?,
            })
        })
    }

    /// Close the currently focused floating window.
    #[rhai_fn(name = "close_floating")]
    pub fn close_floating(_: &mut ActionApi) -> Action {
        Action::CloseFloating { floating_id: None }
    }

    /// Close a floating window by id.
    #[rhai_fn(return_raw, name = "close_floating_id")]
    pub fn close_floating_id(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        floating_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::CloseFloating {
                floating_id: Some(parse_floating_id(floating_id)?),
            })
        })
    }

    /// Close the currently focused view.
    #[rhai_fn(name = "close_view")]
    pub fn close_view(_: &mut ActionApi) -> Action {
        Action::CloseView { node_id: None }
    }

    /// Close a view by node id.
    #[rhai_fn(return_raw, name = "close_node")]
    pub fn close_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::CloseView {
                node_id: Some(parse_node_id(node_id)?),
            })
        })
    }

    /// Kill the currently focused buffer.
    #[rhai_fn(name = "kill_buffer")]
    pub fn kill_buffer(_: &mut ActionApi) -> Action {
        Action::KillBuffer { buffer_id: None }
    }

    /// Kill a buffer by id.
    #[rhai_fn(return_raw, name = "kill_buffer_id")]
    pub fn kill_buffer_id(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::KillBuffer {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
            })
        })
    }

    /// Detach the currently focused buffer.
    #[rhai_fn(name = "detach_buffer")]
    pub fn detach_buffer(_: &mut ActionApi) -> Action {
        Action::DetachBuffer { buffer_id: None }
    }

    /// Detach a buffer by id.
    #[rhai_fn(return_raw, name = "detach_buffer_id")]
    pub fn detach_buffer_id(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::DetachBuffer {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
            })
        })
    }

    /// Open the history of a buffer in a new view.
    /// `scope` accepts `visible` or `full`. `placement` accepts `floating` or `tab`.
    /// Example: `action.open_buffer_history(12, "visible", "floating")`.
    #[rhai_fn(return_raw, name = "open_buffer_history")]
    pub fn open_buffer_history(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        scope: &str,
        placement: &str,
    ) -> RhaiResultOf<Action> {
        let position = ctx.call_position();
        with_call_position(ctx, || {
            Ok(Action::OpenBufferHistory {
                buffer_id: parse_buffer_id(buffer_id)?,
                scope: parse_buffer_history_scope(scope, position)?,
                placement: parse_buffer_history_placement(placement, position)?,
            })
        })
    }

    /// Zoom the current node.
    #[rhai_fn(name = "zoom_current_node")]
    pub fn zoom_current_node(_: &mut ActionApi) -> Action {
        Action::ZoomNode { node_id: None }
    }

    /// Unzoom the current session.
    #[rhai_fn(name = "unzoom_current_session")]
    pub fn unzoom_current_session(_: &mut ActionApi) -> Action {
        Action::UnzoomNode { session_id: None }
    }

    /// Toggle zoom on a node.
    #[rhai_fn(return_raw, name = "toggle_zoom_node")]
    pub fn toggle_zoom_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::ToggleZoomNode {
                node_id: Some(parse_node_id(node_id)?),
            })
        })
    }

    /// Swap the current node with a sibling.
    #[rhai_fn(return_raw, name = "swap_current_node")]
    pub fn swap_current_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        second_node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SwapSiblingNodes {
                first_node_id: None,
                second_node_id: parse_node_id(second_node_id)?,
            })
        })
    }

    /// Break the current node into a new tab or floating window.
    /// `destination` accepts `tab` or `floating`.
    /// Example: `action.break_current_node("floating")`.
    #[rhai_fn(return_raw, name = "break_current_node")]
    pub fn break_current_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        destination: &str,
    ) -> RhaiResultOf<Action> {
        let position = ctx.call_position();
        with_call_position(ctx, || {
            Ok(Action::BreakNode {
                node_id: None,
                destination: parse_node_break_destination(destination, position)?,
            })
        })
    }

    /// Join a buffer at the current node.
    /// `placement` accepts `tab-after`, `tab-before`, `left`, `right`, `up`, or `down`.
    /// Example: `action.join_buffer_here(12, "tab-after")`.
    #[rhai_fn(return_raw, name = "join_buffer_here")]
    pub fn join_buffer_here(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        placement: &str,
    ) -> RhaiResultOf<Action> {
        let position = ctx.call_position();
        with_call_position(ctx, || {
            Ok(Action::JoinBufferAtNode {
                node_id: None,
                buffer_id: parse_buffer_id(buffer_id)?,
                placement: parse_node_join_placement(placement, position)?,
            })
        })
    }

    /// Move the current node before a sibling.
    #[rhai_fn(return_raw, name = "move_current_node_before")]
    pub fn move_current_node_before(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        sibling_node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::MoveNodeBefore {
                node_id: None,
                sibling_node_id: parse_node_id(sibling_node_id)?,
            })
        })
    }

    /// Move a node after a sibling.
    #[rhai_fn(return_raw, name = "move_node_after")]
    pub fn move_node_after(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        node_id: i64,
        sibling_node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::MoveNodeAfter {
                node_id: Some(parse_node_id(node_id)?),
                sibling_node_id: parse_node_id(sibling_node_id)?,
            })
        })
    }

    /// Move a buffer into a specific node.
    #[rhai_fn(return_raw, name = "move_buffer_to_node")]
    pub fn move_buffer_to_node(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        node_id: i64,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::MoveBufferToNode {
                buffer_id: parse_buffer_id(buffer_id)?,
                node_id: parse_node_id(node_id)?,
            })
        })
    }

    /// Move a buffer into a new floating window.
    ///
    /// # Options
    ///
    /// - `x` (i16): horizontal offset from the anchor (default: 0)
    /// - `y` (i16): vertical offset from the anchor (default: 0)
    /// - `width` (FloatingSize): window width, as a percentage (e.g., 50%) or pixel value (default: 50%)
    /// - `height` (FloatingSize): window height, as a percentage or pixel value (default: 50%)
    /// - `anchor` (FloatingAnchor): anchor point for positioning, e.g., "top_left", "center" (default: center)
    /// - `title` (Option\<String\>): window title (default: none)
    /// - `focus` (bool): whether to focus the window after creation (default: true)
    /// - `close_on_empty` (bool): whether to close the window when its buffer empties (default: true)
    #[rhai_fn(return_raw, name = "move_buffer_to_floating")]
    pub fn move_buffer_to_floating(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        options: Map,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            let spec = parse_floating_options(options)?;
            Ok(Action::MoveBufferToFloating {
                buffer_id: parse_buffer_id(buffer_id)?,
                geometry: spec.geometry,
                title: spec.title,
                focus: spec.focus,
                close_on_empty: spec.close_on_empty,
            })
        })
    }

    /// Send raw byte values to the focused buffer.
    ///
    /// Use this when you need to emit an exact byte sequence instead of key notation.
    ///
    /// # Example
    ///
    /// ```rhai
    /// // Send the ANSI "cursor up" sequence: ESC [ A
    /// action.send_bytes_current([0x1b, 0x5b, 0x41])
    /// ```
    #[rhai_fn(return_raw, name = "send_bytes_current")]
    pub fn send_bytes_current(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        bytes: Array,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SendBytes {
                buffer_id: None,
                bytes: parse_bytes(bytes)?,
            })
        })
    }

    /// Send a string of bytes to the focused buffer.
    #[rhai_fn(name = "send_bytes_current")]
    pub fn send_bytes_current_string(_: &mut ActionApi, bytes: &str) -> Action {
        Action::SendBytes {
            buffer_id: None,
            bytes: bytes.as_bytes().to_vec(),
        }
    }

    /// Send a string of bytes to a specific buffer.
    #[rhai_fn(return_raw, name = "send_bytes")]
    pub fn send_bytes_string(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        bytes: &str,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SendBytes {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                bytes: bytes.as_bytes().to_vec(),
            })
        })
    }

    /// Send raw byte values to a specific buffer.
    #[rhai_fn(return_raw, name = "send_bytes")]
    pub fn send_bytes_array(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        bytes: Array,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::SendBytes {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                bytes: parse_bytes(bytes)?,
            })
        })
    }

    /// Send a key notation sequence to the focused buffer.
    #[rhai_fn(return_raw, name = "send_keys_current")]
    pub fn send_keys_current(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        notation: &str,
    ) -> RhaiResultOf<Action> {
        let position = ctx.call_position();
        with_call_position(ctx, || {
            Ok(Action::SendKeys {
                buffer_id: None,
                keys: parse_key_sequence(notation)
                    .map_err(|error| runtime_error_at(error.to_string(), position))?,
            })
        })
    }

    /// Send a key notation sequence to a specific buffer.
    #[rhai_fn(return_raw, name = "send_keys")]
    pub fn send_keys(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        buffer_id: i64,
        notation: &str,
    ) -> RhaiResultOf<Action> {
        let position = ctx.call_position();
        with_call_position(ctx, || {
            Ok(Action::SendKeys {
                buffer_id: Some(parse_buffer_id(buffer_id)?),
                keys: parse_key_sequence(notation)
                    .map_err(|error| runtime_error_at(error.to_string(), position))?,
            })
        })
    }

    /// Scroll one page upward in local scrollback.
    #[rhai_fn(name = "scroll_page_up")]
    pub fn scroll_page_up(_: &mut ActionApi) -> Action {
        Action::ScrollPageUp
    }

    /// Scroll one page downward in local scrollback.
    #[rhai_fn(name = "scroll_page_down")]
    pub fn scroll_page_down(_: &mut ActionApi) -> Action {
        Action::ScrollPageDown
    }

    /// Scroll one line upward in local scrollback.
    #[rhai_fn(name = "scroll_line_up")]
    pub fn scroll_line_up(_: &mut ActionApi) -> Action {
        Action::ScrollLineUp
    }

    /// Scroll one line downward in local scrollback.
    #[rhai_fn(name = "scroll_line_down")]
    pub fn scroll_line_down(_: &mut ActionApi) -> Action {
        Action::ScrollLineDown
    }

    /// Scroll to the top of local scrollback.
    #[rhai_fn(name = "scroll_to_top")]
    pub fn scroll_to_top(_: &mut ActionApi) -> Action {
        Action::ScrollToTop
    }

    /// Scroll to the bottom of local scrollback.
    #[rhai_fn(name = "scroll_to_bottom")]
    pub fn scroll_to_bottom(_: &mut ActionApi) -> Action {
        Action::ScrollToBottom
    }

    /// Re-enable following live output.
    #[rhai_fn(name = "follow_output")]
    pub fn follow_output(_: &mut ActionApi) -> Action {
        Action::FollowOutput
    }

    /// Enter incremental search mode.
    #[rhai_fn(name = "enter_search_mode")]
    pub fn enter_search_mode(_: &mut ActionApi) -> Action {
        Action::EnterSearchMode
    }

    /// Cancel the active search.
    #[rhai_fn(name = "cancel_search")]
    pub fn cancel_search(_: &mut ActionApi) -> Action {
        Action::CancelSearch
    }

    /// Commit the active search.
    #[rhai_fn(name = "commit_search")]
    pub fn commit_search(_: &mut ActionApi) -> Action {
        Action::CommitSearch
    }

    /// Jump to the next search match.
    #[rhai_fn(name = "search_next")]
    pub fn search_next(_: &mut ActionApi) -> Action {
        Action::SearchNext
    }

    /// Jump to the previous search match.
    #[rhai_fn(name = "search_prev")]
    pub fn search_prev(_: &mut ActionApi) -> Action {
        Action::SearchPrev
    }

    /// Enter character selection mode.
    #[rhai_fn(name = "enter_select_char")]
    pub fn enter_select_char(_: &mut ActionApi) -> Action {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Character,
        }
    }

    /// Enter line selection mode.
    #[rhai_fn(name = "enter_select_line")]
    pub fn enter_select_line(_: &mut ActionApi) -> Action {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Line,
        }
    }

    /// Enter block selection mode.
    #[rhai_fn(name = "enter_select_block")]
    pub fn enter_select_block(_: &mut ActionApi) -> Action {
        Action::EnterSelect {
            kind: crate::state::SelectionKind::Block,
        }
    }

    /// Move the active selection left.
    #[rhai_fn(name = "select_move_left")]
    pub fn select_move_left(_: &mut ActionApi) -> Action {
        Action::SelectMove {
            direction: NavigationDirection::Left,
        }
    }

    /// Move the active selection right.
    #[rhai_fn(name = "select_move_right")]
    pub fn select_move_right(_: &mut ActionApi) -> Action {
        Action::SelectMove {
            direction: NavigationDirection::Right,
        }
    }

    /// Move the active selection up.
    #[rhai_fn(name = "select_move_up")]
    pub fn select_move_up(_: &mut ActionApi) -> Action {
        Action::SelectMove {
            direction: NavigationDirection::Up,
        }
    }

    /// Move the active selection down.
    #[rhai_fn(name = "select_move_down")]
    pub fn select_move_down(_: &mut ActionApi) -> Action {
        Action::SelectMove {
            direction: NavigationDirection::Down,
        }
    }

    /// Copy the current selection into the clipboard.
    #[rhai_fn(name = "yank_selection")]
    pub fn yank_selection(_: &mut ActionApi) -> Action {
        Action::CopySelection
    }

    /// Copy the current selection into the clipboard.
    #[rhai_fn(name = "copy_selection")]
    pub fn copy_selection(_: &mut ActionApi) -> Action {
        Action::CopySelection
    }

    /// Cancel the current selection.
    #[rhai_fn(name = "cancel_selection")]
    pub fn cancel_selection(_: &mut ActionApi) -> Action {
        Action::CancelSelection
    }

    /// Emit a client notification.
    #[rhai_fn(return_raw, name = "notify")]
    pub fn notify(
        ctx: NativeCallContext,
        _: &mut ActionApi,
        level: &str,
        message: &str,
    ) -> RhaiResultOf<Action> {
        with_call_position(ctx, || {
            Ok(Action::Notify {
                level: parse_notify_level(level)?,
                message: message.to_owned(),
            })
        })
    }

    /// Run another named action by name.
    #[rhai_fn(name = "run_named_action")]
    pub fn run_named_action(_: &mut ActionApi, name: &str) -> Action {
        Action::RunNamedAction {
            name: name.to_owned(),
        }
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_tree_api {
    use super::{
        Array, Dynamic, Map, NativeCallContext, SplitDirection, TabSpec, TreeApi, TreeSpec,
        build_split, build_tabs, parse_buffer_id, parse_buffer_spawn, parse_index, parse_sizes,
        parse_split_direction, with_call_position,
    };

    /// Build a tree reference to the currently focused buffer.
    #[rhai_fn(name = "buffer_current")]
    pub fn buffer_current(_: &mut TreeApi) -> TreeSpec {
        TreeSpec::BufferCurrent
    }

    /// Build a tree reference to the currently focused buffer.
    #[rhai_fn(name = "current_buffer")]
    pub fn current_buffer(_: &mut TreeApi) -> TreeSpec {
        TreeSpec::BufferCurrent
    }

    /// Build a tree reference to the currently focused node.
    #[rhai_fn(name = "current_node")]
    pub fn current_node(_: &mut TreeApi) -> TreeSpec {
        TreeSpec::CurrentNode
    }

    /// Build an empty buffer tree node.
    #[rhai_fn(name = "buffer_empty")]
    pub fn buffer_empty(_: &mut TreeApi) -> TreeSpec {
        TreeSpec::BufferEmpty
    }

    /// Attach an existing buffer by id.
    #[rhai_fn(return_raw, name = "buffer_attach")]
    pub fn buffer_attach(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        buffer_id: i64,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            Ok(TreeSpec::BufferAttach {
                buffer_id: parse_buffer_id(buffer_id)?,
            })
        })
    }

    /// Spawn a new buffer from a command array.
    ///
    /// Supported `options` keys are `title` (`string`), `cwd` (`string`), and `env`
    /// (`map<string, string>`). Unknown keys are rejected.
    ///
    /// # Example
    ///
    /// ```rhai
    /// tree.buffer_spawn(["/bin/zsh"], #{ title: "shell" })
    /// ```
    #[rhai_fn(return_raw, name = "buffer_spawn")]
    pub fn buffer_spawn_simple(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        command: Array,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            Ok(TreeSpec::BufferSpawn(super::BufferSpawnSpec {
                title: None,
                command: super::parse_string_array(Dynamic::from(command))?,
                cwd: None,
                env: Default::default(),
            }))
        })
    }

    #[rhai_fn(return_raw, name = "buffer_spawn")]
    pub fn buffer_spawn(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        command: Array,
        options: Map,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            Ok(TreeSpec::BufferSpawn(parse_buffer_spawn(command, options)?))
        })
    }

    /// Build a single tab specification.
    #[rhai_fn(name = "tab")]
    pub fn tab(_: &mut TreeApi, title: &str, tree: TreeSpec) -> TabSpec {
        TabSpec {
            title: title.to_owned(),
            tree: Box::new(tree),
        }
    }

    /// Build a tabs container with the first tab active.
    #[rhai_fn(return_raw, name = "tabs")]
    pub fn tabs(ctx: NativeCallContext, _: &mut TreeApi, tabs: Array) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || build_tabs(tabs, 0))
    }

    /// Build a tabs container with an explicit active tab.
    #[rhai_fn(return_raw, name = "tabs_with_active")]
    pub fn tabs_with_active(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        tabs: Array,
        active: i64,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || build_tabs(tabs, parse_index(active, "active tab")?))
    }

    /// Build a horizontal split.
    #[rhai_fn(return_raw, name = "split_h")]
    pub fn split_h(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        children: Array,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            build_split(SplitDirection::Horizontal, children, Vec::new())
        })
    }

    /// Build a vertical split.
    #[rhai_fn(return_raw, name = "split_v")]
    pub fn split_v(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        children: Array,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            build_split(SplitDirection::Vertical, children, Vec::new())
        })
    }

    /// Build a split with an explicit direction string.
    #[rhai_fn(return_raw, name = "split")]
    pub fn split(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        direction: &str,
        children: Array,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            build_split(parse_split_direction(direction)?, children, Vec::new())
        })
    }

    /// Build a split with explicit sizes for each child.
    #[rhai_fn(return_raw, name = "split")]
    pub fn split_with_sizes(
        ctx: NativeCallContext,
        _: &mut TreeApi,
        direction: &str,
        children: Array,
        sizes: Array,
    ) -> RhaiResultOf<TreeSpec> {
        with_call_position(ctx, || {
            build_split(
                parse_split_direction(direction)?,
                children,
                parse_sizes(sizes)?,
            )
        })
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_system_api {
    use super::{Dynamic, SystemApi, which};

    /// Read an environment variable, if it is set.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "env")]
    pub fn env(_: &mut SystemApi, name: &str) -> Dynamic {
        std::env::var(name)
            .ok()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    /// Resolve an executable from `PATH`, if it is found.
    ///
    /// ReturnType: `string | ()`
    #[rhai_fn(name = "which")]
    pub fn which_fn(_: &mut SystemApi, name: &str) -> Dynamic {
        which(name)
            .map(|path| Dynamic::from(path.display().to_string()))
            .unwrap_or(Dynamic::UNIT)
    }

    /// Return the current Unix timestamp in seconds.
    #[rhai_fn(name = "now")]
    pub fn now(_: &mut SystemApi) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_ui_api {
    use super::{
        Array, BarSegment, BarSpec, Map, NativeCallContext, StyleSpec, UiApi, parse_bar_segments,
        parse_segment_options, with_call_position,
    };

    /// Create a [`BarSegment`] from a [`UiApi`] receiver and text using default styling.
    ///
    /// `segment(_: UiApi, text: String) -> BarSegment` produces plain text with default
    /// [`StyleSpec`] values and no click target.
    ///
    /// The overloaded `segment(_: UiApi, text: String, options: Map) -> BarSegment` supports
    /// `fg`, `bg`, `bold`, `italic`, `underline`, `dim`, `blink`, and `target` keys to override
    /// styling and attach an optional interaction target. `dim` is a boolean that renders the
    /// text with reduced intensity for a muted appearance, and `blink` is a boolean that enables
    /// blinking text for that segment.
    #[rhai_fn(name = "segment")]
    pub fn segment(_: &mut UiApi, text: &str) -> BarSegment {
        BarSegment {
            text: text.to_owned(),
            style: StyleSpec::default(),
            target: None,
        }
    }

    /// Create a [`BarSegment`] from a [`UiApi`] receiver, text, and an `options: Map`.
    ///
    /// `segment(_: UiApi, text: String, options: Map) -> BarSegment` supports `fg`, `bg`,
    /// `bold`, `italic`, `underline`, `dim`, `blink`, and `target` keys to override styling and
    /// attach an optional interaction target. `dim` is a boolean that renders the text with
    /// reduced intensity for a muted appearance, and `blink` is a boolean that enables blinking
    /// text for that segment.
    #[rhai_fn(return_raw, name = "segment")]
    pub fn segment_with_options(
        ctx: NativeCallContext,
        _: &mut UiApi,
        text: &str,
        options: Map,
    ) -> RhaiResultOf<BarSegment> {
        with_call_position(ctx, || {
            let (style, target) = parse_segment_options(options)?;
            Ok(BarSegment {
                text: text.to_owned(),
                style,
                target,
            })
        })
    }

    /// Build a full bar specification from left, center, and right segments.
    #[rhai_fn(return_raw, name = "bar")]
    pub fn bar(
        ctx: NativeCallContext,
        _: &mut UiApi,
        left: Array,
        center: Array,
        right: Array,
    ) -> RhaiResultOf<BarSpec> {
        with_call_position(ctx, || {
            Ok(BarSpec {
                left: parse_bar_segments(left)?,
                center: parse_bar_segments(center)?,
                right: parse_bar_segments(right)?,
            })
        })
    }
}

#[allow(dead_code)]
#[export_module]
mod documented_theme_runtime_api {
    use super::{Dynamic, ThemeRuntimeApi};

    /// Read a named color from the active runtime palette, if it exists.
    ///
    /// ReturnType: `RgbColor | ()`
    #[rhai_fn(name = "color")]
    pub fn color(theme: &mut ThemeRuntimeApi, name: &str) -> Dynamic {
        theme
            .theme
            .palette
            .get(name)
            .copied()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }
}

fn build_split(
    direction: SplitDirection,
    children: Array,
    sizes: Vec<u16>,
) -> ScriptResult<TreeSpec> {
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

fn build_tabs(tabs: Array, active: usize) -> ScriptResult<TreeSpec> {
    let tabs = parse_tabs(tabs)?;
    TabsSpec::try_new(tabs, active)
        .map(TreeSpec::Tabs)
        .map_err(runtime_error)
}

fn parse_tabs(tabs: Array) -> ScriptResult<Vec<TabSpec>> {
    let mut parsed = Vec::with_capacity(tabs.len());
    for tab in tabs {
        let Some(tab) = tab.try_cast::<TabSpec>() else {
            return Err(runtime_error("expected TabSpec values"));
        };
        parsed.push(tab);
    }
    Ok(parsed)
}

fn parse_tree_array(children: Array) -> ScriptResult<Vec<TreeSpec>> {
    let mut parsed = Vec::with_capacity(children.len());
    for child in children {
        let Some(tree) = child.try_cast::<TreeSpec>() else {
            return Err(runtime_error("expected TreeSpec values"));
        };
        parsed.push(tree);
    }
    Ok(parsed)
}

fn parse_action_array(actions: Array) -> ScriptResult<Vec<Action>> {
    let mut parsed = Vec::with_capacity(actions.len());
    for action in actions {
        let Some(action) = action.try_cast::<Action>() else {
            return Err(runtime_error("expected Action values"));
        };
        parsed.push(action);
    }
    Ok(parsed)
}

fn parse_bar_segments(segments: Array) -> ScriptResult<Vec<BarSegment>> {
    let mut parsed = Vec::with_capacity(segments.len());
    for segment in segments {
        let Some(segment) = segment.try_cast::<BarSegment>() else {
            return Err(runtime_error("ui.bar expects BarSegment values"));
        };
        parsed.push(segment);
    }
    Ok(parsed)
}

fn parse_buffer_spawn(command: Array, mut options: Map) -> ScriptResult<BufferSpawnSpec> {
    let parsed = BufferSpawnSpec {
        title: parse_optional_string(options.remove("title"))?,
        command: parse_string_array(Dynamic::from(command))?,
        cwd: parse_optional_string(options.remove("cwd"))?,
        env: parse_string_map(options.remove("env"))?,
    };
    if !options.is_empty() {
        return Err(runtime_error(format!(
            "unknown buffer_spawn option(s): {}",
            unexpected_option_keys(&options)
        )));
    }
    Ok(parsed)
}

fn parse_floating_spec(tree: TreeSpec, options: Map) -> ScriptResult<FloatingSpec> {
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

fn parse_floating_options(mut options: Map) -> ScriptResult<ParsedFloatingOptions> {
    let parsed = ParsedFloatingOptions {
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
    };
    if !options.is_empty() {
        return Err(runtime_error(format!(
            "unknown floating option(s): {}",
            unexpected_option_keys(&options)
        )));
    }
    Ok(parsed)
}

fn parse_segment_options(mut options: Map) -> ScriptResult<(StyleSpec, Option<BarTarget>)> {
    let parsed = (
        StyleSpec {
            fg: parse_optional_color(options.remove("fg"))?,
            bg: parse_optional_color(options.remove("bg"))?,
            bold: parse_bool_field(options.remove("bold"))?.unwrap_or(false),
            italic: parse_bool_field(options.remove("italic"))?.unwrap_or(false),
            underline: parse_bool_field(options.remove("underline"))?.unwrap_or(false),
            dim: parse_bool_field(options.remove("dim"))?.unwrap_or(false),
            blink: parse_bool_field(options.remove("blink"))?.unwrap_or(false),
        },
        parse_bar_target(options.remove("target"))?,
    );
    if !options.is_empty() {
        return Err(runtime_error(format!(
            "unknown segment option(s): {}",
            unexpected_option_keys(&options)
        )));
    }
    Ok(parsed)
}

fn unexpected_option_keys(options: &Map) -> String {
    let mut keys = options.keys().map(ToString::to_string).collect::<Vec<_>>();
    keys.sort();
    keys.join(", ")
}

fn parse_bar_target(value: Option<Dynamic>) -> ScriptResult<Option<BarTarget>> {
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
    let parsed = match kind.as_str() {
        "tab" => BarTarget::Tab {
            tabs_node_id: parse_node_id(parse_required_i64(&mut target, "tabs_node_id")?)?,
            index: parse_index(parse_required_i64(&mut target, "index")?, "target index")?,
        },
        "floating" => BarTarget::Floating {
            floating_id: parse_floating_id(parse_required_i64(&mut target, "floating_id")?)?,
        },
        "buffer" => BarTarget::Buffer {
            buffer_id: parse_buffer_id(parse_required_i64(&mut target, "buffer_id")?)?,
        },
        _ => return Err(runtime_error(format!("unknown bar target kind '{kind}'"))),
    };
    if !target.is_empty() {
        return Err(runtime_error(format!(
            "unknown bar target option(s): {}",
            unexpected_option_keys(&target)
        )));
    }
    Ok(Some(parsed))
}

fn parse_optional_color(value: Option<Dynamic>) -> ScriptResult<Option<RgbColor>> {
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

fn parse_sizes(values: Array) -> ScriptResult<Vec<u16>> {
    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value.try_cast::<i64>() else {
            return Err(runtime_error("split sizes must be integers"));
        };
        parsed.push(parse_amount(value, "split size")?);
    }
    Ok(parsed)
}

fn parse_string_array(value: Dynamic) -> ScriptResult<Vec<String>> {
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
) -> ScriptResult<std::collections::BTreeMap<String, String>> {
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

fn parse_optional_string(value: Option<Dynamic>) -> ScriptResult<Option<String>> {
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

fn parse_required_string(options: &mut Map, key: &str) -> ScriptResult<String> {
    parse_optional_string(options.remove(key))?
        .ok_or_else(|| runtime_error(format!("missing '{key}' field")))
}

fn parse_required_i64(options: &mut Map, key: &str) -> ScriptResult<i64> {
    let value = options
        .remove(key)
        .ok_or_else(|| runtime_error(format!("missing '{key}' field")))?;
    value
        .try_cast::<i64>()
        .ok_or_else(|| runtime_error(format!("'{key}' must be an integer")))
}

fn parse_bool_field(value: Option<Dynamic>) -> ScriptResult<Option<bool>> {
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

fn parse_i16_field(value: Option<Dynamic>, label: &str) -> ScriptResult<Option<i16>> {
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

fn parse_floating_size(value: Option<Dynamic>) -> ScriptResult<Option<FloatingSize>> {
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

fn parse_floating_anchor(value: Option<Dynamic>) -> ScriptResult<Option<FloatingAnchor>> {
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

fn parse_bytes(bytes: Array) -> ScriptResult<Vec<u8>> {
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

fn parse_count(value: i64, label: &str) -> ScriptResult<usize> {
    if value < 0 {
        return Err(runtime_error(format!("{label} must be zero or greater")));
    }
    usize::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_amount(value: i64, label: &str) -> ScriptResult<u16> {
    if value <= 0 {
        return Err(runtime_error(format!("{label} must be greater than zero")));
    }
    u16::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_index(value: i64, label: &str) -> ScriptResult<usize> {
    if value < 0 {
        return Err(runtime_error(format!("{label} must be zero or greater")));
    }
    usize::try_from(value).map_err(|_| runtime_error(format!("{label} is too large")))
}

fn parse_buffer_id(value: i64) -> ScriptResult<BufferId> {
    if value < 0 {
        return Err(runtime_error("buffer id must be zero or greater"));
    }
    Ok(BufferId(value as u64))
}

fn parse_node_id(value: i64) -> ScriptResult<NodeId> {
    if value < 0 {
        return Err(runtime_error("node id must be zero or greater"));
    }
    Ok(NodeId(value as u64))
}

fn parse_floating_id(value: i64) -> ScriptResult<FloatingId> {
    if value < 0 {
        return Err(runtime_error("floating id must be zero or greater"));
    }
    Ok(FloatingId(value as u64))
}

fn parse_notify_level(value: &str) -> ScriptResult<NotifyLevel> {
    match value {
        "info" => Ok(NotifyLevel::Info),
        "warn" => Ok(NotifyLevel::Warn),
        "error" => Ok(NotifyLevel::Error),
        _ => Err(runtime_error(format!("unknown notify level '{value}'"))),
    }
}

fn parse_split_direction(value: &str) -> ScriptResult<SplitDirection> {
    match value.to_ascii_lowercase().as_str() {
        "h" | "horizontal" => Ok(SplitDirection::Horizontal),
        "v" | "vertical" => Ok(SplitDirection::Vertical),
        _ => Err(runtime_error(format!("unknown split direction '{value}'"))),
    }
}

fn parse_buffer_history_scope(value: &str, position: Position) -> RhaiResultOf<BufferHistoryScope> {
    match value {
        "visible" => Ok(BufferHistoryScope::Visible),
        "full" => Ok(BufferHistoryScope::Full),
        _ => Err(runtime_error_at(
            format!("invalid scope: {value}"),
            position,
        )),
    }
}

fn parse_buffer_history_placement(
    value: &str,
    position: Position,
) -> RhaiResultOf<BufferHistoryPlacement> {
    match value {
        "floating" => Ok(BufferHistoryPlacement::Floating),
        "tab" => Ok(BufferHistoryPlacement::Tab),
        _ => Err(runtime_error_at(
            format!("invalid placement: {value}"),
            position,
        )),
    }
}

fn parse_node_break_destination(
    value: &str,
    position: Position,
) -> RhaiResultOf<NodeBreakDestination> {
    match value {
        "tab" => Ok(NodeBreakDestination::Tab),
        "floating" => Ok(NodeBreakDestination::Floating),
        _ => Err(runtime_error_at(
            format!("invalid destination: {value}"),
            position,
        )),
    }
}

fn parse_node_join_placement(value: &str, position: Position) -> RhaiResultOf<NodeJoinPlacement> {
    match value {
        "tab-after" => Ok(NodeJoinPlacement::TabAfter),
        "tab-before" => Ok(NodeJoinPlacement::TabBefore),
        "left" => Ok(NodeJoinPlacement::Left),
        "right" => Ok(NodeJoinPlacement::Right),
        "up" => Ok(NodeJoinPlacement::Up),
        "down" => Ok(NodeJoinPlacement::Down),
        _ => Err(runtime_error_at(
            format!("invalid placement: {value}"),
            position,
        )),
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
        let Some(metadata) = candidate.metadata().ok() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }
        #[cfg(not(unix))]
        {
            return Some(candidate);
        }
        #[cfg(unix)]
        return Some(candidate);
    }
    None
}

fn with_call_position<T>(
    ctx: NativeCallContext<'_>,
    build: impl FnOnce() -> RhaiResultOf<T>,
) -> RhaiResultOf<T> {
    let position = ctx.call_position();
    build().map_err(|mut error| {
        error.set_position(position);
        error
    })
}

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    runtime_error_at(message, Position::NONE)
}

fn runtime_error_at(message: impl Into<String>, position: Position) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), position).into()
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Dynamic, Map, StyleSpec, parse_bar_target, parse_buffer_history_placement,
        parse_buffer_history_scope, parse_buffer_spawn, parse_node_break_destination,
        parse_node_join_placement, parse_notify_level, parse_segment_options,
        parse_split_direction, register_documented_registration_runtime_api, registration_scope,
    };
    use embers_core::BufferId;
    use embers_protocol::{
        BufferHistoryPlacement, BufferHistoryScope, NodeBreakDestination, NodeJoinPlacement,
    };
    use rhai::{Engine, Position};

    fn eval_action(script: &str) -> Result<Action, Box<rhai::EvalAltResult>> {
        let mut engine = Engine::new();
        register_documented_registration_runtime_api(&mut engine);
        let mut scope = registration_scope();
        engine.eval_with_scope(&mut scope, script)
    }

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

    #[test]
    fn parse_history_and_node_enums_accept_known_literals() {
        let position = Position::NONE;
        for (literal, scope) in [
            ("visible", BufferHistoryScope::Visible),
            ("full", BufferHistoryScope::Full),
        ] {
            assert!(
                parse_buffer_history_scope(literal, position).is_ok(),
                "scope literal {literal:?} should parse"
            );
            assert_eq!(
                eval_action(&format!(
                    "action.open_buffer_history(12, {literal:?}, \"floating\")"
                ))
                .expect("history builder should parse"),
                Action::OpenBufferHistory {
                    buffer_id: BufferId(12),
                    scope,
                    placement: BufferHistoryPlacement::Floating,
                }
            );
        }
        for (literal, placement) in [
            ("floating", BufferHistoryPlacement::Floating),
            ("tab", BufferHistoryPlacement::Tab),
        ] {
            assert!(
                parse_buffer_history_placement(literal, position).is_ok(),
                "placement literal {literal:?} should parse"
            );
            assert_eq!(
                eval_action(&format!(
                    "action.open_buffer_history(12, \"full\", {literal:?})"
                ))
                .expect("history placement builder should parse"),
                Action::OpenBufferHistory {
                    buffer_id: BufferId(12),
                    scope: BufferHistoryScope::Full,
                    placement,
                }
            );
        }
        for (literal, destination) in [
            ("tab", NodeBreakDestination::Tab),
            ("floating", NodeBreakDestination::Floating),
        ] {
            assert!(
                parse_node_break_destination(literal, position).is_ok(),
                "break destination literal {literal:?} should parse"
            );
            assert_eq!(
                eval_action(&format!("action.break_current_node({literal:?})"))
                    .expect("break builder should parse"),
                Action::BreakNode {
                    node_id: None,
                    destination,
                }
            );
        }
        for (literal, placement) in [
            ("tab-after", NodeJoinPlacement::TabAfter),
            ("tab-before", NodeJoinPlacement::TabBefore),
            ("left", NodeJoinPlacement::Left),
            ("right", NodeJoinPlacement::Right),
            ("up", NodeJoinPlacement::Up),
            ("down", NodeJoinPlacement::Down),
        ] {
            assert!(
                parse_node_join_placement(literal, position).is_ok(),
                "join placement literal {literal:?} should parse"
            );
            assert_eq!(
                eval_action(&format!("action.join_buffer_here(12, {literal:?})"))
                    .expect("join builder should parse"),
                Action::JoinBufferAtNode {
                    node_id: None,
                    buffer_id: BufferId(12),
                    placement,
                }
            );
        }

        for literal in ["", "invalid"] {
            assert!(
                parse_buffer_history_scope(literal, position)
                    .expect_err("invalid scope should fail")
                    .to_string()
                    .contains(&format!("invalid scope: {literal}"))
            );
            assert!(
                parse_buffer_history_placement(literal, position)
                    .expect_err("invalid placement should fail")
                    .to_string()
                    .contains(&format!("invalid placement: {literal}"))
            );
            assert!(
                eval_action(&format!(
                    "action.open_buffer_history(12, {literal:?}, \"floating\")"
                ))
                .expect_err("history scope builder should fail")
                .to_string()
                .contains(&format!("invalid scope: {literal}"))
            );
            assert!(
                eval_action(&format!(
                    "action.open_buffer_history(12, \"full\", {literal:?})"
                ))
                .expect_err("history placement builder should fail")
                .to_string()
                .contains(&format!("invalid placement: {literal}"))
            );
            assert!(
                parse_node_break_destination(literal, position)
                    .expect_err("invalid destination should fail")
                    .to_string()
                    .contains(&format!("invalid destination: {literal}"))
            );
            assert!(
                eval_action(&format!("action.break_current_node({literal:?})"))
                    .expect_err("break builder should fail")
                    .to_string()
                    .contains(&format!("invalid destination: {literal}"))
            );
            assert!(
                parse_node_join_placement(literal, position)
                    .expect_err("invalid join placement should fail")
                    .to_string()
                    .contains(&format!("invalid placement: {literal}"))
            );
            assert!(
                eval_action(&format!("action.join_buffer_here(12, {literal:?})"))
                    .expect_err("join builder should fail")
                    .to_string()
                    .contains(&format!("invalid placement: {literal}"))
            );
        }
    }

    #[test]
    fn parse_buffer_spawn_rejects_unknown_options() {
        let command = vec![Dynamic::from("/bin/sh")];
        let mut options = Map::new();
        options.insert("bogus".into(), Dynamic::TRUE);

        let error = parse_buffer_spawn(command, options).expect_err("unknown option should fail");
        assert_eq!(
            error.to_string(),
            "Runtime error: unknown buffer_spawn option(s): bogus"
        );
    }

    #[test]
    fn parse_bar_target_rejects_unknown_options() {
        let mut target = Map::new();
        target.insert("kind".into(), Dynamic::from("buffer"));
        target.insert("buffer_id".into(), Dynamic::from(7_i64));
        target.insert("bogus".into(), Dynamic::TRUE);

        let error = parse_bar_target(Some(Dynamic::from(target)))
            .expect_err("unknown target option should fail");
        assert_eq!(
            error.to_string(),
            "Runtime error: unknown bar target option(s): bogus"
        );
    }

    #[test]
    fn parse_segment_options_preserves_blink_flag() {
        let mut options = Map::new();
        options.insert("blink".into(), Dynamic::TRUE);

        let (style, target) = parse_segment_options(options).expect("segment options parse");
        assert_eq!(
            style,
            StyleSpec {
                blink: true,
                ..StyleSpec::default()
            }
        );
        assert_eq!(target, None);
    }
}
