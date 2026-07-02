use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::IpAddr;

mod client;
mod daemon;
mod enroll;
mod peer;
mod protocol;
mod tunnel;

#[derive(Parser)]
#[clap(
    name = "pai-sho",
    about = "What happens when you want dumbpipe to stay running, handle a few ports at once, and reconnect when your laptop wakes up",
    version
)]
struct Cli {
    /// Path to Unix socket
    #[arg(long, default_value = "/tmp/pai-sho.sock")]
    socket: String,

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
        /// Add peer(s) on startup
        #[arg(short = 'a', long = "add")]
        peers: Vec<String>,
        /// Expose port(s) on startup (repeat or comma-separate)
        #[arg(short = 'e', long = "expose", value_delimiter = ',')]
        ports: Vec<u16>,
        /// Path to the daemon's secret key (created if missing).
        /// Defaults to $XDG_STATE_HOME/pai-sho/key (~/.local/state/pai-sho/key)
        #[arg(long = "key")]
        key_path: Option<std::path::PathBuf>,
        /// One-time enrollment token to present to added peers
        #[arg(long)]
        enroll: Option<String>,
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

    /// Mint a one-time enrollment token (valid 5 minutes)
    GrantToken {
        /// Label to pin the enrolling peer under
        #[arg(long)]
        label: String,
    },
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

    let socket_path = std::path::Path::new(&cli.socket);

    match cli.command {
        Command::Daemon {
            host,
            peers,
            ports,
            key_path,
            enroll,
        } => {
            daemon::run(host, socket_path, peers, ports, key_path, enroll).await?;
        }
        _ => {
            client::send_command(socket_path, cli.command).await?;
        }
    }

    Ok(())
}
