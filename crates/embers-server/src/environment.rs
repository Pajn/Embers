//! Session and global environments, tmux `set-environment` style.
//!
//! A buffer created for a session starts with the server's base environment,
//! then the global environment, then the session's, then the variables its
//! creator passed; later layers win. The environments only affect buffers
//! created after a change, as in tmux, and are not saved with the workspace.

use std::collections::BTreeMap;

use embers_core::{MuxError, Result, SessionId};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvironmentStore {
    global: BTreeMap<String, String>,
    sessions: BTreeMap<SessionId, BTreeMap<String, String>>,
}

impl EnvironmentStore {
    /// The global environment, or `session_id`'s own variables.
    pub fn get(&self, session_id: Option<SessionId>) -> BTreeMap<String, String> {
        match session_id {
            None => self.global.clone(),
            Some(session_id) => self.sessions.get(&session_id).cloned().unwrap_or_default(),
        }
    }

    /// Set `name` to `value`, or remove it when `value` is `None`, in the global
    /// environment or `session_id`'s.
    pub fn set(
        &mut self,
        session_id: Option<SessionId>,
        name: &str,
        value: Option<String>,
    ) -> Result<()> {
        validate_name(name)?;
        if value.as_deref().is_some_and(|value| value.contains('\0')) {
            return Err(MuxError::invalid_input(format!(
                "environment value for '{name}' must not contain NUL"
            )));
        }
        let env = match session_id {
            None => &mut self.global,
            Some(session_id) => self.sessions.entry(session_id).or_default(),
        };
        match value {
            Some(value) => {
                env.insert(name.to_owned(), value);
            }
            None => {
                env.remove(name);
            }
        }
        if let Some(session_id) = session_id
            && self
                .sessions
                .get(&session_id)
                .is_some_and(BTreeMap::is_empty)
        {
            self.sessions.remove(&session_id);
        }
        Ok(())
    }

    /// The variables a buffer created for `session_id` gets before its own:
    /// the global environment overlaid with the session's.
    pub fn for_new_buffer(&self, session_id: Option<SessionId>) -> BTreeMap<String, String> {
        let mut env = self.global.clone();
        if let Some(session) = session_id.and_then(|session_id| self.sessions.get(&session_id)) {
            env.extend(
                session
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        env
    }

    /// Drop the environments of sessions for which `is_live` is false.
    pub fn retain_sessions(&mut self, mut is_live: impl FnMut(SessionId) -> bool) {
        self.sessions.retain(|session_id, _| is_live(*session_id));
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(MuxError::invalid_input(format!(
            "invalid environment variable name '{name}'"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_variables_override_global_ones_for_new_buffers() {
        let mut store = EnvironmentStore::default();
        store.set(None, "EDITOR", Some("vi".to_owned())).unwrap();
        store.set(None, "PAGER", Some("less".to_owned())).unwrap();
        store
            .set(Some(SessionId(1)), "EDITOR", Some("nvim".to_owned()))
            .unwrap();

        let env = store.for_new_buffer(Some(SessionId(1)));
        assert_eq!(env.get("EDITOR").map(String::as_str), Some("nvim"));
        assert_eq!(env.get("PAGER").map(String::as_str), Some("less"));
        assert_eq!(
            store.for_new_buffer(Some(SessionId(2))).get("EDITOR"),
            Some(&"vi".to_owned())
        );
        assert_eq!(store.get(Some(SessionId(1))).len(), 1);
    }

    #[test]
    fn unsetting_removes_the_variable_and_empty_sessions() {
        let mut store = EnvironmentStore::default();
        store
            .set(Some(SessionId(1)), "FOO", Some("1".to_owned()))
            .unwrap();
        store.set(Some(SessionId(1)), "FOO", None).unwrap();
        assert!(store.get(Some(SessionId(1))).is_empty());
        assert!(store.sessions.is_empty());
    }

    #[test]
    fn invalid_names_and_values_are_rejected() {
        let mut store = EnvironmentStore::default();
        assert!(store.set(None, "", Some("x".to_owned())).is_err());
        assert!(store.set(None, "A=B", Some("x".to_owned())).is_err());
        assert!(store.set(None, "A", Some("x\0y".to_owned())).is_err());
    }

    #[test]
    fn retain_sessions_drops_closed_sessions() {
        let mut store = EnvironmentStore::default();
        store
            .set(Some(SessionId(1)), "FOO", Some("1".to_owned()))
            .unwrap();
        store
            .set(Some(SessionId(2)), "FOO", Some("2".to_owned()))
            .unwrap();
        store.retain_sessions(|session_id| session_id == SessionId(2));
        assert!(store.get(Some(SessionId(1))).is_empty());
        assert_eq!(store.get(Some(SessionId(2))).len(), 1);
    }
}
