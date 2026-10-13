//! Which emulator panes run on: Ghostty's with the `ghostty` feature (the
//! default), alacritty's without.

use std::time::Duration;

use embers_test_support::TestServer;
use tokio::time::sleep;

use crate::support::{run_cli, stdout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panes_run_on_the_emulator_the_build_chose() {
    let server = TestServer::start().await.expect("start server");
    // DECALN fills the screen with E and, in Ghostty but not alacritty, homes
    // the cursor, so the `b` after it lands in the first column.
    run_cli(
        &server,
        [
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

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let screen = loop {
        let screen = stdout(&run_cli(&server, ["capture-pane", "-p", "-t", "t"]));
        if screen.contains('E') {
            break screen;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for DECALN; pane shows:\n{screen}"
        );
        sleep(Duration::from_millis(50)).await;
    };
    let expected = if cfg!(feature = "ghostty") {
        "bEEEEEEEEEEE"
    } else {
        "EbEEEEEEEEEE"
    };
    assert_eq!(screen.lines().next(), Some(expected), "{screen}");

    server.shutdown().await.expect("shutdown server");
}
