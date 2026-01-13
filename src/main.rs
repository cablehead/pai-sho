use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::IpAddr;
use std::path::PathBuf;

mod client;
mod daemon;
mod peer;
mod protocol;
mod tunnel;

#[derive(Parser)]
#[command(name = "pai-sho")]
#[command(about = "P2P TCP port forwarding over iroh")]
struct Cli {
    /// Path to Unix socket
    #[arg(long, default_value = "/tmp/pai-sho.sock")]
    socket: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon
    Daemon {
        /// Host address for forwarding exposed ports
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Data directory (default: ~/.pai-sho)
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Add a peer (returns assigned IP)
    AddPeer {
        /// Peer's ticket (endpoint ID)
        ticket: String,
    },

    /// Remove a peer
    RemovePeer {
        /// Peer's ticket
        ticket: String,
    },

    /// Expose a port to peers
    Expose { port: u16 },

    /// Stop exposing a port
    Unexpose { port: u16 },

    /// List peers, exposed ports, and bindings
    List,

    /// Print daemon's ticket
    Ticket,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("pai_sho=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Daemon { host, data_dir } => {
            daemon::run(host, &cli.socket, data_dir).await?;
        }
        _ => {
            client::send_command(&cli.socket, cli.command).await?;
        }
    }

    Ok(())
}
