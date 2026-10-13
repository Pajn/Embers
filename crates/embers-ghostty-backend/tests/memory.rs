//! How much memory a pane costs in each backend. A measurement, run on demand,
//! one backend and kind of content per process so neither sees the other's
//! freed memory:
//!
//! ```sh
//! for backend in alacritty ghostty ghostty-compressed; do
//!   for content in empty plain styled alternate; do
//!     MEMORY_CASE=$backend:$content cargo test -p embers-ghostty-backend --release --test memory -- --ignored --nocapture
//!   done
//! done
//! ```

use std::process::Command;

use embers_core::PtySize;
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{AlacrittyTerminalBackend, TerminalBackend};

/// Panes per measurement; the cost of one is the growth over all of them
/// divided by this.
const PANES: usize = 40;
/// Embers' default scrollback ceiling.
const SCROLLBACK_LINES: usize = 10_000;

/// The process's resident memory, in KiB.
fn resident_kib() -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("ps reports a number")
}

/// The memory macOS charges the process, in KiB: unlike RSS it leaves out
/// memory handed back with `MADV_FREE`, which libghostty frees compressed
/// pages with. `None` elsewhere.
fn footprint_kib() -> Option<u64> {
    let output = Command::new("footprint")
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    let report = String::from_utf8_lossy(&output.stdout);
    let rest = report.split("Footprint: ").nth(1)?;
    let mut words = rest.split_whitespace();
    let amount: f64 = words.next()?.parse().ok()?;
    let kib = match words.next()? {
        "B" => amount / 1024.0,
        "KB" => amount,
        "MB" => amount * 1024.0,
        "GB" => amount * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(kib as u64)
}

/// What a pane is given: `empty`, a prompt; `plain` and `styled`, enough
/// lines to fill the scrollback, without and with style changes; and
/// `alternate`, a full-screen program on the alternate screen.
fn output(content: &str) -> Vec<u8> {
    let mut output = Vec::new();
    match content {
        "empty" => output.extend_from_slice(b"user@host ~ % "),
        "plain" | "styled" => {
            for line in 0..SCROLLBACK_LINES + 100 {
                let text = if content == "plain" {
                    format!(
                        "{line:>6} some ordinary output of a build or a log, to fill a line\r\n"
                    )
                } else {
                    format!(
                        "\x1b[3{}m{line:>6}\x1b[0m \x1b[1msome\x1b[0m \x1b[4mordinary\x1b[0m \
                         \x1b[38;5;{}moutput\x1b[0m of a \x1b[7mbuild\x1b[0m or a log\r\n",
                        line % 8,
                        line % 256
                    )
                };
                output.extend_from_slice(text.as_bytes());
            }
        }
        "alternate" => {
            output.extend_from_slice(b"\x1b[?1049h");
            for row in 1..=50 {
                output.extend_from_slice(
                    format!("\x1b[{row};1H\x1b[4{}m{}\x1b[0m", row % 8, "x".repeat(200)).as_bytes(),
                );
            }
        }
        other => panic!("unknown content {other}"),
    }
    output
}

#[test]
#[ignore = "a measurement, not a check"]
fn memory_per_pane() {
    let Ok(case) = std::env::var("MEMORY_CASE") else {
        eprintln!("set MEMORY_CASE=<backend>:<content>");
        return;
    };
    let (backend, content) = case.split_once(':').expect("<backend>:<content>");
    let size = PtySize::new(200, 50);
    let output = output(content);

    let before = resident_kib();
    let footprint_before = footprint_kib();
    let mut panes: Vec<Box<dyn TerminalBackend>> = Vec::new();
    let mut ghostty_bytes = 0;
    for _ in 0..PANES {
        let pane: Box<dyn TerminalBackend> = match backend {
            "alacritty" => {
                let mut pane = AlacrittyTerminalBackend::new(size, SCROLLBACK_LINES);
                pane.ingest_bytes(&output);
                Box::new(pane)
            }
            "ghostty" | "ghostty-compressed" => {
                let mut pane = GhosttyTerminalBackend::new(size, SCROLLBACK_LINES);
                pane.ingest_bytes(&output);
                if backend == "ghostty-compressed" {
                    assert!(pane.compress_scrollback(), "compression is unsupported");
                }
                ghostty_bytes += pane.memory_usage().resident_bytes;
                Box::new(pane)
            }
            other => panic!("unknown backend {other}"),
        };
        panes.push(pane);
    }
    let after = resident_kib();
    let footprint = footprint_kib()
        .zip(footprint_before)
        .map(|(after, before)| {
            format!(
                ", footprint {:.2} MiB",
                after.saturating_sub(before) as f64 / PANES as f64 / 1024.0
            )
        })
        .unwrap_or_default();

    let per_pane = (after.saturating_sub(before)) as f64 / PANES as f64 / 1024.0;
    let lines = panes[0].metadata().total_lines;
    let counted = if ghostty_bytes > 0 {
        format!(
            "  (libghostty counts {:.2} MiB)",
            ghostty_bytes as f64 / PANES as f64 / 1024.0 / 1024.0
        )
    } else {
        String::new()
    };
    println!(
        "MEMORY {backend:>18} {content:>9}: RSS {per_pane:6.2} MiB per pane{footprint}, \
         {lines} lines{counted}"
    );
}
