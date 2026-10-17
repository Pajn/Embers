//! Hosts for tests: [`HostConnector`] adapters that reach a host over a
//! stream the test controls instead of a command.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use embers_core::{MuxError, Result};
use tokio::net::UnixStream;
use tokio::task::AbortHandle;

use crate::remote::{BoxFuture, HostConnector};
use crate::upstream::Upstream;

/// Reaches each host's server on its socket, through a relay that
/// [`Self::cut`] severs, as when the network goes down.
#[derive(Clone, Default)]
pub(crate) struct SocketHosts {
    inner: Arc<Mutex<SocketHostsInner>>,
}

#[derive(Default)]
struct SocketHostsInner {
    sockets: HashMap<String, PathBuf>,
    /// Hosts that cannot be reached now.
    down: Vec<String>,
    relays: HashMap<String, Vec<AbortHandle>>,
}

impl SocketHosts {
    /// Reach `host` on `socket`.
    pub(crate) fn add(&self, host: &str, socket: PathBuf) {
        self.lock().sockets.insert(host.to_owned(), socket);
    }

    /// Sever the connections to `host` and refuse new ones until
    /// [`Self::restore`].
    pub(crate) fn cut(&self, host: &str) {
        let mut inner = self.lock();
        inner.down.push(host.to_owned());
        for relay in inner.relays.remove(host).unwrap_or_default() {
            relay.abort();
        }
    }

    /// Let `host` be reached again.
    pub(crate) fn restore(&self, host: &str) {
        self.lock().down.retain(|down| down != host);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SocketHostsInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl HostConnector for SocketHosts {
    fn connect<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Upstream>> {
        Box::pin(async move {
            let socket =
                {
                    let inner = self.lock();
                    if inner.down.iter().any(|down| down == host) {
                        return Err(MuxError::transport(format!("{host} cannot be reached")));
                    }
                    inner.sockets.get(host).cloned().ok_or_else(|| {
                        MuxError::transport(format!("no server is known for {host}"))
                    })?
                };
            let mut server = UnixStream::connect(&socket)
                .await
                .map_err(|error| MuxError::transport(format!("{host}: {error}")))?;
            let (mut relayed, ours) = tokio::io::duplex(64 * 1024);
            let relay = tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut relayed, &mut server).await;
            });
            self.lock()
                .relays
                .entry(host.to_owned())
                .or_default()
                .push(relay.abort_handle());
            let (reader, writer) = tokio::io::split(ours);
            Upstream::over(format!("{host}'s socket"), reader, writer).await
        })
    }
}
