use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

mod client;
mod daemon;
mod peer;
mod protocol;
mod tunnel;

#[derive(Parser)]
#[command(name = "pai-sho")]
#[command(about = "Peer-to-peer TCP port forwarding over iroh")]
struct Cli {
    /// Path to Unix socket
    #[arg(long, default_value = "/tmp/pai-sho.sock")]
    socket: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon
    Daemon {
        /// Host address for forwarding exposed ports
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Data directory for persistent state (default: ~/.pai-sho)
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Add a peer
    AddPeer {
        /// Peer's iroh ticket
        ticket: String,
        /// Friendly name for this peer
        #[arg(long)]
        name: String,
        /// Local IP to bind peer's ports (e.g., 127.0.0.2)
        #[arg(long)]
        ip: Ipv4Addr,
    },

    /// Remove a peer
    RemovePeer {
        /// Peer name
        name: String,
    },

    /// Expose a port to peers
    Expose {
        /// Port number to expose
        port: u16,
    },

    /// Stop exposing a port
    Unexpose {
        /// Port number to unexpose
        port: u16,
    },

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
