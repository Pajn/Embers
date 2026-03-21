use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub const SOCKET_ENV_VAR: &str = "EMBERS_SOCKET";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub socket_path: PathBuf,
    pub buffer_env: BTreeMap<String, OsString>,
}

impl ServerConfig {
    pub fn new(socket_path: PathBuf) -> Self {
        let mut buffer_env = BTreeMap::new();
        buffer_env.insert(
            SOCKET_ENV_VAR.to_owned(),
            socket_path.as_os_str().to_owned(),
        );
        Self {
            socket_path,
            buffer_env,
        }
    }
}
