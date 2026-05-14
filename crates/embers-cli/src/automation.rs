use std::io::Write as _;
use std::path::PathBuf;

use clap::Parser;
use embers_core::{MuxError, Result, SessionId, new_request_id};
use embers_protocol::{
    BufferRecord, ClientMessage, ProtocolClient, ServerEnvelope, ServerEvent, SubscribeRequest,
    SubscriptionAckResponse,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{Cli, CliConnection, execute_command};

pub async fn run(socket: PathBuf, target: Option<String>, all_sessions: bool) -> Result<()> {
    let mut request_connection = CliConnection::connect(&socket).await?;
    let subscription_session = if all_sessions {
        None
    } else if let Some(target) = target.as_deref() {
        Some(
            request_connection
                .resolve_session_record(Some(target))
                .await?
                .id,
        )
    } else {
        None
    };

    let mut event_client = if all_sessions || subscription_session.is_some() {
        Some(
            ProtocolClient::connect(&socket)
                .await
                .map_err(|error| MuxError::transport(error.to_string()))?,
        )
    } else {
        None
    };
    let subscription_id = if let Some(event_client) = event_client.as_mut() {
        Some(
            subscribe(event_client, subscription_session)
                .await?
                .subscription_id,
        )
    } else {
        None
    };

    emit_record(&json!({
        "kind": "hello",
        "mode": "automation",
        "subscription": {
            "all_sessions": all_sessions,
            "session_id": subscription_session.map(u64::from),
            "subscription_id": subscription_id,
        },
    }))?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut sequence = 0_u64;

    if let Some(event_client) = event_client.as_mut() {
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else {
                        break;
                    };
                    if let Some(record) = handle_command_line(&mut request_connection, &line, &mut sequence).await {
                        emit_record(&record)?;
                    }
                }
                envelope = event_client.recv() => {
                    match envelope.map_err(|error| MuxError::transport(error.to_string()))? {
                        Some(ServerEnvelope::Event(event)) => emit_record(&event_record(&event))?,
                        Some(ServerEnvelope::Response(response)) => {
                            emit_record(&json!({
                                "kind": "protocol_response",
                                "response": format!("{response:?}"),
                            }))?;
                        }
                        None => break,
                    }
                }
            }
        }
    } else {
        while let Some(line) = lines.next_line().await? {
            if let Some(record) =
                handle_command_line(&mut request_connection, &line, &mut sequence).await
            {
                emit_record(&record)?;
            }
        }
    }

    Ok(())
}

async fn subscribe(
    client: &mut ProtocolClient,
    session_id: Option<SessionId>,
) -> Result<SubscriptionAckResponse> {
    let response = client
        .request(&ClientMessage::Subscribe(SubscribeRequest {
            request_id: new_request_id(),
            session_id,
        }))
        .await
        .map_err(|error| MuxError::transport(error.to_string()))?;
    match response {
        embers_protocol::ServerResponse::SubscriptionAck(response) => Ok(response),
        other => Err(MuxError::protocol(format!(
            "unexpected response to automation subscribe: {other:?}"
        ))),
    }
}

async fn handle_command_line(
    connection: &mut CliConnection,
    line: &str,
    sequence: &mut u64,
) -> Option<Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    *sequence += 1;
    let seq = *sequence;
    let argv = match shell_words::split(trimmed) {
        Ok(argv) => argv,
        Err(error) => {
            return Some(error_record(
                seq,
                trimmed,
                MuxError::invalid_input(error.to_string()),
            ));
        }
    };
    if argv.is_empty() {
        return None;
    }

    let cli =
        match Cli::try_parse_from(std::iter::once("embers").chain(argv.iter().map(String::as_str)))
        {
            Ok(cli) => cli,
            Err(error) => {
                return Some(error_record(
                    seq,
                    trimmed,
                    MuxError::invalid_input(error.to_string()),
                ));
            }
        };
    if cli.socket.is_some() || cli.config.is_some() || cli.log.is_some() || cli.verbose != 0 {
        return Some(error_record(
            seq,
            trimmed,
            MuxError::invalid_input("automation commands cannot override global CLI flags"),
        ));
    }
    let Some(command) = cli.command else {
        return Some(error_record(
            seq,
            trimmed,
            MuxError::invalid_input("automation input requires a subcommand"),
        ));
    };

    Some(match execute_command(connection, command).await {
        Ok(stdout) => json!({
            "kind": "response",
            "seq": seq,
            "command": trimmed,
            "ok": true,
            "stdout": stdout,
        }),
        Err(error) => error_record(seq, trimmed, error),
    })
}

fn emit_record(value: &Value) -> Result<()> {
    let line =
        serde_json::to_string(value).map_err(|error| MuxError::internal(error.to_string()))?;
    println!("{line}");
    std::io::stdout().flush()?;
    Ok(())
}

fn error_record(seq: u64, command: &str, error: MuxError) -> Value {
    json!({
        "kind": "response",
        "seq": seq,
        "command": command,
        "ok": false,
        "error": {
            "code": error_code(&error),
            "message": error.to_string(),
        },
    })
}

fn error_code(error: &MuxError) -> &'static str {
    match error {
        MuxError::Wire(error) => match error.code {
            embers_core::ErrorCode::Unknown => "unknown",
            embers_core::ErrorCode::InvalidRequest => "invalid_request",
            embers_core::ErrorCode::ProtocolViolation => "protocol_violation",
            embers_core::ErrorCode::Transport => "transport",
            embers_core::ErrorCode::NotFound => "not_found",
            embers_core::ErrorCode::Conflict => "conflict",
            embers_core::ErrorCode::Unsupported => "unsupported",
            embers_core::ErrorCode::Timeout => "timeout",
            embers_core::ErrorCode::Internal => "internal",
        },
        MuxError::Io(_) | MuxError::Transport(_) | MuxError::Pty(_) => "transport",
        MuxError::Protocol(_) => "protocol_violation",
        MuxError::InvalidInput(_) => "invalid_request",
        MuxError::NotFound(_) => "not_found",
        MuxError::Conflict(_) => "conflict",
        MuxError::Unsupported(_) => "unsupported",
        MuxError::Timeout(_) => "timeout",
        MuxError::Internal(_) => "internal",
    }
}

fn event_record(event: &ServerEvent) -> Value {
    json!({
        "kind": "event",
        "event": event_value(event),
    })
}

fn event_value(event: &ServerEvent) -> Value {
    match event {
        ServerEvent::SessionCreated(event) => json!({
            "type": "session_created",
            "session": session_value(&event.session),
        }),
        ServerEvent::SessionClosed(event) => json!({
            "type": "session_closed",
            "session_id": u64::from(event.session_id),
        }),
        ServerEvent::SessionRenamed(event) => json!({
            "type": "session_renamed",
            "session_id": u64::from(event.session_id),
            "name": event.name,
        }),
        ServerEvent::BufferCreated(event) => json!({
            "type": "buffer_created",
            "buffer": buffer_value(&event.buffer),
        }),
        ServerEvent::BufferPipeChanged(event) => json!({
            "type": "buffer_pipe_changed",
            "session_id": event.session_id.map(u64::from),
            "buffer": buffer_value(&event.buffer),
        }),
        ServerEvent::BufferDetached(event) => json!({
            "type": "buffer_detached",
            "buffer_id": u64::from(event.buffer_id),
        }),
        ServerEvent::NodeChanged(event) => json!({
            "type": "node_changed",
            "session_id": u64::from(event.session_id),
        }),
        ServerEvent::FloatingChanged(event) => json!({
            "type": "floating_changed",
            "session_id": u64::from(event.session_id),
            "floating_id": event.floating_id.map(u64::from),
        }),
        ServerEvent::FocusChanged(event) => json!({
            "type": "focus_changed",
            "session_id": u64::from(event.session_id),
            "focused_leaf_id": event.focused_leaf_id.map(u64::from),
            "focused_floating_id": event.focused_floating_id.map(u64::from),
        }),
        ServerEvent::RenderInvalidated(event) => json!({
            "type": "render_invalidated",
            "buffer_id": u64::from(event.buffer_id),
        }),
        ServerEvent::ClientChanged(event) => json!({
            "type": "client_changed",
            "client": client_value(&event.client),
            "previous_session_id": event.previous_session_id.map(u64::from),
        }),
    }
}

fn session_value(session: &embers_protocol::SessionRecord) -> Value {
    json!({
        "id": u64::from(session.id),
        "name": session.name,
        "root_node_id": u64::from(session.root_node_id),
        "floating_ids": session.floating_ids.iter().copied().map(u64::from).collect::<Vec<_>>(),
        "focused_leaf_id": session.focused_leaf_id.map(u64::from),
        "focused_floating_id": session.focused_floating_id.map(u64::from),
        "zoomed_node_id": session.zoomed_node_id.map(u64::from),
    })
}

fn client_value(client: &embers_protocol::ClientRecord) -> Value {
    json!({
        "id": client.id,
        "current_session_id": client.current_session_id.map(u64::from),
        "subscribed_all_sessions": client.subscribed_all_sessions,
        "subscribed_session_ids": client.subscribed_session_ids.iter().copied().map(u64::from).collect::<Vec<_>>(),
    })
}

fn buffer_value(buffer: &BufferRecord) -> Value {
    json!({
        "id": u64::from(buffer.id),
        "title": buffer.title,
        "command": buffer.command,
        "cwd": buffer.cwd,
        "kind": crate::buffer_kind_label(buffer.kind),
        "state": crate::buffer_state_label(buffer.state),
        "pid": buffer.pid,
        "attachment_node_id": buffer.attachment_node_id.map(u64::from),
        "read_only": buffer.read_only,
        "helper_source_buffer_id": buffer.helper_source_buffer_id.map(u64::from),
        "helper_scope": buffer.helper_scope.map(crate::history_scope_label),
        "pty_size": {
            "cols": buffer.pty_size.cols,
            "rows": buffer.pty_size.rows,
        },
        "activity": format!("{:?}", buffer.activity).to_lowercase(),
        "last_snapshot_seq": buffer.last_snapshot_seq,
        "exit_code": buffer.exit_code,
        "pipe": buffer.pipe.as_ref().map(buffer_pipe_value),
    })
}

fn buffer_pipe_value(pipe: &embers_protocol::BufferPipeRecord) -> Value {
    json!({
        "command": pipe.command,
        "state": crate::buffer_pipe_state_label(pipe.state),
        "pid": pipe.pid,
        "exit_code": pipe.exit_code,
        "stop_reason": pipe.stop_reason.map(crate::buffer_pipe_stop_reason_label),
    })
}
