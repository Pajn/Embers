use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mux_core::{MuxError, Result, new_request_id};
use mux_protocol::{ClientMessage, PingRequest, ProtocolClient, ServerResponse};

#[derive(Debug, Parser)]
#[command(
    name = "mux-cli",
    about = "Phase-0 control surface for the embers workspace"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Ping {
        #[arg(long)]
        socket: PathBuf,
        #[arg(default_value = "phase0")]
        payload: String,
    },
}

pub async fn execute(cli: Cli) -> Result<String> {
    match cli.command {
        Command::Ping { socket, payload } => ping(socket, payload).await,
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let output = execute(cli).await?;
    println!("{output}");
    Ok(())
}

async fn ping(socket: PathBuf, payload: String) -> Result<String> {
    let mut client = ProtocolClient::connect(&socket)
        .await
        .map_err(|error| MuxError::transport(error.to_string()))?;
    let request = ClientMessage::Ping(PingRequest {
        request_id: new_request_id(),
        payload: payload.clone(),
    });

    match client
        .request(&request)
        .await
        .map_err(|error| MuxError::transport(error.to_string()))?
    {
        ServerResponse::Pong(response) => Ok(format!("pong {}", response.payload)),
        ServerResponse::Error(response) => Err(response.error.into()),
        other => Err(MuxError::protocol(format!(
            "unexpected response to ping request: {other:?}"
        ))),
    }
}
