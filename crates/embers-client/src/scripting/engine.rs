use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rhai::plugin::*;
use rhai::{
    Array, CallFnOptions, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map,
    NativeCallContext, Position,
};

use crate::config::ConfigOrigin;
use crate::config::LoadedConfigSource;
use crate::input::{
    BindingSpec, FallbackPolicy, KeySequence, ModeSpec, builtin_modes, expand_leader,
    parse_key_sequence,
};

use super::error::ScriptError;
use super::runtime::{
    normalize_actions, normalize_bar, register_runtime_api, registration_scope, runtime_scope,
};
use super::types::{
    LoadedConfig, ModeHooks, MouseSettings, RgbColor, ScriptFunctionRef, ThemeSpec,
};
use super::{Action, Context, RhaiResultOf, ScriptResult, TabBarContext};
type SharedRegistration = Arc<Mutex<RegistrationState>>;

thread_local! {
    /// [`ACTIVE_REGISTRATION`] holds the current [`SharedRegistration`] for registration-time
    /// Rhai callbacks on this thread. It is managed by [`with_active_registration`] and consumed
    /// by [`clone_active_registration`].
    static ACTIVE_REGISTRATION: RefCell<Option<SharedRegistration>> = const { RefCell::new(None) };
}

pub struct ScriptEngine {
    engine: Engine,
    loaded: LoadedConfig,
}

impl ScriptEngine {
    pub fn load(source: &LoadedConfigSource) -> Result<Self, ScriptError> {
        Self::load_with_overlay("", source)
    }

    pub fn load_with_overlay(
        builtins: &str,
        source: &LoadedConfigSource,
    ) -> Result<Self, ScriptError> {
        let registration = Arc::new(Mutex::new(RegistrationState::default()));
        let mut engine = Engine::new();
        engine.set_max_expr_depths(256, 256);
        engine.set_max_operations(1_000_000);
        register_api(&mut engine);
        register_runtime_api(&mut engine);

        let mut scope = registration_scope();
        scope.push("tabbar", TabbarApi::new());
        scope.push("theme", ThemeApi::new());
        scope.push("mouse", MouseApi::new());

        if !builtins.is_empty() {
            let builtins_source = LoadedConfigSource {
                origin: ConfigOrigin::BuiltIn,
                path: None,
                source: builtins.to_owned(),
                source_hash: 0,
            };
            let builtins_ast = engine
                .compile(builtins)
                .map_err(|error| ScriptError::compile(&builtins_source, error))?;
            let _ = with_active_registration(&registration, || {
                engine.eval_ast_with_scope::<Dynamic>(&mut scope, &builtins_ast)
            })
            .map_err(|error| ScriptError::runtime(&builtins_source, error))?;
        }

        let ast = engine
            .compile(&source.source)
            .map_err(|error| ScriptError::compile(source, error))?;

        let _ = with_active_registration(&registration, || {
            engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
        })
        .map_err(|error| ScriptError::runtime(source, error))?;

        let loaded = registration
            .lock()
            .expect("registration lock")
            .clone()
            .build_loaded_config(source, ast)?;

        Ok(Self { engine, loaded })
    }

    pub fn loaded_config(&self) -> &LoadedConfig {
        &self.loaded
    }

    pub fn has_action(&self, name: &str) -> bool {
        self.loaded.has_action(name)
    }

    pub fn has_event_handlers(&self, event: &str) -> bool {
        self.loaded.has_event_handlers(event)
    }

    pub fn has_tab_bar_formatter(&self) -> bool {
        self.loaded.has_tab_bar_formatter()
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn run_named_action(
        &self,
        name: &str,
        context: Context,
    ) -> Result<Vec<Action>, ScriptError> {
        let callback = self.loaded.named_actions.get(name).ok_or_else(|| {
            ScriptError::validation_path(
                self.loaded.source_path.as_deref(),
                Position::NONE,
                format!("unknown named action '{name}'"),
            )
        })?;
        self.invoke_action_function(&callback.name, context)
    }

    pub fn dispatch_event(
        &self,
        event: &str,
        context: Context,
    ) -> Result<Vec<Action>, ScriptError> {
        let Some(handlers) = self.loaded.event_handlers.get(event) else {
            return Ok(Vec::new());
        };

        let mut actions = Vec::new();
        for handler in handlers {
            actions.extend(self.invoke_action_function(&handler.name, context.clone())?);
        }
        Ok(actions)
    }

    pub fn run_enter_hook(&self, mode: &str, context: Context) -> Result<Vec<Action>, ScriptError> {
        self.run_mode_hook(mode, ModeHook::Enter, context)
    }

    pub fn run_leave_hook(&self, mode: &str, context: Context) -> Result<Vec<Action>, ScriptError> {
        self.run_mode_hook(mode, ModeHook::Leave, context)
    }

    fn run_mode_hook(
        &self,
        mode: &str,
        hook: ModeHook,
        context: Context,
    ) -> Result<Vec<Action>, ScriptError> {
        let Some(hooks) = self.loaded.mode_hooks.get(mode) else {
            return Ok(Vec::new());
        };
        let callback = match hook {
            ModeHook::Enter => hooks.on_enter.as_ref(),
            ModeHook::Leave => hooks.on_leave.as_ref(),
        };
        let Some(callback) = callback else {
            return Ok(Vec::new());
        };
        self.invoke_action_function(&callback.name, context)
    }

    pub fn format_tab_bar(
        &self,
        bar_context: TabBarContext,
    ) -> Result<Option<super::BarSpec>, ScriptError> {
        let Some(formatter) = &self.loaded.tab_bar_formatter else {
            return Ok(None);
        };
        self.invoke_bar_function(&formatter.name, bar_context)
            .map(Some)
    }

    fn invoke_action_function(
        &self,
        function_name: &str,
        context: Context,
    ) -> Result<Vec<Action>, ScriptError> {
        let mut scope = runtime_scope(Some(context.clone()), self.loaded.theme.clone());
        let result = self
            .engine
            .call_fn_with_options::<Dynamic>(
                CallFnOptions::new().eval_ast(false),
                &mut scope,
                &self.loaded.ast,
                function_name,
                (context,),
            )
            .map_err(|error| {
                ScriptError::runtime_path(self.loaded.source_path.as_deref(), error)
            })?;
        normalize_actions(result).map_err(|message| {
            ScriptError::validation_path(
                self.loaded.source_path.as_deref(),
                Position::NONE,
                message,
            )
        })
    }

    fn invoke_bar_function(
        &self,
        function_name: &str,
        bar_context: TabBarContext,
    ) -> Result<super::BarSpec, ScriptError> {
        let mut scope = runtime_scope(None, self.loaded.theme.clone());
        let result = self
            .engine
            .call_fn_with_options::<Dynamic>(
                CallFnOptions::new().eval_ast(false),
                &mut scope,
                &self.loaded.ast,
                function_name,
                (bar_context,),
            )
            .map_err(|error| {
                ScriptError::runtime_path(self.loaded.source_path.as_deref(), error)
            })?;
        normalize_bar(result).map_err(|message| {
            ScriptError::validation_path(
                self.loaded.source_path.as_deref(),
                Position::NONE,
                message,
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeHook {
    Enter,
    Leave,
}

#[derive(Clone, Debug, Default)]
struct RegistrationState {
    leader: Option<KeySequence>,
    custom_modes: BTreeMap<String, ModeSpec>,
    mode_hooks: BTreeMap<String, ModeHooks>,
    binding_ops: Vec<BindingOperation>,
    named_actions: BTreeMap<String, ScriptFunctionRef>,
    event_handlers: BTreeMap<String, Vec<ScriptFunctionRef>>,
    tab_bar_formatter: Option<ScriptFunctionRef>,
    mouse: MouseSettings,
    theme: ThemeSpec,
}

impl RegistrationState {
    fn build_loaded_config(
        self,
        source: &LoadedConfigSource,
        ast: rhai::AST,
    ) -> Result<LoadedConfig, ScriptError> {
        let mut modes = builtin_modes();
        modes.extend(self.custom_modes);

        let mut bindings = BTreeMap::<String, Vec<BindingSpec<Vec<Action>>>>::new();
        for operation in self.binding_ops {
            match operation {
                BindingOperation::Bind(pending) => {
                    if !modes.contains_key(&pending.mode) {
                        return Err(ScriptError::validation(
                            source,
                            pending.position,
                            format!("binding uses unknown mode '{}'", pending.mode),
                        ));
                    }

                    validate_action_refs(
                        source,
                        pending.position,
                        &self.named_actions,
                        &pending.target,
                    )?;

                    let sequence = expand_leader(
                        pending.raw_sequence.clone(),
                        self.leader.as_deref().unwrap_or(&[]),
                    )
                    .map_err(|error| {
                        ScriptError::validation(source, pending.position, error.to_string())
                    })?;

                    let mode_bindings = bindings.entry(pending.mode.clone()).or_default();
                    if mode_bindings
                        .iter()
                        .any(|binding| binding.sequence == sequence)
                    {
                        return Err(ScriptError::validation(
                            source,
                            pending.position,
                            format!(
                                "duplicate binding '{}' in mode '{}'",
                                pending.notation, pending.mode
                            ),
                        ));
                    }

                    mode_bindings.push(BindingSpec {
                        notation: pending.notation,
                        sequence,
                        target: pending.target,
                    });
                }
                BindingOperation::Unbind(pending) => {
                    if !modes.contains_key(&pending.mode) {
                        return Err(ScriptError::validation(
                            source,
                            pending.position,
                            format!("unbind uses unknown mode '{}'", pending.mode),
                        ));
                    }
                    let sequence =
                        expand_leader(pending.raw_sequence, self.leader.as_deref().unwrap_or(&[]))
                            .map_err(|error| {
                                ScriptError::validation(source, pending.position, error.to_string())
                            })?;
                    if let Some(mode_bindings) = bindings.get_mut(&pending.mode) {
                        mode_bindings.retain(|binding| binding.sequence != sequence);
                    }
                }
            }
        }

        for mode_name in self.mode_hooks.keys() {
            if !modes.contains_key(mode_name) {
                return Err(ScriptError::validation(
                    source,
                    Position::NONE,
                    format!("mode hooks reference unknown mode '{mode_name}'"),
                ));
            }
        }

        Ok(LoadedConfig {
            source_path: source.path.clone(),
            source_hash: source.source_hash,
            ast,
            leader: self.leader.unwrap_or_default(),
            modes,
            mode_hooks: self.mode_hooks,
            bindings,
            named_actions: self.named_actions,
            event_handlers: self.event_handlers,
            tab_bar_formatter: self.tab_bar_formatter,
            mouse: self.mouse,
            theme: self.theme,
        })
    }
}

/// Thread-local slot holding the current [`SharedRegistration`] while Rhai config callbacks run.
///
/// [`with_active_registration`] is responsible for setting [`ACTIVE_REGISTRATION`] before
/// invoking registration-time API code and restoring the previous value afterward.
fn with_active_registration<T>(
    registration: &SharedRegistration,
    callback: impl FnOnce() -> T,
) -> T {
    ACTIVE_REGISTRATION.with(|active| {
        struct RestoreRegistration<'a> {
            active: &'a RefCell<Option<SharedRegistration>>,
            previous: Option<SharedRegistration>,
        }

        impl Drop for RestoreRegistration<'_> {
            fn drop(&mut self) {
                self.active.replace(self.previous.take());
            }
        }

        let previous = active.replace(Some(registration.clone()));
        let _restore = RestoreRegistration { active, previous };

        callback()
    })
}

/// Clone the [`SharedRegistration`] currently stored in [`ACTIVE_REGISTRATION`].
///
/// This expects an active registration scope to have been established by
/// [`with_active_registration`]. Callers must use [`with_active_registration`] first, or
/// otherwise guarantee that [`ACTIVE_REGISTRATION`] has been populated before calling
/// [`clone_active_registration`].
fn clone_active_registration(position: Position) -> ScriptResult<SharedRegistration> {
    ACTIVE_REGISTRATION.with(|active| {
        active.borrow().as_ref().cloned().ok_or_else(|| {
            runtime_error(
                "registration API called without an active registration state",
                position,
            )
        })
    })
}

fn validate_action_refs(
    source: &LoadedConfigSource,
    position: Position,
    named_actions: &BTreeMap<String, ScriptFunctionRef>,
    actions: &[Action],
) -> Result<(), ScriptError> {
    for action in actions {
        match action {
            Action::RunNamedAction { name } => {
                if !named_actions.contains_key(name) {
                    return Err(ScriptError::validation(
                        source,
                        position,
                        format!("binding references unknown action '{name}'"),
                    ));
                }
            }
            Action::Chain(actions) => {
                validate_action_refs(source, position, named_actions, actions)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct PendingBinding {
    mode: String,
    notation: String,
    raw_sequence: KeySequence,
    target: Vec<Action>,
    position: Position,
}

#[derive(Clone, Debug)]
struct PendingUnbinding {
    mode: String,
    raw_sequence: KeySequence,
    position: Position,
}

#[derive(Clone, Debug)]
enum BindingOperation {
    Bind(PendingBinding),
    Unbind(PendingUnbinding),
}

#[derive(Clone)]
pub(crate) struct TabbarApi {}

impl TabbarApi {
    fn new() -> Self {
        Self {}
    }

    fn set_formatter(&self, position: Position, formatter: FnPtr) -> ScriptResult<()> {
        let registration = clone_active_registration(position)?;
        let mut registration = registration.lock().expect("registration lock");
        if registration.tab_bar_formatter.is_some() {
            return Err(runtime_error("tab bar formatter already defined", position));
        }
        registration.tab_bar_formatter = Some(checked_function_ref(
            formatter,
            "tab bar formatter",
            position,
        )?);
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct ThemeApi {}

impl ThemeApi {
    fn new() -> Self {
        Self {}
    }

    fn set_palette(&self, position: Position, palette: Map) -> ScriptResult<()> {
        let registration = clone_active_registration(position)?;
        let mut registration = registration.lock().expect("registration lock");
        for (name, value) in palette {
            let Some(value) = value.try_cast::<ImmutableString>() else {
                return Err(runtime_error(
                    format!("palette color '{name}' must be a string"),
                    position,
                ));
            };
            let color = RgbColor::parse(value.as_str())
                .map_err(|error| runtime_error(error.to_string(), position))?;
            if registration.theme.palette.contains_key(name.as_str()) {
                return Err(runtime_error(
                    format!("palette color '{name}' is already defined"),
                    position,
                ));
            }
            registration.theme.palette.insert(name.to_string(), color);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct MouseApi {}

impl MouseApi {
    fn new() -> Self {
        Self {}
    }

    fn set_click_focus(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .click_focus = value;
        Ok(())
    }

    fn set_click_forward(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .click_forward = value;
        Ok(())
    }

    fn set_wheel_scroll(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .wheel_scroll = value;
        Ok(())
    }

    fn set_wheel_forward(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .wheel_forward = value;
        Ok(())
    }
}

#[export_module]
mod registration_globals {
    use super::*;

    /// Set the leader sequence used in binding notations.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_leader("<C-a>");
    /// ```
    ///
    /// # rhai-autodocs:index:1
    #[rhai_fn(return_raw, global, name = "set_leader")]
    pub fn set_leader(ctx: NativeCallContext, notation: &str) -> RhaiResultOf<()> {
        let sequence = parse_key_sequence(notation)
            .map_err(|error| runtime_error(error.to_string(), ctx.call_position()))?;
        let registration = clone_active_registration(ctx.call_position())?;
        let mut registration = registration.lock().expect("registration lock");
        if registration.leader.is_some() {
            return Err(runtime_error(
                "leader key is already defined",
                ctx.call_position(),
            ));
        }
        registration.leader = Some(sequence);
        Ok(())
    }

    #[rhai_fn(return_raw, global, name = "define_mode")]
    pub fn define_mode(ctx: NativeCallContext, mode_name: &str) -> RhaiResultOf<()> {
        define_mode_impl(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode_name.into(),
            Map::new(),
        )
    }

    /// Define a custom input mode with hooks and fallback options.
    ///
    /// Supported options are `fallback`, `on_enter`, and `on_leave`.
    ///
    /// # rhai-autodocs:index:3
    #[rhai_fn(return_raw, global, name = "define_mode")]
    pub fn define_mode_with_options(
        ctx: NativeCallContext,
        mode_name: &str,
        options: Map,
    ) -> RhaiResultOf<()> {
        define_mode_impl(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode_name.into(),
            options,
        )
    }

    /// Bind a key notation to an [`Action`], a string action name, or an array of actions.
    ///
    /// Use the `Action` overload for inline builders such as `action.focus_left()`, the string
    /// overload for a named action registered with `define_action`, or an array to chain multiple
    /// actions in sequence.
    ///
    /// # Example
    ///
    /// ```rhai
    /// bind("normal", "<leader>ws", "workspace-split");
    /// ```
    ///
    /// # rhai-autodocs:index:4
    #[rhai_fn(return_raw, global, name = "bind")]
    pub fn bind_named(
        ctx: NativeCallContext,
        mode: &str,
        notation: &str,
        action_name: &str,
    ) -> RhaiResultOf<()> {
        register_binding(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode.into(),
            notation.into(),
            vec![Action::RunNamedAction {
                name: action_name.to_owned(),
            }],
        )
    }

    #[rhai_fn(return_raw, global, name = "bind")]
    pub fn bind_action(
        ctx: NativeCallContext,
        mode: &str,
        notation: &str,
        action: Action,
    ) -> RhaiResultOf<()> {
        register_binding(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode.into(),
            notation.into(),
            vec![action],
        )
    }

    #[rhai_fn(return_raw, global, name = "bind")]
    pub fn bind_actions(
        ctx: NativeCallContext,
        mode: &str,
        notation: &str,
        actions: Array,
    ) -> RhaiResultOf<()> {
        let target = actions
            .into_iter()
            .map(|action: Dynamic| {
                action
                    .try_cast::<Action>()
                    .ok_or_else(|| runtime_error("bind expects Action values", ctx.call_position()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        register_binding(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode.into(),
            notation.into(),
            target,
        )
    }

    /// Remove a previously bound key sequence.
    ///
    /// # rhai-autodocs:index:5
    #[rhai_fn(return_raw, global, name = "unbind")]
    pub fn unbind(ctx: NativeCallContext, mode: &str, notation: &str) -> RhaiResultOf<()> {
        register_unbinding(
            &clone_active_registration(ctx.call_position())?,
            ctx.call_position(),
            mode.into(),
            notation.into(),
        )
    }

    /// Register a function pointer as a named action callable from bindings.
    ///
    /// # rhai-autodocs:index:6
    #[rhai_fn(return_raw, global, name = "define_action")]
    pub fn define_action(ctx: NativeCallContext, name: &str, callback: FnPtr) -> RhaiResultOf<()> {
        let registration = clone_active_registration(ctx.call_position())?;
        let mut registration = registration.lock().expect("registration lock");
        if registration.named_actions.contains_key(name) {
            return Err(runtime_error(
                format!("action '{name}' is already defined"),
                ctx.call_position(),
            ));
        }
        registration.named_actions.insert(
            name.to_owned(),
            checked_function_ref(callback, "named action", ctx.call_position())?,
        );
        Ok(())
    }

    /// Attach a callback to an emitted event such as `buffer_bell`.
    ///
    /// # rhai-autodocs:index:7
    #[rhai_fn(return_raw, global, name = "on")]
    pub fn on(ctx: NativeCallContext, event_name: &str, callback: FnPtr) -> RhaiResultOf<()> {
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .event_handlers
            .entry(event_name.to_owned())
            .or_default()
            .push(checked_function_ref(
                callback,
                "event handler",
                ctx.call_position(),
            )?);
        Ok(())
    }
}

#[export_module]
mod tabbar_registration_api {
    use super::*;

    /// Register the function used to format the tab bar.
    ///
    /// # rhai-autodocs:index:20
    #[rhai_fn(return_raw, name = "set_formatter")]
    pub fn set_formatter(
        ctx: NativeCallContext,
        tabbar: TabbarApi,
        callback: FnPtr,
    ) -> RhaiResultOf<()> {
        tabbar.set_formatter(ctx.call_position(), callback)
    }
}

#[export_module]
mod theme_registration_api {
    use super::*;

    /// Add named colors to the theme palette.
    ///
    /// # rhai-autodocs:index:21
    #[rhai_fn(return_raw, name = "set_palette")]
    pub fn set_palette(ctx: NativeCallContext, theme: ThemeApi, palette: Map) -> RhaiResultOf<()> {
        theme.set_palette(ctx.call_position(), palette)
    }
}

#[export_module]
mod mouse_registration_api {
    use super::*;

    /// Toggle focus-on-click behavior.
    ///
    /// # rhai-autodocs:index:22
    #[rhai_fn(return_raw, name = "set_click_focus")]
    pub fn set_click_focus(
        ctx: NativeCallContext,
        mouse: MouseApi,
        value: bool,
    ) -> RhaiResultOf<()> {
        mouse.set_click_focus(ctx.call_position(), value)
    }

    /// Toggle forwarding mouse clicks into the focused buffer.
    ///
    /// # rhai-autodocs:index:23
    #[rhai_fn(return_raw, name = "set_click_forward")]
    pub fn set_click_forward(
        ctx: NativeCallContext,
        mouse: MouseApi,
        value: bool,
    ) -> RhaiResultOf<()> {
        mouse.set_click_forward(ctx.call_position(), value)
    }

    /// Toggle client-side wheel scrolling.
    ///
    /// # rhai-autodocs:index:24
    #[rhai_fn(return_raw, name = "set_wheel_scroll")]
    pub fn set_wheel_scroll(
        ctx: NativeCallContext,
        mouse: MouseApi,
        value: bool,
    ) -> RhaiResultOf<()> {
        mouse.set_wheel_scroll(ctx.call_position(), value)
    }

    /// Toggle wheel event forwarding into the focused buffer.
    ///
    /// # rhai-autodocs:index:25
    #[rhai_fn(return_raw, name = "set_wheel_forward")]
    pub fn set_wheel_forward(
        ctx: NativeCallContext,
        mouse: MouseApi,
        value: bool,
    ) -> RhaiResultOf<()> {
        mouse.set_wheel_forward(ctx.call_position(), value)
    }
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<TabbarApi>("TabbarApi");
    engine.register_type_with_name::<ThemeApi>("ThemeApi");
    engine.register_type_with_name::<MouseApi>("MouseApi");
    engine.register_global_module(rhai::exported_module!(registration_globals).into());
    engine.register_global_module(rhai::exported_module!(tabbar_registration_api).into());
    engine.register_global_module(rhai::exported_module!(theme_registration_api).into());
    engine.register_global_module(rhai::exported_module!(mouse_registration_api).into());
}

pub(crate) fn register_documented_registration_api(engine: &mut Engine) {
    register_api(engine);
}

pub(crate) fn documentation_registration_scope() -> rhai::Scope<'static> {
    let mut scope = registration_scope();
    scope.push("tabbar", TabbarApi::new());
    scope.push("theme", ThemeApi::new());
    scope.push("mouse", MouseApi::new());
    scope
}

fn define_mode_impl(
    registration: &SharedRegistration,
    position: Position,
    mode_name: ImmutableString,
    mut options: Map,
) -> ScriptResult<()> {
    let fallback_policy = parse_fallback_policy(options.remove("fallback"), position)?;
    let on_enter =
        parse_optional_function_ref(options.remove("on_enter"), "mode on_enter", position)?;
    let on_leave =
        parse_optional_function_ref(options.remove("on_leave"), "mode on_leave", position)?;
    if !options.is_empty() {
        let unknown = options.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(runtime_error(
            format!("unknown mode option(s): {unknown}"),
            position,
        ));
    }

    let mut registration = registration.lock().expect("registration lock");
    if registration.custom_modes.contains_key(mode_name.as_str()) {
        return Err(runtime_error(
            format!("mode '{mode_name}' is already defined"),
            position,
        ));
    }
    registration.custom_modes.insert(
        mode_name.to_string(),
        ModeSpec::new(mode_name.to_string(), fallback_policy),
    );
    registration
        .mode_hooks
        .insert(mode_name.to_string(), ModeHooks { on_enter, on_leave });
    Ok(())
}

fn register_binding(
    registration: &SharedRegistration,
    position: Position,
    mode: ImmutableString,
    notation: ImmutableString,
    target: Vec<Action>,
) -> ScriptResult<()> {
    let raw_sequence = parse_key_sequence(notation.as_str())
        .map_err(|error| runtime_error(error.to_string(), position))?;
    registration
        .lock()
        .expect("registration lock")
        .binding_ops
        .push(BindingOperation::Bind(PendingBinding {
            mode: mode.to_string(),
            notation: notation.to_string(),
            raw_sequence,
            target,
            position,
        }));
    Ok(())
}

fn register_unbinding(
    registration: &SharedRegistration,
    position: Position,
    mode: ImmutableString,
    notation: ImmutableString,
) -> ScriptResult<()> {
    let raw_sequence = parse_key_sequence(notation.as_str())
        .map_err(|error| runtime_error(error.to_string(), position))?;
    registration
        .lock()
        .expect("registration lock")
        .binding_ops
        .push(BindingOperation::Unbind(PendingUnbinding {
            mode: mode.to_string(),
            raw_sequence,
            position,
        }));
    Ok(())
}

fn parse_fallback_policy(
    value: Option<Dynamic>,
    position: Position,
) -> ScriptResult<FallbackPolicy> {
    let Some(value) = value else {
        return Ok(FallbackPolicy::Ignore);
    };
    if value.is_unit() {
        return Ok(FallbackPolicy::Ignore);
    }
    let Some(value) = value.try_cast::<ImmutableString>() else {
        return Err(runtime_error(
            "mode fallback must be 'pass_to_buffer' or 'ignore'",
            position,
        ));
    };
    match value.as_str() {
        "pass_to_buffer" => Ok(FallbackPolicy::Passthrough),
        "ignore" => Ok(FallbackPolicy::Ignore),
        other => Err(runtime_error(
            format!("unknown fallback policy '{other}'"),
            position,
        )),
    }
}

fn function_ref(callback: FnPtr) -> ScriptFunctionRef {
    ScriptFunctionRef::new(callback.fn_name().to_owned())
}

fn parse_optional_function_ref(
    value: Option<Dynamic>,
    role: &str,
    position: Position,
) -> ScriptResult<Option<ScriptFunctionRef>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_unit() {
        return Ok(None);
    }
    let Some(callback) = value.try_cast::<FnPtr>() else {
        return Err(runtime_error(
            format!("{role} must be a function"),
            position,
        ));
    };
    checked_function_ref(callback, role, position).map(Some)
}

fn checked_function_ref(
    callback: FnPtr,
    role: &str,
    position: Position,
) -> ScriptResult<ScriptFunctionRef> {
    if callback.is_curried() {
        return Err(runtime_error(
            format!("{role} callbacks cannot capture curried arguments"),
            position,
        ));
    }
    Ok(function_ref(callback))
}

fn runtime_error(message: impl Into<String>, position: Position) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), position).into()
}
