//! The buffer runtimes of a server: starting each buffer's runtime where its
//! placement says, taking runtimes up again after a restart or once a lost
//! host can be reached again, following them until they end or the server is
//! done with them, and letting a keeper go once a saved workspace no longer
//! needs it.
//!
//! The server keeps the buffer records; this module never reads them. It is
//! told what to start, take up or release, and reports through [`Callbacks`]
//! how each runtime ended and which buffers on hosts run again.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use embers_core::{BufferId, MuxError, PtySize, Result};
use tokio::sync::{Mutex, watch};
use tracing::{debug, info, warn};

use crate::buffer_runtime::{BufferRuntimeCallbacks, BufferRuntimeStatus, RuntimeEnd};
use crate::config::BUFFER_ENV_VAR;
use crate::model::Placement;
use crate::remote::{HostConnection, HostConnector, RemoteHosts};
use crate::runtime_dir::{keeper_socket_path, prepare_keeper_socket_dir};
use crate::runtime_handle::BufferRuntimeHandle;

/// How long to wait before trying to reach a lost host again, doubling after
/// each failure up to [`RECONNECT_MAX_DELAY`].
const RECONNECT_FIRST_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

/// How the runtimes report to the server.
pub(crate) struct Callbacks {
    /// Makes the callbacks for one runtime, once per runtime started or taken
    /// up.
    pub runtime: Arc<dyn Fn() -> BufferRuntimeCallbacks + Send + Sync>,
    /// A buffer interrupted on its host runs there again, with the status its
    /// runtime reported if it did. Its runtime is registered by then.
    pub taken_up: Arc<dyn Fn(BufferId, Option<BufferRuntimeStatus>) + Send + Sync>,
}

/// What to start for a buffer (see [`BufferRuntimes::start`]).
pub(crate) struct StartRequest<'a> {
    pub command: &'a [String],
    pub cwd: Option<&'a Path>,
    /// Variables the buffer's record asks for. A keeper runs the program
    /// with them over the server's own environment; a host under its own.
    pub env: &'a BTreeMap<String, String>,
    pub size: PtySize,
    pub title: &'a str,
    /// The host to run the program on, or this machine.
    pub host: Option<&'a str>,
}

/// A runtime [`BufferRuntimes::start`] started.
pub(crate) struct Started {
    pub placement: Placement,
    pub status: BufferRuntimeStatus,
}

/// What [`BufferRuntimes::restore`] found for a buffer in a keeper.
pub(crate) enum Restored {
    /// Its keeper was taken up again and reported this.
    Running(BufferRuntimeStatus),
    /// Its keeper is gone, or could not be taken up: the buffer is
    /// interrupted and no longer refers to a keeper.
    Gone,
}

/// Where keepers listen and what they run under.
pub(crate) struct KeeperSettings {
    /// The socket of the server the keepers belong to.
    pub server_socket: PathBuf,
    pub runtime_dir: PathBuf,
    /// The environment every keeper's program starts from.
    pub env: BTreeMap<String, OsString>,
}

pub(crate) struct BufferRuntimes {
    inner: Arc<Inner>,
}

struct Inner {
    keepers: KeeperSettings,
    hosts: RemoteHosts,
    callbacks: OnceLock<Callbacks>,
    running: StdMutex<BTreeMap<BufferId, BufferRuntimeHandle>>,
    /// Keepers waiting to hear that the workspace no longer needs them (see
    /// [`BufferRuntimes::awaiting_save`]).
    awaiting_save: StdMutex<BTreeMap<BufferId, BufferRuntimeHandle>>,
    /// Buffers whose runtime reported an exit before it was registered.
    exited_early: StdMutex<BTreeSet<BufferId>>,
    /// Buffers whose runtimes the server is shutting down: however they end
    /// now, their programs may still run for the next server.
    shutting_down: StdMutex<BTreeSet<BufferId>>,
    /// Buffers interrupted on a host, by host, each with its id there, until
    /// they are taken up again or released.
    waiting_for_host: StdMutex<BTreeMap<String, BTreeMap<BufferId, BufferId>>>,
    /// The hosts being reached again, one attempt at a time each.
    reaching: StdMutex<BTreeSet<String>>,
    /// Set once the server shuts down; hosts are no longer reached then.
    stopping: watch::Sender<bool>,
    /// Whether some client forwards kitty graphics queries to a host
    /// terminal that answers them, as every keeper and host was last told.
    /// Held while they are told, so they hear changes in order.
    host_graphics: Mutex<bool>,
}

impl std::fmt::Debug for BufferRuntimes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BufferRuntimes")
            .field("running", &lock(&self.inner.running).len())
            .field("awaiting_save", &lock(&self.inner.awaiting_save).len())
            .field("waiting_for_host", &lock(&self.inner.waiting_for_host))
            .finish_non_exhaustive()
    }
}

impl BufferRuntimes {
    /// Runtimes whose keepers follow `keepers`, and whose hosts are reached
    /// through `hosts`.
    pub(crate) fn new(keepers: KeeperSettings, hosts: Arc<dyn HostConnector>) -> Self {
        Self {
            inner: Arc::new(Inner {
                keepers,
                hosts: RemoteHosts::new(hosts),
                callbacks: OnceLock::new(),
                running: StdMutex::default(),
                awaiting_save: StdMutex::default(),
                exited_early: StdMutex::default(),
                shutting_down: StdMutex::default(),
                waiting_for_host: StdMutex::default(),
                reaching: StdMutex::default(),
                stopping: watch::Sender::new(false),
                host_graphics: Mutex::new(false),
            }),
        }
    }

    /// Set how the runtimes report, unless that already was. Needed before
    /// any runtime is started or taken up.
    pub(crate) fn set_callbacks(&self, callbacks: impl FnOnce() -> Callbacks) {
        self.inner.callbacks.get_or_init(callbacks);
    }

    /// The runtime of `buffer_id`, while one runs it for this server.
    pub(crate) fn get(&self, buffer_id: BufferId) -> Option<BufferRuntimeHandle> {
        lock(&self.inner.running).get(&buffer_id).cloned()
    }

    /// Start a runtime for `buffer_id`: a keeper on this machine, or a buffer
    /// on the requested host. A runtime started but not running cleanly is
    /// stopped again and waits for a save (see [`Self::awaiting_save`]).
    pub(crate) async fn start(
        &self,
        buffer_id: BufferId,
        request: StartRequest<'_>,
    ) -> Result<Started> {
        let inner = &self.inner;
        let callbacks = inner.runtime_callbacks()?;
        let runtime = match request.host {
            // The host's server sets the program's own environment, its
            // socket and buffer id included.
            Some(host) => {
                let connection = inner.hosts.connect(host).await?;
                BufferRuntimeHandle::spawn_remote(
                    buffer_id,
                    connection,
                    Some(request.title.to_owned()),
                    request.command,
                    request.cwd,
                    request.env.clone(),
                    request.size,
                    callbacks,
                )
                .await?
            }
            None => {
                let mut env = inner.keepers.env.clone();
                for (key, value) in request.env {
                    env.insert(key.clone(), OsString::from(value));
                }
                // The buffer's own id goes in last so no hint can name another
                // buffer.
                env.insert(
                    BUFFER_ENV_VAR.to_owned(),
                    OsString::from(buffer_id.to_string()),
                );
                BufferRuntimeHandle::spawn_keeper(
                    buffer_id,
                    inner.keeper_socket(buffer_id)?,
                    request.command,
                    request.cwd,
                    &env,
                    request.size,
                    callbacks,
                )
                .await?
            }
        };
        // The caller drops the buffer record on error, so stop the keeper too
        // rather than leave it running with nothing referring to it.
        let status = match runtime.status().await {
            Ok(status) => status,
            Err(error) => {
                let _ = runtime.kill().await;
                inner.stop_unregistered(buffer_id, runtime).await;
                return Err(error);
            }
        };
        let placement = runtime.placement();
        if !inner.register(buffer_id, runtime, Some(&status)).await {
            return Err(MuxError::conflict("the server is shutting down"));
        }
        Ok(Started { placement, status })
    }

    /// Take up again the runtimes a previous server left, by where it saved
    /// each buffer to run: a keeper is taken up before this returns, with
    /// what was found for it reported here; a host is reached in the
    /// background, each buffer reported through [`Callbacks::taken_up`] once
    /// it runs again, or as exited when its host no longer has it. A buffer
    /// on a host stays interrupted while the host cannot be reached.
    pub(crate) async fn restore(
        &self,
        placements: Vec<(BufferId, Placement)>,
    ) -> Vec<(BufferId, Restored)> {
        let inner = &self.inner;
        let mut restored = Vec::new();
        let mut hosts = BTreeSet::new();
        for (buffer_id, placement) in placements {
            match placement {
                Placement::Local {
                    socket_path: Some(socket_path),
                } => {
                    let outcome = inner.restore_keeper(buffer_id, socket_path).await;
                    restored.push((buffer_id, outcome));
                }
                Placement::Remote {
                    host,
                    buffer_id: Some(remote_id),
                } => {
                    lock(&inner.waiting_for_host)
                        .entry(host.clone())
                        .or_default()
                        .insert(buffer_id, remote_id);
                    hosts.insert(host);
                }
                // Nothing runs it anywhere to take up.
                Placement::Local { socket_path: None }
                | Placement::Remote {
                    buffer_id: None, ..
                } => {}
            }
        }
        // Reaching a host can take long, and must not hold up the start.
        for host in hosts {
            Inner::reach_host(inner, host, Duration::ZERO);
        }
        restored
    }

    /// Stop the runtime of `buffer_id`, whose program the server does not
    /// want after all, as when the buffer's record went away while it
    /// started. Its keeper waits for a save (see [`Self::awaiting_save`]).
    pub(crate) async fn discard(&self, buffer_id: BufferId) {
        let runtime = lock(&self.inner.running).remove(&buffer_id);
        if let Some(runtime) = runtime {
            let _ = runtime.kill().await;
            self.inner.stop_unregistered(buffer_id, runtime).await;
        }
    }

    /// The server is done with the runtime of `buffer_id`, whose record it
    /// removed: no callback reports on the buffer after this, and it is no
    /// longer taken up if its host was lost. A host removes the buffer once
    /// it has exited; a keeper waits for a save (see [`Self::awaiting_save`]).
    pub(crate) async fn release(&self, buffer_id: BufferId) {
        for buffers in lock(&self.inner.waiting_for_host).values_mut() {
            buffers.remove(&buffer_id);
        }
        let runtime = lock(&self.inner.running).remove(&buffer_id);
        if let Some(runtime) = runtime {
            if let Err(error) = runtime.release().await {
                debug!(%buffer_id, %error, "failed to release buffer runtime");
            }
            self.inner.await_save(buffer_id, runtime);
        }
    }

    /// The buffers whose keepers wait to hear that the workspace no longer
    /// needs them: their program exited or the server released them. A
    /// keeper whose child has exited stays alive for the next server until
    /// then: if this server crashed after reading the exit but before saving
    /// it, a restart would otherwise find no keeper, take the pane for
    /// interrupted and run its command again.
    pub(crate) fn awaiting_save(&self) -> Vec<BufferId> {
        lock(&self.inner.awaiting_save).keys().copied().collect()
    }

    /// Tell the keepers of `buffer_ids`, from [`Self::awaiting_save`], that
    /// a save that no longer needs them has completed.
    pub(crate) async fn saved(&self, buffer_ids: Vec<BufferId>) {
        let keepers: Vec<_> = {
            let mut awaiting = lock(&self.inner.awaiting_save);
            buffer_ids
                .into_iter()
                .filter_map(|buffer_id| {
                    awaiting
                        .remove(&buffer_id)
                        .map(|keeper| (buffer_id, keeper))
                })
                .collect()
        };
        for (buffer_id, keeper) in keepers {
            if let Err(error) = keeper.acknowledge_exit_recorded().await {
                debug!(%buffer_id, %error, "failed to tell keeper its exit was saved");
            }
        }
    }

    /// Tell every keeper and host whether any client now forwards kitty
    /// graphics queries to a host terminal that answers them, when that
    /// changed. A host hears it once, for all its buffers.
    pub(crate) async fn set_host_graphics(&self, enabled: bool) {
        let mut host_graphics = self.inner.host_graphics.lock().await;
        if *host_graphics == enabled {
            return;
        }
        *host_graphics = enabled;
        let keepers: Vec<_> = lock(&self.inner.running)
            .iter()
            .filter_map(|(buffer_id, runtime)| match runtime {
                BufferRuntimeHandle::Keeper(keeper) => Some((*buffer_id, keeper.clone())),
                BufferRuntimeHandle::Remote(_) => None,
            })
            .collect();
        for (buffer_id, keeper) in keepers {
            // A keeper that predates this rejects it, and never holds replies.
            if let Err(error) = keeper.set_host_graphics(enabled).await {
                debug!(%buffer_id, %error, "failed to tell keeper about host graphics");
            }
        }
        self.inner.hosts.set_host_graphics(enabled).await;
    }

    /// Stop following every runtime, as the server shuts down. Each buffer
    /// that reports an end from now on is interrupted, however its runtime
    /// ended, and lost hosts are no longer reached. With `kill` (kill-server)
    /// every program is ended too, along with the keepers waiting for a save;
    /// otherwise keepers keep running so a restarted server can take them up,
    /// and programs on hosts keep running there.
    pub(crate) async fn shut_down(&self, kill: bool) {
        let inner = &self.inner;
        inner.stopping.send_replace(true);
        lock(&inner.waiting_for_host).clear();
        let runtimes: Vec<_> = {
            let runtimes = lock(&inner.running);
            lock(&inner.shutting_down).extend(runtimes.keys().copied());
            runtimes.values().cloned().collect()
        };
        if kill {
            let mut ending = std::mem::take(&mut *lock(&inner.awaiting_save));
            ending.extend(lock(&inner.running).clone());
            // Keepers stop concurrently: each may wait out a grace period for a
            // program that ignores SIGHUP.
            let mut terminations = tokio::task::JoinSet::new();
            for runtime in ending.into_values() {
                terminations.spawn(async move {
                    if let Err(error) = runtime.terminate().await {
                        debug!(%error, "failed to terminate buffer runtime");
                    }
                });
            }
            while terminations.join_next().await.is_some() {}
        }
        for runtime in runtimes {
            if let Err(error) = runtime.release().await {
                debug!(%error, "failed to release buffer runtime during shutdown");
            }
        }
        lock(&inner.running).clear();
    }

    /// Let go of the keepers still waiting for a save, after the last one:
    /// they stay up for the next server.
    pub(crate) fn close(&self) {
        lock(&self.inner.awaiting_save).clear();
    }
}

impl Inner {
    fn callbacks(&self) -> Result<&Callbacks> {
        self.callbacks
            .get()
            .ok_or_else(|| MuxError::internal("buffer runtime callbacks are not set"))
    }

    /// The callbacks for a new runtime, with its end recorded here before
    /// the server hears of it.
    fn runtime_callbacks(self: &Arc<Self>) -> Result<BufferRuntimeCallbacks> {
        let mut callbacks = (self.callbacks()?.runtime)();
        let on_ended = callbacks.on_ended.clone();
        // A runtime outliving its server's runtimes records nothing here.
        let inner = Arc::downgrade(self);
        callbacks.on_ended = Arc::new(move |buffer_id, end| {
            let Some(inner) = Weak::upgrade(&inner) else {
                on_ended(buffer_id, end);
                return;
            };
            let (end, lost_host) = inner.ended(buffer_id, end);
            // The server records the interrupt before the host is tried again.
            on_ended(buffer_id, end);
            if let Some(host) = lost_host {
                Self::reach_host(&inner, host, RECONNECT_FIRST_DELAY);
            }
        });
        Ok(callbacks)
    }

    /// Record that `buffer_id`'s runtime ended as `end`. Returns how the
    /// server should take it, and the host to reach again when it was lost
    /// there.
    fn ended(&self, buffer_id: BufferId, end: RuntimeEnd) -> (RuntimeEnd, Option<String>) {
        let shutting_down = lock(&self.shutting_down).remove(&buffer_id);
        let end = if shutting_down {
            RuntimeEnd::Interrupted
        } else {
            end
        };
        let mut running = lock(&self.running);
        let mut lost_host = None;
        match end {
            RuntimeEnd::Interrupted => {
                let runtime = running.remove(&buffer_id);
                drop(running);
                if !shutting_down
                    && let Some(Placement::Remote {
                        host,
                        buffer_id: Some(remote_id),
                    }) = runtime.as_ref().map(BufferRuntimeHandle::placement)
                {
                    lock(&self.waiting_for_host)
                        .entry(host.clone())
                        .or_default()
                        .insert(buffer_id, remote_id);
                    lost_host = Some(host);
                }
            }
            // The runtime stays registered: an exited buffer can still be
            // shown and captured. Its keeper is held for a save from now on.
            RuntimeEnd::Exited(_) => match running.get(&buffer_id) {
                Some(runtime) => {
                    if matches!(runtime, BufferRuntimeHandle::Keeper(_)) {
                        lock(&self.awaiting_save).insert(buffer_id, runtime.clone());
                    }
                }
                None => {
                    lock(&self.exited_early).insert(buffer_id);
                }
            },
        }
        (end, lost_host)
    }

    async fn restore_keeper(
        self: &Arc<Self>,
        buffer_id: BufferId,
        socket_path: PathBuf,
    ) -> Restored {
        if !socket_path.exists() {
            debug!(
                %buffer_id,
                socket_path = %socket_path.display(),
                "skipping runtime restore because keeper socket is missing"
            );
            return Restored::Gone;
        }
        let attached = match self.runtime_callbacks() {
            Ok(callbacks) => {
                BufferRuntimeHandle::attach_keeper(buffer_id, socket_path.clone(), callbacks).await
            }
            Err(error) => Err(error),
        };
        let runtime = match attached {
            Ok(runtime) => runtime,
            Err(error) => {
                debug!(
                    %buffer_id,
                    socket_path = %socket_path.display(),
                    %error,
                    "failed to restore buffer runtime"
                );
                return Restored::Gone;
            }
        };
        let mut status = match runtime.status().await {
            Ok(status) => status,
            Err(error) => {
                debug!(
                    %buffer_id,
                    socket_path = %socket_path.display(),
                    %error,
                    "failed to restore buffer runtime"
                );
                return Restored::Gone;
            }
        };
        // A pipe's command ran under the previous server; it is not taken up.
        if status.pipe.as_ref().is_some_and(|pipe| pipe.running) {
            match runtime.stop_pipe().await {
                Ok(pipe) => status.pipe = Some(pipe),
                Err(error) => {
                    debug!(
                        %buffer_id,
                        socket_path = %socket_path.display(),
                        %error,
                        "failed to stop restored buffer pipe"
                    );
                    let _ = runtime.kill().await;
                    self.stop_unregistered(buffer_id, runtime).await;
                    return Restored::Gone;
                }
            }
        }
        if !self.register(buffer_id, runtime, Some(&status)).await {
            return Restored::Gone;
        }
        Restored::Running(status)
    }

    /// Keep trying to reach `host` after `delay`, waiting longer after each
    /// failure, and take up its interrupted buffers once it answers. One
    /// attempt runs per host at a time; it gives up once no buffer waits for
    /// the host or the server shuts down.
    fn reach_host(self: &Arc<Self>, host: String, delay: Duration) {
        if *self.stopping.borrow() || !lock(&self.reaching).insert(host.clone()) {
            return;
        }
        let Ok(tokio) = tokio::runtime::Handle::try_current() else {
            lock(&self.reaching).remove(&host);
            return;
        };
        let inner = self.clone();
        let mut stopping = self.stopping.subscribe();
        tokio.spawn(async move {
            tokio::select! {
                () = inner.reach_until_taken_up(&host, delay) => {}
                _ = stopping.wait_for(|stopping| *stopping) => {}
            }
            lock(&inner.reaching).remove(&host);
            // A buffer lost after the last attempt found this task still
            // running and left it to it.
            if inner.waits_for(&host) {
                Self::reach_host(&inner, host, RECONNECT_FIRST_DELAY);
            }
        });
    }

    async fn reach_until_taken_up(self: &Arc<Self>, host: &str, mut delay: Duration) {
        loop {
            tokio::time::sleep(delay).await;
            if !self.waits_for(host) {
                return;
            }
            match self.hosts.connect(host).await {
                // Buffers left interrupted are tried again with the rest.
                Ok(connection) => {
                    if self.take_up_on_host(host, connection).await {
                        return;
                    }
                }
                // Only the first attempt after a restart says so: a host it
                // cannot reach is worth a warning, a retry is not.
                Err(error) if delay.is_zero() => {
                    warn!(%host, %error, "could not reach host to take up its buffers again");
                }
                Err(error) => {
                    debug!(%host, %error, ?delay, "host still cannot be reached");
                }
            }
            delay = (delay * 2).clamp(RECONNECT_FIRST_DELAY, RECONNECT_MAX_DELAY);
        }
    }

    fn waits_for(&self, host: &str) -> bool {
        lock(&self.waiting_for_host)
            .get(host)
            .is_some_and(|buffers| !buffers.is_empty())
    }

    /// Take up the buffers waiting for `host` on `connection`. Returns
    /// whether none is left waiting for a reason trying again could fix:
    /// taking it up failed some other way than the host no longer having it.
    async fn take_up_on_host(self: &Arc<Self>, host: &str, connection: HostConnection) -> bool {
        let buffers: Vec<_> = lock(&self.waiting_for_host)
            .get(host)
            .map(|buffers| buffers.iter().map(|(&id, &remote)| (id, remote)).collect())
            .unwrap_or_default();
        let mut done = true;
        for (buffer_id, remote_id) in buffers {
            let attached = match self.runtime_callbacks() {
                Ok(callbacks) => {
                    BufferRuntimeHandle::attach_remote(
                        buffer_id,
                        connection.clone(),
                        remote_id,
                        callbacks,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            // Released while its host was being reached.
            let waiting = lock(&self.waiting_for_host)
                .get_mut(host)
                .is_some_and(|buffers| buffers.remove(&buffer_id).is_some());
            match attached {
                Ok(runtime) if !waiting => {
                    let _ = runtime.release().await;
                }
                Ok(runtime) => {
                    let status = runtime.status().await.ok();
                    if self.register(buffer_id, runtime, status.as_ref()).await
                        && let Ok(callbacks) = self.callbacks()
                    {
                        info!(%buffer_id, %host, "took up buffer on host again");
                        (callbacks.taken_up)(buffer_id, status);
                    }
                }
                Err(_) if !waiting => {}
                // The host no longer has it, as if its program exited.
                Err(error) if error.is_not_found() => {
                    if let Ok(callbacks) = self.callbacks() {
                        ((callbacks.runtime)().on_ended)(buffer_id, RuntimeEnd::Exited(None));
                    }
                }
                Err(error) => {
                    warn!(%buffer_id, %host, %error, "could not take up buffer on host again");
                    lock(&self.waiting_for_host)
                        .entry(host.to_owned())
                        .or_default()
                        .insert(buffer_id, remote_id);
                    done = false;
                }
            }
        }
        done
    }

    /// Register `runtime` as `buffer_id`'s, as it reported `status`. Returns
    /// whether it was: once the server shuts down, a runtime is let go of
    /// instead.
    async fn register(
        &self,
        buffer_id: BufferId,
        runtime: BufferRuntimeHandle,
        status: Option<&BufferRuntimeStatus>,
    ) -> bool {
        if *self.stopping.borrow() {
            self.stop_unregistered(buffer_id, runtime).await;
            return false;
        }
        lock(&self.running).insert(buffer_id, runtime.clone());
        // A keeper starts out holding no replies; tell this one if it should.
        // A host was told when it was reached.
        if let BufferRuntimeHandle::Keeper(keeper) = &runtime {
            let host_graphics = self.host_graphics.lock().await;
            if *host_graphics && let Err(error) = keeper.set_host_graphics(true).await {
                debug!(%buffer_id, %error, "failed to tell keeper about host graphics");
            }
        }
        // Its program may have exited before it was registered, when its end
        // found nothing to hold for a save, or before it was taken up again,
        // which a runtime reports as its status rather than as an end.
        let exited = status.is_some_and(|status| !status.running);
        if lock(&self.exited_early).remove(&buffer_id) || exited {
            self.await_save(buffer_id, runtime);
        }
        true
    }

    /// Stop following a runtime that is not registered, and hold its keeper
    /// for a save.
    async fn stop_unregistered(&self, buffer_id: BufferId, runtime: BufferRuntimeHandle) {
        let _ = runtime.release().await;
        lock(&self.exited_early).remove(&buffer_id);
        self.await_save(buffer_id, runtime);
    }

    fn await_save(&self, buffer_id: BufferId, runtime: BufferRuntimeHandle) {
        if matches!(runtime, BufferRuntimeHandle::Keeper(_)) {
            lock(&self.awaiting_save).insert(buffer_id, runtime);
        }
    }

    fn keeper_socket(&self, buffer_id: BufferId) -> Result<PathBuf> {
        let path = keeper_socket_path(
            &self.keepers.server_socket,
            &self.keepers.runtime_dir,
            buffer_id,
        );
        prepare_keeper_socket_dir(&path)?;
        Ok(path)
    }
}

/// Recover from poisoning: these maps are plain bookkeeping and must not
/// bring down the server if some other task panicked while holding one.
fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::time::Duration;

    use embers_core::{BufferId, PtySize};
    use tempfile::{TempDir, tempdir};
    use tokio::sync::mpsc;

    use embers_core::RequestId;
    use embers_protocol::{
        BufferRecordState, BufferRequest, ClientMessage, KillServerRequest, ProtocolClient,
        ServerResponse,
    };

    use super::{BufferRuntimes, Callbacks, KeeperSettings, Restored, StartRequest, Started, lock};
    use crate::buffer_runtime::{BufferRuntimeCallbacks, BufferRuntimeStatus, RuntimeEnd};
    use crate::model::Placement;
    use crate::remote::{CommandConnector, HostConnector};
    use crate::testing_hosts::SocketHosts;
    use crate::{Server, ServerConfig, ServerHandle};

    /// Runtimes whose keepers live in a fresh directory, and the ends they
    /// report.
    fn new_runtimes() -> (
        BufferRuntimes,
        mpsc::UnboundedReceiver<(BufferId, RuntimeEnd)>,
        TempDir,
    ) {
        let reached = Reaching::new(Arc::new(CommandConnector::new(Vec::new())));
        (reached.runtimes, reached.ended, reached.dir)
    }

    /// Runtimes reaching hosts through a connector, and what they report.
    struct Reaching {
        runtimes: BufferRuntimes,
        ended: mpsc::UnboundedReceiver<(BufferId, RuntimeEnd)>,
        taken_up: mpsc::UnboundedReceiver<(BufferId, Option<BufferRuntimeStatus>)>,
        dir: TempDir,
    }

    impl Reaching {
        fn new(hosts: Arc<dyn HostConnector>) -> Self {
            let dir = tempdir().expect("tempdir");
            let runtimes = BufferRuntimes::new(
                KeeperSettings {
                    server_socket: dir.path().join("mux.sock"),
                    runtime_dir: dir.path().join("runtime"),
                    env: std::env::vars_os()
                        .filter_map(|(key, value)| Some((key.into_string().ok()?, value)))
                        .collect(),
                },
                hosts,
            );
            let (ends, ended) = mpsc::unbounded_channel();
            let (took_up, taken_up) = mpsc::unbounded_channel();
            runtimes.set_callbacks(|| Callbacks {
                runtime: Arc::new(move || {
                    let ends = ends.clone();
                    BufferRuntimeCallbacks {
                        on_ended: Arc::new(move |buffer_id, end| {
                            let _ = ends.send((buffer_id, end));
                        }),
                        ..BufferRuntimeCallbacks::default()
                    }
                }),
                taken_up: Arc::new(move |buffer_id, status| {
                    let _ = took_up.send((buffer_id, status));
                }),
            });
            Self {
                runtimes,
                ended,
                taken_up,
                dir,
            }
        }
    }

    /// An embers server playing a host.
    struct Host {
        handle: ServerHandle,
        socket: std::path::PathBuf,
        _dir: TempDir,
    }

    impl Host {
        async fn start(hosts: &SocketHosts, name: &str) -> Self {
            let dir = tempdir().expect("tempdir");
            let socket = dir.path().join("host.sock");
            let handle = Server::new(ServerConfig::new(socket.clone()))
                .start()
                .await
                .expect("start the host's server");
            hosts.add(name, socket.clone());
            Self {
                handle,
                socket,
                _dir: dir,
            }
        }

        async fn buffer_state(&self, buffer_id: BufferId) -> Option<BufferRecordState> {
            let mut client = ProtocolClient::connect(&self.socket)
                .await
                .expect("connect to the host");
            match client
                .request(&ClientMessage::Buffer(BufferRequest::Get {
                    request_id: RequestId(1),
                    buffer_id,
                }))
                .await
                .expect("ask the host")
            {
                ServerResponse::Buffer(response) => Some(response.buffer.state),
                _ => None,
            }
        }

        /// Stop the host's server along with its programs.
        async fn kill(self) {
            let mut client = ProtocolClient::connect(&self.socket)
                .await
                .expect("connect to the host");
            let _ = client
                .request(&ClientMessage::KillServer(KillServerRequest {
                    request_id: RequestId(2),
                }))
                .await;
            self.handle.shutdown().await.expect("shut down the host");
        }
    }

    async fn start_on(runtimes: &BufferRuntimes, buffer_id: BufferId, host: &str) -> BufferId {
        let command = ["/bin/sh".to_owned(), "-c".to_owned(), "sleep 30".to_owned()];
        let started = runtimes
            .start(
                buffer_id,
                StartRequest {
                    command: &command,
                    cwd: None,
                    env: &BTreeMap::new(),
                    size: PtySize::new(80, 24),
                    title: "test",
                    host: Some(host),
                },
            )
            .await
            .expect("start on the host");
        match started.placement {
            Placement::Remote {
                buffer_id: Some(remote_id),
                ..
            } => remote_id,
            other => panic!("expected a buffer on {host}, got {other:?}"),
        }
    }

    fn on_host(host: &str, remote_id: BufferId) -> Placement {
        Placement::Remote {
            host: host.to_owned(),
            buffer_id: Some(remote_id),
        }
    }

    /// What is reported within the first retry and a margin.
    async fn within_a_retry<T>(receiver: &mut mpsc::UnboundedReceiver<T>) -> Option<T> {
        tokio::time::timeout(Duration::from_secs(3), receiver.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn a_buffer_runs_on_its_host_as_a_buffer_of_the_hosts_server() {
        let hosts = SocketHosts::default();
        let host = Host::start(&hosts, "devbox").await;
        let reaching = Reaching::new(Arc::new(hosts));

        let remote_id = start_on(&reaching.runtimes, BufferId(1), "devbox").await;

        assert_eq!(
            host.buffer_state(remote_id).await,
            Some(BufferRecordState::Running)
        );
        assert!(reaching.runtimes.get(BufferId(1)).is_some());
        reaching.runtimes.shut_down(true).await;
        host.kill().await;
    }

    #[tokio::test]
    async fn a_lost_host_interrupts_its_buffers_until_it_is_reached_again() {
        let hosts = SocketHosts::default();
        let host = Host::start(&hosts, "devbox").await;
        let mut reaching = Reaching::new(Arc::new(hosts.clone()));
        start_on(&reaching.runtimes, BufferId(1), "devbox").await;

        hosts.cut("devbox");
        assert_eq!(
            next_end(&mut reaching.ended).await,
            (BufferId(1), RuntimeEnd::Interrupted)
        );
        assert!(reaching.runtimes.get(BufferId(1)).is_none());

        hosts.restore("devbox");
        let (buffer_id, status) = within_a_retry(&mut reaching.taken_up)
            .await
            .expect("taken up once the host can be reached");
        assert_eq!(buffer_id, BufferId(1));
        assert!(status.is_some_and(|status| status.running));
        assert!(reaching.runtimes.get(BufferId(1)).is_some());
        reaching.runtimes.shut_down(true).await;
        host.kill().await;
    }

    #[tokio::test]
    async fn a_restarted_server_takes_up_its_buffers_on_hosts_in_the_background() {
        let hosts = SocketHosts::default();
        let host = Host::start(&hosts, "devbox").await;
        let before = Reaching::new(Arc::new(hosts.clone()));
        let remote_id = start_on(&before.runtimes, BufferId(1), "devbox").await;
        before.runtimes.shut_down(false).await;

        let mut after = Reaching::new(Arc::new(hosts));
        let restored = after
            .runtimes
            .restore(vec![(BufferId(1), on_host("devbox", remote_id))])
            .await;

        assert!(restored.is_empty());
        let (buffer_id, _) = within_a_retry(&mut after.taken_up)
            .await
            .expect("taken up in the background");
        assert_eq!(buffer_id, BufferId(1));
        after.runtimes.shut_down(true).await;
        host.kill().await;
    }

    #[tokio::test]
    async fn a_buffer_gone_from_its_host_has_exited() {
        let hosts = SocketHosts::default();
        let host = Host::start(&hosts, "devbox").await;
        let mut reaching = Reaching::new(Arc::new(hosts));

        reaching
            .runtimes
            .restore(vec![(BufferId(1), on_host("devbox", BufferId(999)))])
            .await;

        assert_eq!(
            next_end(&mut reaching.ended).await,
            (BufferId(1), RuntimeEnd::Exited(None))
        );
        assert!(reaching.taken_up.try_recv().is_err());
        reaching.runtimes.shut_down(true).await;
        host.kill().await;
    }

    #[tokio::test]
    async fn a_released_buffer_is_not_taken_up_when_its_host_returns() {
        let hosts = SocketHosts::default();
        let host = Host::start(&hosts, "devbox").await;
        let before = Reaching::new(Arc::new(hosts.clone()));
        let remote_id = start_on(&before.runtimes, BufferId(1), "devbox").await;
        before.runtimes.shut_down(false).await;
        hosts.cut("devbox");

        let mut after = Reaching::new(Arc::new(hosts.clone()));
        after
            .runtimes
            .restore(vec![(BufferId(1), on_host("devbox", remote_id))])
            .await;
        after.runtimes.release(BufferId(1)).await;
        hosts.restore("devbox");

        assert!(within_a_retry(&mut after.taken_up).await.is_none());
        assert!(after.runtimes.get(BufferId(1)).is_none());
        after.runtimes.shut_down(true).await;
        host.kill().await;
    }

    async fn start(runtimes: &BufferRuntimes, buffer_id: BufferId, script: &str) -> Started {
        let command = ["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
        runtimes
            .start(
                buffer_id,
                StartRequest {
                    command: &command,
                    cwd: None,
                    env: &BTreeMap::new(),
                    size: PtySize::new(80, 24),
                    title: "test",
                    host: None,
                },
            )
            .await
            .expect("start keeper")
    }

    async fn next_end(
        ended: &mut mpsc::UnboundedReceiver<(BufferId, RuntimeEnd)>,
    ) -> (BufferId, RuntimeEnd) {
        tokio::time::timeout(Duration::from_secs(10), ended.recv())
            .await
            .expect("an end is reported")
            .expect("callbacks still alive")
    }

    #[tokio::test]
    async fn a_keeper_runs_the_buffer_on_this_machine() {
        let (runtimes, _ended, _dir) = new_runtimes();
        let buffer = BufferId(1);

        let started = start(&runtimes, buffer, "sleep 30").await;

        assert!(started.status.running);
        let Placement::Local {
            socket_path: Some(socket_path),
        } = &started.placement
        else {
            panic!("expected a keeper, got {:?}", started.placement);
        };
        assert!(socket_path.exists(), "{}", socket_path.display());
        assert!(runtimes.get(buffer).is_some());
        runtimes.shut_down(true).await;
    }

    #[tokio::test]
    async fn an_exited_keeper_waits_for_a_save() {
        let (runtimes, mut ended, _dir) = new_runtimes();
        let buffer = BufferId(2);
        start(&runtimes, buffer, "read line; exit 7").await;
        runtimes
            .get(buffer)
            .expect("runtime")
            .write(b"\n".to_vec())
            .await
            .expect("write");

        assert_eq!(
            next_end(&mut ended).await,
            (buffer, RuntimeEnd::Exited(Some(7)))
        );
        // An exited buffer can still be shown, so its runtime stays.
        assert!(runtimes.get(buffer).is_some());
        assert_eq!(runtimes.awaiting_save(), vec![buffer]);

        runtimes.saved(vec![buffer]).await;
        assert!(runtimes.awaiting_save().is_empty());
        runtimes.shut_down(true).await;
    }

    #[tokio::test]
    async fn a_released_keeper_waits_for_a_save() {
        let (runtimes, _ended, _dir) = new_runtimes();
        let buffer = BufferId(3);
        start(&runtimes, buffer, "sleep 30").await;

        runtimes.release(buffer).await;

        assert!(runtimes.get(buffer).is_none());
        assert_eq!(runtimes.awaiting_save(), vec![buffer]);
        runtimes.shut_down(true).await;
    }

    #[tokio::test]
    async fn a_shut_down_keeper_is_taken_up_by_the_next_server() {
        let (runtimes, _ended, _dir) = new_runtimes();
        let buffer = BufferId(4);
        let started = start(&runtimes, buffer, "sleep 30").await;
        let socket_path = started
            .placement
            .socket_path()
            .expect("keeper")
            .to_path_buf();

        runtimes.shut_down(false).await;
        assert!(runtimes.get(buffer).is_none());

        // Another server, with keepers elsewhere, takes it up by its socket.
        let (next, _next_ended, _next_dir) = new_runtimes();
        let restored = next
            .restore(vec![(
                buffer,
                Placement::Local {
                    socket_path: Some(socket_path),
                },
            )])
            .await;
        assert!(matches!(
            restored.as_slice(),
            [(id, Restored::Running(status))] if *id == buffer && status.running
        ));
        assert!(next.get(buffer).is_some());
        next.shut_down(true).await;
    }

    #[tokio::test]
    async fn a_missing_keeper_is_gone() {
        let (runtimes, _ended, dir) = new_runtimes();

        let restored = runtimes
            .restore(vec![(
                BufferId(5),
                Placement::Local {
                    socket_path: Some(dir.path().join("missing.sock")),
                },
            )])
            .await;

        assert!(matches!(restored.as_slice(), [(_, Restored::Gone)]));
        assert!(runtimes.get(BufferId(5)).is_none());
    }

    #[tokio::test]
    async fn a_keeper_that_does_not_answer_is_gone() {
        let (runtimes, _ended, dir) = new_runtimes();
        let socket_path = dir.path().join("stale.sock");
        drop(UnixListener::bind(&socket_path).expect("bind stale socket"));

        let restored = runtimes
            .restore(vec![(
                BufferId(6),
                Placement::Local {
                    socket_path: Some(socket_path),
                },
            )])
            .await;

        assert!(matches!(restored.as_slice(), [(_, Restored::Gone)]));
        assert!(runtimes.get(BufferId(6)).is_none());
    }

    #[tokio::test]
    async fn nothing_starts_before_the_callbacks_are_set() {
        let dir = tempdir().expect("tempdir");
        let runtimes = BufferRuntimes::new(
            KeeperSettings {
                server_socket: dir.path().join("mux.sock"),
                runtime_dir: dir.path().join("runtime"),
                env: BTreeMap::new(),
            },
            Arc::new(CommandConnector::new(Vec::new())),
        );
        let command = ["/bin/true".to_owned()];

        let started = runtimes
            .start(
                BufferId(7),
                StartRequest {
                    command: &command,
                    cwd: None,
                    env: &BTreeMap::new(),
                    size: PtySize::new(80, 24),
                    title: "test",
                    host: None,
                },
            )
            .await;

        assert!(started.is_err());
    }

    #[test]
    fn an_end_during_shutdown_is_an_interrupt() {
        let (runtimes, _ended, _dir) = new_runtimes();
        let inner = &runtimes.inner;
        lock(&inner.shutting_down).insert(BufferId(8));

        assert_eq!(
            inner.ended(BufferId(8), RuntimeEnd::Exited(Some(0))),
            (RuntimeEnd::Interrupted, None)
        );
        // Only the end the shutdown caused.
        assert_eq!(
            inner.ended(BufferId(8), RuntimeEnd::Exited(Some(0))),
            (RuntimeEnd::Exited(Some(0)), None)
        );
    }

    #[tokio::test]
    async fn a_buffer_on_a_host_waits_for_it_until_released() {
        let (runtimes, _ended, _dir) = new_runtimes();

        let restored = runtimes
            .restore(vec![(
                BufferId(9),
                Placement::Remote {
                    host: "devbox".to_owned(),
                    buffer_id: Some(BufferId(3)),
                },
            )])
            .await;

        // Its host is reached in the background, so nothing is reported now.
        assert!(restored.is_empty());
        assert!(runtimes.inner.waits_for("devbox"));
        runtimes.release(BufferId(9)).await;
        assert!(!runtimes.inner.waits_for("devbox"));
        runtimes.shut_down(false).await;
    }
}
