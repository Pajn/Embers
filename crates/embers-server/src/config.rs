use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use embers_core::BufferId;

use crate::model::RemainOnExit;
use crate::runtime_dir::keeper_socket_path;

pub const SOCKET_ENV_VAR: &str = "EMBERS_SOCKET";
/// Environment variable set in every buffer's environment to the buffer's
/// id, like tmux's `TMUX_PANE`, so a program can name its own pane: `embers`
/// commands run inside a pane without a target act on that pane.
pub const BUFFER_ENV_VAR: &str = "EMBERS_BUFFER";

/// `TERM` advertised to buffer child processes. Embers ships no terminfo of its
/// own; alacritty's emulation is closest to xterm and `xterm-256color` exists
/// everywhere. A user-supplied `TERM` in the buffer spawn env still overrides
/// this (hints are merged after the base env). The entry lacks `Smulx`,
/// `Setulc` and `Ms`; programs that probe the terminal (DECRQSS, XTGETTCAP)
/// learn them from the keeper's answers instead (see `RawByteRouter`).
pub const TERM_ENV_VAR: &str = "TERM";
/// Default `TERM` value injected into buffer children.
pub const DEFAULT_TERM: &str = "xterm-256color";
/// `COLORTERM` advertised to buffer child processes. Truthful at the parser
/// level: alacritty accepts 24-bit SGR sequences.
pub const COLORTERM_ENV_VAR: &str = "COLORTERM";
/// Default `COLORTERM` value injected into buffer children.
pub const DEFAULT_COLORTERM: &str = "truecolor";

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

/// Environment variable setting the server-wide default of the
/// `remain-on-exit` buffer option (`on`, `failed` or `off`), for buffers that
/// do not set it themselves.
pub const REMAIN_ON_EXIT_ENV_VAR: &str = "EMBERS_REMAIN_ON_EXIT";

/// The server-wide `remain-on-exit` default from [`REMAIN_ON_EXIT_ENV_VAR`];
/// off when unset.
fn remain_on_exit_from_env() -> RemainOnExit {
    std::env::var(REMAIN_ON_EXIT_ENV_VAR)
        .map(|value| RemainOnExit::parse(&value))
        .unwrap_or_default()
}

/// Environment variable turning restore of panes whose keeper is gone on or off.
pub const RESTORE_ENV_VAR: &str = "EMBERS_RESTORE";
/// Environment variable replacing [`DEFAULT_RESTORE_COMMANDS`].
pub const RESTORE_COMMANDS_ENV_VAR: &str = "EMBERS_RESTORE_COMMANDS";

/// Programs that are run again when a pane is restored, matched against the
/// name of the program the pane last had in the foreground. The same safe
/// default as tmux-resurrect: viewers, editors and monitors that are harmless
/// to start twice.
pub const DEFAULT_RESTORE_COMMANDS: &[&str] = &[
    "vi", "vim", "nvim", "emacs", "man", "less", "more", "tail", "top", "htop", "irssi", "weechat",
    "mutt",
];

/// How the server brings back panes whose runtime keeper is gone, for example
/// after a reboot. Panes whose keeper is still alive are always reattached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreConfig {
    /// Start a fresh process for a pane whose keeper is gone. When off, such a
    /// pane is left `Interrupted`.
    pub enabled: bool,
    /// Program names that are run again in a restored pane. Anything else
    /// restores as a plain shell.
    pub commands: Vec<String>,
}

impl Default for RestoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            commands: DEFAULT_RESTORE_COMMANDS
                .iter()
                .map(|command| (*command).to_owned())
                .collect(),
        }
    }
}

impl RestoreConfig {
    /// Build the restore settings from defaults, applying any environment
    /// overrides. `EMBERS_RESTORE` set to `0`, `off`, `false` or `no` turns
    /// restore off. `EMBERS_RESTORE_COMMANDS` replaces the allowlist with its
    /// comma- or whitespace-separated names; set but empty, no program is run
    /// again.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = std::env::var(RESTORE_ENV_VAR) {
            config.enabled = !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            );
        }
        if let Ok(value) = std::env::var(RESTORE_COMMANDS_ENV_VAR) {
            config.commands = parse_command_list(&value);
        }
        config
    }
}

fn parse_command_list(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
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
    pub restore: RestoreConfig,
    /// Whether exited buffers that do not set the `remain-on-exit` option
    /// stay in their views.
    pub remain_on_exit: RemainOnExit,
}

impl ServerConfig {
    pub fn new(socket_path: PathBuf) -> Self {
        let mut buffer_env = BTreeMap::new();
        buffer_env.insert(
            SOCKET_ENV_VAR.to_owned(),
            socket_path.as_os_str().to_owned(),
        );
        // Base terminal env for buffer children. User env hints are merged after
        // this base (see `Server::spawn_buffer_runtime`), so a caller-specified
        // TERM/COLORTERM still wins.
        buffer_env.insert(TERM_ENV_VAR.to_owned(), OsString::from(DEFAULT_TERM));
        buffer_env.insert(
            COLORTERM_ENV_VAR.to_owned(),
            OsString::from(DEFAULT_COLORTERM),
        );
        let workspace_path = socket_path.with_extension("workspace.json");
        let runtime_dir = socket_path.with_extension("runtimes");
        Self {
            socket_path,
            workspace_path,
            runtime_dir,
            buffer_env,
            limits: ResourceLimits::from_env(),
            restore: RestoreConfig::from_env(),
            remain_on_exit: remain_on_exit_from_env(),
        }
    }

    /// Path of the runtime keeper socket for `buffer_id`: inside
    /// [`Self::runtime_dir`] when that fits the Unix socket path limit,
    /// otherwise inside a short private directory under `/tmp/embers-<uid>/`.
    pub fn keeper_socket_path(&self, buffer_id: BufferId) -> PathBuf {
        keeper_socket_path(&self.socket_path, &self.runtime_dir, buffer_id)
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
    fn restore_command_list_splits_on_commas_and_whitespace() {
        assert_eq!(
            parse_command_list(" vim, less\ttail ,,"),
            vec!["vim".to_owned(), "less".to_owned(), "tail".to_owned()]
        );
        assert!(parse_command_list("").is_empty());
        assert!(parse_command_list(" , ").is_empty());
    }

    #[test]
    fn remain_on_exit_values_parse_like_the_buffer_option() {
        assert_eq!(RemainOnExit::parse("on"), RemainOnExit::On);
        assert_eq!(RemainOnExit::parse(" Failed "), RemainOnExit::Failed);
        assert_eq!(RemainOnExit::parse("off"), RemainOnExit::Off);
        assert_eq!(RemainOnExit::parse("yes"), RemainOnExit::Off);
    }

    #[test]
    fn restore_defaults_to_enabled_with_the_safe_allowlist() {
        let config = RestoreConfig::default();
        assert!(config.enabled);
        assert!(config.commands.iter().any(|command| command == "vim"));
        assert!(!config.commands.iter().any(|command| command == "sh"));
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
