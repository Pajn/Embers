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
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use embers_core::{ActivityState, BufferId, MuxError, PtySize, Result, TerminalSnapshot};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize as PortablePtySize,
    PtySystem,
};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::{AlacrittyTerminalBackend, RawByteRouter, TerminalBackend};

const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(25);
const CONNECT_RETRY_ATTEMPTS: usize = 1200;
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct BufferRuntimeUpdate {
    pub sequence: u64,
    pub activity: ActivityState,
    pub title: Option<Option<String>>,
}

#[derive(Clone, Debug)]
pub struct BufferRuntimeStatus {
    pub pid: Option<u32>,
    pub sequence: u64,
    pub activity: ActivityState,
    pub title: Option<String>,
    pub running: bool,
    pub exit_code: Option<i32>,
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
    Write { bytes: Vec<u8> },
    Resize { size: PtySize },
    Snapshot { cwd: Option<PathBuf> },
    VisibleSnapshot { cwd: Option<PathBuf> },
    ScrollbackSlice { start_line: u64, line_count: u32 },
    Kill,
}

#[derive(Serialize, Deserialize)]
enum KeeperResponse {
    Status(KeeperStatus),
    Snapshot(KeeperSnapshot),
    VisibleSnapshot(TerminalSnapshot),
    ScrollbackSlice(KeeperScrollbackSlice),
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
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeeperSnapshot {
    pub sequence: u64,
    pub size: PtySize,
    pub lines: Vec<String>,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeeperScrollbackSlice {
    pub start_line: u64,
    pub total_lines: u64,
    pub lines: Vec<String>,
}

struct KeeperRuntime {
    surface: Mutex<KeeperSurface>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
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
}

impl KeeperSurface {
    fn new(size: PtySize) -> Self {
        Self {
            router: RawByteRouter,
            backend: Box::new(AlacrittyTerminalBackend::new(size)),
            size,
        }
    }

    fn route_output(&mut self, bytes: &[u8]) -> ActivityState {
        self.router.route_output(self.backend.as_mut(), bytes);
        self.backend.take_activity()
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
        KeeperScrollbackSlice {
            start_line: slice.start_line,
            total_lines: slice.total_lines,
            lines: slice.lines,
        }
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
                    thread::sleep(KEEPER_PTY_RETRY_DELAY * (attempt + 1) as u32);
                }
            }
        }
    }
    let pair = match pair {
        Some(pair) => pair,
        None => {
            let error = last_error.ok_or_else(|| {
                MuxError::pty(format!(
                    "failed to openpty after {} attempts with no error details",
                    KEEPER_PTY_MAX_RETRIES + 1
                ))
            })?;
            return Err(MuxError::pty(format!(
                "failed to openpty after {} attempts: {error}",
                KEEPER_PTY_MAX_RETRIES + 1
            )));
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

    let runtime = Arc::new(KeeperRuntime {
        surface: Mutex::new(KeeperSurface::new(cli.size)),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
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
        Ok(KeeperStatus {
            pid: self.pid,
            sequence,
            activity,
            title,
            running: exit_code.is_none(),
            exit_code: exit_code.flatten(),
        })
    }

    fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.ensure_running()?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| MuxError::internal("runtime keeper writer lock poisoned"))?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
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
                let activity = surface.route_output(&buffer[..read]);
                runtime.sequence.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut state) = runtime.activity.lock() {
                    *state = activity;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn keeper_wait_loop(runtime: Arc<KeeperRuntime>, mut child: Box<dyn Child + Send + Sync>) {
    let exit_code = child.wait().ok().and_then(exit_status_code);
    if let Ok(mut state) = runtime.exit_code.lock() {
        *state = Some(exit_code);
    }
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
                            (callbacks.on_exit)(inner.buffer_id, None);
                            break;
                        }
                    }
                };

                if status.sequence != last_sequence
                    || status.title != last_title
                    || status.activity != last_activity
                {
                    let title = (status.title != last_title).then(|| status.title.clone());
                    (callbacks.on_output)(
                        inner.buffer_id,
                        BufferRuntimeUpdate {
                            sequence: status.sequence,
                            activity: status.activity,
                            title,
                        },
                    );
                    last_sequence = status.sequence;
                    last_title = status.title.clone();
                    last_activity = status.activity;
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
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use embers_core::{ActivityState, BufferId, MuxError};

    use super::{
        BufferRuntimeCallbacks, BufferRuntimeInner, BufferRuntimeStatus, KeeperConnection,
        MAX_FRAME_SIZE, RuntimeThreads, read_message, spawn_status_poller,
    };

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
                on_output: Arc::new(move |buffer_id, _| {
                    output_tx
                        .send(buffer_id)
                        .expect("send unexpected output notification");
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
        assert!(output_rx.try_recv().is_err());
    }
}
