use std::collections::BTreeMap;

use crate::input::keyparse::KeySequence;

pub const NORMAL_MODE: &str = "normal";
pub const COPY_MODE: &str = "copy";
pub const SELECT_MODE: &str = "select";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum FallbackPolicy {
    #[default]
    Passthrough,
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeSpec {
    pub name: String,
    pub fallback_policy: FallbackPolicy,
}

impl ModeSpec {
    pub fn new(name: impl Into<String>, fallback_policy: FallbackPolicy) -> Self {
        Self {
            name: name.into(),
            fallback_policy,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputState {
    current_mode: String,
    pending_sequence: KeySequence,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            current_mode: NORMAL_MODE.to_owned(),
            pending_sequence: Vec::new(),
        }
    }
}

impl InputState {
    pub fn current_mode(&self) -> &str {
        &self.current_mode
    }

    pub fn pending_sequence(&self) -> &[crate::input::KeyToken] {
        &self.pending_sequence
    }

    pub fn set_mode(&mut self, mode: impl Into<String>) {
        let mode = mode.into();
        if self.current_mode != mode {
            self.current_mode = mode;
            self.pending_sequence.clear();
        }
    }

    pub fn clear_pending(&mut self) {
        self.pending_sequence.clear();
    }

    pub(crate) fn push_pending(&mut self, key: crate::input::KeyToken) {
        self.pending_sequence.push(key);
    }
}

pub fn builtin_modes() -> BTreeMap<String, ModeSpec> {
    BTreeMap::from([
        (
            NORMAL_MODE.to_owned(),
            ModeSpec::new(NORMAL_MODE, FallbackPolicy::Passthrough),
        ),
        (
            COPY_MODE.to_owned(),
            ModeSpec::new(COPY_MODE, FallbackPolicy::Ignore),
        ),
        (
            SELECT_MODE.to_owned(),
            ModeSpec::new(SELECT_MODE, FallbackPolicy::Ignore),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::{InputState, builtin_modes};

    #[test]
    fn switching_modes_clears_pending_state() {
        let mut state = InputState::default();
        state.push_pending(crate::input::KeyToken::Char('a'));

        state.set_mode("copy");

        assert_eq!(state.current_mode(), "copy");
        assert!(state.pending_sequence().is_empty());
    }

    #[test]
    fn builtin_modes_include_normal_copy_and_select() {
        let modes = builtin_modes();
        assert!(modes.contains_key("normal"));
        assert!(modes.contains_key("copy"));
        assert!(modes.contains_key("select"));
    }
}
