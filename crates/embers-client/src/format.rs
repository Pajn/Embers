//! A small subset of tmux formats, for `display-message` and prompt text.
//!
//! `#S`, `#I`, `#W`, `#D` and `#T` (and their long `#{...}` names) expand to
//! the session name, window number, window name, pane id and pane title;
//! `#{pane_current_path}` and `#{pane_current_command}` have no short form.
//! `##` is a literal `#`. An unknown `#{name}` expands to nothing, like tmux;
//! any other `#` is kept as is. There are no conditionals or modifiers.
//!
//! The ids, counts and flags tmux scripts read back after creating something
//! (`#{session_id}`, `#{window_id}`, `#{buffer_id}`, `#{pane_index}`,
//! `#{pane_active}` and so on) have long names only; see
//! [`FormatContext::lookup`]. Callers that know more than the client state,
//! such as the CLI with its layout projection, add variables in
//! [`FormatContext::extra`].

use std::collections::BTreeMap;

use embers_core::{BufferId, NodeId, SessionId};
use embers_protocol::{BufferRecordState, NodeRecordKind};

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
    pub session_id: Option<SessionId>,
    /// The node a window shows: its root tab's child.
    pub window_id: Option<NodeId>,
    pub window_active: bool,
    pub window_panes: usize,
    pub session_windows: usize,
    pub buffer_id: Option<BufferId>,
    /// The pane's position among its window's panes, in `list-panes` order.
    pub pane_index: Option<usize>,
    pub pane_active: bool,
    pub pane_pid: Option<u32>,
    pub pane_width: u16,
    pub pane_height: u16,
    pub pane_dead: bool,
    pub pane_dead_status: Option<i32>,
    /// More variables by long name, checked after the built-in ones.
    pub extra: BTreeMap<String, String>,
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
            .and_then(|buffer| buffer.process_name())
            .unwrap_or_default();

        let root_tabs = state
            .nodes
            .get(&session.root_node_id)
            .and_then(|root| root.tabs.as_ref());
        let window_id = match root_tabs {
            Some(tabs) => tabs.tabs.get(window_position).map(|tab| tab.child_id),
            None => Some(session.root_node_id),
        };
        let window_leaves = window_id
            .map(|window_id| visible_leaves(state, window_id))
            .unwrap_or_default();

        Some(Self {
            session_id: Some(session_id),
            window_id,
            window_active: root_tabs.is_none_or(|tabs| tabs.active as usize == window_position),
            window_panes: window_leaves.len(),
            session_windows: root_tabs.map_or(1, |tabs| tabs.tabs.len()),
            buffer_id: buffer.map(|buffer| buffer.id),
            pane_index: pane_id
                .and_then(|pane_id| window_leaves.iter().position(|leaf| *leaf == pane_id)),
            pane_active: pane_id.is_some() && pane_id == session.focused_leaf_id,
            pane_pid: buffer.and_then(|buffer| buffer.pid),
            pane_width: buffer.map_or(0, |buffer| buffer.pty_size.cols),
            pane_height: buffer.map_or(0, |buffer| buffer.pty_size.rows),
            pane_dead: buffer.is_some_and(|buffer| buffer.state == BufferRecordState::Exited),
            pane_dead_status: buffer.and_then(|buffer| buffer.exit_code),
            extra: BTreeMap::new(),
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
            // tmux's form, `%N`, which `-t` takes back: the pane showing
            // buffer N.
            "pane_id" => self
                .buffer_id
                .map(|buffer_id| format!("%{buffer_id}"))
                .unwrap_or_default(),
            // The layout node the pane is, which plain-number targets take.
            "node_id" => self
                .pane_id
                .map(|pane_id| pane_id.to_string())
                .unwrap_or_default(),
            "pane_title" => self.pane_title.clone(),
            "pane_current_path" => self.pane_current_path.clone(),
            "pane_current_command" => self.pane_current_command.clone(),
            "session_id" => self
                .session_id
                .map(|session_id| format!("${}", session_id.0))
                .unwrap_or_default(),
            "window_id" => self
                .window_id
                .map(|window_id| format!("@{window_id}"))
                .unwrap_or_default(),
            "window_active" => flag(self.window_active),
            "window_panes" => self.window_panes.to_string(),
            "session_windows" => self.session_windows.to_string(),
            "buffer_id" => self
                .buffer_id
                .map(|buffer_id| buffer_id.to_string())
                .unwrap_or_default(),
            "pane_index" => self
                .pane_index
                .map(|index| index.to_string())
                .unwrap_or_default(),
            "pane_active" => flag(self.pane_active),
            "pane_pid" => self.pane_pid.map(|pid| pid.to_string()).unwrap_or_default(),
            "pane_width" => self.pane_width.to_string(),
            "pane_height" => self.pane_height.to_string(),
            "pane_dead" => flag(self.pane_dead),
            "pane_dead_status" => self
                .pane_dead_status
                .map(|status| status.to_string())
                .unwrap_or_default(),
            _ => return self.extra.get(name).cloned(),
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

fn flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_owned()
}

/// The panes shown under `node_id`, following each tabs node's active tab, in
/// layout order.
fn visible_leaves(state: &ClientState, node_id: NodeId) -> Vec<NodeId> {
    let Some(node) = state.nodes.get(&node_id) else {
        return Vec::new();
    };
    match node.kind {
        NodeRecordKind::BufferView => vec![node_id],
        NodeRecordKind::Split => node
            .split
            .iter()
            .flat_map(|split| split.child_ids.iter())
            .flat_map(|child_id| visible_leaves(state, *child_id))
            .collect(),
        NodeRecordKind::Tabs => node
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tabs.get(tabs.active as usize))
            .map(|tab| visible_leaves(state, tab.child_id))
            .unwrap_or_default(),
    }
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
            buffer_id: Some(BufferId(12)),
            pane_index: Some(1),
            pane_active: true,
            extra: BTreeMap::from([("pane_at_left".to_owned(), "1".to_owned())]),
            ..FormatContext::default()
        }
    }

    #[test]
    fn expands_short_and_long_variables() {
        assert_eq!(
            expand_format("[#S] #I:#W #D #T", &context()),
            "[work] 2:editor %12 vim"
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
    fn expands_ids_flags_and_extra_variables() {
        assert_eq!(
            expand_format(
                "#{buffer_id} #{pane_index} #{pane_active} #{window_active} #{pane_at_left}",
                &context()
            ),
            "12 1 1 0 1"
        );
    }

    #[test]
    fn keeps_literals_and_drops_unknown_long_names() {
        assert_eq!(expand_format("100## #x #", &context()), "100# #x #");
        assert_eq!(expand_format("a#{nope}b", &context()), "ab");
        assert_eq!(expand_format("a#{open", &context()), "a#{open");
        assert_eq!(expand_format("", &context()), "");
    }
}
