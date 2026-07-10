use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use embers_core::{
    ActivityState, BufferId, MuxError, PtySize, Result, SnapshotLine, StyledRun, TerminalSnapshot,
};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize as PortablePtySize,
    PtySystem,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::{AlacrittyTerminalBackend, RawByteRouter, TerminalBackend};

const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);
const CONNECT_RETRY_ATTEMPTS: usize = 1200;
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
const KEEPER_PIPE_WRITE_QUEUE_CAPACITY: usize = 64;
/// Bound on bytes queued for the PTY master. Every writer — client input
/// (`KeeperRequest::Write`) and emulator replies (device-attribute /
/// cursor-position / mode-query responses) — hands off through this queue, which
/// a single dedicated writer thread drains. Because no caller holds a lock during
/// the blocking `write_all`, a child that stops reading its input backpressures
/// the queue instead of stalling request handling or the read loop. The read loop
/// (the only thread draining child output) must never block, so it drops replies
/// best-effort when the queue is full and keeps draining, which lets the child
/// make progress and unblocks the writer.
const KEEPER_WRITE_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug)]
pub struct BufferRuntimeUpdate {
    pub sequence: u64,
    pub activity: ActivityState,
    pub title: Option<Option<String>>,
    pub pipe: Option<Option<BufferRuntimePipeStatus>>,
    /// `Some(value)` when the working directory changed in this update (`value`
    /// is `None` when it became unknown); `None` when cwd is unchanged.
    pub cwd: Option<Option<PathBuf>>,
}

#[derive(Clone, Debug)]
pub struct BufferRuntimeStatus {
    pub pid: Option<u32>,
    pub sequence: u64,
    pub activity: ActivityState,
    pub title: Option<String>,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub pipe: Option<BufferRuntimePipeStatus>,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BufferRuntimePipeStopReason {
    Requested,
    PipeExited,
    WriteFailed,
    BufferExited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferRuntimePipeStatus {
    pub command: Vec<String>,
    pub running: bool,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stop_reason: Option<BufferRuntimePipeStopReason>,
}

#[derive(Clone)]
pub struct BufferRuntimeHandle {
    inner: Arc<BufferRuntimeInner>,
}

struct BufferRuntimeInner {
    buffer_id: BufferId,
    pid: Option<u32>,
    socket_path: PathBuf,
    connection: Mutex<KeeperConnection>,
    stop: AtomicBool,
    threads: Mutex<RuntimeThreads>,
}

#[derive(Default)]
struct RuntimeThreads {
    poller: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub struct BufferRuntimeCallbacks {
    pub on_output: Arc<dyn Fn(BufferId, BufferRuntimeUpdate) + Send + Sync>,
    pub on_exit: Arc<dyn Fn(BufferId, Option<i32>) + Send + Sync>,
}

#[derive(Clone)]
pub struct RuntimeKeeperCli {
    pub socket_path: PathBuf,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, OsString>,
    pub size: PtySize,
}

struct KeeperConnection {
    stream: UnixStream,
}

#[derive(Serialize, Deserialize)]
enum KeeperRequest {
    Status,
    Write {
        bytes: Vec<u8>,
    },
    Resize {
        size: PtySize,
    },
    Snapshot {
        cwd: Option<PathBuf>,
    },
    VisibleSnapshot {
        cwd: Option<PathBuf>,
    },
    ScrollbackSlice {
        start_line: u64,
        line_count: u32,
    },
    StartPipe {
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    },
    StopPipe,
    StopPipeAfterExit,
    Kill,
}

#[derive(Serialize, Deserialize)]
enum KeeperResponse {
    Status(KeeperStatus),
    Snapshot(KeeperSnapshot),
    VisibleSnapshot(TerminalSnapshot),
    ScrollbackSlice(KeeperScrollbackSlice),
    PipeStatus(BufferRuntimePipeStatus),
    Ok,
    Error { message: String },
}

#[derive(Clone, Serialize, Deserialize)]
struct KeeperStatus {
    pid: Option<u32>,
    sequence: u64,
    activity: ActivityState,
    title: Option<String>,
    running: bool,
    exit_code: Option<i32>,
    pipe: Option<BufferRuntimePipeStatus>,
    // Defaulted so a newer server stays compatible with an older keeper that
    // predates live-cwd reporting.
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeeperSnapshot {
    pub sequence: u64,
    pub size: PtySize,
    pub lines: Vec<String>,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// Budget for scrollback-slice styling. Above this the keeper drops styles and
/// ships plain text, keeping the JSON (16 MiB) and client frame (8 MiB) caps
/// safe. Well below the caps so text and framing overhead still fit.
const MAX_STYLE_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Rough per-run cost when a [`StyledRun`] is serialized to keeper JSON. Used
/// only to estimate whether a slice's styling fits the budget.
const ESTIMATED_STYLED_RUN_JSON_BYTES: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
pub struct KeeperScrollbackSlice {
    pub start_line: u64,
    pub total_lines: u64,
    pub lines: Vec<String>,
    /// Parallel per-line style runs. A field distinct from `lines` (rather than
    /// retyping `lines`) keeps the keeper JSON compatible with older keeper
    /// processes: `#[serde(default)]` makes a missing `styles` decode as no
    /// styling, and a short/empty inner vec means that line is plain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<Vec<StyledRun>>,
}

impl KeeperScrollbackSlice {
    /// Zip `lines` and `styles` back into styled snapshot lines. A missing or
    /// short `styles` entry yields a plain line, so old keepers degrade to text.
    pub fn into_snapshot_lines(self) -> Vec<SnapshotLine> {
        let mut styles = self.styles.into_iter();
        self.lines
            .into_iter()
            .map(|text| SnapshotLine {
                text,
                runs: styles.next().unwrap_or_default(),
            })
            .collect()
    }
}

struct KeeperRuntime {
    surface: Mutex<KeeperSurface>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// All PTY-master writes are submitted here and serialized by the writer
    /// thread that owns the underlying writer (see `keeper_writer_loop`). Keeping
    /// the writer off the request path means no handler holds a lock across a
    /// blocking write.
    write_tx: SyncSender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pipe: Mutex<Option<KeeperPipe>>,
    sequence: AtomicU64,
    activity: Mutex<ActivityState>,
    exit_code: Mutex<Option<Option<i32>>>,
    pid: Option<u32>,
}

struct KeeperSurface {
    router: RawByteRouter,
    backend: Box<dyn TerminalBackend>,
    size: PtySize,
}

struct KeeperPipe {
    command: Vec<String>,
    child: ProcessCommandChild,
    writer_tx: Option<SyncSender<Vec<u8>>>,
    writer_state: Arc<Mutex<KeeperPipeWriterState>>,
    writer_thread: Option<thread::JoinHandle<()>>,
    stop_reason: Option<BufferRuntimePipeStopReason>,
    exit_code: Option<i32>,
}

type ProcessCommandChild = std::process::Child;

#[derive(Default)]
struct KeeperPipeWriterState {
    failed: bool,
}

impl std::fmt::Debug for BufferRuntimeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BufferRuntimeHandle")
            .field("buffer_id", &self.inner.buffer_id)
            .field("pid", &self.inner.pid)
            .field("socket_path", &self.inner.socket_path)
            .finish()
    }
}

impl BufferRuntimeHandle {
    pub async fn spawn(
        buffer_id: BufferId,
        socket_path: PathBuf,
        command: &[String],
        cwd: Option<&Path>,
        env: &BTreeMap<String, OsString>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let command = command.to_vec();
        let cwd = cwd.map(Path::to_path_buf);
        let env = env.clone();
        tokio::task::spawn_blocking(move || {
            Self::spawn_blocking(buffer_id, socket_path, command, cwd, env, size, callbacks)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    fn spawn_blocking(
        buffer_id: BufferId,
        socket_path: PathBuf,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, OsString>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        if command.is_empty() {
            return Err(MuxError::invalid_input("buffer command must not be empty"));
        }
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }

        let cli = RuntimeKeeperCli {
            socket_path: socket_path.clone(),
            command,
            cwd,
            env,
            size,
        };
        spawn_runtime_keeper(cli)?;

        Self::attach_blocking(buffer_id, socket_path, callbacks)
    }

    pub async fn attach(
        buffer_id: BufferId,
        socket_path: PathBuf,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            Self::attach_blocking(buffer_id, socket_path, callbacks)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    fn attach_blocking(
        buffer_id: BufferId,
        socket_path: PathBuf,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let stream = connect_to_keeper(&socket_path)?;
        let mut connection = KeeperConnection { stream };
        let initial = connection.status()?;
        let inner = Arc::new(BufferRuntimeInner {
            buffer_id,
            pid: initial.pid,
            socket_path,
            connection: Mutex::new(connection),
            stop: AtomicBool::new(false),
            threads: Mutex::new(RuntimeThreads::default()),
        });

        let poller = spawn_status_poller(inner.clone(), callbacks, initial)?;
        inner
            .threads
            .lock()
            .map_err(|_| MuxError::internal("buffer runtime thread registry lock poisoned"))?
            .poller = Some(poller);

        Ok(Self { inner })
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    pub fn socket_path(&self) -> &Path {
        &self.inner.socket_path
    }

    pub async fn status(&self) -> Result<BufferRuntimeStatus> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.status()
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn capture_snapshot(&self, cwd: Option<PathBuf>) -> Result<KeeperSnapshot> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.snapshot(cwd)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn capture_visible_snapshot(&self, cwd: Option<PathBuf>) -> Result<TerminalSnapshot> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.visible_snapshot(cwd)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn capture_scrollback_slice(
        &self,
        start_line: u64,
        line_count: u32,
    ) -> Result<KeeperScrollbackSlice> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.scrollback_slice(start_line, line_count)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.write(bytes)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.resize(size)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn kill(&self) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.kill()
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn start_pipe(
        &self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<BufferRuntimePipeStatus> {
        validate_pipe_command(&command)?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.start_pipe(command, cwd, env)
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn stop_pipe(&self) -> Result<BufferRuntimePipeStatus> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.stop_pipe()
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub(crate) async fn stop_pipe_after_exit(&self) -> Result<BufferRuntimePipeStatus> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.stop_pipe_after_exit()
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn join_threads(&self) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.join_threads_blocking())
            .await
            .map_err(|error| MuxError::internal(error.to_string()))
    }
}

impl BufferRuntimeInner {
    fn join_threads_blocking(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let poller = match self.threads.lock() {
            Ok(mut threads) => threads.poller.take(),
            Err(poisoned) => {
                error!(
                    %self.buffer_id,
                    "buffer runtime thread registry lock poisoned during shutdown"
                );
                poisoned.into_inner().poller.take()
            }
        };
        if let Some(poller) = poller
            && poller.thread().id() != thread::current().id()
        {
            let _ = poller.join();
        }
    }
}

impl Drop for BufferRuntimeInner {
    fn drop(&mut self) {
        self.join_threads_blocking();
    }
}

impl KeeperConnection {
    fn request(&mut self, request: KeeperRequest) -> Result<KeeperResponse> {
        write_message(&mut self.stream, &request)?;
        match read_message(&mut self.stream)? {
            Some(KeeperResponse::Error { message }) => Err(MuxError::transport(message)),
            Some(response) => Ok(response),
            None => Err(MuxError::transport("runtime keeper disconnected")),
        }
    }

    fn status(&mut self) -> Result<BufferRuntimeStatus> {
        match self.request(KeeperRequest::Status)? {
            KeeperResponse::Status(status) => Ok(BufferRuntimeStatus {
                pid: status.pid,
                sequence: status.sequence,
                activity: status.activity,
                title: status.title,
                running: status.running,
                exit_code: status.exit_code,
                pipe: status.pipe,
                cwd: status.cwd,
            }),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper status response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn write(&mut self, bytes: Vec<u8>) -> Result<()> {
        match self.request(KeeperRequest::Write { bytes })? {
            KeeperResponse::Ok => Ok(()),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper write response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn resize(&mut self, size: PtySize) -> Result<()> {
        match self.request(KeeperRequest::Resize { size })? {
            KeeperResponse::Ok => Ok(()),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper resize response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn snapshot(&mut self, cwd: Option<PathBuf>) -> Result<KeeperSnapshot> {
        match self.request(KeeperRequest::Snapshot { cwd })? {
            KeeperResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper snapshot response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn visible_snapshot(&mut self, cwd: Option<PathBuf>) -> Result<TerminalSnapshot> {
        match self.request(KeeperRequest::VisibleSnapshot { cwd })? {
            KeeperResponse::VisibleSnapshot(snapshot) => Ok(snapshot),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper visible snapshot response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn scrollback_slice(
        &mut self,
        start_line: u64,
        line_count: u32,
    ) -> Result<KeeperScrollbackSlice> {
        match self.request(KeeperRequest::ScrollbackSlice {
            start_line,
            line_count,
        })? {
            KeeperResponse::ScrollbackSlice(slice) => Ok(slice),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper scrollback response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn kill(&mut self) -> Result<()> {
        match self.request(KeeperRequest::Kill)? {
            KeeperResponse::Ok => Ok(()),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper kill response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn start_pipe(
        &mut self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<BufferRuntimePipeStatus> {
        validate_pipe_command(&command)?;
        match self.request(KeeperRequest::StartPipe { command, cwd, env })? {
            KeeperResponse::PipeStatus(status) => Ok(status),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper start pipe response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn stop_pipe(&mut self) -> Result<BufferRuntimePipeStatus> {
        match self.request(KeeperRequest::StopPipe)? {
            KeeperResponse::PipeStatus(status) => Ok(status),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper stop pipe response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn stop_pipe_after_exit(&mut self) -> Result<BufferRuntimePipeStatus> {
        match self.request(KeeperRequest::StopPipeAfterExit)? {
            KeeperResponse::PipeStatus(status) => Ok(status),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper stop pipe after exit response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }
}

impl KeeperSurface {
    fn new(size: PtySize) -> Self {
        // The keeper inherits the server's environment, so resolving the
        // scrollback ceiling here applies a single operator-configured value to
        // this buffer (see `MAX_SCROLLBACK_LINES_ENV_VAR`).
        let max_scrollback_lines = crate::config::ResourceLimits::from_env().max_scrollback_lines;
        Self {
            router: RawByteRouter::default(),
            backend: Box::new(AlacrittyTerminalBackend::new(size, max_scrollback_lines)),
            size,
        }
    }

    /// Ingest PTY output and return the resulting activity plus any terminal
    /// replies the emulator generated (device-attribute / cursor-position / mode
    /// queries). The caller writes the replies back to the PTY master.
    fn route_output(&mut self, bytes: &[u8]) -> (ActivityState, Vec<u8>) {
        self.router.route_output(self.backend.as_mut(), bytes);
        self.backend.take_events()
    }

    fn resize(&mut self, size: PtySize) {
        self.size = size;
        self.backend.resize(size);
    }

    fn capture_lines(&self) -> Vec<String> {
        self.backend.capture_scrollback()
    }

    fn capture_visible_snapshot(&self, sequence: u64, cwd: Option<PathBuf>) -> TerminalSnapshot {
        self.backend.visible_snapshot(sequence, self.size, cwd)
    }

    fn capture_scrollback_slice(&self, start_line: u64, line_count: u32) -> KeeperScrollbackSlice {
        let slice = self
            .backend
            .capture_scrollback_slice(start_line, line_count);
        let mut lines = Vec::with_capacity(slice.lines.len());
        let mut styles = Vec::with_capacity(slice.lines.len());
        for line in slice.lines {
            lines.push(line.text);
            styles.push(line.runs);
        }

        // Styles are best-effort: a large `line_count` could produce a styled
        // payload big enough to blow the keeper JSON (16 MiB) or client frame
        // (8 MiB) caps. If the estimated styling exceeds the budget, ship plain
        // text — the text always survives.
        let run_count: usize = styles.iter().map(Vec::len).sum();
        if run_count.saturating_mul(ESTIMATED_STYLED_RUN_JSON_BYTES) > MAX_STYLE_PAYLOAD_BYTES
            || styles.iter().all(Vec::is_empty)
        {
            styles.clear();
        }

        KeeperScrollbackSlice {
            start_line: slice.start_line,
            total_lines: slice.total_lines,
            lines,
            styles,
        }
    }
}

impl KeeperPipe {
    fn spawn(
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<Self> {
        let Some(program) = command.first() else {
            return Err(MuxError::invalid_input(
                "buffer pipe command must not be empty",
            ));
        };
        let mut child = ProcessCommand::new(program);
        child.args(&command[1..]);
        if let Some(cwd) = cwd {
            child.current_dir(cwd);
        }
        child.envs(env);
        child.stdin(Stdio::piped());
        child.stdout(Stdio::null());
        child.stderr(Stdio::null());
        let mut child = child.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MuxError::internal("buffer pipe stdin was not piped"))?;
        let (writer_tx, writer_rx) = sync_channel(KEEPER_PIPE_WRITE_QUEUE_CAPACITY);
        let writer_state = Arc::new(Mutex::new(KeeperPipeWriterState::default()));
        let writer_thread_state = writer_state.clone();
        let writer_pid = child.id();
        let writer_thread = thread::Builder::new()
            .name(format!("keeper-pipe-writer-{writer_pid}"))
            .spawn(move || keeper_pipe_write_loop(stdin, writer_rx, writer_thread_state))
            .map_err(|error| MuxError::internal(error.to_string()))?;
        Ok(Self {
            command,
            child,
            writer_tx: Some(writer_tx),
            writer_state,
            writer_thread: Some(writer_thread),
            stop_reason: None,
            exit_code: None,
        })
    }

    fn status(&mut self) -> BufferRuntimePipeStatus {
        self.refresh();
        BufferRuntimePipeStatus {
            command: self.command.clone(),
            running: self.exit_code.is_none() && self.stop_reason.is_none(),
            pid: (self.exit_code.is_none() && self.stop_reason.is_none()).then(|| self.child.id()),
            exit_code: self.exit_code,
            stop_reason: self.stop_reason,
        }
    }

    fn refresh(&mut self) {
        if self.exit_code.is_some() || self.stop_reason.is_some() {
            return;
        }
        if self.writer_failed() {
            let _ = self.terminate(BufferRuntimePipeStopReason::WriteFailed);
            return;
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            self.exit_code = exit_status_code(status.into());
            self.stop_reason
                .get_or_insert(BufferRuntimePipeStopReason::PipeExited);
            self.close_writer();
            self.join_writer();
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.exit_code.is_some() || self.stop_reason.is_some() {
            return;
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            self.exit_code = exit_status_code(status.into());
            self.stop_reason
                .get_or_insert(BufferRuntimePipeStopReason::PipeExited);
            self.close_writer();
            self.join_writer();
            return;
        }
        let Some(writer_tx) = self.writer_tx.as_ref() else {
            return;
        };
        match writer_tx.try_send(bytes.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.record_write_failure();
            }
        }
    }

    fn stop(&mut self, reason: BufferRuntimePipeStopReason) -> Result<BufferRuntimePipeStatus> {
        self.refresh();
        if self.exit_code.is_some() || self.stop_reason.is_some() {
            return Err(MuxError::conflict("buffer pipe is not running"));
        }
        self.terminate(reason)?;
        Ok(self.status())
    }

    fn writer_failed(&self) -> bool {
        self.writer_state
            .lock()
            .map(|state| state.failed)
            .unwrap_or(true)
    }

    fn record_write_failure(&mut self) {
        if let Ok(mut state) = self.writer_state.lock() {
            state.failed = true;
        }
        self.close_writer();
    }

    fn close_writer(&mut self) {
        let _ = self.writer_tx.take();
    }

    fn join_writer(&mut self) {
        if let Some(writer_thread) = self.writer_thread.take() {
            let _ = writer_thread.join();
        }
    }

    fn terminate(&mut self, reason: BufferRuntimePipeStopReason) -> Result<()> {
        self.stop_reason = Some(reason);
        self.close_writer();
        if let Err(error) = self.child.kill()
            && error.kind() != std::io::ErrorKind::InvalidInput
        {
            return Err(error.into());
        }
        if let Ok(status) = self.child.wait() {
            self.exit_code = exit_status_code(status.into());
        }
        self.join_writer();
        Ok(())
    }
}

impl Drop for KeeperPipe {
    fn drop(&mut self) {
        if self.exit_code.is_none() && self.stop_reason.is_none() {
            self.close_writer();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.join_writer();
    }
}

/// Maximum retries for PTY allocation in runtime keeper
const KEEPER_PTY_MAX_RETRIES: usize = 3;

/// Delay between PTY allocation retries
const KEEPER_PTY_RETRY_DELAY: Duration = Duration::from_millis(100);

pub fn run_runtime_keeper(cli: RuntimeKeeperCli) -> Result<()> {
    let Some(program) = cli.command.first() else {
        return Err(MuxError::invalid_input(
            "runtime keeper command must not be empty",
        ));
    };

    if let Some(parent) = cli.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if cli.socket_path.exists() {
        let _ = fs::remove_file(&cli.socket_path);
    }
    let listener = UnixListener::bind(&cli.socket_path)?;
    let _cleanup = SocketCleanup::new(cli.socket_path.clone());

    let pty_system = NativePtySystem::default();
    let mut last_error = None;

    // Try to open PTY with retries.
    let mut pair = None;
    for attempt in 0..=KEEPER_PTY_MAX_RETRIES {
        match pty_system.openpty(to_portable_size(cli.size)) {
            Ok(opened_pair) => {
                pair = Some(opened_pair);
                break;
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < KEEPER_PTY_MAX_RETRIES {
                    thread::sleep(KEEPER_PTY_RETRY_DELAY * 2_u32.pow(attempt as u32));
                }
            }
        }
    }
    let pair = match pair {
        Some(pair) => pair,
        None => {
            // Each failed retry stores the PTY allocation error before continuing, so
            // last_error is normally populated; fall back to a generic message if not
            // rather than panicking on this hot spawn path.
            let attempts = KEEPER_PTY_MAX_RETRIES + 1;
            let detail = match last_error {
                Some(error) => format!("failed to openpty after {attempts} attempts: {error}"),
                None => format!("failed to openpty after {attempts} attempts"),
            };
            return Err(MuxError::pty(detail));
        }
    };

    let mut command_builder = CommandBuilder::new(program);
    command_builder.args(&cli.command[1..]);
    if let Some(cwd) = &cli.cwd {
        command_builder.cwd(cwd);
    }
    for (key, value) in &cli.env {
        command_builder.env(key, value);
    }

    let child = pair
        .slave
        .spawn_command(command_builder)
        .map_err(|error| MuxError::pty(error.to_string()))?;
    let pid = child.process_id();
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| MuxError::pty(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| MuxError::pty(error.to_string()))?;

    // A single writer thread owns the writer and drains every PTY-master write
    // (client input and emulator replies) from this bounded queue, so no request
    // handler holds a lock across a blocking write. The only senders live in the
    // shared runtime; the thread ends once the last runtime reference is dropped.
    let (write_tx, write_rx) = sync_channel(KEEPER_WRITE_QUEUE_CAPACITY);
    let writer_join = thread::Builder::new()
        .name(format!("keeper-writer-{}", cli.socket_path.display()))
        .spawn(move || keeper_writer_loop(writer, write_rx))
        .map_err(|error| MuxError::internal(error.to_string()))?;

    let runtime = Arc::new(KeeperRuntime {
        surface: Mutex::new(KeeperSurface::new(cli.size)),
        master: Mutex::new(pair.master),
        write_tx,
        killer: Mutex::new(killer),
        pipe: Mutex::new(None),
        sequence: AtomicU64::new(0),
        activity: Mutex::new(ActivityState::Idle),
        exit_code: Mutex::new(None),
        pid,
    });

    let reader_runtime = runtime.clone();
    let reader_join = thread::Builder::new()
        .name(format!("keeper-reader-{}", cli.socket_path.display()))
        .spawn(move || keeper_read_loop(reader_runtime, reader))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let wait_runtime = runtime.clone();
    let wait_join = thread::Builder::new()
        .name(format!("keeper-wait-{}", cli.socket_path.display()))
        .spawn(move || keeper_wait_loop(wait_runtime, child))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let mut terminate = false;
    while !terminate {
        let (mut stream, _) = listener.accept()?;
        terminate = handle_keeper_client(runtime.clone(), &mut stream)?;
    }

    let _ = reader_join.join();
    let _ = wait_join.join();
    // Drop the last runtime reference so the write channel disconnects and the
    // writer thread finishes draining before we join it.
    drop(runtime);
    let _ = writer_join.join();
    Ok(())
}

fn handle_keeper_client(runtime: Arc<KeeperRuntime>, stream: &mut UnixStream) -> Result<bool> {
    loop {
        let request = match read_message::<KeeperRequest>(stream) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(false),
            Err(error) => {
                let response = KeeperResponse::Error {
                    message: error.to_string(),
                };
                if write_message(stream, &response).is_err() {
                    return Ok(false);
                }
                continue;
            }
        };
        let (response, terminate) = match handle_keeper_request(&runtime, request) {
            Ok(result) => result,
            Err(error) => {
                let response = KeeperResponse::Error {
                    message: error.to_string(),
                };
                if write_message(stream, &response).is_err() {
                    return Ok(false);
                }
                continue;
            }
        };
        if write_message(stream, &response).is_err() {
            return Ok(false);
        }
        if terminate {
            return Ok(true);
        }
    }
}

fn handle_keeper_request(
    runtime: &Arc<KeeperRuntime>,
    request: KeeperRequest,
) -> Result<(KeeperResponse, bool)> {
    match request {
        KeeperRequest::Status => Ok((KeeperResponse::Status(runtime.status()?), false)),
        KeeperRequest::Write { bytes } => {
            runtime.write(bytes)?;
            Ok((KeeperResponse::Ok, false))
        }
        KeeperRequest::Resize { size } => {
            runtime.resize(size)?;
            Ok((KeeperResponse::Ok, false))
        }
        KeeperRequest::Snapshot { cwd } => {
            Ok((KeeperResponse::Snapshot(runtime.snapshot(cwd)?), false))
        }
        KeeperRequest::VisibleSnapshot { cwd } => Ok((
            KeeperResponse::VisibleSnapshot(runtime.visible_snapshot(cwd)?),
            false,
        )),
        KeeperRequest::ScrollbackSlice {
            start_line,
            line_count,
        } => Ok((
            KeeperResponse::ScrollbackSlice(runtime.scrollback_slice(start_line, line_count)?),
            false,
        )),
        KeeperRequest::StartPipe { command, cwd, env } => Ok((
            KeeperResponse::PipeStatus(runtime.start_pipe(command, cwd, env)?),
            false,
        )),
        KeeperRequest::StopPipe => Ok((KeeperResponse::PipeStatus(runtime.stop_pipe()?), false)),
        KeeperRequest::StopPipeAfterExit => Ok((
            KeeperResponse::PipeStatus(runtime.stop_pipe_after_exit()?),
            false,
        )),
        KeeperRequest::Kill => {
            runtime.kill()?;
            Ok((KeeperResponse::Ok, false))
        }
    }
}

impl KeeperRuntime {
    fn ensure_running(&self) -> Result<()> {
        if self
            .exit_code
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper exit lock poisoned"))?
            .is_some()
        {
            return Err(MuxError::conflict("buffer runtime has already exited"));
        }
        Ok(())
    }

    fn status(&self) -> Result<KeeperStatus> {
        let exit_code = *self
            .exit_code
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper exit lock poisoned"))?;
        let surface = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?;
        let activity = *self
            .activity
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper activity lock poisoned"))?;
        let sequence = self.sequence.load(Ordering::Relaxed);
        let title = surface.backend.metadata().title.clone();
        let reported_cwd = surface.router.reported_cwd();
        drop(surface);
        let running = exit_code.is_none();
        // Prefer an OSC 7 report from the shell (handles ssh'd shells reporting
        // remote paths like tmux); fall back to resolving the direct child's cwd
        // from its pid. Only the keeper is guaranteed same-host as the child. The
        // pid fallback may hit the filesystem, so run it without the surface lock.
        // Only resolve while the child is still running: after exit the OS may
        // have recycled the pid to an unrelated process, so a stale OSC 7 report
        // or nothing is safer than resolving another process's directory.
        let cwd = reported_cwd.or_else(|| {
            if running {
                self.pid.and_then(resolve_pid_cwd)
            } else {
                None
            }
        });
        let pipe = self
            .pipe
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper pipe lock poisoned"))?
            .as_mut()
            .map(KeeperPipe::status);
        Ok(KeeperStatus {
            pid: self.pid,
            sequence,
            activity,
            title,
            running,
            exit_code: exit_code.flatten(),
            pipe,
            cwd,
        })
    }

    fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.ensure_running()?;
        // Hand off to the writer thread. This is a bounded send: it blocks only
        // when the queue is full (backpressure from a child that is not reading
        // its input) and never holds the writer, so it cannot stall other writers
        // or the read loop. A send error means the writer thread is gone.
        self.write_tx
            .send(bytes)
            .map_err(|_| MuxError::internal("runtime keeper writer channel closed"))
    }

    fn resize(&self, size: PtySize) -> Result<()> {
        self.ensure_running()?;
        let master = self
            .master
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper master lock poisoned"))?;
        master
            .resize(to_portable_size(size))
            .map_err(|error| MuxError::pty(error.to_string()))?;
        self.surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?
            .resize(size);
        Ok(())
    }

    fn snapshot(&self, cwd: Option<PathBuf>) -> Result<KeeperSnapshot> {
        let surface = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?;
        Ok(KeeperSnapshot {
            sequence: self.sequence.load(Ordering::Relaxed),
            size: surface.size,
            lines: surface.capture_lines(),
            title: surface.backend.metadata().title,
            cwd,
        })
    }

    fn visible_snapshot(&self, cwd: Option<PathBuf>) -> Result<TerminalSnapshot> {
        let surface = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?;
        Ok(surface.capture_visible_snapshot(self.sequence.load(Ordering::Relaxed), cwd))
    }

    fn scrollback_slice(&self, start_line: u64, line_count: u32) -> Result<KeeperScrollbackSlice> {
        let surface = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?;
        Ok(surface.capture_scrollback_slice(start_line, line_count))
    }

    fn kill(&self) -> Result<()> {
        self.ensure_running()?;
        let mut killer = self
            .killer
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper killer lock poisoned"))?;
        killer
            .kill()
            .map_err(|error| MuxError::pty(error.to_string()))
    }

    fn start_pipe(
        &self,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    ) -> Result<BufferRuntimePipeStatus> {
        validate_pipe_command(&command)?;
        self.ensure_running()?;
        let mut pipe = self
            .pipe
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper pipe lock poisoned"))?;
        if let Some(existing) = pipe.as_mut()
            && existing.status().running
        {
            return Err(MuxError::conflict("buffer pipe is already running"));
        }
        let mut spawned = KeeperPipe::spawn(command, cwd, env)?;
        let status = spawned.status();
        *pipe = Some(spawned);
        Ok(status)
    }

    fn stop_pipe(&self) -> Result<BufferRuntimePipeStatus> {
        self.stop_pipe_with_reason(BufferRuntimePipeStopReason::Requested, true)
    }

    fn stop_pipe_after_exit(&self) -> Result<BufferRuntimePipeStatus> {
        self.stop_pipe_with_reason(BufferRuntimePipeStopReason::BufferExited, false)
    }

    fn stop_pipe_with_reason(
        &self,
        reason: BufferRuntimePipeStopReason,
        require_runtime_running: bool,
    ) -> Result<BufferRuntimePipeStatus> {
        if require_runtime_running {
            self.ensure_running()?;
        }
        let mut pipe = self
            .pipe
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper pipe lock poisoned"))?;
        let Some(pipe) = pipe.as_mut() else {
            return Err(MuxError::conflict("buffer pipe is not running"));
        };
        if !pipe.status().running {
            return Err(MuxError::conflict("buffer pipe is not running"));
        }
        pipe.stop(reason)
    }
}

fn keeper_pipe_write_loop(
    mut stdin: ChildStdin,
    writer_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    writer_state: Arc<Mutex<KeeperPipeWriterState>>,
) {
    for bytes in writer_rx {
        if stdin.write_all(&bytes).and_then(|()| stdin.flush()).is_ok() {
            continue;
        }
        if let Ok(mut state) = writer_state.lock() {
            state.failed = true;
        }
        break;
    }
}

fn keeper_read_loop(runtime: Arc<KeeperRuntime>, mut reader: Box<dyn Read + Send>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let mut surface = match runtime.surface.lock() {
                    Ok(surface) => surface,
                    Err(_) => break,
                };
                let bytes = &buffer[..read];
                let (activity, replies) = surface.route_output(bytes);
                if let Ok(mut pipe) = runtime.pipe.lock()
                    && let Some(pipe) = pipe.as_mut()
                {
                    pipe.write(bytes);
                }
                runtime.sequence.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut state) = runtime.activity.lock() {
                    *state = activity;
                }
                // Release the surface lock before handing off the replies.
                drop(surface);
                if !replies.is_empty() {
                    // Never write to the master from this thread: a blocked write
                    // would stop draining child output and could deadlock. Hand
                    // the reply to the writer thread without blocking; if the queue
                    // is full (writer stuck on a full input buffer) drop the reply
                    // — continuing to drain lets the child progress and recover.
                    match runtime.write_tx.try_send(replies) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            debug!("terminal query reply queue full; dropping reply");
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Own the PTY-master writer and serialize every write submitted through the
/// runtime's write queue (client input and emulator replies).
///
/// Running on its own thread means a master write that blocks (child not draining
/// its input) applies backpressure to the queue rather than stalling any request
/// handler or the read loop. A write can still fail once the child has exited
/// while the queue drains; log at debug and keep going. Ends when every sender —
/// all held in the shared runtime — is dropped.
fn keeper_writer_loop(mut writer: Box<dyn Write + Send>, write_rx: Receiver<Vec<u8>>) {
    for bytes in write_rx {
        if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            debug!(%error, "failed to write to pty master");
        }
    }
}

fn keeper_wait_loop(runtime: Arc<KeeperRuntime>, mut child: Box<dyn Child + Send + Sync>) {
    let exit_code = child.wait().ok().and_then(exit_status_code);
    if let Ok(mut state) = runtime.exit_code.lock() {
        *state = Some(exit_code);
    }
    let _ = runtime.stop_pipe_after_exit();
}

fn validate_pipe_command(command: &[String]) -> Result<()> {
    if command.is_empty() {
        return Err(MuxError::invalid_input(
            "buffer pipe command must not be empty",
        ));
    }
    if command[0].is_empty() {
        return Err(MuxError::invalid_input(
            "buffer pipe command first segment must not be empty",
        ));
    }
    Ok(())
}

fn notify_pipe_removed(
    callbacks: &BufferRuntimeCallbacks,
    buffer_id: BufferId,
    last_sequence: &mut u64,
    activity: ActivityState,
    last_pipe: &mut Option<BufferRuntimePipeStatus>,
    saw_exit: bool,
) {
    if last_pipe.is_none() && saw_exit {
        return;
    }
    *last_sequence = last_sequence.saturating_add(1);
    (callbacks.on_output)(
        buffer_id,
        BufferRuntimeUpdate {
            sequence: *last_sequence,
            activity,
            title: None,
            pipe: Some(None),
            cwd: None,
        },
    );
    *last_pipe = None;
}

/// Compute the update to emit when a freshly polled status differs from the
/// last one reported to the callbacks, or `None` when nothing changed. Only the
/// fields that actually changed are populated, so a cwd-only change — which does
/// not advance `sequence` — is still emitted and distinguishable from no change.
fn status_update_delta(
    status: &BufferRuntimeStatus,
    last_sequence: u64,
    last_title: &Option<String>,
    last_activity: ActivityState,
    last_pipe: &Option<BufferRuntimePipeStatus>,
    last_cwd: &Option<PathBuf>,
) -> Option<BufferRuntimeUpdate> {
    let changed = status.sequence != last_sequence
        || status.title != *last_title
        || status.activity != last_activity
        || status.pipe != *last_pipe
        || status.cwd != *last_cwd;
    if !changed {
        return None;
    }
    Some(BufferRuntimeUpdate {
        sequence: status.sequence,
        activity: status.activity,
        title: (status.title != *last_title).then(|| status.title.clone()),
        pipe: (status.pipe != *last_pipe).then(|| status.pipe.clone()),
        cwd: (status.cwd != *last_cwd).then(|| status.cwd.clone()),
    })
}

fn spawn_status_poller(
    inner: Arc<BufferRuntimeInner>,
    callbacks: BufferRuntimeCallbacks,
    initial: BufferRuntimeStatus,
) -> Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("buffer-{}-poller", inner.buffer_id))
        .spawn(move || {
            let mut last_sequence = initial.sequence;
            let mut last_title = initial.title.clone();
            let mut last_activity = initial.activity;
            let mut last_pipe = initial.pipe.clone();
            // Seed as unknown (not `initial.cwd`) so the first poll always reports
            // the current directory: an OSC 7 report or pid resolution may already
            // be present at attach, and the buffer record must still learn it.
            let mut last_cwd: Option<PathBuf> = None;
            let mut saw_exit = !initial.running;

            while !inner.stop.load(Ordering::Relaxed) {
                let status = {
                    let mut connection = match inner.connection.lock() {
                        Ok(connection) => connection,
                        Err(_) => break,
                    };
                    match connection.status() {
                        Ok(status) => status,
                        Err(error) => {
                            error!(%error, %inner.buffer_id, "status poll failed");
                            notify_pipe_removed(
                                &callbacks,
                                inner.buffer_id,
                                &mut last_sequence,
                                last_activity,
                                &mut last_pipe,
                                saw_exit,
                            );
                            (callbacks.on_exit)(inner.buffer_id, None);
                            break;
                        }
                    }
                };

                if let Some(update) = status_update_delta(
                    &status,
                    last_sequence,
                    &last_title,
                    last_activity,
                    &last_pipe,
                    &last_cwd,
                ) {
                    (callbacks.on_output)(inner.buffer_id, update);
                    last_sequence = status.sequence;
                    last_title = status.title.clone();
                    last_activity = status.activity;
                    last_pipe = status.pipe.clone();
                    last_cwd = status.cwd.clone();
                }

                if !saw_exit && !status.running {
                    saw_exit = true;
                    (callbacks.on_exit)(inner.buffer_id, status.exit_code);
                }

                thread::sleep(STATUS_POLL_INTERVAL);
            }
        })
        .map_err(|error| MuxError::internal(error.to_string()))
}

/// Resolve the current working directory of a running process by pid.
///
/// This is the primary, zero-shell-config cwd source (OSC 7 overrides it when the
/// shell emits it). Only meaningful when called on the same host as the process,
/// which the keeper always is.
#[cfg(target_os = "linux")]
fn resolve_pid_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "macos")]
fn resolve_pid_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    // Safety: `proc_pidinfo` writes up to `size` bytes into `info` and reports how
    // many it wrote; we only read the buffer when it filled the whole struct.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size as libc::c_int,
        )
    };
    if written <= 0 || (written as usize) < size {
        return None;
    }
    let path = &info.pvi_cdir.vip_path;
    // Safety: `vip_path` is a fixed-size NUL-terminated C string buffer.
    let bytes = unsafe {
        std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), std::mem::size_of_val(path))
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resolve_pid_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

fn connect_to_keeper(socket_path: &Path) -> Result<UnixStream> {
    for _ in 0..CONNECT_RETRY_ATTEMPTS {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                thread::sleep(CONNECT_RETRY_DELAY);
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(MuxError::timeout(format!(
        "timed out connecting to runtime keeper {}",
        socket_path.display()
    )))
}

fn spawn_runtime_keeper(cli: RuntimeKeeperCli) -> Result<()> {
    if let Some(keeper_exe) = resolve_runtime_keeper_executable() {
        let mut keeper = ProcessCommand::new(keeper_exe);
        keeper
            .arg("__runtime-keeper")
            .arg("--keeper-socket")
            .arg(&cli.socket_path)
            .arg("--cols")
            .arg(cli.size.cols.to_string())
            .arg("--rows")
            .arg(cli.size.rows.to_string());
        if let Some(cwd) = &cli.cwd {
            keeper.arg("--cwd").arg(cwd);
        }
        for (key, value) in &cli.env {
            keeper.arg("--env").arg(format!(
                "{}=base64:{}",
                key,
                encode_runtime_keeper_env_value(value.as_os_str())
            ));
        }
        keeper.arg("--");
        keeper.args(&cli.command);
        keeper.stdin(Stdio::null());
        keeper.stdout(Stdio::null());
        keeper.stderr(Stdio::null());
        keeper.spawn()?;
        return Ok(());
    }

    thread::Builder::new()
        .name(format!("runtime-keeper-{}", cli.socket_path.display()))
        .spawn(move || {
            if let Err(error) = run_runtime_keeper(cli) {
                error!(%error, "runtime keeper thread failed");
            }
        })
        .map_err(|error| MuxError::internal(error.to_string()))?;
    Ok(())
}

fn resolve_runtime_keeper_executable() -> Option<PathBuf> {
    if let Some(path) = env::var_os("EMBERS_RUNTIME_KEEPER_BIN").map(PathBuf::from)
        && is_executable_file(&path)
    {
        return Some(path);
    }
    if let Some(path) = env::var_os("CARGO_BIN_EXE_embers").map(PathBuf::from)
        && is_executable_file(&path)
    {
        return Some(path);
    }
    let current_exe = env::current_exe().ok();
    if let Some(current_exe) = current_exe.as_ref() {
        if current_exe
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "embers" || name == "embers-cli")
            && is_executable_file(current_exe)
        {
            return Some(current_exe.clone());
        }

        if let Some(parent) = current_exe.parent() {
            if parent.file_name().is_some_and(|name| name == "deps") {
                let candidate = parent.parent()?.join(binary_name("embers"));
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }

            for stem in ["embers", "embers-cli", "embers-runtime-keeper"] {
                let candidate = parent.join(binary_name(stem));
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    for stem in ["embers", "embers-cli", "embers-runtime-keeper"] {
        if let Some(path) = resolve_binary_on_path(stem) {
            return Some(path);
        }
    }

    None
}

fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return false;
    }
    true
}

fn resolve_binary_on_path(stem: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let binary_name = binary_name(stem);
    for entry in env::split_paths(&path) {
        let candidate = entry.join(&binary_name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn encode_runtime_keeper_env_value(value: &OsStr) -> String {
    #[cfg(unix)]
    {
        base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
    }
    #[cfg(windows)]
    {
        let encoded = value
            .encode_wide()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>();
        base64::engine::general_purpose::STANDARD.encode(encoded)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        base64::engine::general_purpose::STANDARD.encode(value.to_string_lossy().as_bytes())
    }
}

fn write_message<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let payload =
        serde_json::to_vec(value).map_err(|error| MuxError::internal(error.to_string()))?;
    let len = u32::try_from(payload.len())
        .map_err(|_| MuxError::internal("runtime keeper payload exceeded u32 length"))?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

fn read_message<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> Result<Option<T>> {
    let mut len_bytes = [0_u8; 4];
    match stream.read_exact(&mut len_bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let len = usize::try_from(u32::from_le_bytes(len_bytes))
        .map_err(|_| MuxError::protocol("runtime keeper frame length exceeds platform limits"))?;
    if len == 0 || len > MAX_FRAME_SIZE {
        return Err(MuxError::protocol(format!(
            "runtime keeper frame length {len} is out of range"
        )));
    }
    let mut payload = vec![0_u8; len];
    stream.read_exact(&mut payload)?;
    let value =
        serde_json::from_slice(&payload).map_err(|error| MuxError::internal(error.to_string()))?;
    Ok(Some(value))
}

fn keeper_response_kind(response: &KeeperResponse) -> &'static str {
    match response {
        KeeperResponse::Status(_) => "status",
        KeeperResponse::Snapshot(_) => "snapshot",
        KeeperResponse::VisibleSnapshot(_) => "visible_snapshot",
        KeeperResponse::ScrollbackSlice(_) => "scrollback_slice",
        KeeperResponse::PipeStatus(_) => "pipe_status",
        KeeperResponse::Ok => "ok",
        KeeperResponse::Error { .. } => "error",
    }
}

fn exit_status_code(status: portable_pty::ExitStatus) -> Option<i32> {
    if status.signal().is_some() {
        None
    } else {
        i32::try_from(status.exit_code()).ok()
    }
}

fn to_portable_size(size: PtySize) -> PortablePtySize {
    PortablePtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: size.pixel_width,
        pixel_height: size.pixel_height,
    }
}

struct SocketCleanup {
    socket_path: PathBuf,
}

impl SocketCleanup {
    fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use embers_core::{ActivityState, BufferId, MuxError};

    use super::{
        BufferRuntimeCallbacks, BufferRuntimeHandle, BufferRuntimeInner, BufferRuntimePipeStatus,
        BufferRuntimePipeStopReason, BufferRuntimeStatus, KEEPER_PIPE_WRITE_QUEUE_CAPACITY,
        KeeperConnection, KeeperPipe, MAX_FRAME_SIZE, RuntimeThreads, read_message,
        spawn_status_poller, status_update_delta,
    };

    fn status_with_cwd(sequence: u64, cwd: Option<&str>) -> BufferRuntimeStatus {
        BufferRuntimeStatus {
            pid: None,
            sequence,
            activity: ActivityState::Idle,
            title: Some("shell".to_owned()),
            running: true,
            exit_code: None,
            pipe: None,
            cwd: cwd.map(std::path::PathBuf::from),
        }
    }

    #[test]
    fn status_update_delta_emits_cwd_change_without_sequence_advance() {
        // known -> known: cwd changes while the snapshot sequence is unchanged.
        let status = status_with_cwd(7, Some("/new"));
        let update = status_update_delta(
            &status,
            7,
            &Some("shell".to_owned()),
            ActivityState::Idle,
            &None,
            &Some(std::path::PathBuf::from("/old")),
        )
        .expect("cwd change should emit an update even at the same sequence");
        assert_eq!(update.sequence, 7);
        assert_eq!(update.cwd, Some(Some(std::path::PathBuf::from("/new"))));
        assert!(update.title.is_none());
        assert!(update.pipe.is_none());
    }

    #[test]
    fn status_update_delta_emits_cwd_cleared_to_unknown() {
        // known -> unknown: cwd becomes None while the sequence is unchanged;
        // the Some/None distinction must survive as `Some(None)`.
        let status = status_with_cwd(7, None);
        let update = status_update_delta(
            &status,
            7,
            &Some("shell".to_owned()),
            ActivityState::Idle,
            &None,
            &Some(std::path::PathBuf::from("/old")),
        )
        .expect("cwd clear should emit an update even at the same sequence");
        assert_eq!(update.cwd, Some(None));
    }

    #[test]
    fn status_update_delta_returns_none_when_unchanged() {
        let status = status_with_cwd(7, Some("/same"));
        let update = status_update_delta(
            &status,
            7,
            &Some("shell".to_owned()),
            ActivityState::Idle,
            &None,
            &Some(std::path::PathBuf::from("/same")),
        );
        assert!(update.is_none());
    }

    #[test]
    fn join_threads_waits_for_poller_shutdown() {
        let (stream, _peer) = UnixStream::pair().expect("create socket pair");
        let inner = Arc::new(BufferRuntimeInner {
            buffer_id: BufferId(1),
            pid: None,
            socket_path: "/tmp/test-buffer.sock".into(),
            connection: std::sync::Mutex::new(KeeperConnection { stream }),
            stop: std::sync::atomic::AtomicBool::new(false),
            threads: std::sync::Mutex::new(RuntimeThreads::default()),
        });
        let (tx, rx) = mpsc::channel();
        let poller_inner = inner.clone();
        let poller = thread::spawn(move || {
            while !poller_inner.stop.load(std::sync::atomic::Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(40));
            tx.send(()).expect("send shutdown notification");
        });
        inner.threads.lock().expect("lock thread registry").poller = Some(poller);

        let started = Instant::now();
        inner.join_threads_blocking();

        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "join should wait for the poller to finish"
        );
        rx.try_recv()
            .expect("poller should finish before join returns");
    }

    #[test]
    fn read_message_rejects_empty_frame() {
        let (mut stream, mut peer) = UnixStream::pair().expect("create socket pair");
        peer.write_all(&0_u32.to_le_bytes())
            .expect("write frame length");
        drop(peer);

        let error = match read_message::<super::KeeperRequest>(&mut stream) {
            Err(error) => error,
            Ok(_) => panic!("expected frame error"),
        };

        assert!(matches!(error, MuxError::Protocol(_)));
        assert!(error.to_string().contains("out of range"));
    }

    #[test]
    fn read_message_rejects_oversized_frame() {
        let (mut stream, mut peer) = UnixStream::pair().expect("create socket pair");
        peer.write_all(
            &(u32::try_from(MAX_FRAME_SIZE).expect("frame size fits in u32") + 1).to_le_bytes(),
        )
        .expect("write frame length");
        drop(peer);

        let error = match read_message::<super::KeeperRequest>(&mut stream) {
            Err(error) => error,
            Ok(_) => panic!("expected frame error"),
        };

        assert!(matches!(error, MuxError::Protocol(_)));
        assert!(error.to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn start_pipe_rejects_empty_program_before_runtime_request() {
        let (stream, peer) = UnixStream::pair().expect("create socket pair");
        drop(peer);
        let handle = BufferRuntimeHandle {
            inner: Arc::new(BufferRuntimeInner {
                buffer_id: BufferId(1),
                pid: None,
                socket_path: "/tmp/test-buffer.sock".into(),
                connection: std::sync::Mutex::new(KeeperConnection { stream }),
                stop: std::sync::atomic::AtomicBool::new(false),
                threads: std::sync::Mutex::new(RuntimeThreads::default()),
            }),
        };

        let error = handle
            .start_pipe(vec![String::new()], None, BTreeMap::new())
            .await
            .expect_err("empty pipe program should reject before keeper request");

        assert!(matches!(error, MuxError::InvalidInput(_)));
        assert_eq!(
            error.to_string(),
            "invalid input: buffer pipe command first segment must not be empty"
        );
    }

    #[test]
    fn status_poller_exits_on_status_error() {
        let (stream, peer) = UnixStream::pair().expect("create socket pair");
        drop(peer);
        let inner = Arc::new(BufferRuntimeInner {
            buffer_id: BufferId(1),
            pid: None,
            socket_path: "/tmp/test-buffer.sock".into(),
            connection: std::sync::Mutex::new(KeeperConnection { stream }),
            stop: std::sync::atomic::AtomicBool::new(false),
            threads: std::sync::Mutex::new(RuntimeThreads::default()),
        });
        let (exit_tx, exit_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::channel();
        let poller = spawn_status_poller(
            inner,
            BufferRuntimeCallbacks {
                on_output: Arc::new(move |buffer_id, update| {
                    output_tx
                        .send((buffer_id, update))
                        .expect("send output notification");
                }),
                on_exit: Arc::new(move |buffer_id, exit_code| {
                    exit_tx
                        .send((buffer_id, exit_code))
                        .expect("send exit notification");
                }),
            },
            BufferRuntimeStatus {
                pid: None,
                sequence: 0,
                activity: ActivityState::Idle,
                title: None,
                running: true,
                exit_code: None,
                pipe: None,
                cwd: None,
            },
        )
        .expect("spawn poller");

        poller.join().expect("poller exits cleanly");

        assert_eq!(
            exit_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("poller should report exit"),
            (BufferId(1), None)
        );
        let (buffer_id, update) = output_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("poller should report final pipe removal");
        assert_eq!(buffer_id, BufferId(1));
        assert_eq!(update.pipe, Some(None));
    }

    #[test]
    fn status_poller_reports_pipe_removal_before_status_error_exit() {
        let (stream, peer) = UnixStream::pair().expect("create socket pair");
        drop(peer);
        let inner = Arc::new(BufferRuntimeInner {
            buffer_id: BufferId(2),
            pid: None,
            socket_path: "/tmp/test-buffer.sock".into(),
            connection: std::sync::Mutex::new(KeeperConnection { stream }),
            stop: std::sync::atomic::AtomicBool::new(false),
            threads: std::sync::Mutex::new(RuntimeThreads::default()),
        });
        let (exit_tx, exit_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::channel();
        let poller = spawn_status_poller(
            inner,
            BufferRuntimeCallbacks {
                on_output: Arc::new(move |buffer_id, update| {
                    output_tx
                        .send((buffer_id, update))
                        .expect("send output notification");
                }),
                on_exit: Arc::new(move |buffer_id, exit_code| {
                    exit_tx
                        .send((buffer_id, exit_code))
                        .expect("send exit notification");
                }),
            },
            BufferRuntimeStatus {
                pid: None,
                sequence: 7,
                activity: ActivityState::Idle,
                title: None,
                running: true,
                exit_code: None,
                pipe: Some(BufferRuntimePipeStatus {
                    command: vec!["tee".to_owned()],
                    running: true,
                    pid: Some(123),
                    exit_code: None,
                    stop_reason: None,
                }),
                cwd: None,
            },
        )
        .expect("spawn poller");

        poller.join().expect("poller exits cleanly");

        let (buffer_id, update) = output_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("poller should report pipe removal");
        assert_eq!(buffer_id, BufferId(2));
        assert_eq!(update.sequence, 8);
        assert_eq!(update.pipe, Some(None));
        assert_eq!(
            exit_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("poller should report exit"),
            (BufferId(2), None)
        );
    }

    #[test]
    fn keeper_pipe_write_uses_bounded_queue() {
        let mut pipe = KeeperPipe::spawn(
            vec![
                "/bin/sh".to_owned(),
                "-lc".to_owned(),
                "sleep 30".to_owned(),
            ],
            None,
            std::collections::BTreeMap::new(),
        )
        .expect("spawn pipe");
        let payload = vec![b'x'; 128 * 1024];

        let started = Instant::now();
        for _ in 0..(KEEPER_PIPE_WRITE_QUEUE_CAPACITY + 2) {
            pipe.write(&payload);
        }

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "pipe writes should not block when the consumer is slow"
        );

        let status = pipe.status();
        assert!(!status.running);
        assert_eq!(
            status.stop_reason,
            Some(BufferRuntimePipeStopReason::WriteFailed)
        );
    }
}
