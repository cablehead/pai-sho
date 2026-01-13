//! CLI client - sends commands to daemon over Unix socket.

use crate::protocol::{Request, Response};
use crate::Command;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn send_command(socket_path: &Path, command: Command) -> Result<()> {
    let request = match command {
        Command::AddPeer { ticket } => Request::AddPeer { ticket },
        Command::RemovePeer { ticket } => Request::RemovePeer { ticket },
        Command::Expose { port } => Request::Expose { port },
        Command::Unexpose { port } => Request::Unexpose { port },
        Command::List => Request::List,
        Command::Ticket => Request::Ticket,
        Command::Daemon { .. } => unreachable!("daemon handled separately"),
    };

    let stream = UnixStream::connect(socket_path)
        .await
        .context("failed to connect to daemon - is it running?")?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Send request
    let request_json = serde_json::to_string(&request)?;
    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read response
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let response: Response = serde_json::from_str(&line)?;

    // Print response
    match response {
        Response::Ok => println!("OK"),
        Response::Ticket(ticket) => println!("{}", ticket),
        Response::List(info) => {
            println!("PEERS:");
            for peer in &info.peers {
                let status = if peer.connected {
                    "connected"
                } else {
                    "disconnected"
                };
                println!(
                    "  {} ({}) - ports: {:?}",
                    peer.endpoint_id, status, peer.exposed_ports
                );
            }
            println!("\nEXPOSED: {:?}", info.exposed_ports);
            println!("\nBINDINGS:");
            for binding in &info.bindings {
                println!("  127.0.0.1:{}", binding.port);
            }
        }
        Response::Error(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
