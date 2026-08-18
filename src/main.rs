use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};
use std::net::{IpAddr, SocketAddr};

mod client;
mod core;
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
        /// Serve the owned `*.pai-sho` resolver on this UDP address (e.g.
        /// 127.0.0.1:5353). Off when omitted.
        #[arg(long)]
        resolver: Option<SocketAddr>,
        /// Use the TUN owned-network backend on this pre-created device (e.g.
        /// `ps0`). Surfaces bind on the TUN via a userspace stack, and the
        /// `.pai-sho` resolver answers in-stack on 10.99.0.53:53. Loopback when omitted.
        #[arg(long)]
        tun: Option<String>,
        /// Username to own the control socket, chowned right after bind (before
        /// accept), so the CLI needs no sudo when the daemon runs as root.
        #[arg(long = "socket-owner")]
        socket_owner: Option<String>,
        /// Octal mode for the control socket, e.g. `660` (chmod'd after bind).
        #[arg(long = "socket-mode")]
        socket_mode: Option<String>,
        /// This node's own name; the owned resolver answers `<name>.pai-sho`
        /// with 127.0.0.1, so local traffic uses the same origin peers do.
        #[arg(long)]
        name: Option<String>,
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
    #[command(group = ArgGroup::new("grantees").required(true).args(["to", "all"]))]
    Expose {
        port: u16,
        /// Peer key(s) to grant the port to
        #[arg(long = "to")]
        to: Vec<String>,
        /// Grant to every peer known right now. Not a standing rule: a peer
        /// admitted later gets nothing.
        #[arg(long = "all")]
        all: bool,
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
            socket_owner,
            socket_mode,
            name,
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
                socket_owner,
                socket_mode,
                name,
            )
            .await?;
        }
        _ => {
            client::send_command(socket_path, cli.command).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn expose_needs_a_grantee() {
        assert!(parse(&["pai-sho", "expose", "5555"]).is_err());
    }

    #[test]
    fn expose_takes_a_named_peer() {
        let cli = parse(&["pai-sho", "expose", "5555", "--to", "abc"]).unwrap();
        match cli.command {
            Command::Expose { port, to, all } => {
                assert_eq!(port, 5555);
                assert_eq!(to, vec!["abc".to_string()]);
                assert!(!all);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_takes_several_named_peers() {
        let cli = parse(&["pai-sho", "expose", "5555", "--to", "a", "--to", "b"]).unwrap();
        match cli.command {
            Command::Expose { to, .. } => assert_eq!(to, vec!["a".to_string(), "b".to_string()]),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_takes_all() {
        let cli = parse(&["pai-sho", "expose", "5555", "--all"]).unwrap();
        match cli.command {
            Command::Expose { to, all, .. } => {
                assert!(all);
                assert!(to.is_empty());
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_refuses_both_at_once() {
        assert!(parse(&["pai-sho", "expose", "5555", "--to", "a", "--all"]).is_err());
    }

    #[test]
    fn unexpose_needs_no_grantee() {
        let cli = parse(&["pai-sho", "unexpose", "5555"]).unwrap();
        match cli.command {
            Command::Unexpose { port, to } => {
                assert_eq!(port, 5555);
                assert!(to.is_none());
            }
            _ => panic!("wrong command"),
        }
    }
}
