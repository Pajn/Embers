use std::io;
use std::path::PathBuf;

use thiserror::Error;

use super::discover::ConfigOrigin;
use crate::scripting::ScriptError;

pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{origin} config path '{path}' does not exist")]
    MissingConfig { origin: ConfigOrigin, path: PathBuf },
    #[error("{origin} config is missing a canonical path")]
    MissingPath { origin: ConfigOrigin },
    #[error("failed to inspect {origin} config path '{path}': {source}")]
    PathCheck {
        origin: ConfigOrigin,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to canonicalize {origin} config path '{path}': {source}")]
    Canonicalize {
        origin: ConfigOrigin,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read {origin} config file '{path}': {source}")]
    Read {
        origin: ConfigOrigin,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Error)]
pub enum ConfigManagerError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Script(#[from] ScriptError),
}
