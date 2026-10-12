//! Which client `switch-client` and `detach-client` act on without `-c`.

use std::num::NonZeroU64;

use embers_core::{MuxError, Result};
use embers_protocol::SessionRecord;

use crate::CliConnection;

impl CliConnection {
    /// The client a client command acts on: `explicit`, else the command
    /// prompt's client, else `EMBERS_CLIENT` (set for `run_shell` children),
    /// else a client showing the session of the pane the command runs in,
    /// else the only attached client.
    ///
    /// When several clients show the pane's session, the one that attached
    /// last (the highest id) is taken: the server does not track client
    /// activity, which is what tmux would go by.
    pub(crate) async fn target_client(&mut self, explicit: Option<u64>) -> Result<NonZeroU64> {
        if let Some(client_id) = explicit {
            return NonZeroU64::new(client_id)
                .ok_or_else(|| MuxError::invalid_input("client id must be non-zero"));
        }
        if let Some(client_id) = self.default_client {
            return Ok(client_id);
        }
        let clients = self.list_clients().await?;
        if let Some(client_id) = self
            .caller_client
            .filter(|client_id| clients.iter().any(|client| client.id == client_id.get()))
        {
            return Ok(client_id);
        }
        let attached = clients
            .iter()
            .filter(|client| client.current_session_id.is_some())
            .collect::<Vec<_>>();
        if let Some(session_id) = self
            .pane_location()
            .await?
            .and_then(|location| location.session_id())
            && let Some(client) = attached
                .iter()
                .filter(|client| client.current_session_id == Some(session_id))
                .max_by_key(|client| client.id)
        {
            return client_id(client.id);
        }
        match attached.as_slice() {
            [client] => client_id(client.id),
            [] => Err(MuxError::not_found("no client is attached")),
            _ => Err(MuxError::invalid_input(
                "several clients are attached; choose one with -c (see list-clients)",
            )),
        }
    }

    /// The session after (or before) the one `client_id` shows, by name,
    /// wrapping around like tmux `switch-client -n`/`-p`.
    pub(crate) async fn neighbour_session(
        &mut self,
        client_id: NonZeroU64,
        forward: bool,
    ) -> Result<SessionRecord> {
        let current = self.client_record(client_id).await?.current_session_id;
        let mut sessions = self.list_sessions().await?;
        sessions.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        let count = sessions.len();
        if count == 0 {
            return Err(MuxError::not_found("no sessions exist"));
        }
        let position =
            current.and_then(|current| sessions.iter().position(|session| session.id == current));
        let index = match (position, forward) {
            (Some(position), true) => (position + 1) % count,
            (Some(position), false) => (position + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        Ok(sessions.swap_remove(index))
    }
}

fn client_id(id: u64) -> Result<NonZeroU64> {
    NonZeroU64::new(id).ok_or_else(|| MuxError::protocol("server reported client id 0"))
}
