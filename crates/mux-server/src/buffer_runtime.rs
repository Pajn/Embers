use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use mux_core::{BufferId, MuxError, PtySize, Result};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize as PortablePtySize,
    PtySystem,
};

#[derive(Clone)]
pub struct BufferRuntimeHandle {
    inner: Arc<BufferRuntimeInner>,
}

struct BufferRuntimeInner {
    buffer_id: BufferId,
    pid: Option<u32>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

#[derive(Clone)]
pub struct BufferRuntimeCallbacks {
    pub on_output: Arc<dyn Fn(BufferId, Vec<u8>) + Send + Sync>,
    pub on_exit: Arc<dyn Fn(BufferId, Option<i32>) + Send + Sync>,
}

impl std::fmt::Debug for BufferRuntimeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BufferRuntimeHandle")
            .field("buffer_id", &self.inner.buffer_id)
            .field("pid", &self.inner.pid)
            .finish()
    }
}

impl BufferRuntimeHandle {
    pub fn spawn(
        buffer_id: BufferId,
        command: &[String],
        cwd: Option<&Path>,
        size: PtySize,
        callbacks: BufferRuntimeCallbacks,
    ) -> Result<Self> {
        let Some(program) = command.first() else {
            return Err(MuxError::invalid_input("buffer command must not be empty"));
        };

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(to_portable_size(size))
            .map_err(|error| MuxError::pty(error.to_string()))?;

        let mut command_builder = CommandBuilder::new(program);
        command_builder.args(&command[1..]);
        if let Some(cwd) = cwd {
            command_builder.cwd(cwd);
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

        let on_output = callbacks.on_output.clone();
        thread::Builder::new()
            .name(format!("buffer-{buffer_id}-reader"))
            .spawn(move || read_loop(buffer_id, reader, on_output))
            .map_err(|error| MuxError::internal(error.to_string()))?;

        let on_exit = callbacks.on_exit.clone();
        thread::Builder::new()
            .name(format!("buffer-{buffer_id}-wait"))
            .spawn(move || wait_loop(buffer_id, child, on_exit))
            .map_err(|error| MuxError::internal(error.to_string()))?;

        Ok(Self {
            inner: Arc::new(BufferRuntimeInner {
                buffer_id,
                pid,
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                killer: Mutex::new(killer),
            }),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut writer = inner
                .writer
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime writer lock poisoned"))?;
            writer.write_all(&bytes)?;
            writer.flush()?;
            Ok(())
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn resize(&self, size: PtySize) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let master = inner
                .master
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime master lock poisoned"))?;
            master
                .resize(to_portable_size(size))
                .map_err(|error| MuxError::pty(error.to_string()))
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }

    pub async fn kill(&self) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut killer = inner
                .killer
                .lock()
                .map_err(|_| MuxError::internal("buffer runtime killer lock poisoned"))?;
            killer
                .kill()
                .map_err(|error| MuxError::pty(error.to_string()))
        })
        .await
        .map_err(|error| MuxError::internal(error.to_string()))?
    }
}

fn read_loop(
    buffer_id: BufferId,
    mut reader: Box<dyn Read + Send>,
    on_output: Arc<dyn Fn(BufferId, Vec<u8>) + Send + Sync>,
) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => on_output(buffer_id, buffer[..read].to_vec()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn wait_loop(
    buffer_id: BufferId,
    mut child: Box<dyn Child + Send + Sync>,
    on_exit: Arc<dyn Fn(BufferId, Option<i32>) + Send + Sync>,
) {
    let exit_code = child.wait().ok().and_then(exit_status_code);
    on_exit(buffer_id, exit_code);
}

fn exit_status_code(status: portable_pty::ExitStatus) -> Option<i32> {
    if status.signal().is_some() {
        None
    } else {
        Some(status.exit_code() as i32)
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
