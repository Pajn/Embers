use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use embers_core::{Result, init_test_tracing};
use embers_server::{SOCKET_ENV_VAR, Server, ServerConfig, ServerHandle};
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
        Self::start_with(|_| {}).await
    }

    /// Start a server whose configuration `configure` adjusts first.
    pub async fn start_with(configure: impl FnOnce(&mut ServerConfig)) -> Result<Self> {
        init_test_tracing();
        reap_stale_helper_processes();

        let tempdir = tempfile::tempdir()?;
        let socket_path = tempdir.path().join("mux.sock");
        let mut config = ServerConfig::new(socket_path.clone());
        configure(&mut config);
        let handle = Server::new(config).start().await?;

        Ok(Self {
            socket_path,
            _tempdir: tempdir,
            handle: Some(handle),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Shuts down the server and kills any orphaned embers helper processes that
    /// were spawned for this socket during the test.
    pub async fn shutdown(mut self) -> Result<()> {
        let mut shutdown_error = None;
        if let Some(handle) = self.handle.take() {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "TestServer shutdown returned error");
                    shutdown_error = Some(e);
                }
                Err(_) => {
                    tracing::warn!("TestServer shutdown timed out after {:?}", SHUTDOWN_TIMEOUT);
                    shutdown_error = Some(embers_core::MuxError::timeout(format!(
                        "TestServer shutdown timed out after {:?}",
                        SHUTDOWN_TIMEOUT
                    )));
                }
            }
        }
        self.kill_orphaned_processes();
        match shutdown_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Kill any orphaned embers helper processes that were spawned for this
    /// server's socket but are no longer needed.
    fn kill_orphaned_processes(&self) {
        let socket_path_str = self.socket_path.to_string_lossy();
        let runtime_dir = self.socket_path.with_extension("runtimes");
        let pid_path = self.socket_path.with_extension("pid");
        let orphaned = collect_orphaned_processes(
            &socket_path_str,
            &keeper_server_socket_marker(&self.socket_path),
        );
        let mut handled = Vec::new();

        if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
            && let Ok(pid) = pid_str.trim().parse::<i32>()
            && orphaned
                .iter()
                .any(|(candidate_pid, _)| *candidate_pid == pid)
        {
            terminate_process(pid);
            handled.push(pid);
        }

        for (pid, kind) in orphaned {
            if handled.contains(&pid) {
                continue;
            }
            terminate_process(pid);
            if kind == "server" {
                tracing::debug!(pid, "killed orphaned __serve process");
            } else {
                tracing::debug!(pid, "killed orphaned __runtime-keeper process");
            }
        }

        let _ = std::fs::remove_file(&pid_path);
        let _ = std::fs::remove_dir_all(&runtime_dir);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.kill_orphaned_processes();
    }
}

/// The server passes its own socket path to every buffer it spawns through
/// `EMBERS_SOCKET`, which the keeper receives base64-encoded on its command
/// line. Matching on that ties a keeper to its server wherever the keeper's own
/// socket lives.
fn keeper_server_socket_marker(socket_path: &Path) -> String {
    format!(
        "{SOCKET_ENV_VAR}=base64:{}",
        base64::engine::general_purpose::STANDARD.encode(socket_path.as_os_str().as_bytes())
    )
}

fn collect_orphaned_processes(socket_path: &str, keeper_marker: &str) -> Vec<(i32, &'static str)> {
    let mut matches = Vec::new();
    if let Ok(output) = Command::new("ps").args(["-eo", "pid,args"]).output() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            let is_server = line.contains("__serve") && line.contains(socket_path);
            let is_runtime_keeper =
                line.contains("__runtime-keeper") && line.contains(keeper_marker);
            if (is_server || is_runtime_keeper)
                && let Some(pid_str) = line.split_whitespace().next()
                && let Ok(pid) = pid_str.parse::<i32>()
            {
                matches.push((
                    pid,
                    if is_server {
                        "server"
                    } else {
                        "runtime_keeper"
                    },
                ));
            }
        }
    }
    matches
}

fn terminate_process(pid: i32) {
    let _ = Command::new("kill").arg(pid.to_string()).output();
    std::thread::sleep(Duration::from_millis(50));
    if process_exists(pid) {
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
    }
}

fn process_exists(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Stop detached `__serve` processes left behind by earlier tests whose temp
/// directory is gone, for example after a test panicked before its cleanup.
///
/// Only servers run from this build's `embers` binary are considered, so
/// servers of other checkouts or a real installation are never touched, and
/// only once their socket directory no longer exists, which a running test
/// keeps alive. Runtime keepers need no reaping here: a keeper whose socket
/// was removed with its directory stops its child and exits by itself.
///
/// Outside the Embers workspace there is no such binary, and `TestServer`
/// runs its keepers in-process, so there is nothing to reap.
fn reap_stale_helper_processes() {
    let current_target_dir = std::env::current_exe().ok().and_then(|exe| {
        let dir = exe.parent()?;
        Some(if dir.ends_with("deps") {
            dir.parent()?.to_path_buf()
        } else {
            dir.to_path_buf()
        })
    });
    let Some(binary) = embers_binary(std::env::var_os("CARGO_BIN_EXE_embers"), current_target_dir)
    else {
        return;
    };
    let Ok(output) = Command::new("ps").args(["-eo", "pid,args"]).output() else {
        return;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((pid, socket_path)) = stale_server_candidate(line, &binary) else {
            continue;
        };
        if socket_path.parent().is_none_or(Path::exists) {
            continue;
        }
        terminate_process(pid);
        tracing::debug!(
            pid,
            socket = %socket_path.display(),
            "stopped stale __serve process"
        );
    }
}

/// The `embers` binary of this build: `CARGO_BIN_EXE_embers` when Cargo sets
/// it, otherwise an `embers` binary in the test's target directory, the same
/// lookup `assert_cmd` uses. `None` when neither exists, instead of the panic
/// `assert_cmd::cargo::cargo_bin` gives.
fn embers_binary(
    cargo_bin_exe: Option<std::ffi::OsString>,
    target_dir: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if let Some(path) = cargo_bin_exe {
        return Some(path.into());
    }
    let built = target_dir?.join(format!("embers{}", std::env::consts::EXE_SUFFIX));
    built.exists().then_some(built)
}

/// Parse a `ps -eo pid,args` line into the pid and `--socket` path of a
/// `__serve` process run from `binary`. Anything else, including runtime
/// keepers and servers run from other binaries, yields `None`.
fn stale_server_candidate(line: &str, binary: &Path) -> Option<(i32, std::path::PathBuf)> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse::<i32>().ok()?;
    let args = fields.collect::<Vec<_>>();
    if Path::new(args.first()?) != binary || !args.contains(&"__serve") {
        return None;
    }
    let socket_path = args.windows(2).find_map(|window| {
        (window[0] == "--socket").then_some(std::path::PathBuf::from(window[1]))
    })?;
    Some((pid, socket_path))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{embers_binary, stale_server_candidate};

    const BINARY: &str = "/build/target/debug/embers";

    #[test]
    fn finds_no_binary_outside_the_embers_workspace() {
        let target_dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            embers_binary(None, Some(target_dir.path().to_path_buf())),
            None
        );
        assert_eq!(embers_binary(None, None), None);
    }

    #[test]
    fn prefers_cargo_bin_exe_then_the_built_binary() {
        let target_dir = tempfile::tempdir().expect("tempdir");
        let built = target_dir
            .path()
            .join(format!("embers{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&built, b"").expect("write binary");
        assert_eq!(
            embers_binary(
                Some("/cargo/bin/embers".into()),
                Some(target_dir.path().to_path_buf())
            ),
            Some(PathBuf::from("/cargo/bin/embers"))
        );
        assert_eq!(
            embers_binary(None, Some(target_dir.path().to_path_buf())),
            Some(built)
        );
    }

    #[test]
    fn matches_servers_run_from_this_build() {
        assert_eq!(
            stale_server_candidate(
                "  4242 /build/target/debug/embers __serve --socket /tmp/run/mux.sock",
                Path::new(BINARY),
            ),
            Some((4242, PathBuf::from("/tmp/run/mux.sock")))
        );
    }

    #[test]
    fn ignores_servers_run_from_other_binaries() {
        for line in [
            "4242 /other/target/debug/embers __serve --socket /tmp/run/mux.sock",
            "4242 embers __serve --socket /tmp/run/mux.sock",
        ] {
            assert_eq!(
                stale_server_candidate(line, Path::new(BINARY)),
                None,
                "{line}"
            );
        }
    }

    #[test]
    fn ignores_runtime_keepers_and_other_processes() {
        for line in [
            "4242 /build/target/debug/embers __runtime-keeper --keeper-socket /tmp/run/mux.runtimes/buffer-1.sock --cols 80 --rows 24 -- /bin/sh",
            "4242 /build/target/debug/embers --socket /tmp/run/mux.sock list-sessions",
            "4242 /bin/sh -c echo __serve --socket /tmp/run/mux.sock",
            "PID ARGS",
        ] {
            assert_eq!(
                stale_server_candidate(line, Path::new(BINARY)),
                None,
                "{line}"
            );
        }
    }
}
