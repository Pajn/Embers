//! tmux-style `new-session`: a session with a first window, attached unless
//! `-d`.

use embers_client::PanePlacement;
use embers_core::{MuxError, Result, SessionId, Size, new_request_id};
use embers_protocol::{
    ClientMessage, ClientRequest, SessionRecord, SessionRequest, SessionSnapshot,
};

use crate::{
    CliConnection, buffer_command, default_title, expect_session_snapshot,
    rollback_created_buffer_on_error, spawn_size,
};

/// `new-session` in tmux's form (`-s NAME`, or no name at all).
pub(crate) struct NewSession {
    pub detached: bool,
    pub attach_existing: bool,
    pub name: Option<String>,
    pub window_name: Option<String>,
    pub cwd: Option<String>,
    /// The host to run the first window's program on.
    pub host: Option<String>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// The `-F` format to print, when `-P` was given.
    pub format: Option<String>,
    pub command: Vec<String>,
}

impl CliConnection {
    pub(crate) async fn new_session(&mut self, args: NewSession) -> Result<String> {
        let sessions = self.list_sessions().await?;
        let name = args
            .name
            .clone()
            .unwrap_or_else(|| unused_session_name(&sessions));
        if let Some(existing) = sessions.iter().find(|session| session.name == name) {
            if !args.attach_existing {
                return Err(MuxError::conflict(format!("duplicate session: {name}")));
            }
            let snapshot = self.session_snapshot(existing.id).await?;
            if !args.detached {
                self.show_session(&snapshot).await?;
            }
            return self.describe_session(&snapshot, args.format).await;
        }
        if !args.detached {
            self.ensure_not_nested()?;
        }

        let response = self
            .request(ClientMessage::Session(SessionRequest::Create {
                request_id: new_request_id(),
                name: name.clone(),
            }))
            .await?;
        let mut snapshot = expect_session_snapshot(response, "new-session")?;
        let session_id = snapshot.session.id;
        match self.add_first_window(&snapshot, &args).await {
            Ok(created) => snapshot = created,
            Err(error) => {
                self.discard_new_session(session_id).await;
                return Err(error);
            }
        }
        if !args.detached
            && let Err(error) = self.show_session(&snapshot).await
        {
            self.discard_new_session(session_id).await;
            return Err(error);
        }
        self.describe_session(&snapshot, args.format).await
    }

    /// Closes a session that new-session created but could not finish, so a
    /// retry with the same name does not hit "duplicate session".
    async fn discard_new_session(&mut self, session_id: SessionId) {
        if let Err(close_error) = self
            .request(ClientMessage::Session(SessionRequest::Close {
                request_id: new_request_id(),
                session_id,
                force: true,
            }))
            .await
        {
            tracing::warn!(%session_id, %close_error, "failed to remove the new session");
        }
    }

    async fn add_first_window(
        &mut self,
        snapshot: &SessionSnapshot,
        args: &NewSession,
    ) -> Result<SessionSnapshot> {
        let session_id = snapshot.session.id;
        let mut snapshot = snapshot.clone();
        if args.width.is_some() || args.height.is_some() {
            let current = snapshot.session.viewport.unwrap_or(Size {
                width: 80,
                height: 24,
            });
            let viewport = Size {
                width: args.width.unwrap_or(current.width).max(1),
                height: args.height.unwrap_or(current.height).max(1),
            };
            self.request(ClientMessage::Session(SessionRequest::SetViewport {
                request_id: new_request_id(),
                session_id,
                viewport,
                pane_titles: snapshot.session.pane_titles,
            }))
            .await?;
            snapshot.session.viewport = Some(viewport);
        }
        let cwd = match args.host {
            Some(_) => {
                self.remote_spawn_cwd(args.cwd.as_deref(), None, None)
                    .await?
            }
            None => self.spawn_cwd(args.cwd.as_deref(), None).await?,
        };
        let command = buffer_command(args.command.clone());
        let title = args
            .window_name
            .clone()
            .unwrap_or_else(|| default_title(&command, "shell"));
        let size = spawn_size(&snapshot, PanePlacement::RootTab);
        let buffer = self
            .create_buffer(
                Some(title.clone()),
                command,
                cwd,
                size,
                session_id,
                args.host.clone(),
            )
            .await?;
        let response = self
            .request(ClientMessage::Session(SessionRequest::AddRootTab {
                request_id: new_request_id(),
                session_id,
                title,
                buffer_id: Some(buffer.buffer.id),
                child_node_id: None,
            }))
            .await;
        let response =
            rollback_created_buffer_on_error(self, buffer.buffer.id, "new-session", response)
                .await?;
        rollback_created_buffer_on_error(
            self,
            buffer.buffer.id,
            "new-session",
            expect_session_snapshot(response, "new-session"),
        )
        .await
    }

    /// Like tmux, refuse to attach a client from inside a pane: the client
    /// would run nested in the pane.
    fn ensure_not_nested(&self) -> Result<()> {
        if self.default_client.is_none() && self.can_attach && self.pane_buffer.is_some() {
            return Err(MuxError::invalid_input(
                "new-session would attach a client inside this pane; \
                 use -d, then switch-client -t to show the session",
            ));
        }
        Ok(())
    }

    /// Show `snapshot`'s session: switch the command prompt's client to it,
    /// or attach this terminal once the command is done. A command that is
    /// not run from a terminal has nothing to attach and leaves it detached.
    async fn show_session(&mut self, snapshot: &SessionSnapshot) -> Result<()> {
        if let Some(client_id) = self.default_client {
            self.request(ClientMessage::Client(ClientRequest::Switch {
                request_id: new_request_id(),
                client_id: Some(client_id),
                session_id: snapshot.session.id,
                node_id: None,
            }))
            .await?;
            return Ok(());
        }
        if self.can_attach {
            self.ensure_not_nested()?;
            self.attach_session = Some(snapshot.session.name.clone());
        }
        Ok(())
    }

    async fn describe_session(
        &mut self,
        snapshot: &SessionSnapshot,
        format: Option<String>,
    ) -> Result<String> {
        match format {
            Some(format) => self.expand_for_pane(snapshot, None, &format).await,
            None => Ok(String::new()),
        }
    }
}

/// tmux names sessions created without `-s` by number: the lowest unused one.
fn unused_session_name(sessions: &[SessionRecord]) -> String {
    (0_u64..)
        .map(|number| number.to_string())
        .find(|name| sessions.iter().all(|session| session.name != *name))
        .expect("some number is unused")
}
