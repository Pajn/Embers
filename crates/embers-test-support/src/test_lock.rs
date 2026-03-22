use std::fs::{File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(not(unix))]
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

#[cfg(not(unix))]
use std::thread;
#[cfg(not(unix))]
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OwnedMutexGuard};

const TEST_LOCK_FILE_NAME: &str = "embers-integration-tests.lock";
#[cfg(not(unix))]
const FILE_LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);
#[cfg(not(unix))]
const FILE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

fn process_lock() -> Arc<Mutex<()>> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

pub struct InterprocessTestLock {
    _process_guard: OwnedMutexGuard<()>,
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

pub async fn acquire_test_lock() -> io::Result<InterprocessTestLock> {
    let process_guard = process_lock().lock_owned().await;
    let path = std::env::temp_dir().join(TEST_LOCK_FILE_NAME);

    #[cfg(unix)]
    let file = tokio::task::spawn_blocking(move || acquire_file_lock(path))
        .await
        .map_err(|error| io::Error::other(error.to_string()))??;

    #[cfg(not(unix))]
    let (file, path) = tokio::task::spawn_blocking(move || acquire_file_lock(path))
        .await
        .map_err(|error| io::Error::other(error.to_string()))??;

    Ok(InterprocessTestLock {
        _process_guard: process_guard,
        file,
        #[cfg(not(unix))]
        path,
    })
}

#[cfg(unix)]
fn acquire_file_lock(path: std::path::PathBuf) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(not(unix))]
fn acquire_file_lock(path: PathBuf) -> io::Result<(File, PathBuf)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let started = Instant::now();
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                thread::sleep(FILE_LOCK_RETRY_DELAY);
                if started.elapsed() >= FILE_LOCK_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "timed out acquiring integration test lock at {}; remove the orphaned lock file if no other test process is using it",
                            path.display()
                        ),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

impl Drop for InterprocessTestLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
