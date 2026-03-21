#![allow(dead_code)]

use std::ffi::OsStr;
use std::process::Output;

use embers_protocol::{ClientMessage, ServerResponse, SessionRequest, SessionSnapshot};
use embers_test_support::{TestConnection, TestServer, cargo_bin};

pub fn cli_command(server: &TestServer) -> assert_cmd::Command {
    let mut command = cargo_bin("embers");
    command.arg("--socket").arg(server.socket_path());
    command
}

pub fn run_cli<I, S>(server: &TestServer, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = cli_command(server)
        .args(args)
        .output()
        .expect("cli command runs");
    assert!(
        output.status.success(),
        "cli failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

pub async fn session_snapshot_by_name(
    connection: &mut TestConnection,
    name: &str,
) -> SessionSnapshot {
    let response = connection
        .request(&ClientMessage::Session(SessionRequest::List {
            request_id: embers_core::new_request_id(),
        }))
        .await
        .expect("list sessions succeeds");
    let session_id = match response {
        ServerResponse::Sessions(response) => {
            response
                .sessions
                .into_iter()
                .find(|session| session.name == name)
                .expect("session is present")
                .id
        }
        other => panic!("expected sessions response, got {other:?}"),
    };

    let response = connection
        .request(&ClientMessage::Session(SessionRequest::Get {
            request_id: embers_core::new_request_id(),
            session_id,
        }))
        .await
        .expect("session snapshot succeeds");
    match response {
        ServerResponse::SessionSnapshot(response) => response.snapshot,
        other => panic!("expected session snapshot, got {other:?}"),
    }
}
