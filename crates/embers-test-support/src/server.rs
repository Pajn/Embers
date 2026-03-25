use std::path::Path;
use std::process::Command;
use std::time::Duration;

use embers_core::{Result, init_test_tracing};
use embers_server::{Server, ServerConfig, ServerHandle};
use tempfile::TempDir;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct TestServer {
    socket_path: std::path::PathBuf,
    _tempdir: TempDir,
    handle: Option<ServerHandle>,
}

impl TestServer {
    pub async fn start() -> Result<Self> {
        init_test_tracing();
        reap_stale_helper_processes();

        let tempdir = tempfile::tempdir()?;
        let socket_path = tempdir.path().join("mux.sock");
        let handle = Server::new(ServerConfig::new(socket_path.clone()))
            .start()
            .await?;

        Ok(Self {
            socket_path,
            _tempdir: tempdir,
            handle: Some(handle),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Shuts down the server and kills any orphaned embers helper processes that
    /// were spawned for this socket during the test.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "TestServer shutdown returned error");
                }
                Err(_) => {
                    tracing::warn!("TestServer shutdown timed out after {:?}", SHUTDOWN_TIMEOUT);
                }
            }
        }
        self.kill_orphaned_processes();
        Ok(())
    }

    /// Kill any orphaned embers helper processes that were spawned for this
    /// server's socket but are no longer needed.
    fn kill_orphaned_processes(&self) {
        let socket_path_str = self.socket_path.to_string_lossy();
        let runtime_dir = self.socket_path.with_extension("runtimes");
        let runtime_dir_str = runtime_dir.to_string_lossy();
        let pid_path = self.socket_path.with_extension("pid");

        if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
            && let Ok(pid) = pid_str.trim().parse::<i32>()
        {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }

        if let Ok(output) = Command::new("ps").args(["-eo", "pid,args"]).output() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                let is_server = line.contains("__serve") && line.contains(&*socket_path_str);
                let is_runtime_keeper =
                    line.contains("__runtime-keeper") && line.contains(&*runtime_dir_str);
                if (is_server || is_runtime_keeper)
                    && let Some(pid_str) = line.split_whitespace().next()
                    && let Ok(pid) = pid_str.parse::<i32>()
                {
                    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
                    if is_server {
                        tracing::debug!(pid, "killed orphaned __serve process");
                    } else {
                        tracing::debug!(pid, "killed orphaned __runtime-keeper process");
                    }
                }
            }
        }

        let _ = std::fs::remove_file(&pid_path);
        let _ = std::fs::remove_dir_all(&runtime_dir);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.kill_orphaned_processes();
    }
}

fn reap_stale_helper_processes() {
    if let Ok(output) = Command::new("ps").args(["-eo", "pid,args"]).output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Some((pid, socket_path, helper_kind)) = parse_helper_process(line) else {
                continue;
            };
            let Some(parent) = socket_path.parent() else {
                continue;
            };
            if parent.exists() {
                continue;
            }
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
            tracing::debug!(
                pid,
                helper = helper_kind,
                socket = %socket_path.display(),
                "killed stale helper process"
            );
        }
    }
}

fn parse_helper_process(line: &str) -> Option<(i32, std::path::PathBuf, &'static str)> {
    let line = line.trim();
    let is_server = line.contains("__serve");
    let is_runtime_keeper = line.contains("__runtime-keeper");
    if !is_server && !is_runtime_keeper {
        return None;
    }

    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse::<i32>().ok()?;
    let args = fields.collect::<Vec<_>>();
    let socket_path = args.windows(2).find_map(|window| {
        (window[0] == "--socket").then_some(std::path::PathBuf::from(window[1]))
    })?;
    Some((
        pid,
        socket_path,
        if is_server {
            "__serve"
        } else {
            "__runtime-keeper"
        },
    ))
}
