use std::io::{Read, Write};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use embers_core::{MuxError, PtySize, Result};
use portable_pty::{
    CommandBuilder, MasterPty, NativePtySystem, PtyPair, PtySize as PortableSize, PtySystem,
};

const OUTPUT_TAIL_CHARS: usize = 2000;
static PTY_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Checks if PTY devices are available on this system.
/// Returns true if we can successfully open a PTY pair.
pub fn is_pty_available() -> bool {
    if let Some(available) = PTY_AVAILABLE.get() {
        return *available;
    }
    let available = PtyHarness::openpty_with_retry(PtySize::new(80, 24)).is_ok();
    let _ = PTY_AVAILABLE.set(available);
    available
}

pub struct PtyHarness {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    reader_join: Option<thread::JoinHandle<()>>,
}

impl PtyHarness {
    /// Maximum number of retries for PTY allocation failures
    const MAX_RETRIES: usize = 3;

    /// Initial delay between retries on PTY allocation failure
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    /// Maximum random jitter added to retry backoff so concurrent probes do not align exactly.
    const RETRY_JITTER_MS: u64 = 25;

    fn openpty_with_retry(size: PtySize) -> Result<PtyPair> {
        let pty_system = NativePtySystem::default();
        let mut last_error = None;

        for attempt in 0..=Self::MAX_RETRIES {
            match pty_system.openpty(PortableSize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            }) {
                Ok(pair) => return Ok(pair),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < Self::MAX_RETRIES {
                        let base_delay = Self::RETRY_DELAY * (attempt + 1) as u32;
                        let jitter =
                            Duration::from_millis(fastrand::u64(..Self::RETRY_JITTER_MS.max(1)));
                        thread::sleep(base_delay + jitter);
                    }
                }
            }
        }

        Err(MuxError::pty(format!(
            "failed to openpty after {} attempts: {}",
            Self::MAX_RETRIES + 1,
            // Safe: every failed attempt stores the last encountered PTY allocation error.
            last_error.expect("openpty retry loop must capture an error before failing")
        )))
    }

    pub fn spawn(command: &str, args: &[&str], size: PtySize) -> Result<Self> {
        let mut command_builder = CommandBuilder::new(command);
        for arg in args {
            command_builder.arg(arg);
        }

        let pair = Self::openpty_with_retry(size)?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let mut child = pair
            .slave
            .spawn_command(command_builder)
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let (tx, rx) = mpsc::channel();
        let reader_join = thread::Builder::new()
            .name("pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0_u8; 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            if tx.send(buffer[..read].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| {
                let mut message = format!("failed to spawn PTY reader thread: {error}");
                if let Err(kill_error) = child.kill() {
                    message.push_str(&format!("; failed to kill child: {kill_error}"));
                }
                if let Err(wait_error) = child.wait() {
                    message.push_str(&format!("; failed to reap child: {wait_error}"));
                }
                MuxError::pty(message)
            })?;

        Ok(Self {
            master: pair.master,
            child,
            writer,
            output_rx: rx,
            reader_join: Some(reader_join),
        })
    }

    pub fn write_all(&mut self, input: &str) -> Result<()> {
        self.writer.write_all(input.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn read_until_contains(&mut self, needle: &str, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        let mut output = String::new();

        while start.elapsed() < timeout {
            match self.output_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    output.push_str(&String::from_utf8_lossy(&chunk));
                    if output.contains(needle) {
                        return Ok(output);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Err(MuxError::timeout(format!(
            "timed out waiting for output containing {needle:?}; recent output: {:?}",
            tail_excerpt(&output)
        )))
    }

    pub fn wait_for_quiet(&mut self, quiet_for: Duration, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        let mut output = String::new();
        let mut last_activity = Instant::now();

        while start.elapsed() < timeout {
            match self.output_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    output.push_str(&String::from_utf8_lossy(&chunk));
                    last_activity = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if last_activity.elapsed() >= quiet_for {
                        return Ok(output);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(output),
            }
        }

        Err(MuxError::timeout(format!(
            "timed out waiting for quiet PTY output; recent output: {:?}",
            tail_excerpt(&output)
        )))
    }

    pub fn kill(&mut self) -> Result<()> {
        self.child
            .kill()
            .map_err(|error| MuxError::pty(error.to_string()))
    }

    pub fn wait(&mut self) -> Result<()> {
        self.child
            .wait()
            .map_err(|error| MuxError::pty(error.to_string()))?;
        if let Some(join) = self.reader_join.take() {
            let _ = join.join();
        }
        Ok(())
    }
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(join) = self.reader_join.take() {
            let _ = join.join();
        }
    }
}

fn tail_excerpt(output: &str) -> String {
    let total = output.chars().count();
    if total <= OUTPUT_TAIL_CHARS {
        return output.to_owned();
    }

    let tail: String = output
        .chars()
        .skip(total.saturating_sub(OUTPUT_TAIL_CHARS))
        .collect();
    format!("...[truncated {} chars]{}", total - OUTPUT_TAIL_CHARS, tail)
}
