//! tmux target syntax for `-t`, parsed and resolved in one place.
//!
//! Every command that takes a session, window or pane target resolves it
//! here, so `send-keys`, `kill-window`, `switch-client` and the rest accept the
//! same forms:
//!
//! | Form | Means |
//! | --- | --- |
//! | `name` | session `name`, or the start of one name (tmux order: exact, then prefix) |
//! | `=name` | session `name` only |
//! | `$N` | session id `N` |
//! | `session:window` | a window: its number (from the base index), `@N` id, name, or the start of a name; `=name` for the exact name; `^`, `$`, `+[N]`, `-[N]` for the first, last, next or previous window |
//! | `session:window.pane` | pane `pane` of the window, by index in `list-panes` order, counting from 0 |
//! | `:window`, `.pane`, `:` | the same in the current session or window |
//! | `%N` | the pane showing buffer `N`, the value of `$EMBERS_BUFFER` (tmux `$TMUX_PANE`) |
//! | `@N` | the window whose root node is `N` |
//! | `N` | as a pane target, pane (leaf node) `N`, as `list-panes` prints it |
//!
//! A word without `:` is a window of the current session first, then a session,
//! as in tmux; a session target takes the session of whatever window or pane
//! it names. The current session, window and pane are those of the pane the
//! command runs in (`EMBERS_BUFFER`), else of the command prompt's client,
//! else the only session's.

use embers_core::{BufferId, MuxError, NodeId, Result, SessionId};
use embers_protocol::{
    BufferLocationAttachment, BufferRequest, ClientMessage, SessionRecord, SessionSnapshot,
};

use crate::{
    CliConnection, ResolvedPane, ResolvedWindow, expect_buffer_location, protocol_tab,
    resolve_single_window_index, resolve_window_index, resolved_pane, root_tabs, visible_leaf_ids,
    window_active_pane, window_containing, window_title,
};
use embers_core::new_request_id;

/// What a command's `-t` names, which decides how a word without `:` is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Session,
    Window,
    Pane,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionSpec {
    Current,
    Id(SessionId),
    Exact(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowSpec {
    Current,
    Id(NodeId),
    Exact(String),
    /// A window number or name, as `resolve_window_index` reads it, falling
    /// back to the start of a name.
    Name(String),
    Start,
    End,
    Next(u32),
    Previous(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneSpec {
    Current,
    Index(usize),
    Buffer(BufferId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// `%N`: the pane showing buffer `N`.
    Buffer(BufferId),
    /// `@N`: the window whose root node is `N`.
    WindowId(NodeId),
    /// A plain number given as a pane target: that pane (leaf node).
    Node(NodeId),
    /// `[session][:[window][.pane]]`, any part of which may be left out.
    Path {
        session: SessionSpec,
        window: WindowSpec,
        pane: PaneSpec,
    },
    /// A word without `:` given as a window or pane target: a window of the
    /// current session, else a session.
    Word {
        window: WindowSpec,
        session: SessionSpec,
    },
}

impl Target {
    pub(crate) fn parse(raw: &str, kind: TargetKind) -> Result<Self> {
        if let Some(id) = raw.strip_prefix('%') {
            return parse_id(id)
                .map(|id| Self::Buffer(BufferId(id)))
                .ok_or_else(|| invalid(raw, "pane"));
        }
        if let Some(id) = raw.strip_prefix('@').filter(|id| !id.is_empty()) {
            return parse_id(id)
                .map(|id| Self::WindowId(NodeId(id)))
                .ok_or_else(|| invalid(raw, "window"));
        }
        if let Some((session, rest)) = raw.split_once(':') {
            let (window, pane) = match kind {
                TargetKind::Pane => split_pane(rest),
                TargetKind::Session | TargetKind::Window => (rest, None),
            };
            return Ok(Self::Path {
                session: parse_session(session)?,
                window: parse_window(window)?,
                pane: pane
                    .map(parse_pane)
                    .transpose()?
                    .unwrap_or(PaneSpec::Current),
            });
        }
        if raw.starts_with('$') && raw.len() > 1 {
            return Ok(Self::path(parse_session(raw)?, WindowSpec::Current));
        }
        match kind {
            TargetKind::Session => Ok(Self::path(parse_session(raw)?, WindowSpec::Current)),
            TargetKind::Window | TargetKind::Pane => {
                if kind == TargetKind::Pane {
                    if let Some(pane) = raw.strip_prefix('.') {
                        return Ok(Self::Path {
                            session: SessionSpec::Current,
                            window: WindowSpec::Current,
                            pane: parse_pane(pane)?,
                        });
                    }
                    if let Some(id) = parse_id(raw) {
                        return Ok(Self::Node(NodeId(id)));
                    }
                    if let (window, Some(pane)) = split_pane(raw) {
                        return Ok(Self::Path {
                            session: SessionSpec::Current,
                            window: parse_window(window)?,
                            pane: parse_pane(pane)?,
                        });
                    }
                }
                let window = parse_window(raw)?;
                match window {
                    WindowSpec::Name(_) | WindowSpec::Exact(_) => Ok(Self::Word {
                        window,
                        session: parse_session(raw)?,
                    }),
                    window => Ok(Self::path(SessionSpec::Current, window)),
                }
            }
        }
    }

    fn path(session: SessionSpec, window: WindowSpec) -> Self {
        Self::Path {
            session,
            window,
            pane: PaneSpec::Current,
        }
    }
}

fn invalid(raw: &str, what: &str) -> MuxError {
    MuxError::invalid_input(format!("'{raw}' is not a valid {what} target"))
}

fn parse_id(raw: &str) -> Option<u64> {
    (!raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| raw.parse().ok())
        .flatten()
}

/// Split `window.pane`, taking the last `.` only when what follows reads as a
/// pane, so window names may contain dots.
fn split_pane(raw: &str) -> (&str, Option<&str>) {
    match raw.rsplit_once('.') {
        Some((window, pane))
            if pane.is_empty()
                || parse_id(pane).is_some()
                || pane.strip_prefix('%').and_then(parse_id).is_some() =>
        {
            (window, Some(pane))
        }
        _ => (raw, None),
    }
}

fn parse_session(raw: &str) -> Result<SessionSpec> {
    if raw.is_empty() {
        return Ok(SessionSpec::Current);
    }
    if let Some(id) = raw.strip_prefix('$') {
        return parse_id(id)
            .map(|id| SessionSpec::Id(SessionId(id)))
            .ok_or_else(|| invalid(raw, "session"));
    }
    Ok(match raw.strip_prefix('=') {
        Some(name) => SessionSpec::Exact(name.to_owned()),
        None => SessionSpec::Name(raw.to_owned()),
    })
}

fn parse_window(raw: &str) -> Result<WindowSpec> {
    let offset = |rest: &str| -> Result<u32> {
        if rest.is_empty() {
            return Ok(1);
        }
        rest.parse().map_err(|_| invalid(raw, "window"))
    };
    Ok(match raw {
        "" | "@" | "{current}" => WindowSpec::Current,
        "^" | "{start}" => WindowSpec::Start,
        "$" | "{end}" => WindowSpec::End,
        "{next}" => WindowSpec::Next(1),
        "{previous}" => WindowSpec::Previous(1),
        _ => {
            if let Some(rest) = raw.strip_prefix('+') {
                WindowSpec::Next(offset(rest)?)
            } else if let Some(rest) = raw.strip_prefix('-') {
                WindowSpec::Previous(offset(rest)?)
            } else if let Some(id) = raw.strip_prefix('@') {
                WindowSpec::Id(NodeId(parse_id(id).ok_or_else(|| invalid(raw, "window"))?))
            } else if let Some(name) = raw.strip_prefix('=') {
                WindowSpec::Exact(name.to_owned())
            } else {
                WindowSpec::Name(raw.to_owned())
            }
        }
    })
}

fn parse_pane(raw: &str) -> Result<PaneSpec> {
    if raw.is_empty() || raw == "@" || raw == "{active}" {
        return Ok(PaneSpec::Current);
    }
    if let Some(id) = raw.strip_prefix('%') {
        return parse_id(id)
            .map(|id| PaneSpec::Buffer(BufferId(id)))
            .ok_or_else(|| invalid(raw, "pane"));
    }
    parse_id(raw)
        .and_then(|index| usize::try_from(index).ok())
        .map(PaneSpec::Index)
        .ok_or_else(|| invalid(raw, "pane"))
}

impl CliConnection {
    /// The session `target` names, or the current session.
    pub(crate) async fn target_session(&mut self, target: Option<&str>) -> Result<SessionRecord> {
        let Some(raw) = target else {
            return self.current_session().await;
        };
        let session_id = match Target::parse(raw, TargetKind::Session)? {
            Target::Path {
                session: SessionSpec::Current,
                window: WindowSpec::Current,
                pane: PaneSpec::Current,
            } => return self.current_session().await,
            Target::Path {
                session,
                window: WindowSpec::Current,
                pane: PaneSpec::Current,
            } => return self.session_by_spec(&session).await,
            Target::Buffer(_) | Target::Node(_) => {
                self.target_pane(Some(raw)).await?.snapshot.session.id
            }
            _ => self.target_window(Some(raw)).await?.snapshot.session.id,
        };
        self.session_by_spec(&SessionSpec::Id(session_id)).await
    }

    /// What `switch-client -t` shows: the session `target` names, and the
    /// window or pane it names, if any. As in tmux, a target with `:`, `.`
    /// or `%` is read as a pane (`work:2` is window 2 and its active pane),
    /// anything else as a session; `@N` names a window either way.
    pub(crate) async fn switch_target(&mut self, raw: &str) -> Result<(SessionId, Option<NodeId>)> {
        let kind = if raw.contains([':', '.', '%']) {
            TargetKind::Pane
        } else {
            TargetKind::Session
        };
        match Target::parse(raw, kind)? {
            Target::Path {
                window: WindowSpec::Current,
                pane: PaneSpec::Current,
                ..
            } => Ok((self.target_session(Some(raw)).await?.id, None)),
            Target::Buffer(_)
            | Target::Node(_)
            | Target::Path {
                pane: PaneSpec::Index(_) | PaneSpec::Buffer(_),
                ..
            } => {
                let pane = self.target_pane(Some(raw)).await?;
                Ok((pane.snapshot.session.id, Some(pane.leaf_id)))
            }
            Target::WindowId(_) | Target::Path { .. } | Target::Word { .. } => {
                let window = self.target_window(Some(raw)).await?;
                Ok((window.snapshot.session.id, Some(window.child_id)))
            }
        }
    }

    /// The window `target` names, or the current window.
    pub(crate) async fn target_window(&mut self, target: Option<&str>) -> Result<ResolvedWindow> {
        let target = match target {
            Some(raw) => Target::parse(raw, TargetKind::Window)?,
            None => Target::path(SessionSpec::Current, WindowSpec::Current),
        };
        match target {
            Target::Buffer(_) | Target::Node(_) => {
                let pane = self.pane_by_target(target).await?;
                let (index, child_id) = window_containing(&pane.snapshot, pane.leaf_id)?;
                Ok(ResolvedWindow {
                    snapshot: pane.snapshot,
                    index,
                    child_id,
                })
            }
            Target::WindowId(node_id) => self.window_by_id(node_id).await,
            Target::Path {
                session, window, ..
            } => self.window_by_spec(&session, &window).await,
            Target::Word { window, session } => self.window_from_word(&window, &session).await,
        }
    }

    /// The pane `target` names, or the current pane.
    pub(crate) async fn target_pane(&mut self, target: Option<&str>) -> Result<ResolvedPane> {
        match target {
            Some(raw) => {
                let target = Target::parse(raw, TargetKind::Pane)?;
                self.pane_by_target(target).await
            }
            None => self.current_pane().await,
        }
    }

    async fn pane_by_target(&mut self, target: Target) -> Result<ResolvedPane> {
        match target {
            Target::Buffer(buffer_id) => self.pane_showing(buffer_id).await,
            Target::Node(pane_id) => {
                let snapshot = self.find_session_containing_pane(pane_id).await?;
                resolved_pane(snapshot, pane_id)
            }
            Target::WindowId(node_id) => {
                let window = self.window_by_id(node_id).await?;
                self.pane_in(window, &PaneSpec::Current).await
            }
            Target::Word { window, session } => {
                let window = self.window_from_word(&window, &session).await?;
                self.pane_in(window, &PaneSpec::Current).await
            }
            Target::Path {
                session,
                window,
                pane,
            } => {
                if let PaneSpec::Buffer(buffer_id) = pane {
                    return self.pane_showing(buffer_id).await;
                }
                if session == SessionSpec::Current
                    && window == WindowSpec::Current
                    && pane == PaneSpec::Current
                {
                    return self.current_pane().await;
                }
                let window = self.window_by_spec(&session, &window).await?;
                self.pane_in(window, &pane).await
            }
        }
    }

    /// A word without `:`: a window of the current session, else the current
    /// window of a session, like tmux. When neither exists the window's error
    /// is reported.
    async fn window_from_word(
        &mut self,
        window: &WindowSpec,
        session: &SessionSpec,
    ) -> Result<ResolvedWindow> {
        match self.window_by_spec(&SessionSpec::Current, window).await {
            Ok(window) => Ok(window),
            Err(window_error) => match self.window_by_spec(session, &WindowSpec::Current).await {
                Ok(window) => Ok(window),
                Err(_) if window_error.is_not_found() => Err(window_error),
                Err(session_error) => Err(session_error),
            },
        }
    }

    async fn window_by_id(&mut self, node_id: NodeId) -> Result<ResolvedWindow> {
        let snapshot = self.find_session_containing_pane(node_id).await?;
        let (index, child_id) = window_in(&snapshot, &WindowSpec::Id(node_id), None, None)?;
        Ok(ResolvedWindow {
            snapshot,
            index,
            child_id,
        })
    }

    /// Pane `spec` of `window`. The current pane is the one the command runs
    /// in when it is in the window, else the window's active pane.
    async fn pane_in(&mut self, window: ResolvedWindow, spec: &PaneSpec) -> Result<ResolvedPane> {
        let leaves = visible_leaf_ids(&window.snapshot, window.child_id)?;
        let pane_id = match spec {
            PaneSpec::Buffer(buffer_id) => return self.pane_showing(*buffer_id).await,
            PaneSpec::Index(index) => *leaves.get(*index).ok_or_else(|| {
                MuxError::not_found(format!("pane index {index} is out of range"))
            })?,
            PaneSpec::Current => {
                let caller = self
                    .pane_location()
                    .await?
                    .and_then(|location| location.node_id());
                caller
                    .filter(|pane_id| leaves.contains(pane_id))
                    .or_else(|| window_active_pane(&window.snapshot, window.child_id, &leaves))
                    .ok_or_else(|| MuxError::not_found("window has no panes"))?
            }
        };
        resolved_pane(window.snapshot, pane_id)
    }

    async fn pane_showing(&mut self, buffer_id: BufferId) -> Result<ResolvedPane> {
        let response = self
            .request(ClientMessage::Buffer(BufferRequest::GetLocation {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await?;
        let location = expect_buffer_location(response, "buffer location")?;
        match location.attachment {
            BufferLocationAttachment::Session {
                session_id,
                node_id,
            }
            | BufferLocationAttachment::Floating {
                session_id,
                node_id,
                ..
            } => {
                let snapshot = self.session_snapshot(session_id).await?;
                resolved_pane(snapshot, node_id)
            }
            BufferLocationAttachment::Detached => Err(MuxError::not_found(format!(
                "buffer {buffer_id} is not shown in a session"
            ))),
        }
    }

    async fn window_by_spec(
        &mut self,
        session: &SessionSpec,
        window: &WindowSpec,
    ) -> Result<ResolvedWindow> {
        let caller = self
            .pane_location()
            .await?
            .and_then(|location| match location.attachment {
                BufferLocationAttachment::Session {
                    session_id,
                    node_id,
                } => Some((session_id, node_id)),
                _ => None,
            });
        let record = self.session_by_spec(session).await?;
        let snapshot = self.session_snapshot(record.id).await?;
        let current = match caller {
            Some((session_id, node_id)) if session_id == snapshot.session.id => {
                Some(window_containing(&snapshot, node_id)?)
            }
            _ => None,
        };
        let base_index = self.base_index_for(&snapshot);
        let (index, child_id) = window_in(&snapshot, window, base_index, current)?;
        Ok(ResolvedWindow {
            snapshot,
            index,
            child_id,
        })
    }

    async fn session_by_spec(&mut self, spec: &SessionSpec) -> Result<SessionRecord> {
        let sessions = match spec {
            SessionSpec::Current => return self.current_session().await,
            _ => self.list_sessions().await?,
        };
        let find = |predicate: &dyn Fn(&SessionRecord) -> bool| {
            sessions
                .iter()
                .filter(|session| predicate(session))
                .cloned()
                .collect::<Vec<_>>()
        };
        let (matches, label) = match spec {
            SessionSpec::Current => unreachable!("handled above"),
            SessionSpec::Id(id) => (find(&|session| session.id == *id), format!("${}", id.0)),
            SessionSpec::Exact(name) => {
                (find(&|session| session.name == *name), format!("'{name}'"))
            }
            SessionSpec::Name(name) => {
                let exact = find(&|session| session.name == *name);
                let matches = if exact.is_empty() {
                    find(&|session| session.name.starts_with(name.as_str()))
                } else {
                    exact
                };
                (matches, format!("'{name}'"))
            }
        };
        match <[SessionRecord; 1]>::try_from(matches) {
            Ok([session]) => Ok(session),
            Err(matches) if matches.is_empty() => Err(MuxError::not_found(format!(
                "session {label} was not found"
            ))),
            Err(matches) => Err(MuxError::invalid_input(format!(
                "session {label} matches more than one session: {}",
                matches
                    .iter()
                    .map(|session| session.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// The session of the pane the command runs in, else the command
    /// prompt's, else the only one.
    pub(crate) async fn current_session(&mut self) -> Result<SessionRecord> {
        let pane_session = self
            .pane_location()
            .await?
            .and_then(|location| location.session_id());
        let sessions = self.list_sessions().await?;
        if let Some(session_id) = pane_session {
            return sessions
                .into_iter()
                .find(|session| session.id == session_id)
                .ok_or_else(|| MuxError::not_found(format!("session {session_id} was not found")));
        }
        if let Some(session) = self.default_session.and_then(|session_id| {
            sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
        }) {
            return Ok(session);
        }
        match sessions.as_slice() {
            [session] => Ok(session.clone()),
            [] => Err(MuxError::not_found("no sessions exist")),
            _ => Err(MuxError::invalid_input(
                "session target is required when multiple sessions exist",
            )),
        }
    }

    /// The pane the command runs in, else the current session's focused pane.
    async fn current_pane(&mut self) -> Result<ResolvedPane> {
        if let Some(location) = self.pane_location().await? {
            let (Some(session_id), Some(pane_id)) = (location.session_id(), location.node_id())
            else {
                return Err(MuxError::not_found(format!(
                    "buffer {} of this pane ({}) is not shown in a session",
                    location.buffer_id,
                    embers_server::BUFFER_ENV_VAR
                )));
            };
            let snapshot = self.session_snapshot(session_id).await?;
            return resolved_pane(snapshot, pane_id);
        }
        let session = self.current_session().await?;
        let snapshot = self.session_snapshot(session.id).await?;
        let pane_id = snapshot
            .session
            .focused_leaf_id
            .ok_or_else(|| MuxError::not_found("session has no focused pane"))?;
        resolved_pane(snapshot, pane_id)
    }
}

/// Window `spec` of `snapshot`'s session: its root tab index and the node it
/// shows. `current` is the window the command runs in, if it is in this
/// session; otherwise the session's active window is current.
fn window_in(
    snapshot: &SessionSnapshot,
    spec: &WindowSpec,
    base_index: Option<u32>,
    current: Option<(u32, NodeId)>,
) -> Result<(u32, NodeId)> {
    let Some((_, tabs)) = root_tabs(snapshot)? else {
        // A session without root tabs is one window.
        let root = snapshot.session.root_node_id;
        let found = match spec {
            WindowSpec::Current
            | WindowSpec::Start
            | WindowSpec::End
            | WindowSpec::Next(_)
            | WindowSpec::Previous(_) => true,
            WindowSpec::Id(node_id) => *node_id == root,
            WindowSpec::Exact(name) => *name == window_title(snapshot, root)?,
            WindowSpec::Name(name) => {
                resolve_single_window_index(&window_title(snapshot, root)?, Some(name), base_index)
                    .is_ok()
            }
        };
        return found
            .then_some((0, root))
            .ok_or_else(|| window_not_found(spec));
    };
    let count = tabs.tabs.len();
    let current_index = current.map_or(tabs.active, |(index, _)| index);
    let relative = |forward: bool, offset: u32| -> Option<u32> {
        let count = u32::try_from(count).ok().filter(|count| *count > 0)?;
        let offset = offset % count;
        Some(if forward {
            (current_index + offset) % count
        } else {
            (current_index + count - offset) % count
        })
    };
    let index = match spec {
        WindowSpec::Current => Some(current_index),
        WindowSpec::Start => (count > 0).then_some(0),
        WindowSpec::End => count
            .checked_sub(1)
            .map(|last| u32::try_from(last).expect("tab index fits into protocol width")),
        WindowSpec::Next(offset) => relative(true, *offset),
        WindowSpec::Previous(offset) => relative(false, *offset),
        WindowSpec::Id(node_id) => tabs
            .tabs
            .iter()
            .position(|tab| tab.child_id == *node_id)
            .map(|index| u32::try_from(index).expect("tab index fits into protocol width")),
        WindowSpec::Exact(name) => {
            let matches = tabs
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| tab.title == *name)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [index] => Some(u32::try_from(*index).expect("tab index fits into protocol width")),
                [] => None,
                _ => {
                    return Err(MuxError::conflict(format!(
                        "window title '{name}' matched multiple root tabs"
                    )));
                }
            }
        }
        WindowSpec::Name(name) => match resolve_window_index(tabs, Some(name), base_index) {
            Ok(index) => Some(index),
            Err(error) if error.is_not_found() && name.parse::<u64>().is_err() => {
                let matches = tabs
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, tab)| tab.title.starts_with(name.as_str()))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [index] => {
                        Some(u32::try_from(*index).expect("tab index fits into protocol width"))
                    }
                    [] => return Err(error),
                    _ => {
                        return Err(MuxError::conflict(format!(
                            "window '{name}' matches more than one window"
                        )));
                    }
                }
            }
            Err(error) => return Err(error),
        },
    };
    let index = index.ok_or_else(|| window_not_found(spec))?;
    let tab = protocol_tab(tabs, index).ok_or_else(|| {
        MuxError::not_found(format!(
            "window index {index} is not present in session {}",
            snapshot.session.id
        ))
    })?;
    Ok((index, tab.child_id))
}

fn window_not_found(spec: &WindowSpec) -> MuxError {
    MuxError::not_found(match spec {
        WindowSpec::Id(node_id) => format!("window @{node_id} was not found"),
        WindowSpec::Exact(name) | WindowSpec::Name(name) => {
            format!("window '{name}' was not found")
        }
        _ => "the session has no windows".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(session: SessionSpec, window: WindowSpec, pane: PaneSpec) -> Target {
        Target::Path {
            session,
            window,
            pane,
        }
    }

    #[test]
    fn parses_tmux_target_forms() {
        use TargetKind::{Pane, Session, Window};
        let name = |value: &str| value.to_owned();
        assert_eq!(
            Target::parse("work", Session).unwrap(),
            path(
                SessionSpec::Name(name("work")),
                WindowSpec::Current,
                PaneSpec::Current
            )
        );
        assert_eq!(
            Target::parse("=work", Session).unwrap(),
            path(
                SessionSpec::Exact(name("work")),
                WindowSpec::Current,
                PaneSpec::Current
            )
        );
        assert_eq!(
            Target::parse("$3", Window).unwrap(),
            path(
                SessionSpec::Id(SessionId(3)),
                WindowSpec::Current,
                PaneSpec::Current
            )
        );
        assert_eq!(
            Target::parse("work:2.1", Pane).unwrap(),
            path(
                SessionSpec::Name(name("work")),
                WindowSpec::Name(name("2")),
                PaneSpec::Index(1)
            )
        );
        assert_eq!(
            Target::parse("=work:=logs", Window).unwrap(),
            path(
                SessionSpec::Exact(name("work")),
                WindowSpec::Exact(name("logs")),
                PaneSpec::Current
            )
        );
        assert_eq!(
            Target::parse(":v1.2", Window).unwrap(),
            path(
                SessionSpec::Current,
                WindowSpec::Name(name("v1.2")),
                PaneSpec::Current
            )
        );
        assert_eq!(
            Target::parse(".1", Pane).unwrap(),
            path(
                SessionSpec::Current,
                WindowSpec::Current,
                PaneSpec::Index(1)
            )
        );
        assert_eq!(
            Target::parse(":", Pane).unwrap(),
            path(SessionSpec::Current, WindowSpec::Current, PaneSpec::Current)
        );
        assert_eq!(
            Target::parse(":+2", Window).unwrap(),
            path(SessionSpec::Current, WindowSpec::Next(2), PaneSpec::Current)
        );
        assert_eq!(
            Target::parse("^", Window).unwrap(),
            path(SessionSpec::Current, WindowSpec::Start, PaneSpec::Current)
        );
        assert_eq!(
            Target::parse("%7", Session).unwrap(),
            Target::Buffer(BufferId(7))
        );
        assert_eq!(
            Target::parse("@4", Pane).unwrap(),
            Target::WindowId(NodeId(4))
        );
        assert_eq!(Target::parse("12", Pane).unwrap(), Target::Node(NodeId(12)));
        assert_eq!(
            Target::parse("12", Window).unwrap(),
            Target::Word {
                window: WindowSpec::Name(name("12")),
                session: SessionSpec::Name(name("12")),
            }
        );
        assert_eq!(
            Target::parse("logs.1", Pane).unwrap(),
            path(
                SessionSpec::Current,
                WindowSpec::Name(name("logs")),
                PaneSpec::Index(1)
            )
        );
        assert!(Target::parse("%x", Pane).is_err());
        assert!(Target::parse("work:+x", Window).is_err());
    }
}
