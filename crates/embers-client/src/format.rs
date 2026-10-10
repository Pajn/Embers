//! A small subset of tmux formats, for `display-message` and prompt text.
//!
//! `#S`, `#I`, `#W`, `#D` and `#T` (and their long `#{...}` names) expand to
//! the session name, window number, window name, pane id and pane title;
//! `#{pane_current_path}` and `#{pane_current_command}` have no short form.
//! `##` is a literal `#`. An unknown `#{name}` expands to nothing, like tmux;
//! any other `#` is kept as is. There are no conditionals or modifiers.

use std::path::Path;

use embers_core::{NodeId, SessionId};
use embers_protocol::NodeRecordKind;

use crate::numbering::window_number;
use crate::state::ClientState;

/// The values format variables expand to, for one pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatContext {
    pub session_name: String,
    pub window_index: u64,
    pub window_name: String,
    pub pane_id: Option<NodeId>,
    pub pane_title: String,
    pub pane_current_path: String,
    pub pane_current_command: String,
}

impl FormatContext {
    /// The context for `pane_id` in `session_id`, or for the session's
    /// focused pane when `pane_id` is `None`. Windows are numbered from
    /// `base_index` (0 when `None`). Returns `None` for an unknown session.
    pub fn from_state(
        state: &ClientState,
        session_id: SessionId,
        pane_id: Option<NodeId>,
        base_index: Option<u32>,
    ) -> Option<Self> {
        let session = state.sessions.get(&session_id)?;
        let pane_id = pane_id.or(session.focused_leaf_id);
        let buffer = pane_id
            .and_then(|pane_id| state.nodes.get(&pane_id))
            .and_then(|node| node.buffer_view.as_ref())
            .and_then(|view| state.buffers.get(&view.buffer_id));

        let (window_position, window_name) =
            match state.nodes.get(&session.root_node_id).and_then(|root| {
                (root.kind == NodeRecordKind::Tabs)
                    .then_some(root.tabs.as_ref())
                    .flatten()
            }) {
                Some(tabs) => {
                    let position = pane_id
                        .and_then(|pane_id| root_tab_position(state, session.root_node_id, pane_id))
                        .unwrap_or(tabs.active as usize);
                    let name = tabs
                        .tabs
                        .get(position)
                        .map(|tab| tab.title.clone())
                        .unwrap_or_default();
                    (position, name)
                }
                // A session without root tabs is one window named after its pane.
                None => (
                    0,
                    buffer
                        .map(|buffer| buffer.title.clone())
                        .unwrap_or_default(),
                ),
            };

        let pane_current_command = buffer
            .and_then(|buffer| {
                buffer
                    .foreground_command
                    .as_ref()
                    .and_then(|command| command.first())
                    .or_else(|| buffer.command.first())
            })
            .map(|program| program_name(program))
            .unwrap_or_default();

        Some(Self {
            session_name: session.name.clone(),
            window_index: window_number(window_position, base_index),
            window_name,
            pane_id,
            pane_title: buffer
                .map(|buffer| buffer.title.clone())
                .unwrap_or_default(),
            pane_current_path: buffer
                .and_then(|buffer| buffer.cwd.clone())
                .unwrap_or_default(),
            pane_current_command,
        })
    }

    /// The value of the variable `name` (long form), if it is one.
    pub fn lookup(&self, name: &str) -> Option<String> {
        Some(match name {
            "session_name" => self.session_name.clone(),
            "window_index" => self.window_index.to_string(),
            "window_name" => self.window_name.clone(),
            "pane_id" => self
                .pane_id
                .map(|pane_id| pane_id.to_string())
                .unwrap_or_default(),
            "pane_title" => self.pane_title.clone(),
            "pane_current_path" => self.pane_current_path.clone(),
            "pane_current_command" => self.pane_current_command.clone(),
            _ => return None,
        })
    }
}

/// Expand the format variables in `template`.
pub fn expand_format(template: &str, context: &FormatContext) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '#' {
            output.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('#') => {
                chars.next();
                output.push('#');
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    name.push(ch);
                }
                if closed {
                    output.push_str(&context.lookup(&name).unwrap_or_default());
                } else {
                    output.push_str("#{");
                    output.push_str(&name);
                }
            }
            Some(short) => match short_alias(short) {
                Some(name) => {
                    chars.next();
                    output.push_str(&context.lookup(name).unwrap_or_default());
                }
                None => output.push('#'),
            },
            None => output.push('#'),
        }
    }
    output
}

fn short_alias(ch: char) -> Option<&'static str> {
    Some(match ch {
        'S' => "session_name",
        'I' => "window_index",
        'W' => "window_name",
        'D' => "pane_id",
        'T' => "pane_title",
        _ => return None,
    })
}

/// The position of the root tab holding `node_id`, if it is in one.
fn root_tab_position(state: &ClientState, root_id: NodeId, node_id: NodeId) -> Option<usize> {
    let tabs = state.nodes.get(&root_id)?.tabs.as_ref()?;
    let mut current = node_id;
    loop {
        if let Some(position) = tabs.tabs.iter().position(|tab| tab.child_id == current) {
            return Some(position);
        }
        current = state.nodes.get(&current)?.parent_id?;
    }
}

fn program_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> FormatContext {
        FormatContext {
            session_name: "work".to_owned(),
            window_index: 2,
            window_name: "editor".to_owned(),
            pane_id: Some(NodeId(7)),
            pane_title: "vim".to_owned(),
            pane_current_path: "/srv/app".to_owned(),
            pane_current_command: "nvim".to_owned(),
        }
    }

    #[test]
    fn expands_short_and_long_variables() {
        assert_eq!(
            expand_format("[#S] #I:#W #D #T", &context()),
            "[work] 2:editor 7 vim"
        );
        assert_eq!(
            expand_format(
                "#{session_name}:#{window_index} #{pane_current_command} in #{pane_current_path}",
                &context()
            ),
            "work:2 nvim in /srv/app"
        );
    }

    #[test]
    fn keeps_literals_and_drops_unknown_long_names() {
        assert_eq!(expand_format("100## #x #", &context()), "100# #x #");
        assert_eq!(expand_format("a#{nope}b", &context()), "ab");
        assert_eq!(expand_format("a#{open", &context()), "a#{open");
        assert_eq!(expand_format("", &context()), "");
    }

    #[test]
    fn program_name_strips_directories() {
        assert_eq!(program_name("/usr/bin/nvim"), "nvim");
        assert_eq!(program_name("zsh"), "zsh");
    }
}
