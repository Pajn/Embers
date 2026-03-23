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

    /// Shuts down the server and kills any orphaned __serve processes
    /// that were spawned for this socket during the test.
    pub async fn shutdown(mut self) -> Result<()> {
        // First, kill any orphaned __serve processes for our socket
        self.kill_orphaned_servers();

        // Then shutdown our own server with a timeout
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
        Ok(())
    }

    /// Kill any orphaned embers __serve processes that were spawned
    /// for this server's socket but are no longer needed.
    fn kill_orphaned_servers(&self) {
        let socket_path_str = self.socket_path.to_string_lossy();
        let pid_path = self.socket_path.with_extension("pid");

        // First try to kill via PID file (for __serve processes)
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
            && let Ok(pid) = pid_str.trim().parse::<i32>()
        {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }

        // Also try to find and kill any __serve processes referencing our socket
        // This handles cases where the PID file wasn't cleaned up or we need
        // to find the process by socket path
        if let Ok(output) = Command::new("ps").args(["-eo", "pid,args"]).output() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                // Look for __serve processes with our socket path
                if line.contains("__serve")
                    && line.contains(&*socket_path_str)
                    && let Some(pid_str) = line.split_whitespace().next()
                    && let Ok(pid) = pid_str.parse::<i32>()
                {
                    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
                    tracing::debug!(pid, "killed orphaned __serve process");
                }
            }
        }

        // Clean up the pid file
        let _ = std::fs::remove_file(&pid_path);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Ensure any spawned servers are killed even if shutdown wasn't called
        self.kill_orphaned_servers();
    }
}
