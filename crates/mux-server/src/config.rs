use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub socket_path: PathBuf,
}

impl ServerConfig {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}
