use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use embers_core::{
    ActivityState, BufferId, ClipboardWrite, CursorPosition, Hyperlink, MuxError, PassthroughWrite,
    PtySize, Result, SnapshotLine, StyledRun, TerminalSnapshot,
};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize as PortablePtySize,
    PtySystem,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::idle_compaction::{COMPACTION_IDLE, IdleCompaction};
use crate::reply_hold::ReplyHold;
use crate::{BackendEvents, RawByteRouter, TerminalBackend};

/// How long spawning a buffer waits for its runtime keeper to report that it
/// is ready (socket listening, child running) or that it failed to start.
const KEEPER_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Line a keeper process writes to its stdout once it is ready to serve.
const KEEPER_READY_LINE: &str = "ready";
/// Prefix of the line a keeper process writes to its stdout when it fails to
/// start; the rest of the line is the error.
const KEEPER_ERROR_PREFIX: &str = "error: ";
/// Upper bound on the readiness report, so a misbehaving keeper cannot make the
/// server buffer unbounded output.
const KEEPER_READY_LINE_MAX: usize = 4096;
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
/// How often a keeper with no connected server checks whether it is still
/// needed: whether its child is still running and its socket is still the one it
/// bound (servers only ever reattach through that socket).
const KEEPER_SOCKET_CHECK_INTERVAL: Duration = Duration::from_millis(500);
/// How long a keeper waits for its child to exit after SIGHUP, when told to
/// kill it or when it finds itself unreachable, before escalating to SIGKILL.
const KEEPER_KILL_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct BufferRuntimeUpdate {
    pub sequence: u64,
    pub activity: ActivityState,
    pub title: Option<Option<String>>,
    pub pipe: Option<Option<BufferRuntimePipeStatus>>,
    /// `Some(value)` when the working directory changed in this update (`value`
    /// is `None` when it became unknown); `None` when cwd is unchanged.
    pub cwd: Option<Option<PathBuf>>,
    /// `Some(value)` when the foreground command changed in this update
    /// (`value` is `None` once the pane's own process is in the foreground
    /// again); `None` when it is unchanged.
    pub foreground_command: Option<Option<Vec<String>>>,
    /// The runtime's [`BufferRuntimeStatus::bell_sequence`]. The server marks
    /// the buffer `Bell` whenever this advances, so a bell is not lost when
    /// later output replaced `activity` before it was reported.
    pub bell_sequence: u64,
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
    /// Count of OSC 52 clipboard writes the keeper has seen. The poller fetches
    /// the latest write whenever this advances.
    pub clipboard_sequence: u64,
    /// Count of output chunks that rang the bell since the runtime started.
    /// `activity` only describes the latest chunk, so this is what tells a
    /// reader that a bell rang since it last looked.
    pub bell_sequence: u64,
    /// Number of the latest passthrough write the keeper has queued. The poller
    /// fetches the queued writes whenever this advances.
    pub passthrough_sequence: u64,
    /// Command line of the job in the PTY's foreground when that is not the
    /// pane's own process, for example a program a shell is running.
    pub foreground_command: Option<Vec<String>>,
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
    /// A program in the buffer wrote to the clipboard with OSC 52.
    pub on_clipboard: Arc<dyn Fn(BufferId, ClipboardWrite) + Send + Sync>,
    /// A program in the buffer sent sequences for the host terminal (tmux
    /// passthrough), in the order it sent them. Called once per poll with
    /// every write the poll fetched.
    pub on_passthrough: Arc<dyn Fn(BufferId, Vec<PassthroughWrite>) + Send + Sync>,
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
    /// Stop the child (SIGHUP, then SIGKILL) and exit, for kill-server. An
    /// older keeper rejects it; the server then falls back to `Kill` followed
    /// by `ExitRecorded`.
    Shutdown,
    /// Fetch the latest OSC 52 clipboard write. Only sent once a status reply
    /// reports a `clipboard_sequence`, which keepers without clipboard
    /// forwarding never do.
    Clipboard,
    /// Take the queued passthrough writes numbered above `after`, oldest first;
    /// writes up to `after` are dropped. Only sent once a status reply reports
    /// a `passthrough_sequence`.
    Passthrough {
        after: u64,
    },
    /// Whether some client forwards kitty graphics queries to a host terminal
    /// that answers them, so the keeper should hold the replies that follow a
    /// graphics query until the answer arrives (see `crate::reply_hold`). Off
    /// until a server says otherwise, and again once the server disconnects.
    /// An older keeper rejects it and never holds replies.
    SetHostGraphics {
        enabled: bool,
    },
    /// The server has saved a workspace that no longer needs this keeper: the
    /// buffer is recorded as exited, or its record no longer refers to the
    /// keeper. Once its child has exited and no server is attached, the keeper
    /// may exit.
    ExitRecorded,
}

#[derive(Serialize, Deserialize)]
enum KeeperResponse {
    Status(KeeperStatus),
    Clipboard(Option<KeeperClipboard>),
    Passthrough(Vec<KeeperPassthrough>),
    Snapshot(KeeperSnapshot),
    VisibleSnapshot(TerminalSnapshot),
    ScrollbackSlice(KeeperScrollbackSlice),
    PipeStatus(BufferRuntimePipeStatus),
    Ok,
    /// `conflict` marks a conflict with the runtime's state (e.g. its child
    /// already exited), so the server reports it as one rather than as a
    /// transport failure. Absent from older keepers.
    Error {
        message: String,
        #[serde(default)]
        conflict: bool,
    },
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
    // Defaulted for the same reason: an older keeper reports 0 and is never
    // asked for clipboard writes.
    #[serde(default)]
    clipboard_sequence: u64,
    // Defaulted for keepers that predate passthrough forwarding.
    #[serde(default)]
    passthrough_sequence: u64,
    // Defaulted for keepers that predate foreground command reporting.
    #[serde(default)]
    foreground_command: Option<Vec<String>>,
    // Defaulted for keepers that predate bell counting; they report 0, so
    // only their latest activity is seen.
    #[serde(default)]
    bell_sequence: u64,
}

/// The latest OSC 52 clipboard write a keeper has seen. The payload is held
/// (not queued) and fetched separately from status, so status polls stay small
/// and a burst of writes costs one transfer of the last one.
#[derive(Clone, Serialize, Deserialize)]
struct KeeperClipboard {
    sequence: u64,
    write: ClipboardWrite,
}

/// A queued passthrough write. Unlike clipboard writes every one is kept, since
/// image protocols send one image as many sequences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KeeperPassthrough {
    sequence: u64,
    /// Base64 in keeper JSON: a byte array would take up to four times the room.
    #[serde(with = "base64_bytes")]
    bytes: Vec<u8>,
    cursor: CursorPosition,
}

/// Most passthrough bytes a keeper queues for the server. Past this the oldest
/// writes are dropped, which bounds memory while no server is attached.
const MAX_QUEUED_PASSTHROUGH_BYTES: usize = 8 * 1024 * 1024;

/// Most passthrough bytes one `Passthrough` reply carries (at least one write is
/// always sent). Base64 grows it by a third, which stays inside the keeper
/// frame cap (16 MiB); the poller asks again for the rest.
const MAX_PASSTHROUGH_REPLY_BYTES: usize = 8 * 1024 * 1024;

/// Passthrough writes waiting for the server, oldest first, numbered from 1.
#[derive(Debug, Default)]
struct PassthroughQueue {
    last_sequence: u64,
    entries: VecDeque<KeeperPassthrough>,
    bytes: usize,
}

impl PassthroughQueue {
    fn push(&mut self, write: PassthroughWrite) {
        self.last_sequence += 1;
        self.bytes += write.bytes.len();
        self.entries.push_back(KeeperPassthrough {
            sequence: self.last_sequence,
            bytes: write.bytes,
            cursor: write.cursor,
        });
        while self.bytes > MAX_QUEUED_PASSTHROUGH_BYTES
            && let Some(dropped) = self.entries.pop_front()
        {
            debug!(
                sequence = dropped.sequence,
                "passthrough queue full; dropping oldest write"
            );
            self.bytes -= dropped.bytes.len();
        }
    }

    /// Remove and return the writes numbered above `after`, oldest first, up to
    /// [`MAX_PASSTHROUGH_REPLY_BYTES`]. Writes up to `after` are dropped.
    fn take_after(&mut self, after: u64) -> Vec<KeeperPassthrough> {
        let mut taken = Vec::new();
        let mut taken_bytes = 0;
        while let Some(entry) = self.entries.front() {
            if entry.sequence > after
                && !taken.is_empty()
                && taken_bytes + entry.bytes.len() > MAX_PASSTHROUGH_REPLY_BYTES
            {
                break;
            }
            let entry = self.entries.pop_front().expect("front entry exists");
            self.bytes -= entry.bytes.len();
            if entry.sequence > after {
                taken_bytes += entry.bytes.len();
                taken.push(entry);
            }
        }
        taken
    }
}

/// Serde helper that writes bytes as one base64 string.
mod base64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
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
    /// Parallel per-line hyperlink tables referenced by `styles`, kept separate
    /// for the same compatibility reason. Empty when no line carries a link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Vec<Hyperlink>>,
}

impl KeeperScrollbackSlice {
    /// Zip `lines`, `styles` and `links` back into styled snapshot lines. A
    /// missing or short `styles`/`links` entry yields a plain or link-free line,
    /// so old keepers degrade to text.
    pub fn into_snapshot_lines(self) -> Vec<SnapshotLine> {
        let mut styles = self.styles.into_iter();
        let mut links = self.links.into_iter();
        self.lines
            .into_iter()
            .map(|text| SnapshotLine {
                text,
                runs: styles.next().unwrap_or_default(),
                links: links.next().unwrap_or_default(),
            })
            .collect()
    }
}

struct KeeperRuntime {
    surface: Mutex<KeeperSurface>,
    /// Signalled, with the surface lock, when the surface's reply hold starts
    /// waiting, so the hold thread wakes up for its deadline.
    reply_hold_changed: Condvar,
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
    /// Output chunks that rang the bell; see [`BufferRuntimeStatus::bell_sequence`].
    bell_sequence: AtomicU64,
    clipboard: Mutex<Option<KeeperClipboard>>,
    exit_code: Mutex<Option<Option<i32>>>,
    /// Set once a server reports, with `KeeperRequest::ExitRecorded`, that its
    /// saved workspace no longer needs this keeper. Until then a server that
    /// read the child's exit may have crashed before saving it, so the keeper
    /// must stay reachable for the next one to reattach and read it again.
    exit_recorded: AtomicBool,
    /// When to compact the backend: output, resizes and reads of the
    /// scrollback are activity, after which the backend compacts once quiet.
    compaction: IdleCompaction,
    pid: Option<u32>,
}

struct KeeperSurface {
    router: RawByteRouter,
    backend: Box<dyn TerminalBackend>,
    size: PtySize,
    /// Passthrough writes the router took out of the output, kept under the
    /// surface lock the read loop already holds.
    passthrough: PassthroughQueue,
    /// Replies held behind forwarded kitty graphics queries.
    reply_hold: ReplyHold,
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

    /// Tell the keeper to end its program and exit (kill-server). Unlike
    /// [`Self::kill`], the keeper does not stay to serve captures.
    pub async fn terminate(&self) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.shutdown()
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

    /// Tell the keeper whether some client forwards kitty graphics queries to
    /// a host terminal that answers them (see `KeeperRequest::SetHostGraphics`).
    pub async fn set_host_graphics(&self, enabled: bool) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.set_host_graphics(enabled)
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

    /// Tell the keeper the server has saved a workspace that no longer needs
    /// it: the buffer is recorded as exited, or its record no longer refers to
    /// this keeper. Only send this after that save has completed; the keeper
    /// may then exit once its child has and the server disconnects.
    pub async fn acknowledge_exit_recorded(&self) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = inner
                .connection
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime connection lock poisoned"))?;
            connection.exit_recorded()
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
            Some(KeeperResponse::Error { message, conflict }) => Err(if conflict {
                MuxError::conflict(message)
            } else {
                MuxError::transport(message)
            }),
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
                clipboard_sequence: status.clipboard_sequence,
                passthrough_sequence: status.passthrough_sequence,
                foreground_command: status.foreground_command,
                bell_sequence: status.bell_sequence,
            }),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper status response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn clipboard(&mut self) -> Result<Option<KeeperClipboard>> {
        match self.request(KeeperRequest::Clipboard)? {
            KeeperResponse::Clipboard(clipboard) => Ok(clipboard),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper clipboard response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn passthrough(&mut self, after: u64) -> Result<Vec<KeeperPassthrough>> {
        match self.request(KeeperRequest::Passthrough { after })? {
            KeeperResponse::Passthrough(writes) => Ok(writes),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper passthrough response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    /// Fetch every passthrough write numbered above `after`, up to `latest`,
    /// over as many requests as it takes.
    fn passthrough_since(&mut self, after: u64, latest: u64) -> Result<Vec<KeeperPassthrough>> {
        let mut writes = Vec::new();
        let mut after = after;
        while after < latest {
            let batch = self.passthrough(after)?;
            let Some(last) = batch.last() else {
                break;
            };
            after = last.sequence;
            writes.extend(batch);
        }
        Ok(writes)
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

    fn shutdown(&mut self) -> Result<()> {
        match self.request(KeeperRequest::Shutdown) {
            Ok(KeeperResponse::Ok) => Ok(()),
            Ok(other) => Err(MuxError::protocol(format!(
                "unexpected runtime keeper shutdown response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
            // A keeper that predates `Shutdown` rejects it. End its child, then
            // release it: a keeper that knows `ExitRecorded` only exits once a
            // server has acknowledged the exit, and after kill-server none
            // will. One older still rejects `ExitRecorded` too, harmlessly; it
            // exits once its child has and no server is left.
            Err(_) => {
                self.kill()?;
                let _ = self.exit_recorded();
                Ok(())
            }
        }
    }

    fn set_host_graphics(&mut self, enabled: bool) -> Result<()> {
        match self.request(KeeperRequest::SetHostGraphics { enabled })? {
            KeeperResponse::Ok => Ok(()),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper host-graphics response: {other_kind}",
                other_kind = keeper_response_kind(&other)
            ))),
        }
    }

    fn exit_recorded(&mut self) -> Result<()> {
        match self.request(KeeperRequest::ExitRecorded)? {
            KeeperResponse::Ok => Ok(()),
            other => Err(MuxError::protocol(format!(
                "unexpected runtime keeper exit-recorded response: {other_kind}",
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
            backend: crate::terminal_backend::new_terminal_backend(size, max_scrollback_lines),
            size,
            passthrough: PassthroughQueue::default(),
            reply_hold: ReplyHold::default(),
        }
    }

    /// Ingest PTY output and return the resulting activity, the terminal
    /// replies to write now (device-attribute / cursor-position / mode
    /// queries), and any clipboard write. The caller writes the replies back to
    /// the PTY master. Passthrough sequences are queued for the server.
    ///
    /// Replies to queries that follow a forwarded kitty graphics query are held
    /// back by the reply hold rather than returned (see `crate::reply_hold`).
    fn route_output(&mut self, bytes: &[u8], now: Instant) -> BackendEvents {
        let Self {
            router,
            backend,
            passthrough,
            reply_hold,
            ..
        } = self;
        let mut replies = Vec::new();
        router.route_output_with(backend.as_mut(), bytes, |backend, write| {
            // Replies to what came before the write are not held behind it.
            replies.extend(reply_hold.replies(backend.take_replies()));
            reply_hold.forwarded(&write.bytes, now);
            passthrough.push(write);
        });
        let mut events = backend.take_events();
        replies.extend(reply_hold.replies(std::mem::take(&mut events.replies)));
        events.replies = replies;
        events
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
        let mut links = Vec::with_capacity(slice.lines.len());
        for line in slice.lines {
            lines.push(line.text);
            styles.push(line.runs);
            links.push(line.links);
        }

        // Styles are best-effort: a large `line_count` could produce a styled
        // payload big enough to blow the keeper JSON (16 MiB) or client frame
        // (8 MiB) caps. If the estimated styling exceeds the budget, ship plain
        // text — the text always survives. Link tables count at their URI and
        // id length, since one long URI can outweigh many runs.
        let run_count: usize = styles.iter().map(Vec::len).sum();
        let link_bytes: usize = links
            .iter()
            .flatten()
            .map(|link| link.uri.len() + link.id.as_ref().map_or(0, String::len))
            .sum();
        if run_count
            .saturating_mul(ESTIMATED_STYLED_RUN_JSON_BYTES)
            .saturating_add(link_bytes)
            > MAX_STYLE_PAYLOAD_BYTES
            || styles.iter().all(Vec::is_empty)
        {
            styles.clear();
        }
        if styles.is_empty() || links.iter().all(Vec::is_empty) {
            links.clear();
        }

        KeeperScrollbackSlice {
            start_line: slice.start_line,
            total_lines: slice.total_lines,
            lines,
            styles,
            links,
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

/// Run a runtime keeper process. Reports on stdout, as a single line, once the
/// keeper is ready to serve or why it failed to start; the spawning server
/// waits for that line (see [`wait_for_keeper_ready`]).
pub fn run_runtime_keeper(cli: RuntimeKeeperCli) -> Result<()> {
    let mut readiness = KeeperReadiness::Stdout { reported: false };
    let result = serve_runtime_keeper(cli, &mut readiness);
    if let Err(error) = &result {
        readiness.report(Err(error.to_string()));
    }
    result
}

/// Where a keeper reports whether it started. A keeper process reports on its
/// stdout; an in-process keeper thread reports over a channel.
enum KeeperReadiness {
    Stdout { reported: bool },
    Channel(Option<SyncSender<std::result::Result<(), String>>>),
}

impl KeeperReadiness {
    /// Report the start result. Only the first report counts.
    fn report(&mut self, result: std::result::Result<(), String>) {
        match self {
            Self::Stdout { reported } => {
                if std::mem::replace(reported, true) {
                    return;
                }
                let line = match result {
                    Ok(()) => format!("{KEEPER_READY_LINE}\n"),
                    Err(error) => format!("{KEEPER_ERROR_PREFIX}{}\n", error.replace('\n', " ")),
                };
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(line.as_bytes());
                let _ = stdout.flush();
                drop(stdout);
                release_stdout();
            }
            Self::Channel(sender) => {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(result);
                }
            }
        }
    }
}

/// Point stdout at `/dev/null` once readiness is reported, so the keeper does
/// not hold the server's end of the readiness pipe open for its whole life.
fn release_stdout() {
    if let Ok(null) = fs::OpenOptions::new().write(true).open("/dev/null") {
        // SAFETY: plain fd duplication onto stdout; `null` stays open for the call.
        unsafe {
            libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO);
        }
    }
}

fn serve_runtime_keeper(cli: RuntimeKeeperCli, readiness: &mut KeeperReadiness) -> Result<()> {
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
    let bound_socket = socket_identity(&cli.socket_path);
    let _cleanup = SocketCleanup::new(cli.socket_path.clone(), bound_socket);

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
    let _writer = thread::Builder::new()
        .name(format!("keeper-writer-{}", cli.socket_path.display()))
        .spawn(move || keeper_writer_loop(writer, write_rx))
        .map_err(|error| MuxError::internal(error.to_string()))?;

    let runtime = Arc::new(KeeperRuntime {
        surface: Mutex::new(KeeperSurface::new(cli.size)),
        reply_hold_changed: Condvar::new(),
        master: Mutex::new(pair.master),
        write_tx,
        killer: Mutex::new(killer),
        pipe: Mutex::new(None),
        sequence: AtomicU64::new(0),
        activity: Mutex::new(ActivityState::Idle),
        bell_sequence: AtomicU64::new(0),
        clipboard: Mutex::new(None),
        exit_code: Mutex::new(None),
        exit_recorded: AtomicBool::new(false),
        compaction: IdleCompaction::default(),
        pid,
    });

    let reader_runtime = runtime.clone();
    let _reader = thread::Builder::new()
        .name(format!("keeper-reader-{}", cli.socket_path.display()))
        .spawn(move || keeper_read_loop(reader_runtime, reader))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let hold_runtime = runtime.clone();
    let _reply_hold = thread::Builder::new()
        .name(format!("keeper-reply-hold-{}", cli.socket_path.display()))
        .spawn(move || keeper_reply_hold_loop(hold_runtime))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let compaction_runtime = runtime.clone();
    let _compaction = thread::Builder::new()
        .name(format!("keeper-compaction-{}", cli.socket_path.display()))
        .spawn(move || keeper_compaction_loop(compaction_runtime))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    let wait_runtime = runtime.clone();
    let wait_join = thread::Builder::new()
        .name(format!("keeper-wait-{}", cli.socket_path.display()))
        .spawn(move || keeper_wait_loop(wait_runtime, child))
        .map_err(|error| MuxError::internal(error.to_string()))?;
    // Keepers intentionally outlive the server so a restarted server can
    // reattach through the socket. While no server is attached, the keeper
    // exits once there is nothing left to reattach to: its child has exited and
    // a server has saved that (see `KeeperRequest::ExitRecorded`), or its
    // socket was removed or replaced so no server can ever reach it again. In
    // the latter case it stops the child instead of leaving it running
    // unreachable. Until a server has saved the exit the keeper stays, so a
    // restart reads the exit and sees the pane as exited rather than
    // interrupted, even if the server that first read it crashed.
    // The socket is listening and the child is running: from here on a server
    // can connect at any time.
    readiness.report(Ok(()));
    loop {
        if !wait_for_keeper_client(&listener, KEEPER_SOCKET_CHECK_INTERVAL)? {
            if runtime.exit_recorded.load(Ordering::Acquire) && runtime.child_exited() {
                debug!(
                    socket_path = %cli.socket_path.display(),
                    "runtime keeper child exit was recorded and no server is attached; exiting"
                );
                let _ = wait_join.join();
                return Ok(());
            }
            if bound_socket.is_none() || socket_identity(&cli.socket_path) == bound_socket {
                continue;
            }
            debug!(
                socket_path = %cli.socket_path.display(),
                "runtime keeper socket removed or replaced; stopping unreachable runtime"
            );
            stop_runtime_child(&runtime, wait_join);
            return Ok(());
        }
        // On every return from this loop the reader and writer threads are
        // left to finish on their own: the reader only ends once every holder
        // of the PTY slave has closed it, which a surviving grandchild may
        // never do.
        let (mut stream, _) = listener.accept()?;
        let shutdown = handle_keeper_client(runtime.clone(), &mut stream)?;
        // Without a server no client forwards graphics queries any more.
        runtime.set_host_graphics(false);
        if shutdown {
            // The server asked this keeper to shut down (kill-server).
            debug!(
                socket_path = %cli.socket_path.display(),
                "runtime keeper shutdown requested; stopping runtime"
            );
            stop_runtime_child(&runtime, wait_join);
            return Ok(());
        }
    }
}

fn handle_keeper_client(runtime: Arc<KeeperRuntime>, stream: &mut UnixStream) -> Result<bool> {
    loop {
        let request = match read_message::<KeeperRequest>(stream) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(false),
            Err(error) => {
                let response = KeeperResponse::Error {
                    message: error.to_string(),
                    conflict: false,
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
                let response = keeper_error_response(error);
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
            escalate_kill(runtime.clone());
            Ok((KeeperResponse::Ok, false))
        }
        KeeperRequest::Shutdown => Ok((KeeperResponse::Ok, true)),
        KeeperRequest::Clipboard => Ok((
            KeeperResponse::Clipboard(
                runtime
                    .clipboard
                    .lock()
                    .map_err(|_| MuxError::internal("runtime keeper clipboard lock poisoned"))?
                    .clone(),
            ),
            false,
        )),
        KeeperRequest::Passthrough { after } => Ok((
            KeeperResponse::Passthrough(
                runtime
                    .surface
                    .lock()
                    .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?
                    .passthrough
                    .take_after(after),
            ),
            false,
        )),
        KeeperRequest::SetHostGraphics { enabled } => {
            runtime.set_host_graphics(enabled);
            Ok((KeeperResponse::Ok, false))
        }
        KeeperRequest::ExitRecorded => {
            runtime.exit_recorded.store(true, Ordering::Release);
            Ok((KeeperResponse::Ok, false))
        }
    }
}

impl KeeperRuntime {
    fn child_exited(&self) -> bool {
        self.exit_code
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

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
        let bell_sequence = self.bell_sequence.load(Ordering::Relaxed);
        let clipboard_sequence = self
            .clipboard
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper clipboard lock poisoned"))?
            .as_ref()
            .map_or(0, |clipboard| clipboard.sequence);
        let title = surface.backend.metadata().title.clone();
        let reported_cwd = surface.router.reported_cwd();
        let passthrough_sequence = surface.passthrough.last_sequence;
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
        let foreground_command = if running {
            self.foreground_command()
        } else {
            None
        };
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
            clipboard_sequence,
            passthrough_sequence,
            foreground_command,
            bell_sequence,
        })
    }

    /// Command line of the PTY's foreground process group leader when that is
    /// not the direct child: the job a shell is running. `None` while the child
    /// itself is in the foreground (a shell at its prompt) or when the leader
    /// cannot be resolved.
    fn foreground_command(&self) -> Option<Vec<String>> {
        let child_pid = self.pid?;
        let leader = self.master.lock().ok()?.process_group_leader()?;
        let leader = u32::try_from(leader).ok()?;
        if leader == child_pid {
            return None;
        }
        lookup_foreground_command(leader, resolve_pid_argv)
    }

    fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.ensure_running()?;
        // Input that answers the pane's pending kitty graphics queries lets the
        // replies held behind them go, right after it.
        let released = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?
            .reply_hold
            .input(&bytes);
        // Hand off to the writer thread. This is a bounded send: it blocks only
        // when the queue is full (backpressure from a child that is not reading
        // its input) and never holds the writer, so it cannot stall other writers
        // or the read loop. A send error means the writer thread is gone.
        self.write_tx
            .send(bytes)
            .map_err(|_| MuxError::internal("runtime keeper writer channel closed"))?;
        self.write_replies(released);
        Ok(())
    }

    /// Record whether some client forwards kitty graphics queries to a host
    /// terminal that answers them. Turning it off lets held replies go.
    fn set_host_graphics(&self, enabled: bool) {
        let released = self
            .surface
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reply_hold
            .set_enabled(enabled);
        self.write_replies(released);
    }

    /// Hand terminal replies to the writer thread without blocking, as the read
    /// loop does: if the queue is full (the writer is stuck on a child that is
    /// not reading its input) they are dropped.
    fn write_replies(&self, replies: Vec<u8>) {
        if replies.is_empty() {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.write_tx.try_send(replies) {
            debug!("terminal query reply queue full; dropping reply");
        }
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
        self.compaction.note_activity();
        Ok(())
    }

    fn snapshot(&self, cwd: Option<PathBuf>) -> Result<KeeperSnapshot> {
        let surface = self
            .surface
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper surface lock poisoned"))?;
        // Reading the whole scrollback restores what compaction compressed.
        self.compaction.note_activity();
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
        self.compaction.note_activity();
        Ok(surface.capture_scrollback_slice(start_line, line_count))
    }

    /// Send the child SIGHUP, like a terminal hanging up.
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
                let BackendEvents {
                    activity,
                    replies,
                    clipboard,
                } = surface.route_output(bytes, Instant::now());
                if surface.reply_hold.deadline().is_some() {
                    runtime.reply_hold_changed.notify_one();
                }
                if let Ok(mut pipe) = runtime.pipe.lock()
                    && let Some(pipe) = pipe.as_mut()
                {
                    pipe.write(bytes);
                }
                // Count bells before the sequence advances, so a status read
                // that sees the new sequence also sees the bell.
                if activity == ActivityState::Bell {
                    runtime.bell_sequence.fetch_add(1, Ordering::Relaxed);
                }
                runtime.sequence.fetch_add(1, Ordering::Relaxed);
                runtime.compaction.note_activity();
                if let Ok(mut state) = runtime.activity.lock() {
                    *state = activity;
                }
                if let Some(write) = clipboard
                    && let Ok(mut latest) = runtime.clipboard.lock()
                {
                    let sequence = latest.as_ref().map_or(0, |held| held.sequence) + 1;
                    *latest = Some(KeeperClipboard { sequence, write });
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

/// Let replies held behind kitty graphics queries go once their deadline
/// passes without an answer from the host terminal. Sleeps until the read loop
/// signals that the hold started waiting, then until its deadline.
fn keeper_reply_hold_loop(runtime: Arc<KeeperRuntime>) {
    let Ok(mut surface) = runtime.surface.lock() else {
        return;
    };
    loop {
        let now = Instant::now();
        let released = surface.reply_hold.expire(now);
        runtime.write_replies(released);
        let waited = match surface.reply_hold.deadline() {
            Some(deadline) => runtime
                .reply_hold_changed
                .wait_timeout(surface, deadline.saturating_duration_since(now))
                .ok()
                .map(|(surface, _)| surface),
            None => runtime.reply_hold_changed.wait(surface).ok(),
        };
        let Some(next) = waited else {
            return;
        };
        surface = next;
    }
}

/// Compact the backend each time the pane goes quiet (see
/// `crate::idle_compaction`).
fn keeper_compaction_loop(runtime: Arc<KeeperRuntime>) {
    runtime.compaction.run(COMPACTION_IDLE, || {
        runtime
            .surface
            .lock()
            .is_ok_and(|mut surface| surface.backend.compact_step())
    });
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

/// Wait up to `timeout` for a client connection; returns whether one is pending
/// so the caller can `accept` without blocking past the timeout.
fn wait_for_keeper_client(listener: &UnixListener, timeout: Duration) -> Result<bool> {
    let mut poll_fd = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: `poll_fd` is a valid pollfd for the listener, which outlives this call.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error.into());
    }
    Ok(ready > 0)
}

/// Identify the filesystem entry at `path` so a keeper can tell whether its
/// socket is still the one it bound.
fn socket_identity(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

/// Stop the child of a keeper that is about to exit, because no server can
/// reach it any more or the server asked it to shut down: SIGHUP like a regular
/// kill, then SIGKILL if the child ignores it, and wait for it to exit.
fn stop_runtime_child(runtime: &KeeperRuntime, wait_join: thread::JoinHandle<()>) {
    let _ = runtime.kill();
    let deadline = Instant::now() + KEEPER_KILL_GRACE;
    while !wait_join.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if !wait_join.is_finished()
        && let Some(pid) = runtime.pid.and_then(|pid| libc::pid_t::try_from(pid).ok())
    {
        // SAFETY: plain signal delivery. The wait thread had not reaped the child
        // when checked just above, so `pid` still names it.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    let _ = wait_join.join();
}

/// Follow up a requested kill: a child still running [`KEEPER_KILL_GRACE`]
/// after its SIGHUP gets SIGKILL. A program may ignore SIGHUP, and a shell
/// can lose one that arrives while it is still starting up, so without this
/// `kill-pane` could leave the program running.
fn escalate_kill(runtime: Arc<KeeperRuntime>) {
    let spawned = thread::Builder::new()
        .name("keeper-kill-escalation".to_owned())
        .spawn(move || {
            let deadline = Instant::now() + KEEPER_KILL_GRACE;
            while Instant::now() < deadline {
                if runtime.ensure_running().is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(25));
            }
            if runtime.ensure_running().is_ok()
                && let Some(pid) = runtime.pid.and_then(|pid| libc::pid_t::try_from(pid).ok())
            {
                // SAFETY: plain signal delivery. The wait thread records the
                // exit as soon as it has reaped the child, and the child was
                // still recorded as running just above, so `pid` still names it.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        });
    if let Err(error) = spawned {
        debug!(%error, "failed to start kill escalation");
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
    last: &mut BufferRuntimeStatus,
    saw_exit: bool,
) {
    if last.pipe.is_none() && saw_exit {
        return;
    }
    last.sequence = last.sequence.saturating_add(1);
    (callbacks.on_output)(
        buffer_id,
        BufferRuntimeUpdate {
            sequence: last.sequence,
            activity: last.activity,
            title: None,
            pipe: Some(None),
            cwd: None,
            foreground_command: None,
            bell_sequence: last.bell_sequence,
        },
    );
    last.pipe = None;
}

/// Compute the update to emit when a freshly polled status differs from
/// `last`, the status last reported to the callbacks, or `None` when nothing
/// changed. Only the fields that actually changed are populated, so a cwd-only
/// change — which does not advance `sequence` — is still emitted and
/// distinguishable from no change.
fn status_update_delta(
    status: &BufferRuntimeStatus,
    last: &BufferRuntimeStatus,
) -> Option<BufferRuntimeUpdate> {
    let changed = status.sequence != last.sequence
        || status.bell_sequence != last.bell_sequence
        || status.title != last.title
        || status.activity != last.activity
        || status.pipe != last.pipe
        || status.cwd != last.cwd
        || status.foreground_command != last.foreground_command;
    if !changed {
        return None;
    }
    Some(BufferRuntimeUpdate {
        sequence: status.sequence,
        activity: status.activity,
        title: (status.title != last.title).then(|| status.title.clone()),
        pipe: (status.pipe != last.pipe).then(|| status.pipe.clone()),
        cwd: (status.cwd != last.cwd).then(|| status.cwd.clone()),
        foreground_command: (status.foreground_command != last.foreground_command)
            .then(|| status.foreground_command.clone()),
        bell_sequence: status.bell_sequence,
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
            // Seed from the attach-time status so a reattaching server does not
            // replay a clipboard write that was already delivered (or dropped).
            let mut last_clipboard = initial.clipboard_sequence;
            // Likewise for passthrough: what a keeper queued before this server
            // attached was meant for a frame that is long gone.
            let mut last_passthrough = initial.passthrough_sequence;
            let mut saw_exit = !initial.running;
            // The attaching server applies `initial` itself, foreground and bell
            // count included. Seed cwd as unknown (not `initial.cwd`) so the
            // first poll always reports the current directory: an OSC 7 report
            // or pid resolution may already be present at attach, and the
            // buffer record must still learn it.
            let mut last = initial;
            last.cwd = None;

            while !inner.stop.load(Ordering::Relaxed) {
                let status = {
                    let mut connection = match inner.connection.lock() {
                        Ok(connection) => connection,
                        Err(_) => break,
                    };
                    match connection.status().and_then(|status| {
                        let clipboard = if status.clipboard_sequence > last_clipboard {
                            connection.clipboard()?
                        } else {
                            None
                        };
                        let passthrough = if status.passthrough_sequence > last_passthrough {
                            connection
                                .passthrough_since(last_passthrough, status.passthrough_sequence)?
                        } else {
                            Vec::new()
                        };
                        Ok((status, clipboard, passthrough))
                    }) {
                        Ok(status) => status,
                        Err(error) => {
                            error!(%error, %inner.buffer_id, "status poll failed");
                            notify_pipe_removed(&callbacks, inner.buffer_id, &mut last, saw_exit);
                            (callbacks.on_exit)(inner.buffer_id, None);
                            break;
                        }
                    }
                };

                let (status, clipboard, passthrough) = status;
                if let Some(clipboard) = clipboard
                    && clipboard.sequence > last_clipboard
                {
                    last_clipboard = clipboard.sequence;
                    (callbacks.on_clipboard)(inner.buffer_id, clipboard.write);
                }

                if let Some(update) = status_update_delta(&status, &last) {
                    (callbacks.on_output)(inner.buffer_id, update);
                    last = status.clone();
                }

                // After the output update, though clients do not rely on that
                // order: a client refreshes the buffer itself before it draws
                // the frame these writes follow.
                // A fetch may return writes queued after the status reply.
                last_passthrough = passthrough
                    .last()
                    .map_or(0, |write| write.sequence)
                    .max(status.passthrough_sequence)
                    .max(last_passthrough);
                if !passthrough.is_empty() {
                    let writes = passthrough
                        .into_iter()
                        .map(|write| PassthroughWrite {
                            bytes: write.bytes,
                            cursor: write.cursor,
                        })
                        .collect();
                    (callbacks.on_passthrough)(inner.buffer_id, writes);
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

/// Command line of the foreground process group `leader`, read through `argv`.
///
/// It is read on every lookup rather than cached: a process can exec while
/// keeping its pid, even into the same executable with other arguments, so
/// nothing short of the command line itself shows that it changed. A read
/// that races a shell's fork and exec returns the shell's arguments once and
/// the program's from the next lookup on.
fn lookup_foreground_command(
    leader: u32,
    argv: impl Fn(u32) -> Option<Vec<String>>,
) -> Option<Vec<String>> {
    argv(leader).filter(|argv| !argv.is_empty())
}

/// Resolve the argument vector of a running process by pid.
#[cfg(target_os = "linux")]
fn resolve_pid_argv(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        bytes
            .split(|&byte| byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect(),
    )
}

#[cfg(target_os = "macos")]
fn resolve_pid_argv(pid: u32) -> Option<Vec<String>> {
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        libc::c_int::try_from(pid).ok()?,
    ];
    let mut size: libc::size_t = 0;
    // Safety: a null output buffer asks `sysctl` for the size it needs.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::null_mut(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; size];
    // Safety: `buffer` holds `size` writable bytes and `sysctl` writes at most
    // that many, updating `size` to the count it wrote.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    buffer.truncate(size);
    parse_procargs2(&buffer)
}

/// Parse a `KERN_PROCARGS2` buffer: `argc` as a native-endian `int`, the
/// executable path, NUL padding, then `argc` NUL-terminated arguments.
#[cfg(any(target_os = "macos", test))]
fn parse_procargs2(buffer: &[u8]) -> Option<Vec<String>> {
    let argc_bytes: [u8; 4] = buffer.get(..4)?.try_into().ok()?;
    let argc = usize::try_from(i32::from_ne_bytes(argc_bytes)).ok()?;
    let rest = &buffer[4..];
    let mut position = rest.iter().position(|&byte| byte == 0)?;
    while rest.get(position) == Some(&0) {
        position += 1;
    }
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        let remaining = rest.get(position..)?;
        let end = remaining.iter().position(|&byte| byte == 0)?;
        argv.push(String::from_utf8_lossy(&remaining[..end]).into_owned());
        position += end + 1;
    }
    Some(argv)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resolve_pid_argv(_pid: u32) -> Option<Vec<String>> {
    None
}

/// Connect to a keeper's socket. Callers only connect once the keeper is known
/// to be listening (a freshly spawned keeper has reported readiness; a restored
/// one was already serving), so a failure is final rather than retried.
fn connect_to_keeper(socket_path: &Path) -> Result<UnixStream> {
    Ok(UnixStream::connect(socket_path)?)
}

/// Wait for a freshly spawned keeper to report on `pipe` (its stdout) that it
/// is ready, or why it failed to start.
///
/// The keeper's socket file appears at `bind`, before the keeper listens on
/// it, and a connect in between is refused; the keeper also still has to open
/// a PTY and start the child, either of which can fail. Waiting for the
/// keeper's own report covers both instead of guessing from connect errors.
fn wait_for_keeper_ready<P: Read + AsRawFd>(mut pipe: P, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    let mut chunk = [0_u8; 256];
    while !line.contains(&b'\n') {
        if line.len() > KEEPER_READY_LINE_MAX {
            return Err(MuxError::protocol(
                "runtime keeper readiness report is too long",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(MuxError::timeout(
                "timed out waiting for the runtime keeper to start",
            ));
        }
        // Not readable yet (timed out or interrupted): re-check the deadline.
        if !wait_readable(pipe.as_raw_fd(), remaining)? {
            continue;
        }
        match pipe.read(&mut chunk) {
            Ok(0) => {
                return Err(MuxError::pty("runtime keeper exited before it was ready"));
            }
            Ok(read) => line.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    let end = line
        .iter()
        .position(|&byte| byte == b'\n')
        .unwrap_or(line.len());
    let report = String::from_utf8_lossy(&line[..end]);
    if report == KEEPER_READY_LINE {
        return Ok(());
    }
    match report.strip_prefix(KEEPER_ERROR_PREFIX) {
        Some(error) => Err(MuxError::pty(format!(
            "runtime keeper failed to start: {error}"
        ))),
        None => Err(MuxError::protocol(format!(
            "unexpected runtime keeper readiness report {report:?}"
        ))),
    }
}

/// Wait up to `timeout` for `fd` to become readable (or reach end of file).
fn wait_readable(fd: libc::c_int, timeout: Duration) -> Result<bool> {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: `poll_fd` is a valid pollfd for an fd the caller keeps open.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error.into());
    }
    Ok(ready > 0)
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
        if cli.size.pixel_width != 0 || cli.size.pixel_height != 0 {
            keeper
                .arg("--pixel-width")
                .arg(cli.size.pixel_width.to_string())
                .arg("--pixel-height")
                .arg(cli.size.pixel_height.to_string());
        }
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
        keeper.stdout(Stdio::piped());
        keeper.stderr(Stdio::null());
        let mut child = keeper.spawn()?;
        let ready = match child.stdout.take() {
            Some(stdout) => wait_for_keeper_ready(stdout, KEEPER_READY_TIMEOUT),
            None => Err(MuxError::internal("runtime keeper stdout was not piped")),
        };
        if ready.is_err() {
            // A keeper that reported a failure exits by itself; one that never
            // reported is stuck, so make sure it goes away either way.
            let _ = child.kill();
        }
        reap_keeper_process(child);
        return ready;
    }

    let (ready_tx, ready_rx) = sync_channel(1);
    thread::Builder::new()
        .name(format!("runtime-keeper-{}", cli.socket_path.display()))
        .spawn(move || {
            let mut readiness = KeeperReadiness::Channel(Some(ready_tx));
            if let Err(error) = serve_runtime_keeper(cli, &mut readiness) {
                error!(%error, "runtime keeper thread failed");
                readiness.report(Err(error.to_string()));
            }
        })
        .map_err(|error| MuxError::internal(error.to_string()))?;
    match ready_rx.recv_timeout(KEEPER_READY_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(MuxError::pty(format!(
            "runtime keeper failed to start: {error}"
        ))),
        Err(RecvTimeoutError::Timeout) => Err(MuxError::timeout(
            "timed out waiting for the runtime keeper to start",
        )),
        Err(RecvTimeoutError::Disconnected) => {
            Err(MuxError::pty("runtime keeper exited before it was ready"))
        }
    }
}

/// Stack size for the threads that reap keeper processes; they only block in
/// `waitpid`.
const KEEPER_REAPER_STACK_SIZE: usize = 64 * 1024;

/// Wait for a spawned keeper process on a background thread, so it is reaped
/// whenever it exits instead of lingering as a zombie while the server runs.
/// A keeper that outlives the server is reparented and reaped by init.
fn reap_keeper_process(mut child: std::process::Child) {
    let pid = child.id();
    let spawned = thread::Builder::new()
        .name(format!("keeper-reaper-{pid}"))
        .stack_size(KEEPER_REAPER_STACK_SIZE)
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(error) = spawned {
        error!(%error, pid, "failed to start keeper reaper thread");
    }
}

fn resolve_runtime_keeper_executable() -> Option<PathBuf> {
    if let Some(path) = env::var_os(crate::config::RUNTIME_KEEPER_BIN_ENV_VAR).map(PathBuf::from)
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

/// The reply for a request the keeper could not carry out. A conflict keeps
/// its kind; anything else travels as its message.
fn keeper_error_response(error: MuxError) -> KeeperResponse {
    match error {
        MuxError::Conflict(message) => KeeperResponse::Error {
            message,
            conflict: true,
        },
        other => KeeperResponse::Error {
            message: other.to_string(),
            conflict: false,
        },
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
        KeeperResponse::Clipboard(_) => "clipboard",
        KeeperResponse::Passthrough(_) => "passthrough",
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

/// Removes the keeper socket on exit, unless another keeper has since bound a
/// new socket at the same path.
struct SocketCleanup {
    socket_path: PathBuf,
    identity: Option<(u64, u64)>,
}

impl SocketCleanup {
    fn new(socket_path: PathBuf, identity: Option<(u64, u64)>) -> Self {
        Self {
            socket_path,
            identity,
        }
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if self.identity.is_some() && socket_identity(&self.socket_path) != self.identity {
            return;
        }
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
        spawn_status_poller, status_update_delta, wait_for_keeper_ready,
    };

    #[test]
    fn keeper_readiness_accepts_ready_report_split_across_reads() {
        let (reader, mut writer) = UnixStream::pair().expect("socket pair");
        let sender = thread::spawn(move || {
            writer.write_all(b"rea").expect("write first half");
            thread::sleep(Duration::from_millis(20));
            writer.write_all(b"dy\n").expect("write second half");
            writer
        });
        wait_for_keeper_ready(reader, Duration::from_secs(5)).expect("keeper reported ready");
        drop(sender.join());
    }

    #[test]
    fn keeper_readiness_surfaces_reported_start_error() {
        let (reader, mut writer) = UnixStream::pair().expect("socket pair");
        writer
            .write_all(b"error: no such program /missing\n")
            .expect("write error report");
        let error = wait_for_keeper_ready(reader, Duration::from_secs(5))
            .expect_err("start error is reported");
        assert!(
            error.to_string().contains("no such program /missing"),
            "{error}"
        );
    }

    #[test]
    fn keeper_readiness_fails_when_keeper_exits_without_reporting() {
        let (reader, writer) = UnixStream::pair().expect("socket pair");
        drop(writer);
        let error = wait_for_keeper_ready(reader, Duration::from_secs(5))
            .expect_err("silent exit is an error");
        assert!(
            error.to_string().contains("exited before it was ready"),
            "{error}"
        );
    }

    #[test]
    fn keeper_readiness_times_out_on_a_silent_keeper() {
        let (reader, _writer) = UnixStream::pair().expect("socket pair");
        let started = Instant::now();
        let error = wait_for_keeper_ready(reader, Duration::from_millis(100))
            .expect_err("silent keeper times out");
        assert!(matches!(error, MuxError::Timeout(_)), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// What ratatui-image's `Picker::from_query_stdio` writes in one burst: a
    /// kitty graphics query, DA1, the cell size query and a status report,
    /// whose reply it reads up to.
    const GRAPHICS_DETECTION: &[u8] =
        b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c\x1b[16t\x1b[5n";

    /// The emulator's replies to [`GRAPHICS_DETECTION`] in a pane of 9 by 18
    /// pixel cells.
    const DETECTION_REPLIES: &[u8] = b"\x1b[?6c\x1b[6;18;9t\x1b[0n";

    fn detecting_surface(host_graphics: bool) -> super::KeeperSurface {
        let mut surface = super::KeeperSurface::new(embers_core::PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 720,
            pixel_height: 432,
        });
        assert!(surface.reply_hold.set_enabled(host_graphics).is_empty());
        surface
    }

    #[test]
    fn detection_replies_wait_for_the_hosts_graphics_answer() {
        let mut surface = detecting_surface(true);
        // A reply to a query before the graphics query is not held.
        let events = surface.route_output(b"\x1b[5n", Instant::now());
        assert_eq!(events.replies, b"\x1b[0n");

        let events = surface.route_output(GRAPHICS_DETECTION, Instant::now());
        assert!(events.replies.is_empty(), "{:?}", events.replies);
        let forwarded = surface.passthrough.take_after(0);
        assert_eq!(
            forwarded
                .iter()
                .map(|write| &write.bytes[..])
                .collect::<Vec<_>>(),
            vec![&b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"[..]]
        );

        // The host's answer reaches the pane as input; the replies follow it.
        assert_eq!(
            surface.reply_hold.input(b"\x1b_Gi=31;OK\x1b\\"),
            DETECTION_REPLIES
        );
        assert_eq!(
            surface.route_output(b"\x1b[5n", Instant::now()).replies,
            b"\x1b[0n"
        );
    }

    #[test]
    fn detection_replies_go_out_when_the_host_stays_silent() {
        let mut surface = detecting_surface(true);
        let sent = Instant::now();
        assert!(
            surface
                .route_output(GRAPHICS_DETECTION, sent)
                .replies
                .is_empty()
        );

        let deadline = surface.reply_hold.deadline().expect("replies are held");
        assert_eq!(deadline, sent + crate::reply_hold::REPLY_HOLD_TIMEOUT);
        assert!(
            surface
                .reply_hold
                .expire(deadline - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(surface.reply_hold.expire(deadline), DETECTION_REPLIES);
    }

    #[test]
    fn detection_replies_are_immediate_without_host_graphics() {
        let mut surface = detecting_surface(false);
        let events = surface.route_output(GRAPHICS_DETECTION, Instant::now());
        // No graphics answer, then the emulator's: the program sees no kitty
        // graphics support, at once.
        assert_eq!(events.replies, DETECTION_REPLIES);
        assert_eq!(surface.reply_hold.deadline(), None);
    }

    #[test]
    fn scrollback_slice_carries_hyperlinks_through_keeper_json() {
        let mut surface = super::KeeperSurface::new(embers_core::PtySize::new(20, 2));
        let _ = surface.route_output(
            b"\x1b]8;id=x;https://example.com\x1b\\link\x1b]8;;\x1b\\",
            Instant::now(),
        );

        let slice = surface.capture_scrollback_slice(0, 1);
        let json = serde_json::to_string(&slice).expect("serialize slice");
        let decoded: super::KeeperScrollbackSlice =
            serde_json::from_str(&json).expect("deserialize slice");
        let lines = decoded.into_snapshot_lines();

        assert_eq!(lines[0].text, "link");
        let link = lines[0].link(&lines[0].runs[0]).expect("run is linked");
        assert_eq!(link.uri, "https://example.com");
        assert_eq!(link.id.as_deref(), Some("x"));
    }

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
            clipboard_sequence: 0,
            bell_sequence: 0,
            passthrough_sequence: 0,
            foreground_command: None,
        }
    }

    /// The status last reported for a shell buffer titled "shell".
    fn last_status(
        sequence: u64,
        activity: ActivityState,
        cwd: Option<std::path::PathBuf>,
        bell_sequence: u64,
    ) -> BufferRuntimeStatus {
        let mut status = status_with_cwd(sequence, None);
        status.activity = activity;
        status.cwd = cwd;
        status.bell_sequence = bell_sequence;
        status
    }

    #[test]
    fn status_update_delta_emits_cwd_change_without_sequence_advance() {
        // known -> known: cwd changes while the snapshot sequence is unchanged.
        let status = status_with_cwd(7, Some("/new"));
        let update = status_update_delta(
            &status,
            &last_status(
                7,
                ActivityState::Idle,
                Some(std::path::PathBuf::from("/old")),
                0,
            ),
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
            &last_status(
                7,
                ActivityState::Idle,
                Some(std::path::PathBuf::from("/old")),
                0,
            ),
        )
        .expect("cwd clear should emit an update even at the same sequence");
        assert_eq!(update.cwd, Some(None));
    }

    #[test]
    fn status_update_delta_returns_none_when_unchanged() {
        let status = status_with_cwd(7, Some("/same"));
        let update = status_update_delta(
            &status,
            &last_status(
                7,
                ActivityState::Idle,
                Some(std::path::PathBuf::from("/same")),
                0,
            ),
        );
        assert!(update.is_none());
    }

    #[test]
    fn status_update_delta_carries_a_bell_that_later_output_overwrote() {
        // The bell rang, then more output arrived before the poll, so the
        // latest activity is plain output; the bell count still advanced.
        let mut status = status_with_cwd(8, None);
        status.activity = ActivityState::Activity;
        status.bell_sequence = 1;
        let update =
            status_update_delta(&status, &last_status(8, ActivityState::Activity, None, 0))
                .expect("a new bell should emit an update");
        assert_eq!(update.bell_sequence, 1);
    }

    #[test]
    fn status_update_delta_emits_foreground_command_changes() {
        let mut status = status_with_cwd(7, Some("/same"));
        status.foreground_command = Some(vec!["vim".to_owned(), "notes.txt".to_owned()]);
        let update = status_update_delta(
            &status,
            &last_status(
                7,
                ActivityState::Idle,
                Some(std::path::PathBuf::from("/same")),
                0,
            ),
        )
        .expect("a new foreground job should emit an update");
        assert_eq!(
            update.foreground_command,
            Some(Some(vec!["vim".to_owned(), "notes.txt".to_owned()]))
        );
        assert!(update.cwd.is_none());
    }

    #[test]
    fn foreground_command_follows_an_exec_of_the_same_executable() {
        let args = std::cell::RefCell::new(vec!["vim".to_owned(), "a.txt".to_owned()]);
        let lookup = || super::lookup_foreground_command(42, |_| Some(args.borrow().clone()));

        assert_eq!(lookup(), Some(vec!["vim".to_owned(), "a.txt".to_owned()]));
        // The same pid execs the same executable with other arguments.
        *args.borrow_mut() = vec!["vim".to_owned(), "b.txt".to_owned()];
        assert_eq!(lookup(), Some(vec!["vim".to_owned(), "b.txt".to_owned()]));
    }

    #[test]
    fn parse_procargs2_reads_the_argument_vector() {
        let mut buffer = 2_i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/usr/bin/vim\0\0\0\0vim\0notes.txt\0HOME=/x\0");
        assert_eq!(
            super::parse_procargs2(&buffer),
            Some(vec!["vim".to_owned(), "notes.txt".to_owned()])
        );
        assert_eq!(super::parse_procargs2(&buffer[..2]), None);
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
                on_clipboard: Arc::new(|_, _| {}),
                on_passthrough: Arc::new(|_, _| {}),
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
                clipboard_sequence: 0,
                bell_sequence: 0,
                passthrough_sequence: 0,
                foreground_command: None,
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
                on_clipboard: Arc::new(|_, _| {}),
                on_passthrough: Arc::new(|_, _| {}),
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
                clipboard_sequence: 0,
                bell_sequence: 0,
                passthrough_sequence: 0,
                foreground_command: None,
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

    /// Run a keeper on a thread of this process for `command` and wait until
    /// it is ready, returning its socket path and a receiver that yields once
    /// the keeper has returned.
    fn start_in_process_keeper(
        dir: &std::path::Path,
        command: &[&str],
    ) -> (std::path::PathBuf, mpsc::Receiver<()>) {
        let socket_path = dir.join("keeper.sock");
        let cli = super::RuntimeKeeperCli {
            socket_path: socket_path.clone(),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd: None,
            env: BTreeMap::new(),
            size: embers_core::PtySize::new(80, 24),
        };
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut readiness = super::KeeperReadiness::Channel(Some(ready_tx));
            let _ = super::serve_runtime_keeper(cli, &mut readiness);
            let _ = done_tx.send(());
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("keeper reports readiness")
            .expect("keeper starts");
        (socket_path, done_rx)
    }

    fn wait_for_exited_status(socket_path: &std::path::Path) -> BufferRuntimeStatus {
        let stream = super::connect_to_keeper(socket_path).expect("connect to keeper");
        let mut connection = KeeperConnection { stream };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = connection.status().expect("keeper status");
            if !status.running {
                return status;
            }
            assert!(Instant::now() < deadline, "keeper child never exited");
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn killing_an_exited_keeper_child_is_a_conflict() {
        // A caller racing a buffer's exit (e.g. a waiting display-popup after
        // kill-popup) must see the keeper's conflict as a conflict, not a
        // transport failure.
        let dir = tempfile::tempdir().expect("tempdir");
        let (socket_path, _done) =
            start_in_process_keeper(dir.path(), &["/bin/sh", "-c", "exit 0"]);
        wait_for_exited_status(&socket_path);
        let stream = super::connect_to_keeper(&socket_path).expect("connect to keeper");
        let mut connection = KeeperConnection { stream };
        let error = connection.kill().expect_err("the child already exited");
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            error.to_string(),
            "conflict: buffer runtime has already exited"
        );
    }

    #[test]
    fn keeper_retires_only_once_its_exit_is_acknowledged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (socket_path, done) = start_in_process_keeper(dir.path(), &["/bin/sh", "-c", "exit 3"]);

        // A server that read the exit but went away before recording it.
        let status = wait_for_exited_status(&socket_path);
        assert_eq!(status.exit_code, Some(3));
        assert!(
            done.recv_timeout(Duration::from_millis(1500)).is_err(),
            "keeper retired although no server acknowledged recording the exit"
        );

        // The next server reads the exit again, records it and acknowledges.
        let status = wait_for_exited_status(&socket_path);
        assert_eq!(status.exit_code, Some(3));
        let mut stream = super::connect_to_keeper(&socket_path).expect("reconnect to keeper");
        super::write_message(&mut stream, &super::KeeperRequest::ExitRecorded)
            .expect("send acknowledgement");
        assert!(matches!(
            read_message::<super::KeeperResponse>(&mut stream),
            Ok(Some(super::KeeperResponse::Ok))
        ));
        drop(stream);
        done.recv_timeout(Duration::from_secs(10))
            .expect("keeper should retire once its exit was acknowledged and the server left");
        assert!(!socket_path.exists(), "a retired keeper removes its socket");
    }

    /// Serve `stream` like a keeper that knows `ExitRecorded` but predates
    /// `Shutdown`: any other request is answered with an error, as an unknown
    /// variant would be. Returns the requests it received, by name.
    fn serve_keeper_without_shutdown(mut stream: UnixStream) -> Vec<String> {
        let mut received = Vec::new();
        while let Ok(Some(request)) = read_message::<serde_json::Value>(&mut stream) {
            let name = match &request {
                serde_json::Value::String(name) => name.clone(),
                other => other.to_string(),
            };
            let response = match name.as_str() {
                "Kill" | "ExitRecorded" => super::KeeperResponse::Ok,
                _ => super::KeeperResponse::Error {
                    conflict: false,
                    message: format!("unknown variant `{name}`"),
                },
            };
            received.push(name);
            if super::write_message(&mut stream, &response).is_err() {
                break;
            }
        }
        received
    }

    #[test]
    fn shutdown_of_a_keeper_without_shutdown_kills_and_releases_it() {
        let (server, keeper) = UnixStream::pair().expect("socket pair");
        let keeper = thread::spawn(move || serve_keeper_without_shutdown(keeper));
        let mut connection = KeeperConnection { stream: server };
        connection.shutdown().expect("fallback shutdown succeeds");
        drop(connection);
        // Killing the child alone would leave such a keeper waiting for a
        // server to record the exit, which after kill-server never comes.
        assert_eq!(
            keeper.join().expect("fake keeper"),
            ["Shutdown", "Kill", "ExitRecorded"]
        );
    }

    #[test]
    fn keeper_rejects_unknown_requests_and_keeps_serving() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (socket_path, _done) =
            start_in_process_keeper(dir.path(), &["/bin/sh", "-c", "sleep 30"]);
        let mut stream = super::connect_to_keeper(&socket_path).expect("connect to keeper");
        super::write_message(&mut stream, &"NoSuchRequest").expect("send unknown request");
        assert!(matches!(
            read_message::<super::KeeperResponse>(&mut stream),
            Ok(Some(super::KeeperResponse::Error { .. }))
        ));
        let mut connection = KeeperConnection { stream };
        assert!(connection.status().expect("status after error").running);
        connection.kill().expect("kill child");
    }
}
