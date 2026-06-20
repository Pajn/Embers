use std::io;
use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{Span, span};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::{NodeId, RequestId};

/// Filename prefix for the detached server's rolling log files.
pub const SERVER_LOG_FILE_PREFIX: &str = "embers-server";
/// Filename suffix (extension) for the detached server's rolling log files.
pub const SERVER_LOG_FILE_SUFFIX: &str = "log";
/// Number of rotated daily log files to retain before the oldest is pruned.
const SERVER_LOG_MAX_FILES: usize = 7;

static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);
static TRACING: Once = Once::new();
static TEST_TRACING: Once = Once::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestContext {
    pub request_id: RequestId,
}

impl RequestContext {
    pub fn new() -> Self {
        Self {
            request_id: new_request_id(),
        }
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn new_request_id() -> RequestId {
    RequestId(REQUEST_IDS.fetch_add(1, Ordering::Relaxed))
}

pub fn request_span(operation: &str, request_id: RequestId) -> Span {
    span!(
        tracing::Level::INFO,
        "request",
        operation = operation,
        request_id = u64::from(request_id)
    )
}

pub fn init_tracing(default_filter: &str) {
    TRACING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(default_filter))
            .with_target(false)
            .compact()
            .try_init();
    });
}

/// Initializes tracing for the detached server, writing to a daily-rotating log
/// file in `directory` (named `embers-server.<date>.log`) and retaining the most
/// recent [`SERVER_LOG_MAX_FILES`] days. ANSI styling is disabled so the file
/// stays plain text.
///
/// Returns an error if the rotating appender can't be created. The detached
/// server's stdio is discarded, so the log file is its only usable sink; failing
/// startup loudly is preferable to running a daemon whose logs — and the panic
/// traces this hardening installs — silently vanish into the dead stderr.
pub fn init_server_tracing(default_filter: &str, directory: &Path) -> io::Result<()> {
    let mut result = Ok(());
    TRACING.call_once(|| {
        match RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(SERVER_LOG_FILE_PREFIX)
            .filename_suffix(SERVER_LOG_FILE_SUFFIX)
            .max_log_files(SERVER_LOG_MAX_FILES)
            .build(directory)
        {
            Ok(appender) => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new(default_filter))
                    .with_target(false)
                    .with_ansi(false)
                    .compact()
                    .with_writer(appender)
                    .try_init();
            }
            Err(error) => result = Err(io::Error::other(error.to_string())),
        }
    });
    result
}

pub fn init_test_tracing() {
    TEST_TRACING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("debug"))
            .with_target(false)
            .compact()
            .with_test_writer()
            .try_init();
    });
}

pub fn format_focus_path(path: &[NodeId]) -> String {
    let joined = path
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ");
    format!("focus: {joined}")
}

pub fn format_tree_dump<I, S>(title: &str, lines: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut output = String::from(title);
    for line in lines {
        output.push('\n');
        output.push_str(line.as_ref());
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::NodeId;

    use super::{format_focus_path, format_tree_dump};

    #[test]
    fn focus_path_is_human_readable() {
        assert_eq!(format_focus_path(&[NodeId(1), NodeId(7)]), "focus: 1 -> 7");
    }

    #[test]
    fn tree_dump_puts_title_on_first_line() {
        let dump = format_tree_dump("session", ["root tabs", "leaf 1"]);
        assert_eq!(dump, "session\nroot tabs\nleaf 1");
    }
}
