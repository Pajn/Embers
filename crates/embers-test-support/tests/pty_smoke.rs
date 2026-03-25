use std::time::Duration;

use crate::support::integration_test_lock;
use embers_core::PtySize;
use embers_test_support::PtyHarness;

#[test]
#[ignore = "exercises the PTY smoke harness in CI and later end-to-end runs"]
fn pty_round_trips_input() {
    let _guard = integration_test_lock().blocking_lock();
    let mut harness = PtyHarness::spawn(
        "sh",
        &["-lc", "read line; printf '%s' \"$line\""],
        PtySize::new(80, 24),
    )
    .expect("spawn PTY process");
    harness.write_all("phase0-pty\n").expect("write input");

    let output = harness
        .read_until_contains("phase0-pty", Duration::from_secs(3))
        .expect("read output");
    assert!(output.contains("phase0-pty"));

    harness.wait().expect("wait for process");
}
