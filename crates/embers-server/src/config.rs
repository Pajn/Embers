use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub const SOCKET_ENV_VAR: &str = "EMBERS_SOCKET";

/// Environment variable overriding [`ResourceLimits::max_sessions`].
pub const MAX_SESSIONS_ENV_VAR: &str = "EMBERS_MAX_SESSIONS";
/// Environment variable overriding [`ResourceLimits::max_buffers`].
pub const MAX_BUFFERS_ENV_VAR: &str = "EMBERS_MAX_BUFFERS";
/// Environment variable overriding [`ResourceLimits::max_scrollback_lines`].
///
/// This is read by the runtime keeper process when constructing a terminal
/// backend, which inherits the server's environment, so a single value applies
/// to both processes.
pub const MAX_SCROLLBACK_LINES_ENV_VAR: &str = "EMBERS_MAX_SCROLLBACK_LINES";

/// Default ceiling on concurrently live sessions.
pub const DEFAULT_MAX_SESSIONS: usize = 256;
/// Default ceiling on concurrently live buffers. Each buffer owns a PTY-backed
/// child process plus scrollback, so this is the dominant resource bound.
pub const DEFAULT_MAX_BUFFERS: usize = 2048;
/// Default scrollback retained per buffer. Combined with [`DEFAULT_MAX_BUFFERS`]
/// this bounds worst-case server memory.
pub const DEFAULT_MAX_SCROLLBACK_LINES: usize = 10_000;

/// Operator-tunable ceilings that prevent a client from exhausting server
/// resources by creating unbounded sessions, buffers, or scrollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_sessions: usize,
    pub max_buffers: usize,
    pub max_scrollback_lines: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_MAX_SESSIONS,
            max_buffers: DEFAULT_MAX_BUFFERS,
            max_scrollback_lines: DEFAULT_MAX_SCROLLBACK_LINES,
        }
    }
}

impl ResourceLimits {
    /// Build limits from defaults, applying any environment-variable overrides.
    /// A value of `0` or an unparseable value falls back to the default.
    pub fn from_env() -> Self {
        let mut limits = Self::default();
        if let Some(value) = parse_limit_env(MAX_SESSIONS_ENV_VAR) {
            limits.max_sessions = value;
        }
        if let Some(value) = parse_limit_env(MAX_BUFFERS_ENV_VAR) {
            limits.max_buffers = value;
        }
        if let Some(value) = parse_limit_env(MAX_SCROLLBACK_LINES_ENV_VAR) {
            limits.max_scrollback_lines = value;
        }
        limits
    }
}

/// Read a positive `usize` from `var`. Returns `None` when unset, empty, zero,
/// or unparseable, so callers keep their default.
fn parse_limit_env(var: &str) -> Option<usize> {
    std::env::var(var)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub socket_path: PathBuf,
    pub workspace_path: PathBuf,
    pub runtime_dir: PathBuf,
    pub buffer_env: BTreeMap<String, OsString>,
    pub limits: ResourceLimits,
}

impl ServerConfig {
    pub fn new(socket_path: PathBuf) -> Self {
        let mut buffer_env = BTreeMap::new();
        buffer_env.insert(
            SOCKET_ENV_VAR.to_owned(),
            socket_path.as_os_str().to_owned(),
        );
        let workspace_path = socket_path.with_extension("workspace.json");
        let runtime_dir = socket_path.with_extension("runtimes");
        Self {
            socket_path,
            workspace_path,
            runtime_dir,
            buffer_env,
            limits: ResourceLimits::from_env(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_limits_default_to_documented_constants() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_sessions, DEFAULT_MAX_SESSIONS);
        assert_eq!(limits.max_buffers, DEFAULT_MAX_BUFFERS);
        assert_eq!(limits.max_scrollback_lines, DEFAULT_MAX_SCROLLBACK_LINES);
    }

    #[test]
    fn parse_limit_env_rejects_zero_empty_and_garbage() {
        // SAFETY: single-threaded test; we set and remove the var within it.
        let var = "EMBERS_TEST_PARSE_LIMIT_ENV";
        for value in ["0", "", "  ", "nope", "-5"] {
            unsafe { std::env::set_var(var, value) };
            assert_eq!(
                parse_limit_env(var),
                None,
                "value {value:?} should be rejected"
            );
        }
        unsafe { std::env::set_var(var, " 42 ") };
        assert_eq!(parse_limit_env(var), Some(42));
        unsafe { std::env::remove_var(var) };
        assert_eq!(parse_limit_env(var), None);
    }
}
