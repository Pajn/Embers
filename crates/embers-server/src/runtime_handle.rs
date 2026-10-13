use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use embers_core::{BufferId, PtySize, Result, TerminalSnapshot};

use crate::buffer_runtime::{
    BufferRuntimeCallbacks, BufferRuntimePipeStatus, BufferRuntimeStatus, KeeperRuntimeHandle,
    KeeperScrollbackSlice, KeeperSnapshot,
};

/// What runs a buffer's program and emulates its terminal, as the server sees
/// it. Every runtime is a keeper process on this machine for now; the enum is
/// where runtimes reached another way join it.
#[derive(Clone, Debug)]
pub enum BufferRuntimeHandle {
    Keeper(KeeperRuntimeHandle),
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

    /// The socket of the keeper running this buffer, which the workspace
    /// saves so a later server can reattach to it.
    pub fn keeper_socket_path(&self) -> Option<&Path> {
        match self {
            Self::Keeper(keeper) => Some(keeper.socket_path()),
        }
    }

    pub async fn status(&self) -> Result<BufferRuntimeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.status().await,
        }
    }

    pub async fn capture_snapshot(&self, cwd: Option<PathBuf>) -> Result<KeeperSnapshot> {
        match self {
            Self::Keeper(keeper) => keeper.capture_snapshot(cwd).await,
        }
    }

    pub async fn capture_visible_snapshot(&self, cwd: Option<PathBuf>) -> Result<TerminalSnapshot> {
        match self {
            Self::Keeper(keeper) => keeper.capture_visible_snapshot(cwd).await,
        }
    }

    pub async fn capture_scrollback_slice(
        &self,
        start_line: u64,
        line_count: u32,
    ) -> Result<KeeperScrollbackSlice> {
        match self {
            Self::Keeper(keeper) => {
                keeper
                    .capture_scrollback_slice(start_line, line_count)
                    .await
            }
        }
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.write(bytes).await,
        }
    }

    /// End the program for kill-server, leaving nothing to reattach to.
    pub async fn terminate(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.terminate().await,
        }
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.resize(size).await,
        }
    }

    /// Whether some client forwards kitty graphics queries to a host
    /// terminal that answers them.
    pub async fn set_host_graphics(&self, enabled: bool) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.set_host_graphics(enabled).await,
        }
    }

    pub async fn kill(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.kill().await,
        }
    }

    /// Say that a saved workspace no longer needs this runtime. Only call it
    /// once that save has completed.
    pub async fn acknowledge_exit_recorded(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.acknowledge_exit_recorded().await,
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
        }
    }

    pub async fn stop_pipe(&self) -> Result<BufferRuntimePipeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.stop_pipe().await,
        }
    }

    pub(crate) async fn stop_pipe_after_exit(&self) -> Result<BufferRuntimePipeStatus> {
        match self {
            Self::Keeper(keeper) => keeper.stop_pipe_after_exit().await,
        }
    }

    pub async fn join_threads(&self) -> Result<()> {
        match self {
            Self::Keeper(keeper) => keeper.join_threads().await,
        }
    }
}
