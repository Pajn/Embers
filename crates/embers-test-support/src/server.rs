use std::path::Path;

use embers_core::{Result, init_test_tracing};
use embers_server::{Server, ServerConfig, ServerHandle};
use tempfile::TempDir;

#[derive(Debug)]
pub struct TestServer {
    socket_path: std::path::PathBuf,
    tempdir: TempDir,
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
            tempdir,
            handle: Some(handle),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.tempdir.path();
        if let Some(handle) = self.handle.take() {
            handle.shutdown().await
        } else {
            Ok(())
        }
    }
}
