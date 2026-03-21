use std::path::Path;

use rhai::{EvalAltResult, ParseError, Position};
use thiserror::Error;

use crate::config::LoadedConfigSource;

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("failed to compile config '{path}'{location}: {message}")]
    Compile {
        path: String,
        location: String,
        message: String,
    },
    #[error("failed to evaluate config '{path}'{location}: {message}")]
    Runtime {
        path: String,
        location: String,
        message: String,
    },
    #[error("config '{path}' is invalid{location}: {message}")]
    Validation {
        path: String,
        location: String,
        message: String,
    },
}

impl ScriptError {
    pub fn compile(source: &LoadedConfigSource, error: ParseError) -> Self {
        Self::Compile {
            path: source_path(source),
            location: format_location(error.position()),
            message: error.to_string(),
        }
    }

    pub fn runtime(source: &LoadedConfigSource, error: Box<EvalAltResult>) -> Self {
        Self::runtime_path(source.path.as_deref(), error)
    }

    pub fn validation(
        source: &LoadedConfigSource,
        position: Position,
        message: impl Into<String>,
    ) -> Self {
        Self::validation_path(source.path.as_deref(), position, message)
    }

    pub fn runtime_path(path: Option<&Path>, error: Box<EvalAltResult>) -> Self {
        let position = error.position();
        Self::Runtime {
            path: format_path(path),
            location: format_location(position),
            message: error.to_string(),
        }
    }

    pub fn validation_path(
        path: Option<&Path>,
        position: Position,
        message: impl Into<String>,
    ) -> Self {
        Self::Validation {
            path: format_path(path),
            location: format_location(position),
            message: message.into(),
        }
    }
}

fn source_path(source: &LoadedConfigSource) -> String {
    format_path(source.path.as_deref())
}

fn format_path(path: Option<&Path>) -> String {
    path.unwrap_or_else(|| Path::new("<built-in>"))
        .display()
        .to_string()
}

fn format_location(position: Position) -> String {
    if position.is_none() {
        return String::new();
    }

    let line = position.line().unwrap_or(0);
    match position.position() {
        Some(column) => format!(" at {line}:{column}"),
        None => format!(" at {line}"),
    }
}
