mod interactive;

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::num::NonZeroU64;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use base64::Engine as _;
use clap::{Parser, Subcommand};
use embers_core::{
    BufferId, FloatGeometry, FloatingId, MuxError, NodeId, Result, SessionId, SplitDirection,
    new_request_id,
};
use embers_protocol::{
    BufferHistoryPlacement, BufferHistoryScope, BufferLocation, BufferLocationAttachment,
    BufferLocationResponse, BufferRequest, BufferResponse, ClientMessage, ClientRecord,
    ClientRequest, FloatingRecord, FloatingRequest, FloatingResponse, NodeBreakDestination,
    NodeJoinPlacement, NodeRequest, PingRequest, ProtocolClient, ServerResponse, SessionRecord,
    SessionRequest, SessionSnapshot, SnapshotResponse,
};
use embers_server::{SOCKET_ENV_VAR, Server, ServerConfig};
use tokio::time::{Duration, sleep};
use tracing::warn;

#[derive(Debug, Parser)]
#[command(name = "embers", about = "headless terminal multiplexer for embers")]
pub struct Cli {
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true, value_name = "FILTER")]
    pub log: Option<String>,
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    pub fn log_filter(&self) -> String {
        if let Some(filter) = self.log.as_ref().filter(|value| !value.trim().is_empty()) {
            return filter.clone();
        }
        match self.verbose {
            0 => {}
            1 => return "debug".to_owned(),
            _ => return "trace".to_owned(),
        }
        if let Some(filter) = std::env::var("EMBERS_LOG")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return filter;
        }
        if let Some(filter) = std::env::var("RUST_LOG")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return filter;
        }
        "info".to_owned()
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Attach {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "__serve", hide = true)]
    Serve,
    #[command(name = "__runtime-keeper", hide = true)]
    RuntimeKeeper {
        #[arg(long = "keeper-socket")]
        keeper_socket: PathBuf,
        #[arg(long)]
        cols: u16,
        #[arg(long)]
        rows: u16,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long = "env", value_parser = parse_env_arg)]
        env: Vec<(String, OsString)>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Ping {
        #[arg(default_value = "phase0")]
        payload: String,
    },
    #[command(name = "new-session")]
    NewSession { name: String },
    #[command(name = "list-sessions")]
    ListSessions,
    #[command(name = "has-session")]
    HasSession {
        #[arg(short = 't', long = "target")]
        target: String,
    },
    #[command(name = "kill-session")]
    KillSession {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long)]
        force: bool,
    },
    #[command(name = "rename-session")]
    RenameSession {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        name: String,
    },
    #[command(name = "list-buffers")]
    ListBuffers {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long, conflicts_with = "detached")]
        attached: bool,
        #[arg(long, conflicts_with = "attached")]
        detached: bool,
    },
    #[command(name = "attach-buffer")]
    AttachBuffer {
        buffer_id: u64,
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "list-clients")]
    ListClients,
    #[command(name = "detach-client")]
    DetachClient { client_id: u64 },
    #[command(name = "switch-client")]
    SwitchClient {
        client_id: u64,
        #[arg(short = 't', long = "target")]
        target: String,
    },
    Buffer {
        #[command(subcommand)]
        command: BufferCommand,
    },
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    #[command(name = "new-window")]
    NewWindow {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    #[command(name = "list-windows")]
    ListWindows {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "select-window")]
    SelectWindow {
        #[arg(short = 't', long = "target")]
        target: String,
    },
    #[command(name = "rename-window")]
    RenameWindow {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        title: String,
    },
    #[command(name = "kill-window")]
    KillWindow {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "split-window")]
    SplitWindow {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long, conflicts_with = "vertical")]
        horizontal: bool,
        #[arg(long)]
        vertical: bool,
        #[arg(last = true)]
        command: Vec<String>,
    },
    #[command(name = "list-panes")]
    ListPanes {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "select-pane")]
    SelectPane {
        #[arg(short = 't', long = "target")]
        target: String,
    },
    #[command(name = "resize-pane")]
    ResizePane {
        #[arg(short = 't', long = "target")]
        target: String,
        #[arg(long, value_delimiter = ',')]
        sizes: Vec<u16>,
    },
    #[command(name = "send-keys")]
    SendKeys {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long)]
        enter: bool,
        keys: Vec<String>,
    },
    #[command(name = "capture-pane")]
    CapturePane {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "kill-pane")]
    KillPane {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    #[command(name = "display-popup")]
    DisplayPopup {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value_t = 14)]
        x: u16,
        #[arg(long, default_value_t = 4)]
        y: u16,
        #[arg(long, default_value_t = 60)]
        width: u16,
        #[arg(long, default_value_t = 12)]
        height: u16,
        #[arg(last = true)]
        command: Vec<String>,
    },
    #[command(name = "kill-popup")]
    KillPopup {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum BufferCommand {
    Show {
        buffer_id: u64,
    },
    Reveal {
        buffer_id: u64,
        #[arg(long)]
        client: Option<NonZeroU64>,
    },
    History {
        buffer_id: u64,
        #[arg(long, default_value = "full")]
        scope: HistoryScopeArg,
        #[arg(long, default_value = "tab")]
        placement: HistoryPlacementArg,
        #[arg(long)]
        client: Option<NonZeroU64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    Zoom {
        node_id: u64,
    },
    Unzoom {
        #[arg(short = 't', long = "target")]
        target: Option<String>,
    },
    ToggleZoom {
        node_id: u64,
    },
    Swap {
        first_node_id: u64,
        second_node_id: u64,
    },
    Break {
        node_id: u64,
        #[arg(long = "to")]
        destination: BreakDestinationArg,
    },
    JoinBuffer {
        node_id: u64,
        buffer_id: u64,
        #[arg(long = "as", default_value = "tab-after")]
        placement: JoinPlacementArg,
    },
    MoveBefore {
        node_id: u64,
        sibling_id: u64,
    },
    MoveAfter {
        node_id: u64,
        sibling_id: u64,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum HistoryScopeArg {
    Full,
    Visible,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum HistoryPlacementArg {
    Tab,
    Floating,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum BreakDestinationArg {
    Tab,
    Floating,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum JoinPlacementArg {
    Left,
    Right,
    Up,
    Down,
    #[value(name = "tab-before")]
    TabBefore,
    #[value(name = "tab-after")]
    TabAfter,
}

async fn execute(socket: &Path, command: Command) -> Result<String> {
    let mut connection = CliConnection::connect(socket).await?;

    match command {
        Command::Attach { .. } | Command::Serve | Command::RuntimeKeeper { .. } => Err(
            MuxError::internal("interactive commands must be dispatched through run()"),
        ),
        Command::Ping { payload } => {
            let response = connection
                .request(ClientMessage::Ping(PingRequest {
                    request_id: new_request_id(),
                    payload,
                }))
                .await?;
            match response {
                ServerResponse::Pong(response) => Ok(format!("pong {}", response.payload)),
                other => Err(MuxError::protocol(format!(
                    "unexpected response to ping request: {other:?}"
                ))),
            }
        }
        Command::NewSession { name } => {
            let response = connection
                .request(ClientMessage::Session(SessionRequest::Create {
                    request_id: new_request_id(),
                    name,
                }))
                .await?;
            let snapshot = expect_session_snapshot(response, "new-session")?;
            Ok(format!(
                "{}\t{}",
                snapshot.session.id, snapshot.session.name
            ))
        }
        Command::ListSessions => {
            let sessions = connection.list_sessions().await?;
            Ok(format_sessions(&sessions))
        }
        Command::HasSession { target } => {
            connection.resolve_session_record(Some(&target)).await?;
            Ok(String::new())
        }
        Command::KillSession { target, force } => {
            let session = connection.resolve_session_record(target.as_deref()).await?;
            connection
                .request(ClientMessage::Session(SessionRequest::Close {
                    request_id: new_request_id(),
                    session_id: session.id,
                    force,
                }))
                .await?;
            Ok(String::new())
        }
        Command::RenameSession { target, name } => {
            let session = connection.resolve_session_record(target.as_deref()).await?;
            connection
                .request(ClientMessage::Session(SessionRequest::Rename {
                    request_id: new_request_id(),
                    session_id: session.id,
                    name,
                }))
                .await?;
            Ok(String::new())
        }
        Command::ListBuffers {
            target,
            attached,
            detached,
        } => {
            let session_id = match target {
                Some(target) => Some(connection.resolve_session_record(Some(&target)).await?.id),
                None => None,
            };
            let response = connection
                .request(ClientMessage::Buffer(BufferRequest::List {
                    request_id: new_request_id(),
                    session_id,
                    attached_only: attached,
                    detached_only: detached,
                }))
                .await?;
            match response {
                ServerResponse::Buffers(response) => Ok(format_buffers(&response.buffers)),
                other => Err(MuxError::protocol(format!(
                    "unexpected response to list-buffers: {other:?}"
                ))),
            }
        }
        Command::AttachBuffer { buffer_id, target } => {
            let pane = connection.resolve_pane(target.as_deref()).await?;
            let response = connection
                .request(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                    request_id: new_request_id(),
                    buffer_id: BufferId(buffer_id),
                    target_leaf_node_id: pane.leaf_id,
                }))
                .await?;
            expect_session_snapshot(response, "attach-buffer")?;
            Ok(String::new())
        }
        Command::ListClients => {
            let sessions = connection.list_sessions().await?;
            let clients = connection.list_clients().await?;
            Ok(format_clients(&clients, &sessions))
        }
        Command::DetachClient { client_id } => {
            let client_id = NonZeroU64::new(client_id)
                .ok_or_else(|| MuxError::invalid_input("client id must be non-zero"))?;
            connection
                .request(ClientMessage::Client(ClientRequest::Detach {
                    request_id: new_request_id(),
                    client_id: Some(client_id),
                }))
                .await?;
            Ok(String::new())
        }
        Command::SwitchClient { client_id, target } => {
            let client_id = NonZeroU64::new(client_id)
                .ok_or_else(|| MuxError::invalid_input("client id must be non-zero"))?;
            let session = connection.resolve_session_record(Some(&target)).await?;
            match connection
                .request(ClientMessage::Client(ClientRequest::Switch {
                    request_id: new_request_id(),
                    client_id: Some(client_id),
                    session_id: session.id,
                }))
                .await?
            {
                ServerResponse::Client(_) => Ok(String::new()),
                other => Err(MuxError::protocol(format!(
                    "unexpected response to switch-client: {other:?}"
                ))),
            }
        }
        Command::Buffer { command } => match command {
            BufferCommand::Show { buffer_id } => {
                let requested_buffer_id = BufferId(buffer_id);
                let response = connection
                    .request(ClientMessage::Buffer(BufferRequest::Inspect {
                        request_id: new_request_id(),
                        buffer_id: requested_buffer_id,
                    }))
                    .await?;
                let (buffer, location, _) = expect_buffer_with_location(response, "buffer show")?;
                ensure_matching_buffer_id("buffer show", requested_buffer_id, buffer.id)?;
                Ok(format_buffer_details(&buffer, &location))
            }
            BufferCommand::Reveal { buffer_id, client } => {
                let requested_buffer_id = BufferId(buffer_id);
                let response = connection
                    .request(ClientMessage::Buffer(BufferRequest::Reveal {
                        request_id: new_request_id(),
                        buffer_id: requested_buffer_id,
                        client_id: client,
                    }))
                    .await?;
                let location = expect_buffer_location(response, "buffer reveal")?;
                ensure_matching_buffer_id(
                    "buffer reveal",
                    requested_buffer_id,
                    location.buffer_id,
                )?;
                if matches!(location.attachment, BufferLocationAttachment::Detached) {
                    return Err(MuxError::conflict(format!(
                        "buffer {} is detached; use attach-buffer or node join-buffer",
                        buffer_id
                    )));
                }
                Ok(format_buffer_location_line(&location))
            }
            BufferCommand::History {
                buffer_id,
                scope,
                placement,
                client,
            } => {
                let requested_scope = history_scope(scope);
                let requested_placement = history_placement(placement);
                let response = connection
                    .request(ClientMessage::Buffer(BufferRequest::OpenHistory {
                        request_id: new_request_id(),
                        buffer_id: BufferId(buffer_id),
                        scope: requested_scope,
                        placement: requested_placement,
                        client_id: client,
                    }))
                    .await?;
                let (buffer, location, at_root_tab) =
                    expect_buffer_with_location(response, "buffer history")?;
                ensure_history_response(
                    BufferId(buffer_id),
                    requested_scope,
                    requested_placement,
                    &buffer,
                    &location,
                    at_root_tab,
                )?;
                Ok(format_buffer_location_line(&location))
            }
        },
        Command::Node { command } => match command {
            NodeCommand::Zoom { node_id } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::Zoom {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::Zoom")?;
                Ok(String::new())
            }
            NodeCommand::Unzoom { target } => {
                let session = connection.resolve_session_record(target.as_deref()).await?;
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::Unzoom {
                        request_id: new_request_id(),
                        session_id: session.id,
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::Unzoom")?;
                Ok(String::new())
            }
            NodeCommand::ToggleZoom { node_id } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::ToggleZoom {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::ToggleZoom")?;
                Ok(String::new())
            }
            NodeCommand::Swap {
                first_node_id,
                second_node_id,
            } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::SwapSiblings {
                        request_id: new_request_id(),
                        first_node_id: NodeId(first_node_id),
                        second_node_id: NodeId(second_node_id),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::Swap")?;
                Ok(String::new())
            }
            NodeCommand::Break {
                node_id,
                destination,
            } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::BreakNode {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                        destination: break_destination(destination),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::Break")?;
                Ok(String::new())
            }
            NodeCommand::JoinBuffer {
                node_id,
                buffer_id,
                placement,
            } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::JoinBufferAtNode {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                        buffer_id: BufferId(buffer_id),
                        placement: join_placement(placement),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::JoinBuffer")?;
                Ok(String::new())
            }
            NodeCommand::MoveBefore {
                node_id,
                sibling_id,
            } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                        sibling_node_id: NodeId(sibling_id),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::MoveBefore")?;
                Ok(String::new())
            }
            NodeCommand::MoveAfter {
                node_id,
                sibling_id,
            } => {
                let response = connection
                    .request(ClientMessage::Node(NodeRequest::MoveNodeAfter {
                        request_id: new_request_id(),
                        node_id: NodeId(node_id),
                        sibling_node_id: NodeId(sibling_id),
                    }))
                    .await?;
                expect_ok(response, "NodeCommand::MoveAfter")?;
                Ok(String::new())
            }
        },
        Command::NewWindow {
            target,
            title,
            command,
        } => {
            let session = connection.resolve_session_record(target.as_deref()).await?;
            let command = buffer_command(command);
            let window_title = title.unwrap_or_else(|| default_title(&command, "window"));
            let buffer = connection
                .create_buffer(Some(window_title.clone()), command, None)
                .await?;
            let response = connection
                .request(ClientMessage::Session(SessionRequest::AddRootTab {
                    request_id: new_request_id(),
                    session_id: session.id,
                    title: window_title.clone(),
                    buffer_id: Some(buffer.buffer.id),
                    child_node_id: None,
                }))
                .await;
            let response = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "new-window",
                response,
            )
            .await?;
            let snapshot = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "new-window",
                expect_session_snapshot(response, "new-window"),
            )
            .await?;
            let (index, title) = active_root_window(&snapshot)?;
            Ok(format!("{index}\t{title}"))
        }
        Command::ListWindows { target } => {
            let snapshot = connection
                .resolve_session_snapshot(target.as_deref())
                .await?;
            Ok(format_windows(&snapshot)?)
        }
        Command::SelectWindow { target } => {
            let window = connection.resolve_window(Some(&target)).await?;
            connection
                .request(ClientMessage::Session(SessionRequest::SelectRootTab {
                    request_id: new_request_id(),
                    session_id: window.snapshot.session.id,
                    index: window.index,
                }))
                .await?;
            Ok(String::new())
        }
        Command::RenameWindow { target, title } => {
            let window = connection.resolve_window(target.as_deref()).await?;
            connection
                .request(ClientMessage::Session(SessionRequest::RenameRootTab {
                    request_id: new_request_id(),
                    session_id: window.snapshot.session.id,
                    index: window.index,
                    title,
                }))
                .await?;
            Ok(String::new())
        }
        Command::KillWindow { target } => {
            let window = connection.resolve_window(target.as_deref()).await?;
            connection
                .request(ClientMessage::Session(SessionRequest::CloseRootTab {
                    request_id: new_request_id(),
                    session_id: window.snapshot.session.id,
                    index: window.index,
                }))
                .await?;
            Ok(String::new())
        }
        Command::SplitWindow {
            target,
            horizontal,
            vertical: _,
            command,
        } => {
            let pane = connection.resolve_pane(target.as_deref()).await?;
            let command = buffer_command(command);
            let buffer = connection
                .create_buffer(Some(default_title(&command, "pane")), command, None)
                .await?;
            let direction = if horizontal {
                SplitDirection::Horizontal
            } else {
                SplitDirection::Vertical
            };
            let response = connection
                .request(ClientMessage::Node(NodeRequest::Split {
                    request_id: new_request_id(),
                    leaf_node_id: pane.leaf_id,
                    direction,
                    new_buffer_id: buffer.buffer.id,
                }))
                .await;
            let response = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "split-window",
                response,
            )
            .await?;
            let snapshot = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "split-window",
                expect_session_snapshot(response, "split-window"),
            )
            .await?;
            let focused_leaf = snapshot.session.focused_leaf_id.ok_or_else(|| {
                MuxError::protocol("split-window response did not include focused leaf")
            })?;
            Ok(focused_leaf.to_string())
        }
        Command::ListPanes { target } => {
            let window = connection.resolve_window(target.as_deref()).await?;
            let leaf_ids = visible_leaf_ids(&window.snapshot, window.child_id)?;
            Ok(format_panes(&window.snapshot, &leaf_ids)?)
        }
        Command::SelectPane { target } => {
            let pane = connection.resolve_pane(Some(&target)).await?;
            connection
                .request(ClientMessage::Node(NodeRequest::Focus {
                    request_id: new_request_id(),
                    session_id: pane.snapshot.session.id,
                    node_id: pane.leaf_id,
                }))
                .await?;
            Ok(String::new())
        }
        Command::ResizePane { target, sizes } => {
            if sizes.is_empty() {
                return Err(MuxError::invalid_input(
                    "resize-pane requires at least one size value",
                ));
            }
            let pane = connection.resolve_pane(Some(&target)).await?;
            let leaf = node_record(&pane.snapshot, pane.leaf_id)?;
            let parent_id = leaf
                .parent_id
                .ok_or_else(|| MuxError::invalid_input("pane is not inside a resizable split"))?;
            let parent = node_record(&pane.snapshot, parent_id)?;
            if parent.kind != embers_protocol::NodeRecordKind::Split {
                return Err(MuxError::invalid_input(
                    "pane parent is not a split and cannot be resized",
                ));
            }

            connection
                .request(ClientMessage::Node(NodeRequest::Resize {
                    request_id: new_request_id(),
                    node_id: parent_id,
                    sizes,
                }))
                .await?;
            Ok(String::new())
        }
        Command::SendKeys {
            target,
            enter,
            keys,
        } => {
            let pane = connection.resolve_pane(target.as_deref()).await?;
            if keys.is_empty() && !enter {
                return Err(MuxError::invalid_input(
                    "send-keys requires at least one key or --enter",
                ));
            }
            let mut bytes = keys.join(" ").into_bytes();
            if enter {
                bytes.push(b'\r');
            }
            connection
                .request(ClientMessage::Input(embers_protocol::InputRequest::Send {
                    request_id: new_request_id(),
                    buffer_id: pane.buffer_id,
                    bytes,
                }))
                .await?;
            Ok(String::new())
        }
        Command::CapturePane { target } => {
            let pane = connection.resolve_pane(target.as_deref()).await?;
            let response = connection
                .request(ClientMessage::Buffer(BufferRequest::Capture {
                    request_id: new_request_id(),
                    buffer_id: pane.buffer_id,
                }))
                .await?;
            let snapshot = expect_capture(response, "capture-pane")?;
            Ok(snapshot.lines.join("\n"))
        }
        Command::KillPane { target } => {
            let pane = connection.resolve_pane(target.as_deref()).await?;
            connection
                .request(ClientMessage::Node(NodeRequest::Close {
                    request_id: new_request_id(),
                    node_id: pane.leaf_id,
                }))
                .await?;
            Ok(String::new())
        }
        Command::DisplayPopup {
            target,
            title,
            x,
            y,
            width,
            height,
            command,
        } => {
            let session = connection.resolve_session_record(target.as_deref()).await?;
            let command = buffer_command(command);
            let popup_title = title.unwrap_or_else(|| default_title(&command, "popup"));
            let buffer = connection
                .create_buffer(Some(popup_title.clone()), command, None)
                .await?;
            let response = connection
                .request(ClientMessage::Floating(FloatingRequest::Create {
                    request_id: new_request_id(),
                    session_id: session.id,
                    root_node_id: None,
                    buffer_id: Some(buffer.buffer.id),
                    geometry: FloatGeometry::new(x, y, width, height),
                    title: Some(popup_title),
                    focus: true,
                    close_on_empty: true,
                }))
                .await;
            let response = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "display-popup",
                response,
            )
            .await?;
            let popup = rollback_created_buffer_on_error(
                &mut connection,
                buffer.buffer.id,
                "display-popup",
                expect_floating(response, "display-popup"),
            )
            .await?;
            Ok(popup.id.to_string())
        }
        Command::KillPopup { target } => {
            let popup = connection.resolve_popup(target.as_deref()).await?;
            connection
                .request(ClientMessage::Floating(FloatingRequest::Close {
                    request_id: new_request_id(),
                    floating_id: popup.id,
                }))
                .await?;
            Ok(String::new())
        }
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let Cli {
        socket,
        config,
        command,
        ..
    } = cli;

    match command {
        Some(Command::RuntimeKeeper {
            keeper_socket,
            cols,
            rows,
            cwd,
            env,
            command,
        }) => embers_server::run_runtime_keeper(embers_server::RuntimeKeeperCli {
            socket_path: keeper_socket,
            command,
            cwd,
            env: env.into_iter().collect(),
            size: embers_core::PtySize::new(cols, rows),
        }),
        command => {
            let socket = resolve_socket_path(socket.as_deref());
            validate_runtime_socket_parent(&socket)?;

            match command {
                None => {
                    ensure_server_process(&socket).await?;
                    interactive::run(socket, None, config).await
                }
                Some(Command::Attach { target }) => {
                    if !server_is_available(&socket).await {
                        return Err(MuxError::not_found(format!(
                            "no embers server is listening on {}",
                            socket.display()
                        )));
                    }
                    interactive::run(socket, target, config).await
                }
                Some(Command::Serve) => run_server(socket).await,
                Some(command) => {
                    ensure_server_process(&socket).await?;
                    let output = execute(&socket, command).await?;
                    if !output.is_empty() {
                        println!("{output}");
                    }
                    Ok(())
                }
            }
        }
    }
}

fn resolve_socket_path(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os(SOCKET_ENV_VAR).map(PathBuf::from))
        .unwrap_or_else(default_socket_path)
}

fn default_socket_path() -> PathBuf {
    default_runtime_dir().join("embers.sock")
}

fn default_runtime_dir() -> PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty())
    {
        return PathBuf::from(runtime_dir).join("embers");
    }
    #[cfg(unix)]
    {
        let run_user_dir = PathBuf::from(format!("/run/user/{}", effective_uid()));
        if run_user_dir.is_dir() {
            return run_user_dir.join("embers");
        }
    }
    PathBuf::from("/tmp").join(format!("embers-{}", effective_uid()))
}

fn parse_env_arg(value: &str) -> std::result::Result<(String, OsString), String> {
    let Some((key, env_value)) = value.split_once('=') else {
        return Err("expected KEY=VALUE".to_owned());
    };
    if key.is_empty() {
        return Err("environment key must not be empty".to_owned());
    }
    Ok((key.to_owned(), decode_runtime_keeper_env_value(env_value)?))
}

fn decode_runtime_keeper_env_value(value: &str) -> std::result::Result<OsString, String> {
    let Some(encoded) = value.strip_prefix("base64:") else {
        return Ok(OsString::from(value));
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("invalid base64 environment value: {error}"))?;
    #[cfg(unix)]
    {
        Ok(OsString::from_vec(decoded))
    }
    #[cfg(windows)]
    {
        if decoded.len() % 2 != 0 {
            return Err("invalid UTF-16LE environment value: odd-length byte sequence".to_owned());
        }
        let wide = decoded
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        Ok(OsString::from_wide(&wide))
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        String::from_utf8(decoded)
            .map(OsString::from)
            .map_err(|error| format!("invalid UTF-8 environment value: {error}"))
    }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    0
}

fn pid_path(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("pid")
}

async fn server_is_available(socket_path: &Path) -> bool {
    if validate_runtime_socket_parent(socket_path).is_err() {
        return false;
    }
    CliConnection::connect(socket_path).await.is_ok()
}

async fn ensure_server_process(socket_path: &Path) -> Result<()> {
    if server_is_available(socket_path).await {
        return Ok(());
    }

    ensure_socket_parent(socket_path)?;

    let current_exe = std::env::current_exe()?;
    let mut child = ProcessCommand::new(current_exe)
        .arg("__serve")
        .arg("--socket")
        .arg(socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut exited_status = None;
    loop {
        if server_is_available(socket_path).await {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            exited_status.get_or_insert(status);
            if server_is_available(socket_path).await {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            if let Some(status) = exited_status {
                return Err(MuxError::transport(format!(
                    "embers server exited before becoming ready with status {status}"
                )));
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(MuxError::timeout(format!(
                "timed out waiting for embers server at {}",
                socket_path.display()
            )));
        }
        sleep(Duration::from_millis(25)).await;
    }
}

async fn run_server(socket_path: PathBuf) -> Result<()> {
    ensure_socket_parent(&socket_path)?;
    let secure_parent = socket_path
        .parent()
        .is_some_and(|parent| parent == default_runtime_dir().as_path());
    let _pid = ServerPidFile::create(&pid_path(&socket_path), secure_parent)?;
    let handle = Server::new(ServerConfig::new(socket_path)).start().await?;
    wait_for_shutdown_signal().await?;
    handle.shutdown().await
}

fn ensure_socket_parent(socket_path: &Path) -> Result<()> {
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    if parent == default_runtime_dir().as_path() {
        ensure_private_dir(parent)
    } else {
        fs::create_dir_all(parent)?;
        Ok(())
    }
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        validate_private_dir(path)?;
    }
    Ok(())
}

fn validate_runtime_socket_parent(socket_path: &Path) -> Result<()> {
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    if parent != default_runtime_dir().as_path() || !parent.exists() {
        return Ok(());
    }
    validate_private_dir(parent)
}

#[cfg(unix)]
fn validate_private_dir(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(MuxError::invalid_input(format!(
            "runtime directory {} is not a directory",
            path.display()
        )));
    }
    if metadata.uid() != effective_uid() {
        return Err(MuxError::invalid_input(format!(
            "runtime directory {} is not owned by uid {}",
            path.display(),
            effective_uid()
        )));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(MuxError::invalid_input(format!(
            "runtime directory {} must have mode 0700",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> Result<()> {
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    tokio::select! {
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
        _ = hangup.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> Result<()> {
    tokio::signal::ctrl_c().await?;
    Ok(())
}

struct ServerPidFile {
    path: PathBuf,
}

impl ServerPidFile {
    fn create(path: &Path, secure_parent: bool) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if secure_parent {
                ensure_private_dir(parent)?;
            } else {
                fs::create_dir_all(parent)?;
            }
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(MuxError::conflict(format!(
                        "refusing to overwrite symlink pid file {}",
                        path.display()
                    )));
                }
                let pid_text = fs::read_to_string(path).map_err(|error| {
                    MuxError::conflict(format!(
                        "refusing to overwrite unreadable pid file {}: {error}",
                        path.display()
                    ))
                })?;
                let pid = pid_text.trim().parse::<u32>().map_err(|error| {
                    MuxError::conflict(format!(
                        "refusing to overwrite invalid pid file {}: {error}",
                        path.display()
                    ))
                })?;
                if process_is_alive(pid)? {
                    return Err(MuxError::conflict(format!(
                        "refusing to overwrite active pid file {} for running process {pid}",
                        path.display()
                    )));
                }
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        #[cfg(unix)]
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;

        #[cfg(not(unix))]
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;

        file.write_all(std::process::id().to_string().as_bytes())?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> Result<bool> {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        Some(code) => Err(MuxError::transport(format!(
            "failed to validate process {pid}: os error {code}"
        ))),
        None => Err(MuxError::transport(format!(
            "failed to validate process {pid}: unknown os error"
        ))),
    }
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> Result<bool> {
    Err(MuxError::conflict(
        "pid file validation is unsupported on this platform".to_owned(),
    ))
}

impl Drop for ServerPidFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
struct CliConnection {
    client: ProtocolClient,
}

impl CliConnection {
    async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let client = ProtocolClient::connect(path)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?;
        Ok(Self { client })
    }

    async fn request(&mut self, message: ClientMessage) -> Result<ServerResponse> {
        match self
            .client
            .request(&message)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?
        {
            ServerResponse::Error(response) => Err(response.error.into()),
            response => Ok(response),
        }
    }

    async fn list_sessions(&mut self) -> Result<Vec<SessionRecord>> {
        match self
            .request(ClientMessage::Session(SessionRequest::List {
                request_id: new_request_id(),
            }))
            .await?
        {
            ServerResponse::Sessions(response) => Ok(response.sessions),
            other => Err(MuxError::protocol(format!(
                "unexpected response to list-sessions: {other:?}"
            ))),
        }
    }

    async fn list_clients(&mut self) -> Result<Vec<ClientRecord>> {
        match self
            .request(ClientMessage::Client(ClientRequest::List {
                request_id: new_request_id(),
            }))
            .await?
        {
            ServerResponse::Clients(response) => Ok(response.clients),
            other => Err(MuxError::protocol(format!(
                "unexpected response to list-clients: {other:?}"
            ))),
        }
    }

    async fn session_snapshot(&mut self, session_id: SessionId) -> Result<SessionSnapshot> {
        match self
            .request(ClientMessage::Session(SessionRequest::Get {
                request_id: new_request_id(),
                session_id,
            }))
            .await?
        {
            ServerResponse::SessionSnapshot(response) => Ok(response.snapshot),
            other => Err(MuxError::protocol(format!(
                "unexpected response to session get: {other:?}"
            ))),
        }
    }

    async fn create_buffer(
        &mut self,
        title: Option<String>,
        command: Vec<String>,
        cwd: Option<String>,
    ) -> Result<BufferResponse> {
        match self
            .request(ClientMessage::Buffer(BufferRequest::Create {
                request_id: new_request_id(),
                title,
                command,
                cwd,
                env: Default::default(),
            }))
            .await?
        {
            ServerResponse::Buffer(response) => Ok(response),
            other => Err(MuxError::protocol(format!(
                "unexpected response to buffer create: {other:?}"
            ))),
        }
    }

    async fn rollback_created_buffer(&mut self, buffer_id: BufferId, operation: &str) {
        if let Err(error) = self
            .request(ClientMessage::Buffer(BufferRequest::Detach {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to detach created buffer during rollback"
            );
        }
        if let Err(error) = self
            .request(ClientMessage::Buffer(BufferRequest::Kill {
                request_id: new_request_id(),
                buffer_id,
                force: true,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to kill created buffer during rollback"
            );
        }
    }

    async fn resolve_session_record(&mut self, target: Option<&str>) -> Result<SessionRecord> {
        let sessions = self.list_sessions().await?;
        match target {
            Some(target) => sessions
                .into_iter()
                .find(|session| session.name == target)
                .ok_or_else(|| MuxError::not_found(format!("session '{target}' was not found"))),
            None => match sessions.as_slice() {
                [session] => Ok(session.clone()),
                [] => Err(MuxError::not_found("no sessions exist")),
                _ => Err(MuxError::invalid_input(
                    "session target is required when multiple sessions exist",
                )),
            },
        }
    }

    async fn resolve_session_snapshot(&mut self, target: Option<&str>) -> Result<SessionSnapshot> {
        let session = self.resolve_session_record(target).await?;
        self.session_snapshot(session.id).await
    }

    async fn resolve_window(&mut self, target: Option<&str>) -> Result<ResolvedWindow> {
        let (session_target, selector) = split_scoped_target(target);
        let snapshot = self
            .resolve_session_snapshot(session_target.as_deref())
            .await?;
        let (index, child_id) = {
            if let Some((_, root_tabs)) = root_tabs(&snapshot)? {
                let index = resolve_window_index(root_tabs, selector.as_deref())?;
                let tab = protocol_tab(root_tabs, index).ok_or_else(|| {
                    MuxError::not_found(format!(
                        "window index {index} is not present in session {}",
                        snapshot.session.id
                    ))
                })?;
                (index, tab.child_id)
            } else {
                let title = window_title(&snapshot, snapshot.session.root_node_id)?;
                let index = resolve_single_window_index(&title, selector.as_deref())?;
                (index, snapshot.session.root_node_id)
            }
        };
        Ok(ResolvedWindow {
            snapshot,
            index,
            child_id,
        })
    }

    async fn resolve_pane(&mut self, target: Option<&str>) -> Result<ResolvedPane> {
        match target {
            Some(target) => {
                let (session_target, selector) = split_scoped_required(target, "pane target")?;
                let pane_id = parse_node_id(&selector)?;
                let snapshot = if let Some(session_target) = session_target {
                    self.resolve_session_snapshot(Some(&session_target)).await?
                } else {
                    self.find_session_containing_pane(pane_id).await?
                };
                resolved_pane(snapshot, pane_id)
            }
            None => {
                let snapshot = self.resolve_session_snapshot(None).await?;
                let pane_id = snapshot
                    .session
                    .focused_leaf_id
                    .ok_or_else(|| MuxError::not_found("session has no focused pane"))?;
                resolved_pane(snapshot, pane_id)
            }
        }
    }

    async fn resolve_popup(&mut self, target: Option<&str>) -> Result<FloatingRecord> {
        match target {
            Some(target) => {
                let (session_target, selector) = split_scoped_required(target, "popup target")?;
                let popup_id = parse_floating_id(&selector)?;
                if let Some(session_target) = session_target {
                    let snapshot = self.resolve_session_snapshot(Some(&session_target)).await?;
                    floating_record(&snapshot, popup_id).cloned()
                } else {
                    let sessions = self.list_sessions().await?;
                    for session in sessions {
                        let snapshot = self.session_snapshot(session.id).await?;
                        if let Ok(popup) = floating_record(&snapshot, popup_id) {
                            return Ok(popup.clone());
                        }
                    }
                    Err(MuxError::not_found(format!(
                        "popup {popup_id} was not found"
                    )))
                }
            }
            None => {
                let snapshot = self.resolve_session_snapshot(None).await?;
                let popup_id = snapshot
                    .session
                    .focused_floating_id
                    .ok_or_else(|| MuxError::not_found("session has no focused popup"))?;
                floating_record(&snapshot, popup_id).cloned()
            }
        }
    }

    async fn find_session_containing_pane(&mut self, pane_id: NodeId) -> Result<SessionSnapshot> {
        let sessions = self.list_sessions().await?;
        for session in sessions {
            let snapshot = self.session_snapshot(session.id).await?;
            if node_record(&snapshot, pane_id).is_ok() {
                return Ok(snapshot);
            }
        }

        Err(MuxError::not_found(format!("pane {pane_id} was not found")))
    }
}

async fn rollback_created_buffer_on_error<T>(
    connection: &mut CliConnection,
    buffer_id: BufferId,
    operation: &str,
    result: Result<T>,
) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            connection
                .rollback_created_buffer(buffer_id, operation)
                .await;
            Err(error)
        }
    }
}

#[derive(Debug)]
struct ResolvedWindow {
    snapshot: SessionSnapshot,
    index: u32,
    child_id: NodeId,
}

#[derive(Debug)]
struct ResolvedPane {
    snapshot: SessionSnapshot,
    leaf_id: NodeId,
    buffer_id: BufferId,
}

fn resolved_pane(snapshot: SessionSnapshot, pane_id: NodeId) -> Result<ResolvedPane> {
    let leaf = node_record(&snapshot, pane_id)?;
    let buffer_id = leaf
        .buffer_view
        .as_ref()
        .map(|view| view.buffer_id)
        .ok_or_else(|| MuxError::invalid_input(format!("node {pane_id} is not a pane leaf")))?;
    Ok(ResolvedPane {
        snapshot,
        leaf_id: pane_id,
        buffer_id,
    })
}

fn expect_session_snapshot(response: ServerResponse, operation: &str) -> Result<SessionSnapshot> {
    match response {
        ServerResponse::SessionSnapshot(response) => Ok(response.snapshot),
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn expect_floating(response: ServerResponse, operation: &str) -> Result<FloatingRecord> {
    match response {
        ServerResponse::Floating(FloatingResponse { floating, .. }) => Ok(floating),
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn expect_capture(response: ServerResponse, operation: &str) -> Result<SnapshotResponse> {
    match response {
        ServerResponse::Snapshot(snapshot) => Ok(snapshot),
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn expect_buffer_location(response: ServerResponse, operation: &str) -> Result<BufferLocation> {
    match response {
        ServerResponse::BufferLocation(BufferLocationResponse { location, .. }) => Ok(location),
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn expect_ok(response: ServerResponse, operation: &str) -> Result<()> {
    match response {
        ServerResponse::Ok(_) => Ok(()),
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn expect_buffer_with_location(
    response: ServerResponse,
    operation: &str,
) -> Result<(embers_protocol::BufferRecord, BufferLocation, bool)> {
    match response {
        ServerResponse::BufferWithLocation(response) => {
            let (_, buffer, location, at_root_tab) = response.into_parts();
            if buffer.id != location.buffer_id {
                return Err(MuxError::protocol(format!(
                    "{operation} returned buffer {} but location was for buffer {}",
                    buffer.id, location.buffer_id
                )));
            }
            Ok((buffer, location, at_root_tab))
        }
        other => Err(MuxError::protocol(format!(
            "unexpected response to {operation}: {other:?}"
        ))),
    }
}

fn ensure_matching_buffer_id(
    operation: &str,
    requested_buffer_id: BufferId,
    actual_buffer_id: BufferId,
) -> Result<()> {
    if actual_buffer_id == requested_buffer_id {
        return Ok(());
    }

    Err(MuxError::protocol(format!(
        "{operation} returned buffer {actual_buffer_id} for requested buffer {requested_buffer_id}"
    )))
}

fn format_sessions(sessions: &[SessionRecord]) -> String {
    sessions
        .iter()
        .map(|session| format!("{}\t{}", session.id, session.name))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_buffers(buffers: &[embers_protocol::BufferRecord]) -> String {
    buffers
        .iter()
        .map(|buffer| {
            let attachment = buffer
                .attachment_node_id
                .map(|node_id| format!("attached:{node_id}"))
                .unwrap_or_else(|| "detached".to_owned());
            format!(
                "{}\t{}\t{}\t{}",
                buffer.id,
                buffer_state_label(buffer.state),
                attachment,
                buffer.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_clients(clients: &[ClientRecord], sessions: &[SessionRecord]) -> String {
    clients
        .iter()
        .map(|client| {
            let current = client
                .current_session_id
                .map(|session_id| session_label(sessions, session_id))
                .unwrap_or_else(|| "-".to_owned());
            let scope = if client.subscribed_all_sessions {
                "all".to_owned()
            } else if client.subscribed_session_ids.is_empty() {
                "-".to_owned()
            } else {
                client
                    .subscribed_session_ids
                    .iter()
                    .map(|session_id| session_label(sessions, *session_id))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!("{}\t{}\t{}", client.id, current, scope)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_buffer_details(
    buffer: &embers_protocol::BufferRecord,
    location: &BufferLocation,
) -> String {
    let mut lines = vec![
        format!("id\t{}", buffer.id),
        format!(
            "title\t{}",
            serde_json::to_string(&buffer.title).expect("buffer titles serialize to JSON")
        ),
        format!("state\t{}", buffer_state_label(buffer.state)),
        format!("kind\t{}", buffer_kind_label(buffer.kind)),
        format!("read_only\t{}", usize::from(buffer.read_only)),
        format!("location\t{}", format_buffer_location_inline(location)),
    ];
    if let Some(source_buffer_id) = buffer.helper_source_buffer_id {
        lines.push(format!("source_buffer\t{}", source_buffer_id));
    }
    if let Some(scope) = buffer.helper_scope {
        lines.push(format!("history_scope\t{}", history_scope_label(scope)));
    }
    if !buffer.command.is_empty() {
        let serialized_args =
            serde_json::to_string(&buffer.command).expect("buffer commands serialize to JSON");
        lines.push(format!("command\t{serialized_args}"));
    }
    if let Some(cwd) = &buffer.cwd {
        let serialized_cwd =
            serde_json::to_string(cwd).expect("buffer working directories serialize to JSON");
        lines.push(format!("cwd\t{serialized_cwd}"));
    }
    lines.join("\n")
}

fn format_buffer_location_line(location: &BufferLocation) -> String {
    format!(
        "{}\t{}",
        location.buffer_id,
        format_buffer_location_value(location)
    )
}

fn format_buffer_location_inline(location: &BufferLocation) -> String {
    match location.attachment {
        BufferLocationAttachment::Floating {
            session_id,
            node_id,
            floating_id,
        } => {
            format!("session:{session_id} node:{node_id} floating:{floating_id}")
        }
        BufferLocationAttachment::Session {
            session_id,
            node_id,
        } => format!("session:{session_id} node:{node_id}"),
        BufferLocationAttachment::Detached => "detached".to_owned(),
    }
}

fn format_buffer_location_value(location: &BufferLocation) -> String {
    match location.attachment {
        BufferLocationAttachment::Floating {
            session_id,
            node_id,
            floating_id,
        } => {
            format!("session:{session_id}\tnode:{node_id}\tfloating:{floating_id}")
        }
        BufferLocationAttachment::Session {
            session_id,
            node_id,
        } => format!("session:{session_id}\tnode:{node_id}"),
        BufferLocationAttachment::Detached => "detached".to_owned(),
    }
}

fn session_label(sessions: &[SessionRecord], session_id: SessionId) -> String {
    sessions
        .iter()
        .find(|session| session.id == session_id)
        .map(|session| format!("{}:{}", session.id, session.name))
        .unwrap_or_else(|| session_id.to_string())
}

fn format_windows(snapshot: &SessionSnapshot) -> Result<String> {
    if let Some((_, tabs)) = root_tabs(snapshot)? {
        Ok(tabs
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                format!(
                    "{index}\t{}\t{}",
                    usize::from(u32::try_from(index).ok() == Some(tabs.active)),
                    tab.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    } else {
        let title = window_title(snapshot, snapshot.session.root_node_id)?;
        Ok(format!("0\t1\t{title}"))
    }
}

fn format_panes(snapshot: &SessionSnapshot, pane_ids: &[NodeId]) -> Result<String> {
    pane_ids
        .iter()
        .map(|pane_id| {
            let leaf = node_record(snapshot, *pane_id)?;
            let buffer_id = leaf
                .buffer_view
                .as_ref()
                .map(|view| view.buffer_id)
                .ok_or_else(|| MuxError::invalid_input(format!("node {pane_id} is not a pane")))?;
            let buffer = buffer_record(snapshot, buffer_id)?;
            Ok(format!(
                "{}\t{}\t{}\t{}",
                pane_id,
                buffer.id,
                usize::from(snapshot.session.focused_leaf_id == Some(*pane_id)),
                buffer.title
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

fn active_root_window(snapshot: &SessionSnapshot) -> Result<(u32, String)> {
    if let Some((_, tabs)) = root_tabs(snapshot)? {
        let tab = protocol_tab(tabs, tabs.active)
            .ok_or_else(|| MuxError::protocol("session root tabs has invalid active index"))?;
        Ok((tabs.active, tab.title.clone()))
    } else {
        Ok((0, window_title(snapshot, snapshot.session.root_node_id)?))
    }
}

fn root_tabs(
    snapshot: &SessionSnapshot,
) -> Result<Option<(&embers_protocol::NodeRecord, &embers_protocol::TabsRecord)>> {
    let node = node_record(snapshot, snapshot.session.root_node_id)?;
    Ok(node.tabs.as_ref().map(|tabs| (node, tabs)))
}

fn node_record(
    snapshot: &SessionSnapshot,
    node_id: NodeId,
) -> Result<&embers_protocol::NodeRecord> {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| MuxError::not_found(format!("node {node_id} is not present in snapshot")))
}

fn buffer_record(
    snapshot: &SessionSnapshot,
    buffer_id: BufferId,
) -> Result<&embers_protocol::BufferRecord> {
    snapshot
        .buffers
        .iter()
        .find(|buffer| buffer.id == buffer_id)
        .ok_or_else(|| {
            MuxError::not_found(format!("buffer {buffer_id} is not present in snapshot"))
        })
}

fn floating_record(snapshot: &SessionSnapshot, floating_id: FloatingId) -> Result<&FloatingRecord> {
    snapshot
        .floating
        .iter()
        .find(|floating| floating.id == floating_id)
        .ok_or_else(|| {
            MuxError::not_found(format!("popup {floating_id} is not present in snapshot"))
        })
}

fn visible_leaf_ids(snapshot: &SessionSnapshot, node_id: NodeId) -> Result<Vec<NodeId>> {
    let node = node_record(snapshot, node_id)?;
    match node.kind {
        embers_protocol::NodeRecordKind::BufferView => Ok(vec![node.id]),
        embers_protocol::NodeRecordKind::Split => {
            let split = node
                .split
                .as_ref()
                .ok_or_else(|| MuxError::protocol(format!("split node {node_id} is malformed")))?;
            let mut leaves = Vec::new();
            for child_id in &split.child_ids {
                leaves.extend(visible_leaf_ids(snapshot, *child_id)?);
            }
            Ok(leaves)
        }
        embers_protocol::NodeRecordKind::Tabs => {
            let tabs = node
                .tabs
                .as_ref()
                .ok_or_else(|| MuxError::protocol(format!("tabs node {node_id} is malformed")))?;
            let active_child = protocol_tab(tabs, tabs.active).ok_or_else(|| {
                MuxError::protocol(format!("tabs node {node_id} has invalid active index"))
            })?;
            visible_leaf_ids(snapshot, active_child.child_id)
        }
    }
}

fn resolve_window_index(tabs: &embers_protocol::TabsRecord, selector: Option<&str>) -> Result<u32> {
    let Some(selector) = selector else {
        return Ok(tabs.active);
    };

    if let Ok(index) = selector.parse::<u32>() {
        let mut candidates = Vec::new();
        if protocol_tab(tabs, index).is_some() {
            candidates.push(index);
        }
        if let Some(one_based) = index.checked_sub(1)
            && protocol_tab(tabs, one_based).is_some()
        {
            candidates.push(one_based);
        }
        candidates.sort_unstable();
        candidates.dedup();
        return match candidates.as_slice() {
            [only] => Ok(*only),
            [] => Err(MuxError::not_found(format!(
                "window index '{selector}' is out of range"
            ))),
            _ => Err(MuxError::invalid_input(format!(
                "window index '{selector}' is ambiguous between 0-based and 1-based addressing"
            ))),
        };
    }

    let matches = tabs
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.title == selector)
        .map(|(index, _)| u32::try_from(index).expect("tab index fits into protocol width"))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(MuxError::not_found(format!(
            "window '{selector}' was not found"
        ))),
        _ => Err(MuxError::conflict(format!(
            "window title '{selector}' matched multiple root tabs"
        ))),
    }
}

fn resolve_single_window_index(title: &str, selector: Option<&str>) -> Result<u32> {
    let Some(selector) = selector else {
        return Ok(0);
    };

    if let Ok(index) = selector.parse::<u32>() {
        return match index {
            0 | 1 => Ok(0),
            _ => Err(MuxError::not_found(format!(
                "window index '{selector}' is out of range"
            ))),
        };
    }

    if selector == title {
        Ok(0)
    } else {
        Err(MuxError::not_found(format!(
            "window '{selector}' was not found"
        )))
    }
}

fn window_title(snapshot: &SessionSnapshot, node_id: NodeId) -> Result<String> {
    let visible_leaf_ids = visible_leaf_ids(snapshot, node_id)?;
    let leaf_id = snapshot
        .session
        .focused_leaf_id
        .filter(|leaf_id| visible_leaf_ids.contains(leaf_id))
        .or_else(|| visible_leaf_ids.first().copied())
        .ok_or_else(|| MuxError::not_found(format!("window {node_id} has no visible panes")))?;
    let leaf = node_record(snapshot, leaf_id)?;
    let buffer_id = leaf
        .buffer_view
        .as_ref()
        .map(|view| view.buffer_id)
        .ok_or_else(|| MuxError::invalid_input(format!("node {leaf_id} is not a pane leaf")))?;
    Ok(buffer_record(snapshot, buffer_id)?.title.clone())
}

fn protocol_tab(
    tabs: &embers_protocol::TabsRecord,
    index: u32,
) -> Option<&embers_protocol::TabRecord> {
    usize::try_from(index)
        .ok()
        .and_then(|index| tabs.tabs.get(index))
}

fn split_scoped_target(target: Option<&str>) -> (Option<String>, Option<String>) {
    match target {
        Some(target) => {
            if let Some((session, selector)) = target.split_once(':') {
                (Some(session.to_owned()), Some(selector.to_owned()))
            } else {
                (None, Some(target.to_owned()))
            }
        }
        None => (None, None),
    }
}

fn buffer_state_label(state: embers_protocol::BufferRecordState) -> &'static str {
    match state {
        embers_protocol::BufferRecordState::Created => "created",
        embers_protocol::BufferRecordState::Running => "running",
        embers_protocol::BufferRecordState::Interrupted => "interrupted",
        embers_protocol::BufferRecordState::Exited => "exited",
    }
}

fn buffer_kind_label(kind: embers_protocol::BufferRecordKind) -> &'static str {
    match kind {
        embers_protocol::BufferRecordKind::Pty => "pty",
        embers_protocol::BufferRecordKind::Helper => "helper",
    }
}

fn history_scope_label(scope: BufferHistoryScope) -> &'static str {
    match scope {
        BufferHistoryScope::Full => "full",
        BufferHistoryScope::Visible => "visible",
    }
}

fn history_scope(scope: HistoryScopeArg) -> BufferHistoryScope {
    match scope {
        HistoryScopeArg::Full => BufferHistoryScope::Full,
        HistoryScopeArg::Visible => BufferHistoryScope::Visible,
    }
}

fn history_placement(placement: HistoryPlacementArg) -> BufferHistoryPlacement {
    match placement {
        HistoryPlacementArg::Tab => BufferHistoryPlacement::Tab,
        HistoryPlacementArg::Floating => BufferHistoryPlacement::Floating,
    }
}

fn ensure_history_attachment(
    buffer_id: BufferId,
    requested_placement: BufferHistoryPlacement,
    location: &BufferLocation,
) -> Result<()> {
    match (requested_placement, &location.attachment) {
        (BufferHistoryPlacement::Tab, BufferLocationAttachment::Session { .. })
        | (BufferHistoryPlacement::Floating, BufferLocationAttachment::Floating { .. }) => Ok(()),
        (_, BufferLocationAttachment::Detached) => Err(MuxError::protocol(format!(
            "buffer history returned detached helper location for buffer {buffer_id}"
        ))),
        (BufferHistoryPlacement::Tab, BufferLocationAttachment::Floating { .. }) => {
            Err(MuxError::protocol(format!(
                "buffer history returned unexpected attachment for buffer {buffer_id}: expected Tab"
            )))
        }
        (BufferHistoryPlacement::Floating, BufferLocationAttachment::Session { .. }) => {
            Err(MuxError::protocol(format!(
                "buffer history returned unexpected attachment for buffer {buffer_id}: expected Floating"
            )))
        }
    }
}

fn ensure_history_response(
    source_buffer_id: BufferId,
    requested_scope: BufferHistoryScope,
    requested_placement: BufferHistoryPlacement,
    buffer: &embers_protocol::BufferRecord,
    location: &BufferLocation,
    at_root_tab: bool,
) -> Result<()> {
    if buffer.kind != embers_protocol::BufferRecordKind::Helper {
        return Err(MuxError::protocol(format!(
            "buffer history returned buffer {} with unexpected kind {:?}",
            buffer.id, buffer.kind
        )));
    }
    if buffer.helper_source_buffer_id != Some(source_buffer_id) {
        return Err(MuxError::protocol(format!(
            "buffer history returned helper {} for source {:?} instead of {source_buffer_id}",
            buffer.id, buffer.helper_source_buffer_id
        )));
    }
    if buffer.helper_scope != Some(requested_scope) {
        return Err(MuxError::protocol(format!(
            "buffer history returned helper {} with scope {:?} instead of {:?}",
            buffer.id, buffer.helper_scope, requested_scope
        )));
    }

    let (session_id, node_id) = match &location.attachment {
        BufferLocationAttachment::Session {
            session_id,
            node_id,
        }
        | BufferLocationAttachment::Floating {
            session_id,
            node_id,
            ..
        } => (*session_id, *node_id),
        BufferLocationAttachment::Detached => {
            ensure_history_attachment(source_buffer_id, requested_placement, location)?;
            unreachable!("detached buffer history attachments should fail validation")
        }
    };
    if buffer.attachment_node_id != Some(node_id) {
        return Err(MuxError::protocol(format!(
            "buffer history returned helper {} attached to {:?} but location pointed at {node_id}",
            buffer.id, buffer.attachment_node_id
        )));
    }

    ensure_history_attachment(source_buffer_id, requested_placement, location)?;

    if matches!(requested_placement, BufferHistoryPlacement::Tab) && !at_root_tab {
        return Err(MuxError::protocol(format!(
            "buffer history returned helper node {node_id} outside session {session_id} root tabs"
        )));
    }

    Ok(())
}

fn break_destination(destination: BreakDestinationArg) -> NodeBreakDestination {
    match destination {
        BreakDestinationArg::Tab => NodeBreakDestination::Tab,
        BreakDestinationArg::Floating => NodeBreakDestination::Floating,
    }
}

fn join_placement(placement: JoinPlacementArg) -> NodeJoinPlacement {
    match placement {
        JoinPlacementArg::Left => NodeJoinPlacement::Left,
        JoinPlacementArg::Right => NodeJoinPlacement::Right,
        JoinPlacementArg::Up => NodeJoinPlacement::Up,
        JoinPlacementArg::Down => NodeJoinPlacement::Down,
        JoinPlacementArg::TabBefore => NodeJoinPlacement::TabBefore,
        JoinPlacementArg::TabAfter => NodeJoinPlacement::TabAfter,
    }
}

fn split_scoped_required(target: &str, label: &str) -> Result<(Option<String>, String)> {
    let (session, selector) = split_scoped_target(Some(target));
    let selector =
        selector.ok_or_else(|| MuxError::invalid_input(format!("{label} is required")))?;
    Ok((session, selector))
}

fn parse_node_id(raw: &str) -> Result<NodeId> {
    raw.parse::<u64>()
        .map(NodeId)
        .map_err(|_| MuxError::invalid_input(format!("pane target '{raw}' is not a valid pane id")))
}

fn parse_floating_id(raw: &str) -> Result<FloatingId> {
    raw.parse::<u64>().map(FloatingId).map_err(|_| {
        MuxError::invalid_input(format!("popup target '{raw}' is not a valid popup id"))
    })
}

fn buffer_command(command: Vec<String>) -> Vec<String> {
    if command.is_empty() {
        vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())]
    } else {
        command
    }
}

fn default_title(command: &[String], fallback: &str) -> String {
    command
        .first()
        .and_then(|value| {
            Path::new(value)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.to_owned())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use base64::Engine as _;
    use clap::{Parser, error::ErrorKind};
    use embers_core::{ActivityState, BufferId, FloatingId, NodeId, PtySize, SessionId};
    use embers_protocol::{
        BufferLocation, BufferRecord, BufferRecordKind, BufferRecordState, TabRecord, TabsRecord,
    };
    #[cfg(windows)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(windows)]
    use std::os::windows::ffi::OsStringExt;
    use std::path::Path;

    use super::{
        BreakDestinationArg, BufferCommand, Cli, Command, HistoryPlacementArg, HistoryScopeArg,
        JoinPlacementArg, NodeCommand, ensure_history_attachment, format_buffer_details,
        resolve_window_index, split_scoped_required, split_scoped_target,
    };

    #[test]
    fn parser_accepts_global_socket_after_subcommand() {
        let cli = Cli::try_parse_from([
            "embers",
            "new-window",
            "--socket",
            "/tmp/mux.sock",
            "--title",
            "logs",
            "--",
            "/bin/sh",
        ])
        .expect("cli parses");

        match cli.command {
            Some(super::Command::NewWindow { title, command, .. }) => {
                assert_eq!(title.as_deref(), Some("logs"));
                assert_eq!(command, vec!["/bin/sh"]);
            }
            other => panic!("expected new-window command, got {other:?}"),
        }
    }

    #[test]
    fn runtime_keeper_uses_distinct_keeper_socket_flag() {
        let cli = Cli::try_parse_from([
            "embers",
            "__runtime-keeper",
            "--socket",
            "/tmp/global.sock",
            "--keeper-socket",
            "/tmp/keeper.sock",
            "--cols",
            "80",
            "--rows",
            "24",
            "--",
            "/bin/sh",
        ])
        .expect("cli parses");

        assert_eq!(cli.socket.as_deref(), Some(Path::new("/tmp/global.sock")));
        match cli.command {
            Some(super::Command::RuntimeKeeper {
                keeper_socket,
                cols,
                rows,
                command,
                ..
            }) => {
                assert_eq!(keeper_socket, Path::new("/tmp/keeper.sock"));
                assert_eq!((cols, rows), (80, 24));
                assert_eq!(command, vec!["/bin/sh"]);
            }
            other => panic!("expected runtime keeper command, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn runtime_keeper_env_values_decode_base64_losslessly() {
        let (key, value) = super::parse_env_arg("KEY=base64:AP8=").expect("env parses");
        assert_eq!(key, "KEY");
        assert_eq!(value, OsString::from_vec(vec![0, 255]));
    }

    #[cfg(windows)]
    #[test]
    fn runtime_keeper_env_values_decode_utf16le_losslessly() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([0x00, 0xD8, 0x61, 0x00]);
        let (key, value) =
            super::parse_env_arg(&format!("KEY=base64:{encoded}")).expect("env parses");
        assert_eq!(key, "KEY");
        assert_eq!(value, OsString::from_wide(&[0xD800, 0x0061]));
    }

    #[test]
    fn scoped_targets_split_session_prefix() {
        assert_eq!(
            split_scoped_required("main:2", "window target").expect("target parses"),
            (Some("main".to_owned()), "2".to_owned())
        );
        assert_eq!(split_scoped_target(Some("3")), (None, Some("3".to_owned())));
    }

    #[test]
    fn numeric_window_indices_report_ambiguity() {
        let tabs = TabsRecord {
            active: 0,
            tabs: vec![
                TabRecord {
                    title: "one".to_owned(),
                    child_id: NodeId(1),
                },
                TabRecord {
                    title: "two".to_owned(),
                    child_id: NodeId(2),
                },
                TabRecord {
                    title: "three".to_owned(),
                    child_id: NodeId(3),
                },
            ],
        };

        let error = resolve_window_index(&tabs, Some("1")).expect_err("index is ambiguous");
        assert!(
            error
                .to_string()
                .contains("ambiguous between 0-based and 1-based")
        );
        assert_eq!(
            resolve_window_index(&tabs, Some("0")).expect("zero resolves"),
            0
        );
    }

    #[test]
    fn buffer_details_location_row_uses_single_tab_delimiter() {
        let details = format_buffer_details(
            &BufferRecord {
                id: BufferId(7),
                title: "logs".to_owned(),
                command: vec!["/bin/sh".to_owned()],
                cwd: Some("/tmp".to_owned()),
                kind: BufferRecordKind::Pty,
                pid: None,
                env: Default::default(),
                state: BufferRecordState::Running,
                attachment_node_id: Some(NodeId(3)),
                read_only: false,
                helper_source_buffer_id: None,
                helper_scope: None,
                pty_size: PtySize::new(80, 24),
                activity: ActivityState::Idle,
                last_snapshot_seq: 0,
                exit_code: None,
            },
            &BufferLocation::session(BufferId(7), SessionId(1), NodeId(3)),
        );

        let location_line = details
            .lines()
            .find(|line| line.starts_with("location\t"))
            .expect("location row present");
        assert_eq!(location_line.matches('\t').count(), 1);
    }

    #[test]
    fn buffer_details_command_row_preserves_argument_boundaries() {
        let details = format_buffer_details(
            &BufferRecord {
                id: BufferId(8),
                title: "script".to_owned(),
                command: vec![
                    "/bin/sh".to_owned(),
                    "-lc".to_owned(),
                    "printf '%s\\n' 'hello world'".to_owned(),
                ],
                cwd: None,
                kind: BufferRecordKind::Pty,
                pid: None,
                env: Default::default(),
                state: BufferRecordState::Running,
                attachment_node_id: Some(NodeId(4)),
                read_only: false,
                helper_source_buffer_id: None,
                helper_scope: None,
                pty_size: PtySize::new(80, 24),
                activity: ActivityState::Idle,
                last_snapshot_seq: 0,
                exit_code: None,
            },
            &BufferLocation::session(BufferId(8), SessionId(1), NodeId(4)),
        );

        let command_line = details
            .lines()
            .find(|line| line.starts_with("command\t"))
            .expect("command row present");
        let serialized_args = command_line
            .strip_prefix("command\t")
            .expect("command row prefix present");
        let command: Vec<String> =
            serde_json::from_str(serialized_args).expect("command row uses JSON");
        assert_eq!(
            command,
            vec![
                "/bin/sh".to_owned(),
                "-lc".to_owned(),
                "printf '%s\\n' 'hello world'".to_owned(),
            ]
        );
    }

    #[test]
    fn buffer_details_title_and_cwd_rows_use_json() {
        let details = format_buffer_details(
            &BufferRecord {
                id: BufferId(9),
                title: "build\tlogs\nstderr".to_owned(),
                command: Vec::new(),
                cwd: Some("/tmp/work\tspace\nhere".to_owned()),
                kind: BufferRecordKind::Pty,
                pid: None,
                env: Default::default(),
                state: BufferRecordState::Running,
                attachment_node_id: Some(NodeId(5)),
                read_only: false,
                helper_source_buffer_id: None,
                helper_scope: None,
                pty_size: PtySize::new(80, 24),
                activity: ActivityState::Idle,
                last_snapshot_seq: 0,
                exit_code: None,
            },
            &BufferLocation::session(BufferId(9), SessionId(1), NodeId(5)),
        );

        let title_line = details
            .lines()
            .find(|line| line.starts_with("title\t"))
            .expect("title row present");
        let cwd_line = details
            .lines()
            .find(|line| line.starts_with("cwd\t"))
            .expect("cwd row present");

        let title = title_line
            .strip_prefix("title\t")
            .expect("title row prefix present");
        let cwd = cwd_line
            .strip_prefix("cwd\t")
            .expect("cwd row prefix present");

        assert_eq!(
            serde_json::from_str::<String>(title).expect("title row uses JSON"),
            "build\tlogs\nstderr"
        );
        assert_eq!(
            serde_json::from_str::<String>(cwd).expect("cwd row uses JSON"),
            "/tmp/work\tspace\nhere"
        );
    }

    #[test]
    fn buffer_subcommands_parse_expected_flags_and_defaults() {
        let history =
            Cli::try_parse_from(["embers", "buffer", "history", "7"]).expect("history parses");
        match history.command {
            Some(Command::Buffer {
                command:
                    BufferCommand::History {
                        buffer_id,
                        scope,
                        placement,
                        client,
                    },
            }) => {
                assert_eq!(buffer_id, 7);
                assert!(matches!(scope, HistoryScopeArg::Full));
                assert!(matches!(placement, HistoryPlacementArg::Tab));
                assert_eq!(client, None);
            }
            other => panic!("expected buffer history command, got {other:?}"),
        }

        let history_with_flags = Cli::try_parse_from([
            "embers",
            "buffer",
            "history",
            "9",
            "--scope",
            "visible",
            "--placement",
            "floating",
            "--client",
            "5",
        ])
        .expect("history flags parse");
        match history_with_flags.command {
            Some(Command::Buffer {
                command:
                    BufferCommand::History {
                        buffer_id,
                        scope,
                        placement,
                        client,
                    },
            }) => {
                assert_eq!(buffer_id, 9);
                assert!(matches!(scope, HistoryScopeArg::Visible));
                assert!(matches!(placement, HistoryPlacementArg::Floating));
                assert_eq!(client.map(std::num::NonZeroU64::get), Some(5));
            }
            other => panic!("expected flagged buffer history command, got {other:?}"),
        }

        let reveal = Cli::try_parse_from(["embers", "buffer", "reveal", "11", "--client", "6"])
            .expect("reveal parses");
        match reveal.command {
            Some(Command::Buffer {
                command: BufferCommand::Reveal { buffer_id, client },
            }) => {
                assert_eq!(buffer_id, 11);
                assert_eq!(client.map(std::num::NonZeroU64::get), Some(6));
            }
            other => panic!("expected buffer reveal command, got {other:?}"),
        }
    }

    #[test]
    fn history_attachment_validation_accepts_matching_locations() {
        assert!(
            ensure_history_attachment(
                BufferId(7),
                embers_protocol::BufferHistoryPlacement::Tab,
                &BufferLocation::session(BufferId(70), SessionId(1), NodeId(3)),
            )
            .is_ok()
        );
        assert!(
            ensure_history_attachment(
                BufferId(7),
                embers_protocol::BufferHistoryPlacement::Floating,
                &BufferLocation::floating(BufferId(71), SessionId(1), NodeId(4), FloatingId(5)),
            )
            .is_ok()
        );
    }

    #[test]
    fn history_attachment_validation_rejects_mismatched_locations() {
        let floating_error = ensure_history_attachment(
            BufferId(7),
            embers_protocol::BufferHistoryPlacement::Floating,
            &BufferLocation::session(BufferId(70), SessionId(1), NodeId(3)),
        )
        .expect_err("floating history should reject tab helper");
        assert!(floating_error.to_string().contains("expected Floating"));

        let tab_error = ensure_history_attachment(
            BufferId(7),
            embers_protocol::BufferHistoryPlacement::Tab,
            &BufferLocation::floating(BufferId(71), SessionId(1), NodeId(4), FloatingId(5)),
        )
        .expect_err("tab history should reject floating helper");
        assert!(tab_error.to_string().contains("expected Tab"));
    }

    #[test]
    fn node_subcommands_parse_expected_flags_and_reject_legacy_shortcuts() {
        let break_to_floating =
            Cli::try_parse_from(["embers", "node", "break", "11", "--to", "floating"])
                .expect("break parses");
        match break_to_floating.command {
            Some(Command::Node {
                command:
                    NodeCommand::Break {
                        node_id,
                        destination,
                    },
            }) => {
                assert_eq!(node_id, 11);
                assert!(matches!(destination, BreakDestinationArg::Floating));
            }
            other => panic!("expected node break command, got {other:?}"),
        }

        let join_after = Cli::try_parse_from([
            "embers",
            "node",
            "join-buffer",
            "21",
            "34",
            "--as",
            "tab-after",
        ])
        .expect("join-buffer tab-after parses");
        match join_after.command {
            Some(Command::Node {
                command:
                    NodeCommand::JoinBuffer {
                        node_id,
                        buffer_id,
                        placement,
                    },
            }) => {
                assert_eq!(node_id, 21);
                assert_eq!(buffer_id, 34);
                assert!(matches!(placement, JoinPlacementArg::TabAfter));
            }
            other => panic!("expected node join-buffer command, got {other:?}"),
        }

        let join_default = Cli::try_parse_from(["embers", "node", "join-buffer", "21", "34"])
            .expect("join-buffer default parses");
        match join_default.command {
            Some(Command::Node {
                command: NodeCommand::JoinBuffer { placement, .. },
            }) => {
                assert!(matches!(placement, JoinPlacementArg::TabAfter));
            }
            other => panic!("expected default node join-buffer command, got {other:?}"),
        }

        let join_before = Cli::try_parse_from([
            "embers",
            "node",
            "join-buffer",
            "21",
            "34",
            "--as",
            "tab-before",
        ])
        .expect("join-buffer tab-before parses");
        match join_before.command {
            Some(Command::Node {
                command: NodeCommand::JoinBuffer { placement, .. },
            }) => {
                assert!(matches!(placement, JoinPlacementArg::TabBefore));
            }
            other => panic!("expected node join-buffer command, got {other:?}"),
        }

        let legacy_tab_after =
            Cli::try_parse_from(["embers", "node", "join-buffer", "21", "34", "--tab-after"])
                .expect_err("legacy --tab-after should be rejected");
        assert_eq!(legacy_tab_after.kind(), ErrorKind::UnknownArgument);

        let legacy_tab_before =
            Cli::try_parse_from(["embers", "node", "join-buffer", "21", "34", "--tab-before"])
                .expect_err("legacy --tab-before should be rejected");
        assert_eq!(legacy_tab_before.kind(), ErrorKind::UnknownArgument);
    }
}
