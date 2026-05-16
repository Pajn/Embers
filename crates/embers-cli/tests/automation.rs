use std::process::Stdio;
use std::time::Duration;

use embers_test_support::{TestServer, acquire_test_lock, cargo_bin_path};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::time::timeout;

async fn read_record(lines: &mut tokio::io::Lines<BufReader<ChildStdout>>) -> Value {
    let line = timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("automation output arrives before timeout")
        .expect("automation stdout read succeeds")
        .expect("automation output line");
    serde_json::from_str(&line).expect("automation output is valid JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automation_mode_emits_hello_response_and_event_records() {
    let _guard = acquire_test_lock().await.expect("acquire test lock");
    let server = TestServer::start().await.expect("start server");

    let mut child = Command::new(cargo_bin_path("embers"))
        .arg("--socket")
        .arg(server.socket_path())
        .arg("automation")
        .arg("--all-sessions")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn automation mode");

    let mut stdin = child.stdin.take().expect("automation stdin");
    let stdout = child.stdout.take().expect("automation stdout");
    let mut lines = BufReader::new(stdout).lines();

    let hello = read_record(&mut lines).await;
    assert_eq!(hello["kind"], "hello");
    assert_eq!(hello["mode"], "automation");
    assert_eq!(hello["subscription"]["all_sessions"], true);
    assert!(hello["subscription"]["subscription_id"].as_u64().is_some());

    stdin
        .write_all(b"new-session alpha\n")
        .await
        .expect("write automation command");
    stdin.flush().await.expect("flush automation stdin");

    let mut saw_response = false;
    let mut saw_event = false;
    for _ in 0..4 {
        let record = read_record(&mut lines).await;
        match record["kind"].as_str() {
            Some("response") => {
                assert_eq!(record["seq"], 1);
                assert_eq!(record["command"], "new-session alpha");
                assert_eq!(record["ok"], true);
                assert!(
                    record["stdout"]
                        .as_str()
                        .is_some_and(|stdout| stdout.contains("alpha")),
                    "response stdout should mention the created session: {record:?}"
                );
                saw_response = true;
            }
            Some("event") => {
                if record["event"]["type"] == "session_created" {
                    assert_eq!(record["event"]["session"]["name"], "alpha");
                    saw_event = true;
                }
            }
            other => panic!("unexpected automation record kind: {other:?}"),
        }

        if saw_response && saw_event {
            break;
        }
    }

    assert!(
        saw_response,
        "automation mode did not emit a response record"
    );
    assert!(
        saw_event,
        "automation mode did not emit a session_created event"
    );

    drop(stdin);
    let status = timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("automation process exits before timeout")
        .expect("automation process wait succeeds");
    assert!(
        status.success(),
        "automation mode exited unsuccessfully: {status}"
    );

    server.shutdown().await.expect("shutdown server");
}
