use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rhai::{
    CallFnOptions, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext,
    Position,
};

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
use super::{Action, Context, TabBarContext};

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;
type SharedRegistration = Arc<Mutex<RegistrationState>>;

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
        register_api(&mut engine, registration.clone());
        register_runtime_api(&mut engine);

        let composed_source = if builtins.is_empty() {
            source.source.clone()
        } else {
            format!("{builtins}\n{}", source.source)
        };
        let ast = engine
            .compile(&composed_source)
            .map_err(|error| ScriptError::compile(source, error))?;

        let mut scope = registration_scope();
        scope.push_constant("tabbar", TabbarApi::new(registration.clone()));
        scope.push_constant("theme", ThemeApi::new(registration.clone()));
        scope.push_constant("mouse", MouseApi::new(registration.clone()));

        let _ = engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
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
struct TabbarApi {
    registration: SharedRegistration,
}

impl TabbarApi {
    fn new(registration: SharedRegistration) -> Self {
        Self { registration }
    }

    fn set_formatter(&mut self, position: Position, formatter: FnPtr) -> RhaiResult<()> {
        let mut registration = self.registration.lock().expect("registration lock");
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
struct ThemeApi {
    registration: SharedRegistration,
}

impl ThemeApi {
    fn new(registration: SharedRegistration) -> Self {
        Self { registration }
    }

    fn set_palette(&mut self, position: Position, palette: Map) -> RhaiResult<()> {
        let mut registration = self.registration.lock().expect("registration lock");
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
struct MouseApi {
    registration: SharedRegistration,
}

impl MouseApi {
    fn new(registration: SharedRegistration) -> Self {
        Self { registration }
    }

    fn set_click_focus(&mut self, value: bool) {
        self.registration
            .lock()
            .expect("registration lock")
            .mouse
            .click_focus = value;
    }

    fn set_click_forward(&mut self, value: bool) {
        self.registration
            .lock()
            .expect("registration lock")
            .mouse
            .click_forward = value;
    }

    fn set_wheel_scroll(&mut self, value: bool) {
        self.registration
            .lock()
            .expect("registration lock")
            .mouse
            .wheel_scroll = value;
    }

    fn set_wheel_forward(&mut self, value: bool) {
        self.registration
            .lock()
            .expect("registration lock")
            .mouse
            .wheel_forward = value;
    }
}

fn register_api(engine: &mut Engine, registration: SharedRegistration) {
    engine.register_type_with_name::<TabbarApi>("TabbarApi");
    engine.register_type_with_name::<ThemeApi>("ThemeApi");
    engine.register_type_with_name::<MouseApi>("MouseApi");

    let leader_registration = registration.clone();
    engine.register_fn(
        "set_leader",
        move |context: NativeCallContext, notation: ImmutableString| -> RhaiResult<()> {
            let sequence = parse_key_sequence(notation.as_str())
                .map_err(|error| runtime_error(error.to_string(), context.call_position()))?;
            let mut registration = leader_registration.lock().expect("registration lock");
            if registration.leader.is_some() {
                return Err(runtime_error(
                    "leader key is already defined",
                    context.call_position(),
                ));
            }
            registration.leader = Some(sequence);
            Ok(())
        },
    );

    let mode_registration = registration.clone();
    engine.register_fn(
        "define_mode",
        move |context: NativeCallContext, mode_name: ImmutableString| -> RhaiResult<()> {
            define_mode_impl(
                &mode_registration,
                context.call_position(),
                mode_name,
                Map::new(),
            )
        },
    );

    let mode_registration = registration.clone();
    engine.register_fn(
        "define_mode",
        move |context: NativeCallContext,
              mode_name: ImmutableString,
              options: Map|
              -> RhaiResult<()> {
            define_mode_impl(
                &mode_registration,
                context.call_position(),
                mode_name,
                options,
            )
        },
    );

    let bind_registration = registration.clone();
    engine.register_fn(
        "bind",
        move |context: NativeCallContext,
              mode: ImmutableString,
              notation: ImmutableString,
              action_name: ImmutableString|
              -> RhaiResult<()> {
            register_binding(
                &bind_registration,
                context.call_position(),
                mode,
                notation,
                vec![Action::RunNamedAction {
                    name: action_name.to_string(),
                }],
            )
        },
    );

    let unbind_registration = registration.clone();
    engine.register_fn(
        "unbind",
        move |context: NativeCallContext,
              mode: ImmutableString,
              notation: ImmutableString|
              -> RhaiResult<()> {
            register_unbinding(
                &unbind_registration,
                context.call_position(),
                mode,
                notation,
            )
        },
    );

    let bind_registration = registration.clone();
    engine.register_fn(
        "bind",
        move |context: NativeCallContext,
              mode: ImmutableString,
              notation: ImmutableString,
              action: Action|
              -> RhaiResult<()> {
            register_binding(
                &bind_registration,
                context.call_position(),
                mode,
                notation,
                vec![action],
            )
        },
    );

    let bind_registration = registration.clone();
    engine.register_fn(
        "bind",
        move |context: NativeCallContext,
              mode: ImmutableString,
              notation: ImmutableString,
              actions: rhai::Array|
              -> RhaiResult<()> {
            let target = actions
                .into_iter()
                .map(|action| {
                    action.try_cast::<Action>().ok_or_else(|| {
                        runtime_error("bind expects Action values", context.call_position())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            register_binding(
                &bind_registration,
                context.call_position(),
                mode,
                notation,
                target,
            )
        },
    );

    let action_registration = registration.clone();
    engine.register_fn(
        "define_action",
        move |context: NativeCallContext,
              name: ImmutableString,
              callback: FnPtr|
              -> RhaiResult<()> {
            let mut registration = action_registration.lock().expect("registration lock");
            if registration.named_actions.contains_key(name.as_str()) {
                return Err(runtime_error(
                    format!("action '{name}' is already defined"),
                    context.call_position(),
                ));
            }
            registration.named_actions.insert(
                name.into_owned(),
                checked_function_ref(callback, "named action", context.call_position())?,
            );
            Ok(())
        },
    );

    let handler_registration = registration.clone();
    engine.register_fn(
        "on",
        move |context: NativeCallContext,
              event_name: ImmutableString,
              callback: FnPtr|
              -> RhaiResult<()> {
            handler_registration
                .lock()
                .expect("registration lock")
                .event_handlers
                .entry(event_name.into_owned())
                .or_default()
                .push(checked_function_ref(
                    callback,
                    "event handler",
                    context.call_position(),
                )?);
            Ok(())
        },
    );

    engine.register_fn(
        "set_formatter",
        |context: NativeCallContext, tabbar: &mut TabbarApi, callback: FnPtr| -> RhaiResult<()> {
            tabbar.set_formatter(context.call_position(), callback)
        },
    );
    engine.register_fn(
        "set_palette",
        |context: NativeCallContext, theme: &mut ThemeApi, palette: Map| -> RhaiResult<()> {
            theme.set_palette(context.call_position(), palette)
        },
    );
    engine.register_fn(
        "set_click_focus",
        |_: NativeCallContext, mouse: &mut MouseApi, value: bool| {
            mouse.set_click_focus(value);
        },
    );
    engine.register_fn(
        "set_click_forward",
        |_: NativeCallContext, mouse: &mut MouseApi, value: bool| {
            mouse.set_click_forward(value);
        },
    );
    engine.register_fn(
        "set_wheel_scroll",
        |_: NativeCallContext, mouse: &mut MouseApi, value: bool| {
            mouse.set_wheel_scroll(value);
        },
    );
    engine.register_fn(
        "set_wheel_forward",
        |_: NativeCallContext, mouse: &mut MouseApi, value: bool| {
            mouse.set_wheel_forward(value);
        },
    );
}

fn define_mode_impl(
    registration: &SharedRegistration,
    position: Position,
    mode_name: ImmutableString,
    mut options: Map,
) -> RhaiResult<()> {
    let fallback_policy = parse_fallback_policy(options.remove("fallback"))?;
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
) -> RhaiResult<()> {
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
) -> RhaiResult<()> {
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

fn parse_fallback_policy(value: Option<Dynamic>) -> RhaiResult<FallbackPolicy> {
    let Some(value) = value else {
        return Ok(FallbackPolicy::Ignore);
    };
    if value.is_unit() {
        return Ok(FallbackPolicy::Ignore);
    }
    let Some(value) = value.try_cast::<ImmutableString>() else {
        return Err(runtime_error(
            "mode fallback must be 'pass_to_buffer' or 'ignore'",
            Position::NONE,
        ));
    };
    match value.as_str() {
        "pass_to_buffer" => Ok(FallbackPolicy::Passthrough),
        "ignore" => Ok(FallbackPolicy::Ignore),
        other => Err(runtime_error(
            format!("unknown fallback policy '{other}'"),
            Position::NONE,
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
) -> RhaiResult<Option<ScriptFunctionRef>> {
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
) -> RhaiResult<ScriptFunctionRef> {
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
