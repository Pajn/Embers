//! tmux's `set-option -p` and `show-options -p`, on buffer options.
//!
//! Embers keeps options on panes only, as the options of the buffer a pane
//! shows (`buffer set-option`). tmux's user option `@name` is the buffer
//! option `name`, the name the config reads with `buffer.user_option(name)`
//! and `#{@name}` expands to; `remain-on-exit` is the one built-in pane option
//! Embers has, under its own name.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use embers_core::{BufferId, MuxError, Result, new_request_id};
use embers_protocol::{BufferRequest, ClientMessage, ServerResponse};

use crate::popup::REMAIN_ON_EXIT;
use crate::{CliConnection, ensure_matching_buffer_id};

/// The flags of `set-option` and `show-options` that pick where the option
/// lives: `-p` (pane), `-w` (window), `-s` (server), `-g` (global).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Scope {
    pub(crate) pane: bool,
    pub(crate) window: bool,
    pub(crate) server: bool,
    pub(crate) global: bool,
}

impl Scope {
    fn ensure_pane(self, command: &str) -> Result<()> {
        if self.pane && !self.window && !self.server && !self.global {
            return Ok(());
        }
        Err(MuxError::invalid_input(format!(
            "{command}: only pane options (-p) are supported; Embers keeps options on panes"
        )))
    }
}

/// The buffer option tmux option `name` is stored as: `@name` is `name`,
/// and `remain-on-exit` is itself. `None` for any other name.
fn option_key(name: &str) -> Option<&str> {
    match name.strip_prefix('@') {
        Some(key) => (!key.is_empty()).then_some(key),
        None => (name == REMAIN_ON_EXIT).then_some(name),
    }
}

/// The tmux name of buffer option `key`; the inverse of [`option_key`].
fn option_name(key: &str) -> String {
    if key == REMAIN_ON_EXIT {
        key.to_owned()
    } else {
        format!("@{key}")
    }
}

fn invalid_option(name: &str) -> MuxError {
    MuxError::invalid_input(format!("invalid option: {name}"))
}

/// `value` quoted the way tmux's `show-options` prints it, so it reads back
/// as one argument.
fn escape(value: &str) -> String {
    const DOUBLE_QUOTED: &[char] = &[' ', '#', '\'', ';', '$', '{', '}', '%'];
    const SINGLE_QUOTED: &[char] = &[' ', '"'];
    if value.is_empty() {
        return "''".to_owned();
    }
    let quote = if value.contains(DOUBLE_QUOTED) {
        Some('"')
    } else if value.contains(SINGLE_QUOTED) {
        Some('\'')
    } else {
        None
    };
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            '"' | '$' if quote == Some('"') => {
                escaped.push('\\');
                escaped.push(ch);
            }
            ch if ch.is_control() => {
                let _ = write!(escaped, "\\{:03o}", u32::from(ch));
            }
            ch => escaped.push(ch),
        }
    }
    let tilde = if escaped.starts_with('~') { "\\" } else { "" };
    match quote {
        Some('\'') => format!("'{escaped}'"),
        Some(_) => format!("\"{tilde}{escaped}\""),
        None => format!("{tilde}{escaped}"),
    }
}

/// What `set-option` was asked to do.
#[derive(Clone, Debug)]
pub(crate) struct SetOption {
    pub(crate) scope: Scope,
    pub(crate) target: Option<String>,
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    pub(crate) unset: bool,
    pub(crate) only_if_unset: bool,
    pub(crate) quiet: bool,
}

/// What `show-options` was asked to print.
#[derive(Clone, Debug)]
pub(crate) struct ShowOptions {
    pub(crate) scope: Scope,
    pub(crate) target: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) value_only: bool,
    pub(crate) quiet: bool,
}

impl CliConnection {
    /// The options of `buffer_id`.
    pub(crate) async fn buffer_options(
        &mut self,
        buffer_id: BufferId,
        command: &str,
    ) -> Result<BTreeMap<String, String>> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::Get {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await?;
        match response {
            ServerResponse::Buffer(response) => {
                ensure_matching_buffer_id(command, buffer_id, response.buffer.id)?;
                Ok(response.buffer.user_options)
            }
            other => Err(MuxError::protocol(format!(
                "unexpected response to {command}: {other:?}"
            ))),
        }
    }

    /// Set option `key` of `buffer_id` to `value`, or unset it with `None`.
    pub(crate) async fn set_buffer_option(
        &mut self,
        buffer_id: BufferId,
        key: String,
        value: Option<String>,
        command: &str,
    ) -> Result<()> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::SetUserOption {
                request_id: new_request_id(),
                buffer_id,
                key,
                value,
            }))
            .await?;
        match response {
            ServerResponse::Buffer(response) => {
                ensure_matching_buffer_id(command, buffer_id, response.buffer.id)
            }
            other => Err(MuxError::protocol(format!(
                "unexpected response to {command}: {other:?}"
            ))),
        }
    }

    pub(crate) async fn set_option(&mut self, request: SetOption) -> Result<String> {
        request.scope.ensure_pane("set-option")?;
        let Some(key) = option_key(&request.name) else {
            return if request.quiet {
                Ok(String::new())
            } else {
                Err(invalid_option(&request.name))
            };
        };
        let value = match (request.unset, request.value) {
            (true, _) => None,
            (false, Some(value)) => Some(value),
            (false, None) => return Err(MuxError::invalid_input("empty value")),
        };
        let buffer_id = self.resolve_buffer(request.target.as_deref()).await?;
        if request.only_if_unset
            && value.is_some()
            && self
                .buffer_options(buffer_id, "set-option")
                .await?
                .contains_key(key)
        {
            return Err(MuxError::invalid_input(format!(
                "already set: {}",
                request.name
            )));
        }
        self.set_buffer_option(buffer_id, key.to_owned(), value, "set-option")
            .await?;
        Ok(String::new())
    }

    pub(crate) async fn show_options(&mut self, request: ShowOptions) -> Result<String> {
        request.scope.ensure_pane("show-options")?;
        let key = match &request.name {
            Some(name) => match option_key(name) {
                Some(key) => Some(key),
                None if request.quiet => return Ok(String::new()),
                None => return Err(invalid_option(name)),
            },
            None => None,
        };
        let buffer_id = self.resolve_buffer(request.target.as_deref()).await?;
        let options = self.buffer_options(buffer_id, "show-options").await?;
        let mut shown = match key {
            Some(key) => match options.get(key) {
                Some(value) => vec![(option_name(key), value.clone())],
                None if request.quiet => Vec::new(),
                None => return Err(invalid_option(request.name.as_deref().unwrap_or(key))),
            },
            None => options
                .into_iter()
                .map(|(key, value)| (option_name(&key), value))
                .collect(),
        };
        shown.sort();
        Ok(shown
            .into_iter()
            .map(|(name, value)| {
                if request.value_only {
                    value
                } else {
                    format!("{name} {}", escape(&value))
                }
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_options_are_buffer_options_without_the_at() {
        assert_eq!(option_key("@tria-open-socket"), Some("tria-open-socket"));
        assert_eq!(option_key("remain-on-exit"), Some("remain-on-exit"));
        assert_eq!(option_key("@"), None);
        assert_eq!(option_key("status"), None);
        assert_eq!(option_name("is-vim"), "@is-vim");
        assert_eq!(option_name("remain-on-exit"), "remain-on-exit");
    }

    #[test]
    fn values_are_quoted_like_tmux() {
        assert_eq!(escape("/tmp/x.sock"), "/tmp/x.sock");
        assert_eq!(escape(""), "''");
        assert_eq!(escape("a b"), "\"a b\"");
        assert_eq!(escape("it's"), "\"it's\"");
        assert_eq!(escape("a\"b"), "'a\"b'");
        assert_eq!(escape("p q\"r"), "\"p q\\\"r\"");
        assert_eq!(escape("x$y"), "\"x\\$y\"");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("n\nl"), "n\\nl");
        assert_eq!(escape("~home"), "\\~home");
    }
}
