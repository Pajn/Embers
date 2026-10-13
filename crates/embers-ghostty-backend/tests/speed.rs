//! How long a snapshot takes in each backend, on a full screen of styled
//! text. Run on demand: `cargo test -p embers-ghostty-backend --release --test speed -- --ignored
//! --nocapture`.

use std::time::Instant;

use embers_core::PtySize;
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{AlacrittyTerminalBackend, TerminalBackend};

fn time(name: &str, backend: &mut dyn TerminalBackend) {
    let size = PtySize::new(200, 50);
    let mut output = Vec::new();
    for line in 0..2_000 {
        output.extend_from_slice(
            format!(
                "\x1b[3{}m{line:>6} 漢字 some text \x1b[1msome bold\x1b[0m\r\n",
                line % 8
            )
            .repeat(3)
            .as_bytes(),
        );
    }
    let start = Instant::now();
    backend.ingest_bytes(&output);
    let ingest = start.elapsed();

    let rounds = 200;
    let start = Instant::now();
    for sequence in 0..rounds {
        std::hint::black_box(backend.visible_snapshot(sequence, size, None));
    }
    let snapshot = start.elapsed() / rounds as u32;

    let start = Instant::now();
    std::hint::black_box(backend.capture_scrollback());
    let capture = start.elapsed();
    println!(
        "{name:9} ingest {:>5} KB: {ingest:>10.2?}   snapshot 200x50: {snapshot:>10.2?}   \
         capture {} lines: {capture:>10.2?}",
        output.len() / 1024,
        backend.metadata().total_lines,
    );
}

#[test]
#[ignore = "a measurement, not a check"]
fn snapshot_speed() {
    let size = PtySize::new(200, 50);
    time(
        "alacritty",
        &mut AlacrittyTerminalBackend::new(size, 10_000),
    );
    time("ghostty", &mut GhosttyTerminalBackend::new(size, 10_000));
}
