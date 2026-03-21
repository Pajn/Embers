use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use embers_core::{MuxError, PtySize, Result};
use portable_pty::{
    CommandBuilder, MasterPty, NativePtySystem, PtySize as PortableSize, PtySystem,
};

pub struct PtyHarness {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    reader_join: Option<thread::JoinHandle<()>>,
}

impl PtyHarness {
    pub fn spawn(command: &str, args: &[&str], size: PtySize) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PortableSize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            })
            .map_err(|error| MuxError::pty(error.to_string()))?;

        let mut command_builder = CommandBuilder::new(command);
        for arg in args {
            command_builder.arg(arg);
        }

        let child = pair
            .slave
            .spawn_command(command_builder)
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| MuxError::pty(error.to_string()))?;
        let (tx, rx) = mpsc::channel();
        let reader_join = thread::spawn(move || {
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
        });

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
            "timed out waiting for output containing {needle:?}; got {output:?}"
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
        let _ = &self.master;
        Ok(())
    }
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        if let Some(join) = self.reader_join.take() {
            let _ = join.join();
        }
    }
}
