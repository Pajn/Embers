use std::path::PathBuf;

use crate::config::{ConfigOrigin, LoadedConfigSource};
use crate::input::{InputResolution, InputState, parse_key_sequence, resolve_key};

use super::{ScriptEngine, ScriptError};

pub struct ScriptHarness {
    engine: ScriptEngine,
    input_state: InputState,
}

impl ScriptHarness {
    pub fn load(source: &str) -> Result<Self, ScriptError> {
        let loaded_source = LoadedConfigSource {
            origin: ConfigOrigin::BuiltIn,
            path: Some(PathBuf::from("script-harness.rhai")),
            source: source.trim().to_owned(),
            source_hash: 0,
        };
        Ok(Self {
            engine: ScriptEngine::load(&loaded_source)?,
            input_state: InputState::default(),
        })
    }

    pub fn engine(&self) -> &ScriptEngine {
        &self.engine
    }

    pub fn resolve_notation(
        &mut self,
        mode: &str,
        notation: &str,
    ) -> Result<InputResolution<String>, crate::input::KeyParseError> {
        self.input_state.set_mode(mode);
        let mut last_resolution = None;
        for key in parse_key_sequence(notation)? {
            last_resolution = Some(resolve_key(
                &self.engine.loaded_config().bindings,
                &self.engine.loaded_config().modes,
                &mut self.input_state,
                key,
            ));
        }
        Ok(last_resolution.expect("notation parser returned an empty sequence"))
    }
}
