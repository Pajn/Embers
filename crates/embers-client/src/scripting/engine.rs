use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rhai::{
    Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext, Position, Scope,
};

use crate::config::LoadedConfigSource;
use crate::input::{
    BindingSpec, KeySequence, ModeSpec, builtin_modes, expand_leader, parse_key_sequence,
};

use super::error::ScriptError;
use super::types::{LoadedConfig, RgbColor, ScriptFunctionRef, ThemeSpec};

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;
type SharedRegistration = Arc<Mutex<RegistrationState>>;

pub struct ScriptEngine {
    engine: Engine,
    loaded: LoadedConfig,
}

impl ScriptEngine {
    pub fn load(source: &LoadedConfigSource) -> Result<Self, ScriptError> {
        let registration = Arc::new(Mutex::new(RegistrationState::default()));
        let mut engine = Engine::new();
        register_api(&mut engine, registration.clone());

        let ast = engine
            .compile(&source.source)
            .map_err(|error| ScriptError::compile(source, error))?;

        let mut scope = Scope::new();
        scope.push_constant("tabbar", TabbarApi::new(registration.clone()));
        scope.push_constant("theme", ThemeApi::new(registration.clone()));

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

    pub fn has_root_formatter(&self) -> bool {
        self.loaded.has_root_formatter()
    }

    pub fn has_nested_formatter(&self) -> bool {
        self.loaded.has_nested_formatter()
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

#[derive(Clone, Debug, Default)]
struct RegistrationState {
    leader: Option<KeySequence>,
    custom_modes: BTreeMap<String, ModeSpec>,
    bindings: Vec<PendingBinding>,
    named_actions: BTreeMap<String, ScriptFunctionRef>,
    event_handlers: BTreeMap<String, Vec<ScriptFunctionRef>>,
    root_tab_formatter: Option<ScriptFunctionRef>,
    nested_tab_formatter: Option<ScriptFunctionRef>,
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

        let mut bindings = BTreeMap::<String, Vec<BindingSpec<String>>>::new();
        let mut seen = BTreeSet::<(String, KeySequence)>::new();
        for pending in self.bindings {
            if !modes.contains_key(&pending.mode) {
                return Err(ScriptError::validation(
                    source,
                    pending.position,
                    format!("binding uses unknown mode '{}'", pending.mode),
                ));
            }
            if !self.named_actions.contains_key(&pending.action_name) {
                return Err(ScriptError::validation(
                    source,
                    pending.position,
                    format!(
                        "binding references unknown action '{}'",
                        pending.action_name
                    ),
                ));
            }

            let sequence = expand_leader(
                pending.raw_sequence.clone(),
                self.leader.as_deref().unwrap_or(&[]),
            )
            .map_err(|error| {
                ScriptError::validation(source, pending.position, error.to_string())
            })?;

            let seen_key = (pending.mode.clone(), sequence.clone());
            if !seen.insert(seen_key) {
                return Err(ScriptError::validation(
                    source,
                    pending.position,
                    format!(
                        "duplicate binding '{}' in mode '{}'",
                        pending.notation, pending.mode
                    ),
                ));
            }

            bindings.entry(pending.mode).or_default().push(BindingSpec {
                notation: pending.notation,
                sequence,
                target: pending.action_name,
            });
        }

        Ok(LoadedConfig {
            source_path: source.path.clone(),
            source_hash: source.source_hash,
            ast,
            leader: self.leader.unwrap_or_default(),
            modes,
            bindings,
            named_actions: self.named_actions,
            event_handlers: self.event_handlers,
            root_tab_formatter: self.root_tab_formatter,
            nested_tab_formatter: self.nested_tab_formatter,
            theme: self.theme,
        })
    }
}

#[derive(Clone, Debug)]
struct PendingBinding {
    mode: String,
    notation: String,
    raw_sequence: KeySequence,
    action_name: String,
    position: Position,
}

#[derive(Clone)]
struct TabbarApi {
    registration: SharedRegistration,
}

impl TabbarApi {
    fn new(registration: SharedRegistration) -> Self {
        Self { registration }
    }

    fn set_root_formatter(&mut self, position: Position, formatter: FnPtr) -> RhaiResult<()> {
        let mut registration = self.registration.lock().expect("registration lock");
        if registration.root_tab_formatter.is_some() {
            return Err(runtime_error(
                "root tab formatter already defined",
                position,
            ));
        }
        registration.root_tab_formatter = Some(function_ref(formatter));
        Ok(())
    }

    fn set_nested_formatter(&mut self, position: Position, formatter: FnPtr) -> RhaiResult<()> {
        let mut registration = self.registration.lock().expect("registration lock");
        if registration.nested_tab_formatter.is_some() {
            return Err(runtime_error(
                "nested tab formatter already defined",
                position,
            ));
        }
        registration.nested_tab_formatter = Some(function_ref(formatter));
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

fn register_api(engine: &mut Engine, registration: SharedRegistration) {
    engine.register_type_with_name::<TabbarApi>("TabbarApi");
    engine.register_type_with_name::<ThemeApi>("ThemeApi");

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
            let mut registration = mode_registration.lock().expect("registration lock");
            if builtin_modes().contains_key(mode_name.as_str())
                || registration.custom_modes.contains_key(mode_name.as_str())
            {
                return Err(runtime_error(
                    format!("mode '{mode_name}' is already defined"),
                    context.call_position(),
                ));
            }
            registration.custom_modes.insert(
                mode_name.to_string(),
                ModeSpec::new(mode_name.to_string(), crate::input::FallbackPolicy::Ignore),
            );
            Ok(())
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
            let raw_sequence = parse_key_sequence(notation.as_str())
                .map_err(|error| runtime_error(error.to_string(), context.call_position()))?;
            bind_registration
                .lock()
                .expect("registration lock")
                .bindings
                .push(PendingBinding {
                    mode: mode.to_string(),
                    notation: notation.to_string(),
                    raw_sequence,
                    action_name: action_name.to_string(),
                    position: context.call_position(),
                });
            Ok(())
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
            registration
                .named_actions
                .insert(name.into_owned(), function_ref(callback));
            Ok(())
        },
    );

    let handler_registration = registration.clone();
    engine.register_fn(
        "on",
        move |_context: NativeCallContext,
              event_name: ImmutableString,
              callback: FnPtr|
              -> RhaiResult<()> {
            handler_registration
                .lock()
                .expect("registration lock")
                .event_handlers
                .entry(event_name.into_owned())
                .or_default()
                .push(function_ref(callback));
            Ok(())
        },
    );

    engine.register_fn(
        "set_root_formatter",
        |context: NativeCallContext, tabbar: &mut TabbarApi, callback: FnPtr| -> RhaiResult<()> {
            tabbar.set_root_formatter(context.call_position(), callback)
        },
    );
    engine.register_fn(
        "set_nested_formatter",
        |context: NativeCallContext, tabbar: &mut TabbarApi, callback: FnPtr| -> RhaiResult<()> {
            tabbar.set_nested_formatter(context.call_position(), callback)
        },
    );
    engine.register_fn(
        "set_palette",
        |context: NativeCallContext, theme: &mut ThemeApi, palette: Map| -> RhaiResult<()> {
            theme.set_palette(context.call_position(), palette)
        },
    );
}

fn function_ref(callback: FnPtr) -> ScriptFunctionRef {
    ScriptFunctionRef::new(callback.fn_name().to_owned())
}

fn runtime_error(message: impl Into<String>, position: Position) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), position).into()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::{ConfigOrigin, LoadedConfigSource};

    use super::ScriptEngine;

    #[test]
    fn loads_valid_config_and_tracks_registries() {
        let source = inline_source(
            r##"
                fn split_workspace() { () }
                fn on_created() { () }
                fn root_tabs() { () }
                fn nested_tabs() { () }

                set_leader("<C-a>");
                define_mode("locked");
                define_action("workspace-split", split_workspace);
                bind("normal", "<leader>ws", "workspace-split");
                on("session-created", on_created);
                tabbar.set_root_formatter(root_tabs);
                tabbar.set_nested_formatter(nested_tabs);
                theme.set_palette(#{ active: "#00ff00", inactive: "#333333" });
            "##,
        );

        let engine = ScriptEngine::load(&source).unwrap();

        assert_eq!(
            engine.loaded_config().leader,
            crate::parse_key_sequence("<C-a>").unwrap()
        );
        assert!(engine.loaded_config().modes.contains_key("normal"));
        assert!(engine.loaded_config().modes.contains_key("locked"));
        assert!(engine.has_action("workspace-split"));
        assert!(engine.has_event_handlers("session-created"));
        assert!(engine.has_root_formatter());
        assert!(engine.has_nested_formatter());
        assert_eq!(
            engine.loaded_config().bindings["normal"][0].target,
            "workspace-split"
        );
    }

    #[test]
    fn reports_invalid_syntax_with_path_and_location() {
        let source = inline_source("set_leader(");

        let error = ScriptEngine::load(&source)
            .err()
            .expect("script should fail");
        let message = error.to_string();

        assert!(message.contains("inline-config.rhai"));
        assert!(message.contains("failed to compile"));
        assert!(message.contains("at 1"));
    }

    #[test]
    fn duplicate_action_names_fail() {
        let source = inline_source(
            r#"
                fn first() { () }
                fn second() { () }
                define_action("dup", first);
                define_action("dup", second);
            "#,
        );

        let error = ScriptEngine::load(&source)
            .err()
            .expect("script should fail");

        assert!(
            error
                .to_string()
                .contains("action 'dup' is already defined")
        );
    }

    #[test]
    fn duplicate_modes_fail() {
        let source = inline_source(
            r#"
                define_mode("locked");
                define_mode("locked");
            "#,
        );

        let error = ScriptEngine::load(&source)
            .err()
            .expect("script should fail");

        assert!(
            error
                .to_string()
                .contains("mode 'locked' is already defined")
        );
    }

    #[test]
    fn invalid_key_notation_fails() {
        let source = inline_source(
            r#"
                fn noop() { () }
                define_action("noop", noop);
                bind("normal", "<Hyper-x>", "noop");
            "#,
        );

        let error = ScriptEngine::load(&source)
            .err()
            .expect("script should fail");

        assert!(error.to_string().contains("key modifier"));
    }

    #[test]
    fn invalid_palette_values_fail() {
        let source = inline_source(r##"theme.set_palette(#{ active: "green" });"##);

        let error = ScriptEngine::load(&source)
            .err()
            .expect("script should fail");

        assert!(error.to_string().contains("must be in '#RRGGBB' form"));
    }

    fn inline_source(source: &str) -> LoadedConfigSource {
        LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: Some(PathBuf::from("inline-config.rhai")),
            source: source.trim().to_owned(),
            source_hash: 0,
        }
    }
}
