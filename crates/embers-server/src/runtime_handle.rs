use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use embers_core::{BufferId, MuxError, PtySize, Result, TerminalSnapshot};

use crate::buffer_runtime::{
    BufferRuntimeCallbacks, BufferRuntimePipeStatus, BufferRuntimePipeStopReason,
    BufferRuntimeStatus, KeeperRuntimeHandle, RuntimeScrollbackSlice, RuntimeSnapshot,
};
use crate::remote::{HostConnection, RemoteRuntimeHandle};

/// What runs a buffer's program and emulates its terminal, as the server sees
/// it: a keeper process on this machine, or another host's embers server.
#[derive(Clone, Debug)]
pub enum BufferRuntimeHandle {
    Keeper(KeeperRuntimeHandle),
    Remote(RemoteRuntimeHandle),
}

impl BufferRuntimeHandle {
    /// Start a keeper for `buffer_id` listening on `socket_path`, running
    /// `command`.
    pub async fn spawn_keeper(
        buffer_id: BufferId,
        socket_path: PathBuf,
        command: &[String],
        cwd: Option<&Path>,
        env: &BTreeMap<String, OsString>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        KeeperRuntimeHandle::spawn(buffer_id, socket_path, command, cwd, env, size, callbacks)
            .await
            .map(Self::Keeper)
    }

    /// Reattach to the keeper of `buffer_id` listening on `socket_path`, as
    /// left by an earlier server.
    pub async fn attach_keeper(
        buffer_id: BufferId,
        socket_path: PathBuf,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        KeeperRuntimeHandle::attach(buffer_id, socket_path, callbacks)
            .await
            .map(Self::Keeper)
    }

    /// Start `command` for `buffer_id` on `connection`'s host (see
    /// [`RemoteRuntimeHandle::spawn`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_remote(
        buffer_id: BufferId,
        connection: HostConnection,
        title: Option<String>,
        command: &[String],
        cwd: Option<&Path>,
        env: BTreeMap<String, String>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        RemoteRuntimeHandle::spawn(
            buffer_id, connection, title, command, cwd, env, size, callbacks,
        )
        .await
        .map(Self::Remote)
    }

    /// Take up the buffer `remote_id` of `connection`'s host again.
    pub(crate) async fn attach_remote(
        buffer_id: BufferId,
        connection: HostConnection,
        remote_id: BufferId,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        RemoteRuntimeHandle::attach(buffer_id, connection, remote_id, callbacks)
            .await
            .map(Self::Remote)
    }

    /// The socket of the keeper running this buffer, which the workspace
    /// saves so a later server can reattach to it.
    pub fn keeper_socket_path(&self) -> Option<&Path> {
        match self {
            Self::Keeper(keeper) => Some(keeper.socket_path()),
            Self::Remote(_) => None,
        }
    }

    /// The host running this buffer and the buffer's id there.
    pub fn remote(&self) -> Option<(&str, BufferId)> {
        match self {
            Self::Keeper(_) => None,
            Self::Remote(remote) => Some((remote.host(), remote.remote_id())),
        }
    }

    pub async fn status(&self) -> Result<BufferRuntimeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.status().await,
            Self::Remote(remote) => remote.status().await,
        }
    }

    /// `cwd` is what a keeper reports as the buffer's directory; a host
    /// reports its own.
    pub async fn capture_snapshot(&self, cwd: Option<PathBuf>) -> Result<RuntimeSnapshot> {
        match self {
            Self::Keeper(keeper) => keeper.capture_snapshot(cwd).await,
            Self::Remote(remote) => remote.capture_snapshot().await,
        }
    }

    pub async fn capture_visible_snapshot(&self, cwd: Option<PathBuf>) -> Result<TerminalSnapshot> {
        match self {
            Self::Keeper(keeper) => keeper.capture_visible_snapshot(cwd).await,
            Self::Remote(remote) => remote.capture_visible_snapshot().await,
        }
    }

    pub async fn capture_scrollback_slice(
        &self,
        start_line: u64,
        line_count: u32,
    ) -> Result<RuntimeScrollbackSlice> {
        match self {
            Self::Keeper(keeper) => {
                keeper
                    .capture_scrollback_slice(start_line, line_count)
                    .await
            }
            Self::Remote(remote) => {
                remote
                    .capture_scrollback_slice(start_line, line_count)
                    .await
            }
        }
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.write(bytes).await,
            Self::Remote(remote) => remote.write(bytes).await,
        }
    }

    /// End the program for kill-server, leaving nothing to reattach to. A
    /// buffer on another host is killed there, so no detached buffer is left
    /// behind on that host.
    pub async fn terminate(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.terminate().await,
            Self::Remote(remote) => remote.kill().await,
        }
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.resize(size).await,
            Self::Remote(remote) => remote.resize(size).await,
        }
    }

    /// Whether some client forwards kitty graphics queries to a host
    /// terminal that answers them. Another host's buffers do not pass
    /// graphics on yet, so they are not told.
    pub async fn set_host_graphics(&self, enabled: bool) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.set_host_graphics(enabled).await,
            Self::Remote(_) => Ok(()),
        }
    }

    pub async fn kill(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.kill().await,
            Self::Remote(remote) => remote.kill().await,
        }
    }

    /// Say that a saved workspace no longer needs this runtime. Only call it
    /// once that save has completed. Another host keeps its buffers itself.
    pub async fn acknowledge_exit_recorded(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.acknowledge_exit_recorded().await,
            Self::Remote(_) => Ok(()),
        }
    }

    pub async fn start_pipe(
        &self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<BufferRuntimePipeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.start_pipe(command, cwd, env).await,
            Self::Remote(remote) => remote.start_pipe(command, cwd, env).await,
        }
    }

    pub async fn stop_pipe(&self) -> Result<BufferRuntimePipeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.stop_pipe().await,
            Self::Remote(remote) => remote.stop_pipe().await,
        }
    }

    /// The pipe's status once the buffer's program exited. Another host stops
    /// the pipe itself then, so its last status is all there is.
    pub(crate) async fn stop_pipe_after_exit(&self) -> Result<BufferRuntimePipeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.stop_pipe_after_exit().await,
            Self::Remote(remote) => {
                let mut pipe = remote
                    .status()
                    .await?
                    .pipe
                    .ok_or_else(|| MuxError::conflict("the buffer has no pipe"))?;
                if pipe.running {
                    pipe.running = false;
                    pipe.stop_reason = Some(BufferRuntimePipeStopReason::BufferExited);
                }
                Ok(pipe)
            }
        }
    }

    pub async fn join_threads(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.join_threads().await,
            Self::Remote(remote) => remote.join_threads().await,
        }
    }
}
