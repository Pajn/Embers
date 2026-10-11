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
use crate::command_prompt::StatusKeys;
use crate::host_terminal::TerminalFeatures;
use crate::presentation::{StatusLineMode, TabBarPosition};

use super::types::{
    DetachOnDestroy, HintsSettings, LoadedConfig, ModeHooks, MouseSettings, PassthroughMode,
    RgbColor, ScriptFunctionRef, ThemeSpec, parse_terminal_color,
};
use super::{Action, Context, RhaiResultOf, ScriptResult, TabBarContext};
type SharedRegistration = Arc<Mutex<RegistrationState>>;

fn populate_common_registration_scope(scope: &mut rhai::Scope<'static>) {
    scope.push("tabbar", TabbarApi::new());
    scope.push("theme", ThemeApi::new());
    scope.push("mouse", MouseApi::new());
    scope.push("hints", HintsApi::new());
}

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
        populate_common_registration_scope(&mut scope);

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
    tab_bar_position: TabBarPosition,
    tab_bar_visible: Option<bool>,
    mouse: MouseSettings,
    hints: HintsSettings,
    theme: ThemeSpec,
    clipboard: Option<bool>,
    base_index: Option<u32>,
    passthrough: Option<PassthroughMode>,
    terminal_features: TerminalFeatures,
    detach_on_destroy: DetachOnDestroy,
    status_keys: StatusKeys,
    status_line: StatusLineMode,
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
            tab_bar_position: self.tab_bar_position,
            tab_bar_visible: self.tab_bar_visible.unwrap_or(true),
            mouse: self.mouse,
            hints: self.hints,
            theme: self.theme,
            clipboard: self.clipboard.unwrap_or(true),
            base_index: self.base_index,
            passthrough: self.passthrough.unwrap_or_default(),
            terminal_features: self.terminal_features,
            detach_on_destroy: self.detach_on_destroy,
            status_keys: self.status_keys,
            status_line: self.status_line,
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
            Action::RunNamedAction { name } if !named_actions.contains_key(name) => {
                return Err(ScriptError::validation(
                    source,
                    position,
                    format!("binding references unknown action '{name}'"),
                ));
            }
            Action::Chain(actions) | Action::Confirm { actions, .. } => {
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

    fn set_position(&self, position: Position, value: &str) -> ScriptResult<()> {
        let Some(tab_bar) = TabBarPosition::parse(value) else {
            return Err(runtime_error(
                format!("tab bar position must be \"top\" or \"bottom\", not \"{value}\""),
                position,
            ));
        };
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .tab_bar_position = tab_bar;
        Ok(())
    }

    fn set_visible(&self, position: Position, visible: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .tab_bar_visible = Some(visible);
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

    fn set_pane_border_color(
        &self,
        position: Position,
        active: bool,
        value: &str,
    ) -> ScriptResult<()> {
        let color = parse_terminal_color(value).map_err(|_| {
            runtime_error(
                format!("pane border color must be '#RRGGBB' or an ANSI color name such as \"magenta\" or \"brightblack\", not \"{value}\""),
                position,
            )
        })?;
        let registration = clone_active_registration(position)?;
        let mut registration = registration.lock().expect("registration lock");
        let borders = &mut registration.theme.pane_borders;
        if active {
            borders.active = Some(color);
        } else {
            borders.inactive = Some(color);
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

    fn set_drag_resize(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .drag_resize = value;
        Ok(())
    }

    fn set_select(&self, position: Position, value: bool) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .mouse
            .select = value;
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct HintsApi {}

impl HintsApi {
    fn new() -> Self {
        Self {}
    }

    fn set_patterns(&self, position: Position, patterns: Vec<String>) -> ScriptResult<()> {
        clone_active_registration(position)?
            .lock()
            .expect("registration lock")
            .hints
            .patterns = patterns;
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
    /// A notation is a sequence of keys: plain characters, and names or modified keys in angle
    /// brackets, such as `<Enter>`, `<C-a>`, `<M-h>`, `<S-Left>`, `<C-S-Tab>` or `<F5>`
    /// (`C` Ctrl, `M` or `A` Alt, `S` Shift, `D` Super). As in tmux, an uppercase letter is the
    /// shifted key: `<M-H>` is Alt+Shift+h and `<C-H>` is Ctrl+Shift+h, the same as `<C-S-h>`.
    /// Legacy terminals send the same bytes for Ctrl+Shift+h as for Ctrl+h, so `<C-H>` and other
    /// Ctrl+Shift bindings only fire from a host terminal with the kitty keyboard protocol.
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

    /// Choose whether programs in panes may set the system clipboard.
    ///
    /// When on (the default), an OSC 52 clipboard write from a program in a
    /// pane, such as a Neovim yank over SSH, is passed on to the terminal
    /// running the Embers client, which puts it on the system clipboard. This
    /// matches tmux's `set-clipboard on`. When off, those writes are dropped.
    /// Programs can never read the clipboard through Embers: OSC 52 queries are
    /// always ignored.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_clipboard(false);
    /// ```
    ///
    /// # rhai-autodocs:index:8
    #[rhai_fn(return_raw, global, name = "set_clipboard")]
    pub fn set_clipboard(ctx: NativeCallContext, enabled: bool) -> RhaiResultOf<()> {
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .clipboard = Some(enabled);
        Ok(())
    }

    /// Number windows from `index` instead of 0, like tmux `set -g base-index 1`.
    ///
    /// Windows are a session's root tabs. Their numbers are what
    /// `TabInfo.number()` gives tab bar formatters and what
    /// `action.select_window` takes. Windows are always numbered without gaps:
    /// closing one renumbers the windows after it, like tmux with
    /// `renumber-windows on`.
    ///
    /// This setting covers the client and the commands it runs from the command
    /// prompt. The client also reports it to the server for the session it
    /// shows, and the `embers` CLI numbers that session's windows from it
    /// unless `EMBERS_BASE_INDEX` or `--base-index` overrides it. Without this
    /// call the client uses `EMBERS_BASE_INDEX` when it is set, and numbers
    /// windows from 0 otherwise.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_base_index(1);
    /// ```
    ///
    /// # rhai-autodocs:index:9
    #[rhai_fn(return_raw, global, name = "set_base_index")]
    pub fn set_base_index(ctx: NativeCallContext, index: i64) -> RhaiResultOf<()> {
        let index = u32::try_from(index)
            .ok()
            .filter(|index| *index <= crate::numbering::MAX_BASE_INDEX)
            .ok_or_else(|| {
                runtime_error(
                    format!(
                        "base index must be between 0 and {}",
                        crate::numbering::MAX_BASE_INDEX
                    ),
                    ctx.call_position(),
                )
            })?;
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .base_index = Some(index);
        Ok(())
    }

    /// Choose which panes may write to the host terminal through tmux
    /// passthrough, like tmux's `allow-passthrough`.
    ///
    /// Programs that run under tmux wrap sequences meant for the outer
    /// terminal, such as kitty graphics images or desktop notifications, as
    /// `ESC P tmux; ... ESC \`. Embers unwraps them and writes them to the
    /// terminal running the client, with its cursor at the pane's cursor so
    /// images land where the program put them. Kitty graphics commands sent
    /// without the wrapper are passed on the same way.
    ///
    /// - `"off"` (the default, as in tmux): drop them.
    /// - `"on"`: only from panes that are on screen.
    /// - `"all"`: from every pane in the client's session, on screen or not.
    ///   Sequences from a pane that is not on screen are written without
    ///   moving the cursor.
    ///
    /// Passthrough is off by default because it lets any program on screen,
    /// or `cat` of an untrusted file, write raw sequences to the terminal,
    /// bypassing `set_clipboard(false)` and the filtering Embers applies to
    /// pane output. Turning it on only has an effect when the terminal is
    /// known to support passthrough (see `set_terminal_features`).
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_allow_passthrough("on");
    /// ```
    ///
    /// # rhai-autodocs:index:10
    #[rhai_fn(return_raw, global, name = "set_allow_passthrough")]
    pub fn set_allow_passthrough(ctx: NativeCallContext, mode: &str) -> RhaiResultOf<()> {
        let mode = PassthroughMode::parse(mode).ok_or_else(|| {
            runtime_error(
                format!("allow-passthrough must be \"on\", \"off\" or \"all\", not \"{mode}\""),
                ctx.call_position(),
            )
        })?;
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .passthrough = Some(mode);
        Ok(())
    }

    /// Say what the terminal running the client supports, overriding what
    /// Embers detected when it attached.
    ///
    /// Embers asks the terminal at attach and otherwise goes by `TERM` and
    /// `TERM_PROGRAM`, and leaves out what it cannot confirm. This sets each
    /// named capability on (`true`) or off (`false`); capabilities not named
    /// stay as detected. Like tmux's `terminal-features`.
    ///
    /// - `underline_styles`: double, curly, dotted and dashed underlines
    ///   (`CSI 4:n m`). Without it every underline is plain.
    /// - `underline_color`: colored underlines (`CSI 58:...m`). Without it
    ///   underlines take the text color.
    /// - `hyperlinks`: OSC 8 links. Without it links are plain text.
    /// - `passthrough`: sequences programs send for the terminal, such as
    ///   images (see `set_allow_passthrough`). Without it they are dropped.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_terminal_features(#{ hyperlinks: true, passthrough: false });
    /// ```
    ///
    /// # rhai-autodocs:index:11
    #[rhai_fn(return_raw, global, name = "set_terminal_features")]
    pub fn set_terminal_features(ctx: NativeCallContext, features: Map) -> RhaiResultOf<()> {
        let registration = clone_active_registration(ctx.call_position())?;
        let mut registration = registration.lock().expect("registration lock");
        for (name, value) in features {
            let enabled = value.as_bool().map_err(|_| {
                runtime_error(
                    format!("terminal feature '{name}' must be true or false"),
                    ctx.call_position(),
                )
            })?;
            if !registration.terminal_features.set(&name, enabled) {
                return Err(runtime_error(
                    format!(
                        "unknown terminal feature '{name}'; expected one of {}",
                        TerminalFeatures::NAMES.join(", ")
                    ),
                    ctx.call_position(),
                ));
            }
        }
        Ok(())
    }

    /// Choose where the client goes when the session it shows is destroyed,
    /// like tmux's `detach-on-destroy`. A session is destroyed when its last
    /// pane closes, because the pane's program exited or the pane was killed,
    /// or when the session is killed.
    ///
    /// - `"on"` (the default): detach; the client exits.
    /// - `"off"`: switch to the session the client showed before, or else the
    ///   first remaining session.
    /// - `"previous"` / `"next"`: switch to the session listed before or after
    ///   the destroyed one (`embers list-sessions` order), wrapping around.
    ///
    /// When no other session is left, the client exits whatever the setting.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_detach_on_destroy("off");
    /// ```
    ///
    /// # rhai-autodocs:index:12
    #[rhai_fn(return_raw, global, name = "set_detach_on_destroy")]
    pub fn set_detach_on_destroy(ctx: NativeCallContext, value: &str) -> RhaiResultOf<()> {
        let Some(setting) = DetachOnDestroy::parse(value) else {
            return Err(runtime_error(
                format!(
                    "set_detach_on_destroy expects \"on\", \"off\", \"previous\" or \"next\", got {value:?}"
                ),
                ctx.call_position(),
            ));
        };
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .detach_on_destroy = setting;
        Ok(())
    }

    /// Choose the keys the command prompt and the rename prompts are edited
    /// with, like tmux's `status-keys`.
    ///
    /// - `"emacs"` (the default): Left/Right, C-a/C-e, C-w, C-u, C-k and the
    ///   other keys listed in the command prompt docs.
    /// - `"vi"`: the prompt opens in insert mode, where those keys work too.
    ///   Esc switches to normal mode, which has `h` `l` `w` `b` `e` `0` `$`
    ///   to move, `x` `dw` `D` to delete, `cw` `C` to change, `i` `a` `I`
    ///   `A` to insert again and `k` `j` for older and newer history lines.
    ///   Esc in normal mode cancels the prompt; Enter runs it in either mode.
    ///
    /// The copy-mode search prompt keeps its own keys.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_status_keys("vi");
    /// ```
    ///
    /// # rhai-autodocs:index:13
    #[rhai_fn(return_raw, global, name = "set_status_keys")]
    pub fn set_status_keys(ctx: NativeCallContext, value: &str) -> RhaiResultOf<()> {
        let Some(keys) = StatusKeys::parse(value) else {
            return Err(runtime_error(
                format!("set_status_keys expects \"emacs\" or \"vi\", got {value:?}"),
                ctx.call_position(),
            ));
        };
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .status_keys = keys;
        Ok(())
    }

    /// Choose where the status line goes: the command prompt, messages,
    /// `confirm` questions, the search prompt and warnings.
    ///
    /// - `"overlay"` (the default): like tmux, it is drawn over the session's
    ///   tab bar while there is something to show, and the tab bar comes
    ///   back afterwards. With the tab bar hidden, it is drawn over the last
    ///   row of the screen. The panes get every row but the tab bar's, and
    ///   are not resized when a message comes and goes.
    /// - `"separate"`: the last row of the screen is kept for it and always
    ///   shows the session name, so the panes have one row fewer.
    ///
    /// # Example
    ///
    /// ```rhai
    /// set_status_line("separate");
    /// ```
    ///
    /// # rhai-autodocs:index:14
    #[rhai_fn(return_raw, global, name = "set_status_line")]
    pub fn set_status_line(ctx: NativeCallContext, value: &str) -> RhaiResultOf<()> {
        let Some(mode) = StatusLineMode::parse(value) else {
            return Err(runtime_error(
                format!("set_status_line expects \"overlay\" or \"separate\", got {value:?}"),
                ctx.call_position(),
            ));
        };
        clone_active_registration(ctx.call_position())?
            .lock()
            .expect("registration lock")
            .status_line = mode;
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

    /// Put the session's tab bar, its list of windows, at the `"top"` (the
    /// default) or `"bottom"` of the screen, like tmux's `status-position`.
    /// The command prompt and messages are drawn over it while they show, so
    /// they move with it. Tab bars of nested tabs stay at the top of their
    /// own area.
    ///
    /// # Example
    ///
    /// ```rhai
    /// tabbar.set_position("bottom");
    /// ```
    ///
    /// # rhai-autodocs:index:20
    #[rhai_fn(return_raw, name = "set_position")]
    pub fn set_position(
        ctx: NativeCallContext,
        tabbar: TabbarApi,
        value: &str,
    ) -> RhaiResultOf<()> {
        tabbar.set_position(ctx.call_position(), value)
    }

    /// Show (`true`, the default) or hide (`false`) the session's tab bar,
    /// like tmux's `status on` and `status off`. Hidden, it gives its row to
    /// the panes, and the command prompt and messages are drawn over the last
    /// row of the screen while they show. Only the session's own tab bar is
    /// hidden; tab bars of nested tabs, like `set_position`, are not
    /// affected.
    ///
    /// # Example
    ///
    /// ```rhai
    /// tabbar.set_visible(false);
    /// ```
    ///
    /// # rhai-autodocs:index:20
    #[rhai_fn(return_raw, name = "set_visible")]
    pub fn set_visible(
        ctx: NativeCallContext,
        tabbar: TabbarApi,
        visible: bool,
    ) -> RhaiResultOf<()> {
        tabbar.set_visible(ctx.call_position(), visible)
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

    /// Color the dividers between panes, except next to the focused pane,
    /// like tmux's `pane-border-style fg=...`. Takes `"#RRGGBB"` or one of
    /// the sixteen ANSI color names (`"black"` to `"white"` and
    /// `"brightblack"` to `"brightwhite"`), which follow the terminal's color
    /// scheme. Without it dividers are drawn dim.
    ///
    /// # Example
    ///
    /// ```rhai
    /// theme.set_pane_border_color("brightblack");
    /// ```
    ///
    /// # rhai-autodocs:index:21
    #[rhai_fn(return_raw, name = "set_pane_border_color")]
    pub fn set_pane_border_color(
        ctx: NativeCallContext,
        theme: ThemeApi,
        color: &str,
    ) -> RhaiResultOf<()> {
        theme.set_pane_border_color(ctx.call_position(), false, color)
    }

    /// Color the divider cells next to the focused pane, like tmux's
    /// `pane-active-border-style fg=...`. Takes the same colors as
    /// `set_pane_border_color`. Without it they are drawn like the others.
    ///
    /// # Example
    ///
    /// ```rhai
    /// theme.set_pane_active_border_color("magenta");
    /// ```
    ///
    /// # rhai-autodocs:index:21
    #[rhai_fn(return_raw, name = "set_pane_active_border_color")]
    pub fn set_pane_active_border_color(
        ctx: NativeCallContext,
        theme: ThemeApi,
        color: &str,
    ) -> RhaiResultOf<()> {
        theme.set_pane_border_color(ctx.call_position(), true, color)
    }
}

#[export_module]
mod mouse_registration_api {
    use super::*;

    /// Toggle focus-on-click behavior: a left click on a pane focuses it, and
    /// one on a tab in a tab bar selects it. A formatter's segments select the
    /// tab they are tagged with (`ui.segment`'s `tab` option).
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

    /// Toggle resizing splits by dragging their dividers with the left button.
    ///
    /// Only drags that start on a divider are claimed; clicks and drags inside panes behave
    /// as before. The built-in config turns this on.
    ///
    /// # rhai-autodocs:index:26
    #[rhai_fn(return_raw, name = "set_drag_resize")]
    pub fn set_drag_resize(
        ctx: NativeCallContext,
        mouse: MouseApi,
        value: bool,
    ) -> RhaiResultOf<()> {
        mouse.set_drag_resize(ctx.call_position(), value)
    }

    /// Toggle mouse selection in panes that have not requested mouse reporting:
    /// dragging enters copy mode with a selection and copies it on release,
    /// a double click copies a word and a triple click copies a line (tmux's
    /// defaults with `mouse on`). Panes running programs that requested the
    /// mouse keep receiving these events instead.
    ///
    /// # rhai-autodocs:index:27
    #[rhai_fn(return_raw, name = "set_select")]
    pub fn set_select(ctx: NativeCallContext, mouse: MouseApi, value: bool) -> RhaiResultOf<()> {
        mouse.set_select(ctx.call_position(), value)
    }
}

#[export_module]
mod hints_registration_api {
    use super::*;

    /// Replace the hint patterns used by `action.enter_hints()` with the given
    /// regex strings. A non-string element or an invalid regex is an error. An
    /// empty list restores the built-in default set (URLs, paths, SHAs, UUIDs,
    /// IPs, numbers).
    ///
    /// # Example
    ///
    /// ```rhai
    /// hints.set_patterns(["https?://\\S+", "[0-9a-f]{7,40}"]);
    /// ```
    ///
    /// # rhai-autodocs:index:28
    #[rhai_fn(return_raw, name = "set_patterns")]
    pub fn set_patterns(
        ctx: NativeCallContext,
        hints: HintsApi,
        patterns: rhai::Array,
    ) -> RhaiResultOf<()> {
        let position = ctx.call_position();
        let mut parsed = Vec::with_capacity(patterns.len());
        for value in patterns {
            let Some(text) = value.try_cast::<rhai::ImmutableString>() else {
                return Err(runtime_error(
                    "hints.set_patterns expects an array of strings",
                    position,
                ));
            };
            let text = text.to_string();
            // Reject invalid regexes at config time rather than silently
            // dropping them when hints are entered.
            if let Err(error) = regex::Regex::new(&text) {
                return Err(runtime_error(
                    format!("hints.set_patterns has an invalid regex {text:?}: {error}"),
                    position,
                ));
            }
            parsed.push(text);
        }
        hints.set_patterns(position, parsed)
    }
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<TabbarApi>("TabbarApi");
    engine.register_type_with_name::<ThemeApi>("ThemeApi");
    engine.register_type_with_name::<MouseApi>("MouseApi");
    engine.register_type_with_name::<HintsApi>("HintsApi");
    engine.register_global_module(rhai::exported_module!(registration_globals).into());
    engine.register_global_module(rhai::exported_module!(tabbar_registration_api).into());
    engine.register_global_module(rhai::exported_module!(theme_registration_api).into());
    engine.register_global_module(rhai::exported_module!(mouse_registration_api).into());
    engine.register_global_module(rhai::exported_module!(hints_registration_api).into());
}

pub(crate) fn register_documented_registration_api(engine: &mut Engine) {
    register_api(engine);
}

pub(crate) fn documentation_registration_scope() -> rhai::Scope<'static> {
    let mut scope = registration_scope();
    populate_common_registration_scope(&mut scope);
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
