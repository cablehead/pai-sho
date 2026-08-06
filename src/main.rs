use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::{IpAddr, SocketAddr};

mod client;
mod daemon;
mod enroll;
mod grants;
mod netstack;
mod peer;
mod protocol;
mod resolver;
mod surface;
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
        /// Serve the owned `*.ps.internal` resolver on this UDP address (e.g.
        /// 127.0.0.1:5353). Off when omitted.
        #[arg(long)]
        resolver: Option<SocketAddr>,
        /// Use the TUN owned-network backend on this pre-created device (e.g.
        /// `ps0`). Surfaces bind on the TUN via a userspace stack, and the
        /// `.ps.internal` resolver answers in-stack on 10.99.0.53:53. Loopback when omitted.
        #[arg(long)]
        tun: Option<String>,
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

    /// Expose a port to specific peers (a directed grant)
    Expose {
        port: u16,
        /// Peer key(s) to grant the port to; defaults to all known peers
        #[arg(long = "to")]
        to: Vec<String>,
    },

    /// Revoke grants for a port
    Unexpose {
        port: u16,
        /// Revoke only this peer's grant; defaults to every grant for the port
        #[arg(long = "to")]
        to: Option<String>,
    },

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

    /// Pin a peer by its key under a label, no token (host-attested
    /// enrollment). The key is authorized when the peer dials in; nothing
    /// secret travels into the workload. See
    /// docs/adr/0003-host-attested-enrollment.md.
    Pin {
        /// Peer's key (endpoint ID), e.g. reported by the workload over vsock
        key: String,
        /// Label to pin the peer under
        #[arg(long)]
        label: String,
    },

    /// Project a peer's surface to a local address so its ports are reachable.
    /// See docs/adr/0004-peer-surfaces.md.
    Project {
        /// Peer to project (an endpoint key or an enrollment label)
        peer: String,
        /// Local address to bind at; allocated from 127.0.1.0/24 if omitted
        #[arg(long)]
        ip: Option<IpAddr>,
        /// DNS handle to add in /etc/hosts (e.g. `broker`)
        #[arg(long = "as")]
        name: Option<String>,
    },

    /// Take a peer's surface down: unbind its ports, drop its address and name
    Unproject {
        /// Peer to unproject (an endpoint key or an enrollment label)
        peer: String,
    },

    /// List surfaces: every known peer and its projection, if any
    Surfaces,
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
            resolver,
            tun,
        } => {
            daemon::run(
                host,
                socket_path,
                peers,
                ports,
                key_path,
                enroll,
                resolver,
                tun,
            )
            .await?;
        }
        _ => {
            client::send_command(socket_path, cli.command).await?;
        }
    }

    Ok(())
}
