//! `PromptCommandRunner`: command prompt lines parsed with the CLI's
//! definitions and run against a real server, defaulting to the prompting
//! client's session and the client itself.

use std::time::Duration;

use embers_cli::PromptCommandRunner;
use embers_client::{CommandContext, CommandRunner};
use embers_core::new_request_id;
use embers_protocol::{ClientMessage, ClientRequest, ServerEvent, ServerResponse};
use embers_test_support::{TestConnection, TestServer};

use crate::support::{run_cli, session_snapshot_by_name, stdout};

fn words(line: &str) -> Vec<String> {
    shell_words::split(line).expect("valid line")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_commands_default_to_the_clients_session() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-session", "beta"]);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let beta = session_snapshot_by_name(&mut connection, "beta")
        .await
        .session
        .id;

    let runner = PromptCommandRunner::new(server.socket_path().to_path_buf());
    let context = CommandContext {
        session_id: Some(beta),
        client_id: None,
        base_index: Some(1),
    };

    // With two sessions the CLI would need -t; the prompt uses its own.
    let output = runner
        .run(words("new-window --title one -- /bin/sh"), context.clone())
        .await
        .expect("new-window runs");
    assert_eq!(output, "1\tone");
    runner
        .run(words("new-window --title two -- /bin/sh"), context.clone())
        .await
        .expect("new-window runs");
    runner
        .run(words("select-window -t 1"), context.clone())
        .await
        .expect("select-window runs");
    runner
        .run(words("rename-window -- 'first window'"), context.clone())
        .await
        .expect("rename-window runs");

    let listed = runner
        .run(words("list-windows"), context.clone())
        .await
        .expect("list-windows runs");
    assert_eq!(listed, "1\t1\tfirst window\n2\t0\ttwo");
    let alpha_windows = stdout(&run_cli(&server, ["list-windows", "-t", "alpha"]));
    assert_eq!(alpha_windows.trim(), "");

    // The picker kills a window by number with an explicit base index.
    runner
        .run(
            words("kill-window -t 1"),
            CommandContext {
                base_index: Some(0),
                ..context.clone()
            },
        )
        .await
        .expect("kill-window runs");
    let listed = runner
        .run(words("list-windows"), context.clone())
        .await
        .expect("list-windows runs");
    assert_eq!(listed.lines().count(), 1, "{listed}");
    assert!(!listed.contains("two"), "{listed}");

    // Errors come back as text for the status line.
    let error = runner
        .run(words("bogus-command"), context.clone())
        .await
        .expect_err("unknown command");
    assert!(
        error
            .to_string()
            .contains("unrecognized subcommand 'bogus-command'"),
        "{error}"
    );
    let error = runner
        .run(words("attach"), context.clone())
        .await
        .expect_err("attach is refused");
    assert!(
        error
            .to_string()
            .contains("cannot be run from the command prompt")
    );

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn display_message_from_the_prompt_goes_to_the_prompting_client() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);

    let mut client = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    client.subscribe(None).await.expect("subscribe");
    let session_id = session_snapshot_by_name(&mut client, "alpha")
        .await
        .session
        .id;
    let client_id = match client
        .request(&ClientMessage::Client(ClientRequest::Switch {
            request_id: new_request_id(),
            client_id: None,
            session_id,
            node_id: None,
        }))
        .await
        .expect("switch")
    {
        ServerResponse::Client(response) => response.client.id,
        other => panic!("unexpected response {other:?}"),
    };

    let runner = PromptCommandRunner::new(server.socket_path().to_path_buf());
    runner
        .run(
            words("display-message 'hello from #S'"),
            CommandContext {
                session_id: Some(session_id),
                client_id: Some(client_id),
                base_index: None,
            },
        )
        .await
        .expect("display-message runs");

    let event = client
        .wait_for_event(Duration::from_secs(5), |event| {
            matches!(event, ServerEvent::DisplayMessage(_))
        })
        .await
        .expect("display message event");
    let ServerEvent::DisplayMessage(event) = event else {
        unreachable!()
    };
    assert_eq!(event.message, "hello from alpha");

    server.shutdown().await.expect("shutdown server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_lines_with_global_flags_other_than_base_index_are_refused() {
    let server = TestServer::start().await.expect("start server");
    run_cli(&server, ["new-session", "alpha"]);
    run_cli(&server, ["new-window", "-t", "alpha", "--", "/bin/sh"]);
    let mut connection = TestConnection::connect(server.socket_path())
        .await
        .expect("connect");
    let alpha = session_snapshot_by_name(&mut connection, "alpha")
        .await
        .session
        .id;

    let runner = PromptCommandRunner::new(server.socket_path().to_path_buf());
    let context = CommandContext {
        session_id: Some(alpha),
        client_id: None,
        base_index: None,
    };

    // The prompt always talks to the attached server, so a line naming
    // another one must not run against this one.
    let other_socket = server.socket_path().with_file_name("other.sock");
    let line = vec![
        "--socket".to_owned(),
        other_socket.display().to_string(),
        "kill-server".to_owned(),
    ];
    let error = runner
        .run(line, context.clone())
        .await
        .expect_err("--socket is refused");
    assert!(error.to_string().contains("--socket"), "{error}");

    for line in [
        "--config /nonexistent.rhai list-windows",
        "--log debug list-windows",
        "-v list-windows",
        "list-windows --verbose",
    ] {
        let error = runner
            .run(words(line), context.clone())
            .await
            .expect_err("global flag is refused");
        assert!(
            error
                .to_string()
                .contains("cannot be used from the command prompt"),
            "{line}: {error}"
        );
    }

    // The server is still up, and --base-index still applies.
    let listed = runner
        .run(words("--base-index 1 list-windows"), context.clone())
        .await
        .expect("list-windows runs");
    assert!(listed.starts_with("1\t"), "{listed}");

    server.shutdown().await.expect("shutdown server");
}
