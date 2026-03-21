use std::collections::BTreeMap;

use super::keyparse::{KeySequence, KeyToken};
use super::modes::{FallbackPolicy, InputState, ModeSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingSpec<T> {
    pub notation: String,
    pub sequence: KeySequence,
    pub target: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingMatch<T> {
    pub mode: String,
    pub sequence: KeySequence,
    pub target: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputResolution<T> {
    ExactMatch(BindingMatch<T>),
    PrefixMatch,
    Unmatched {
        mode: String,
        sequence: KeySequence,
        fallback_policy: FallbackPolicy,
    },
}

pub fn resolve_key<T: Clone + PartialEq + Eq>(
    bindings: &BTreeMap<String, Vec<BindingSpec<T>>>,
    modes: &BTreeMap<String, ModeSpec>,
    state: &mut InputState,
    key: KeyToken,
) -> InputResolution<T> {
    state.push_pending(key);
    let mode = state.current_mode().to_owned();
    let pending = state.pending_sequence().to_vec();
    let mode_bindings = bindings.get(&mode).cloned().unwrap_or_default();

    if let Some(binding) = mode_bindings
        .iter()
        .find(|binding| binding.sequence == pending)
        .cloned()
    {
        state.clear_pending();
        return InputResolution::ExactMatch(BindingMatch {
            mode,
            sequence: binding.sequence,
            target: binding.target,
        });
    }

    if mode_bindings
        .iter()
        .any(|binding| binding.sequence.starts_with(&pending))
    {
        return InputResolution::PrefixMatch;
    }

    let fallback_policy = modes
        .get(&mode)
        .map(|mode| mode.fallback_policy)
        .unwrap_or(FallbackPolicy::Ignore);
    state.clear_pending();
    InputResolution::Unmatched {
        mode,
        sequence: pending,
        fallback_policy,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{BindingSpec, InputResolution, resolve_key};
    use crate::input::{FallbackPolicy, InputState, KeyToken, ModeSpec, builtin_modes};

    #[test]
    fn exact_match_beats_prefix() {
        let mut state = InputState::default();
        let bindings = bindings(&[("normal", "ab", "exact"), ("normal", "abc", "longer")]);
        let modes = builtin_modes();

        assert_eq!(
            resolve_key(&bindings, &modes, &mut state, KeyToken::Char('a')),
            InputResolution::PrefixMatch
        );
        assert_eq!(
            resolve_key(&bindings, &modes, &mut state, KeyToken::Char('b')),
            InputResolution::ExactMatch(super::BindingMatch {
                mode: "normal".to_owned(),
                sequence: vec![KeyToken::Char('a'), KeyToken::Char('b')],
                target: "exact".to_owned(),
            })
        );
    }

    #[test]
    fn unmatched_sequences_follow_mode_fallback_policy() {
        let mut state = InputState::default();
        let bindings = bindings(&[]);
        let modes = BTreeMap::from([(
            "locked".to_owned(),
            ModeSpec::new("locked", FallbackPolicy::Ignore),
        )]);
        state.set_mode("locked");

        assert_eq!(
            resolve_key(&bindings, &modes, &mut state, KeyToken::Char('x')),
            InputResolution::Unmatched {
                mode: "locked".to_owned(),
                sequence: vec![KeyToken::Char('x')],
                fallback_policy: FallbackPolicy::Ignore,
            }
        );
    }

    #[test]
    fn mode_specific_bindings_resolve_independently() {
        let mut state = InputState::default();
        let bindings = bindings(&[("normal", "a", "normal-a"), ("copy", "a", "copy-a")]);
        let modes = builtin_modes();

        assert_eq!(
            resolve_key(&bindings, &modes, &mut state, KeyToken::Char('a')),
            InputResolution::ExactMatch(super::BindingMatch {
                mode: "normal".to_owned(),
                sequence: vec![KeyToken::Char('a')],
                target: "normal-a".to_owned(),
            })
        );

        state.set_mode("copy");

        assert_eq!(
            resolve_key(&bindings, &modes, &mut state, KeyToken::Char('a')),
            InputResolution::ExactMatch(super::BindingMatch {
                mode: "copy".to_owned(),
                sequence: vec![KeyToken::Char('a')],
                target: "copy-a".to_owned(),
            })
        );
    }

    fn bindings(entries: &[(&str, &str, &str)]) -> BTreeMap<String, Vec<BindingSpec<String>>> {
        let mut bindings = BTreeMap::<String, Vec<BindingSpec<String>>>::new();
        for (mode, sequence, target) in entries {
            bindings
                .entry((*mode).to_owned())
                .or_default()
                .push(BindingSpec {
                    notation: (*sequence).to_owned(),
                    sequence: sequence.chars().map(KeyToken::Char).collect(),
                    target: (*target).to_owned(),
                });
        }
        bindings
    }
}
