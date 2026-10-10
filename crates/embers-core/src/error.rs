use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Unknown,
    InvalidRequest,
    ProtocolViolation,
    Transport,
    NotFound,
    Conflict,
    Unsupported,
    Timeout,
    Internal,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Unknown => "unknown",
            Self::InvalidRequest => "invalid-request",
            Self::ProtocolViolation => "protocol-violation",
            Self::Transport => "transport",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::Unsupported => "unsupported",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        };

        formatter.write_str(label)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct WireError {
    pub code: ErrorCode,
    pub message: String,
}

impl WireError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum MuxError {
    #[error("{0}")]
    Wire(#[from] WireError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("pty error: {0}")]
    Pty(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, MuxError>;

impl MuxError {
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
    }

    pub fn pty(message: impl Into<String>) -> Self {
        Self::Pty(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Whether this says that what was asked about does not exist, raised
    /// here or reported by the server.
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::NotFound(_) => true,
            Self::Wire(error) => error.code == ErrorCode::NotFound,
            _ => false,
        }
    }

    /// Whether this is a conflict with the current state, raised here or
    /// reported by the server.
    pub fn is_conflict(&self) -> bool {
        match self {
            Self::Conflict(_) => true,
            Self::Wire(error) => error.code == ErrorCode::Conflict,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, MuxError, WireError};

    #[test]
    fn not_found_and_conflict_are_recognised_from_the_server_too() {
        assert!(MuxError::not_found("gone").is_not_found());
        assert!(MuxError::from(WireError::new(ErrorCode::NotFound, "gone")).is_not_found());
        assert!(!MuxError::from(WireError::new(ErrorCode::Conflict, "busy")).is_not_found());
        assert!(MuxError::from(WireError::new(ErrorCode::Conflict, "busy")).is_conflict());
        assert!(MuxError::conflict("busy").is_conflict());
    }

    #[test]
    fn wire_error_renders_with_code() {
        let error = WireError::new(ErrorCode::InvalidRequest, "missing payload");

        assert_eq!(error.to_string(), "invalid-request: missing payload");
    }
}
