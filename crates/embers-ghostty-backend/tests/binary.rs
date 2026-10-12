//! `embers-ghostty` runs Embers with panes emulated by Ghostty, on a server of
//! its own.

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn embers_ghostty(runtime_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_embers-ghostty"))
        .env("XDG_RUNTIME_DIR", runtime_dir)
        // As inside a pane of another Embers server.
        .env("EMBERS_SOCKET", runtime_dir.join("other.sock"))
        .args(args)
        .output()
        .expect("embers-ghostty runs")
}

/// Kills the server when the test ends, passed or not.
struct Server<'a>(&'a Path);

impl Drop for Server<'_> {
    fn drop(&mut self) {
        let _ = embers_ghostty(self.0, &["kill-server"]);
    }
}

#[test]
fn panes_are_emulated_by_ghostty_on_a_server_of_its_own() {
    let runtime_dir = tempfile::tempdir().expect("temporary runtime dir");
    let runtime_dir = runtime_dir.path();
    let _server = Server(runtime_dir);

    // DECALN fills the screen with E and, in Ghostty but not alacritty, homes
    // the cursor, so the `b` after it lands in the first column.
    let created = embers_ghostty(
        runtime_dir,
        &[
            "new-session",
            "-d",
            "-s",
            "t",
            "-x",
            "12",
            "-y",
            "3",
            "--",
            "sh",
            "-c",
            "printf 'a\\033#8b'; sleep 30",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let screen = loop {
        let captured = embers_ghostty(runtime_dir, &["capture-pane", "-p", "-t", "t"]);
        let screen = String::from_utf8_lossy(&captured.stdout).into_owned();
        if screen.contains('E') || Instant::now() > deadline {
            break screen;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(screen.lines().next(), Some("bEEEEEEEEEEE"), "{screen}");

    let socket = runtime_dir.join("embers").join("embers-ghostty.sock");
    assert!(socket.exists(), "no socket at {}", socket.display());
    assert!(!runtime_dir.join("other.sock").exists());
}
