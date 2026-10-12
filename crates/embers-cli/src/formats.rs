//! tmux formats for CLI output: `display-message`, `-P -F` on the commands
//! that create things, and `list-* -F`.
//!
//! [`FormatContext`] expands what the session's state holds. The CLI adds the
//! variables that need its layout projection or the client list:
//! `#{pane_at_left}`, `#{pane_at_right}`, `#{pane_at_top}`,
//! `#{pane_at_bottom}` (1 when no pane is drawn on that side, as
//! `select-pane -L/-R/-U/-D` sees it) and `#{session_attached}` (how many
//! clients show the session).

use std::collections::BTreeMap;
use std::path::Path;

use embers_client::{
    ClientState, FormatContext, NavigationDirection, PaneTitles, PresentationModel, drawn_viewport,
    expand_format,
};
use embers_core::{NodeId, Result, Size};
use embers_protocol::SessionSnapshot;

use crate::CliConnection;

/// What the CLI projects a session at when no client ever drew it: the
/// server's default PTY size.
const DEFAULT_VIEWPORT: Size = Size {
    width: 80,
    height: 24,
};

/// The area a session is drawn in: the viewport the client showing it last
/// reported (`SessionRequest::SetViewport`), else the extent its panes' PTY
/// sizes add up to, else 80x24.
pub(crate) fn session_viewport(snapshot: &SessionSnapshot) -> Size {
    snapshot.session.viewport.unwrap_or_else(|| {
        let mut state = ClientState::default();
        state.apply_session_snapshot(snapshot.clone());
        drawn_viewport(&state, snapshot.session.id).unwrap_or(DEFAULT_VIEWPORT)
    })
}

/// `snapshot`'s session laid out the way a client draws it at
/// [`session_viewport`] with the title rows it reported, with the window and
/// tabs holding `pane_id` shown, so a pane of a window that is not current is
/// laid out too.
pub(crate) fn project(
    snapshot: &SessionSnapshot,
    pane_id: NodeId,
) -> Result<(ClientState, PresentationModel)> {
    let viewport = session_viewport(snapshot);
    let mut shown = snapshot.clone();
    let mut child_id = pane_id;
    while let Some(parent_id) = shown
        .nodes
        .iter()
        .find(|node| node.id == child_id)
        .and_then(|node| node.parent_id)
    {
        if let Some(tabs) = shown
            .nodes
            .iter_mut()
            .find(|node| node.id == parent_id)
            .and_then(|node| node.tabs.as_mut())
            && let Some(index) = tabs.tabs.iter().position(|tab| tab.child_id == child_id)
        {
            tabs.active = u32::try_from(index).expect("tab index fits into protocol width");
        }
        child_id = parent_id;
    }
    let mut state = ClientState::default();
    state.apply_session_snapshot(shown);
    let presentation = PresentationModel::project_with(
        &state,
        snapshot.session.id,
        viewport,
        PaneTitles::reported(&snapshot.session),
    )?;
    Ok((state, presentation))
}

impl CliConnection {
    /// Expand `template` for pane `pane_id` of `snapshot`'s session, or for
    /// its focused pane when `None`.
    pub(crate) async fn expand_for_pane(
        &mut self,
        snapshot: &SessionSnapshot,
        pane_id: Option<NodeId>,
        template: &str,
    ) -> Result<String> {
        self.expand_for_pane_with(snapshot, pane_id, template, BTreeMap::new())
            .await
    }

    /// Like [`Self::expand_for_pane`], with `extra` variables on top, such as
    /// a client's own for `list-clients -F`.
    pub(crate) async fn expand_for_pane_with(
        &mut self,
        snapshot: &SessionSnapshot,
        pane_id: Option<NodeId>,
        template: &str,
        extra: BTreeMap<String, String>,
    ) -> Result<String> {
        let base_index = self.base_index_for(snapshot);
        let mut state = ClientState::default();
        state.apply_session_snapshot(snapshot.clone());
        let Some(mut context) =
            FormatContext::from_state(&state, snapshot.session.id, pane_id, base_index)
        else {
            return Ok(expand_format(
                template,
                &FormatContext {
                    extra,
                    ..FormatContext::default()
                },
            ));
        };
        context.extra.extend(extra);
        if template.contains("pane_at_")
            && let Some(pane_id) = context.pane_id
        {
            let (_, presentation) = project(snapshot, pane_id)?;
            for (name, direction) in [
                ("pane_at_left", NavigationDirection::Left),
                ("pane_at_right", NavigationDirection::Right),
                ("pane_at_top", NavigationDirection::Up),
                ("pane_at_bottom", NavigationDirection::Down),
            ] {
                let at_edge = presentation.focus_target_from(pane_id, direction).is_none();
                context
                    .extra
                    .insert(name.to_owned(), usize::from(at_edge).to_string());
            }
        }
        if template.contains("session_attached") {
            let attached = self
                .list_clients()
                .await?
                .iter()
                .filter(|client| client.current_session_id == Some(snapshot.session.id))
                .count();
            context
                .extra
                .insert("session_attached".to_owned(), attached.to_string());
        }
        Ok(expand_format(template, &context))
    }

    /// The working directory for a pane created in `snapshot`'s session (or
    /// in no session yet): `-c`, expanded for the pane the command runs in
    /// when it is in that session, else for the session's focused pane; or
    /// without `-c`, the directory the command runs in, like tmux.
    pub(crate) async fn spawn_cwd(
        &mut self,
        cwd: Option<&str>,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<Option<String>> {
        let Some(cwd) = cwd else {
            return Ok(self.working_dir.clone());
        };
        let caller = self
            .pane_location()
            .await?
            .and_then(|location| location.session_id().zip(location.node_id()));
        let (snapshot, pane_id) = match (snapshot, caller) {
            (Some(snapshot), Some((session_id, pane_id))) if session_id == snapshot.session.id => {
                (Some(snapshot.clone()), Some(pane_id))
            }
            (Some(snapshot), _) => (Some(snapshot.clone()), None),
            (None, Some((session_id, pane_id))) => (
                Some(self.session_snapshot(session_id).await?),
                Some(pane_id),
            ),
            (None, None) => (None, None),
        };
        let expanded = match snapshot {
            Some(snapshot) => self.expand_for_pane(&snapshot, pane_id, cwd).await?,
            None => expand_format(cwd, &FormatContext::default()),
        };
        Ok(self.absolute_dir(expanded))
    }

    /// [`Self::spawn_cwd`] with `-c` expanded for `pane_id`, the pane the new
    /// one is created from.
    pub(crate) async fn spawn_cwd_for_pane(
        &mut self,
        cwd: Option<&str>,
        snapshot: &SessionSnapshot,
        pane_id: NodeId,
    ) -> Result<Option<String>> {
        let Some(cwd) = cwd else {
            return Ok(self.working_dir.clone());
        };
        let expanded = self.expand_for_pane(snapshot, Some(pane_id), cwd).await?;
        Ok(self.absolute_dir(expanded))
    }

    /// `dir` taken relative to the directory the command runs in; the
    /// default when it expanded to nothing.
    fn absolute_dir(&self, dir: String) -> Option<String> {
        if dir.is_empty() {
            return self.working_dir.clone();
        }
        match &self.working_dir {
            Some(base) if Path::new(&dir).is_relative() => {
                Some(Path::new(base).join(dir).to_string_lossy().into_owned())
            }
            _ => Some(dir),
        }
    }
}
