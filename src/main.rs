use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};
use std::net::{IpAddr, SocketAddr};

mod client;
mod core;
mod daemon;
mod enroll;
mod grants;
#[cfg(test)]
mod live_tests;
mod netstack;
mod peer;
mod protocol;
mod resolver;
mod surface;
mod tunnel;

#[derive(Parser)]
#[clap(
    name = "pai-sho",
    about = "Reach a machine's ports from your laptop, each under its own name",
    long_about = "Spin up a box in the middle of nowhere, with no way in. Drop one \
binary on it. No open ports, no public IP: it dials home and punches through. \
Reach your boxes from your laptop, each under its own name like \
vibenv-ndyg.pai-sho.\n\n\
Access is default deny and per peer. You grant a specific port to a specific \
peer's key, and that peer alone can reach it.",
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
        /// Accept an invitation, or a peer's key, on startup (repeatable)
        #[arg(short = 'a', long = "accept")]
        accept: Vec<String>,
        /// Expose port(s) on startup (repeat or comma-separate)
        #[arg(short = 'e', long = "expose", value_delimiter = ',')]
        ports: Vec<u16>,
        /// Path to the daemon's secret key (created if missing).
        /// Defaults to $XDG_STATE_HOME/pai-sho/key (~/.local/state/pai-sho/key)
        #[arg(long = "key")]
        key_path: Option<std::path::PathBuf>,
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

    /// Extend an invitation. Without a key, prints a one-time invitation valid
    /// 5 minutes. With one, authorizes that key alone and creates no secret.
    Invite {
        /// Peer's key, for when nothing secret can safely travel to it.
        /// See docs/adr/0003-host-attested-enrollment.md
        key: Option<String>,
        /// What to call the peer that takes this up
        #[arg(long = "as")]
        name: Option<String>,
        /// Port(s) to grant along with the invitation (repeat or comma-separate)
        #[arg(long = "expose", value_delimiter = ',')]
        expose: Vec<u16>,
    },

    /// Take up an invitation, or reach a peer you already know by key
    Accept {
        /// An invitation, or a bare key
        handle: String,
        /// What to call this peer locally
        #[arg(long = "as")]
        name: Option<String>,
    },

    /// Forget a peer: close it, unbind its ports, revoke its grants
    Forget {
        /// Peer to forget (a key or a name)
        peer: String,
    },

    /// Grant a local port to named peers. Nothing is reachable without one.
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

    /// Revoke grants for a port. Bare, it revokes every grant for that port.
    Unexpose {
        port: u16,
        /// Revoke only this peer's grant; defaults to every grant for the port
        #[arg(long = "to")]
        to: Option<String>,
    },

    /// List peers, their grants, and where their ports are bound
    List,

    /// Print this daemon's key
    Key,

    /// Override where a peer's ports are bound. Peers are projected
    /// automatically, so this is only needed to pin an address or rename one.
    /// See docs/adr/0004-peer-surfaces.md.
    Project {
        /// Peer to project (a key or the name you gave it)
        peer: String,
        /// Local address to bind at; allocated from 127.0.1.0/24 if omitted
        #[arg(long)]
        ip: Option<IpAddr>,
        /// Rename this peer's surface (e.g. `broker`)
        #[arg(long = "as")]
        name: Option<String>,
    },

    /// Take a peer's surface down: unbind its ports, drop its address and name
    Unproject {
        /// Peer to unproject (a key or the name you gave it)
        peer: String,
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
            accept,
            ports,
            key_path,
            resolver,
            tun,
            socket_owner,
            socket_mode,
            name,
        } => {
            daemon::run(
                host,
                socket_path,
                accept,
                ports,
                key_path,
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
mod cli_tests {
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

    #[test]
    fn invite_needs_nothing() {
        let cli = parse(&["pai-sho", "invite"]).unwrap();
        match cli.command {
            Command::Invite { key, name, expose } => {
                assert!(key.is_none());
                assert!(name.is_none());
                assert!(expose.is_empty());
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn invite_takes_a_key_a_name_and_ports() {
        let cli = parse(&[
            "pai-sho",
            "invite",
            "abc",
            "--as",
            "rustdev",
            "--expose",
            "3001,7331",
        ])
        .unwrap();
        match cli.command {
            Command::Invite { key, name, expose } => {
                assert_eq!(key, Some("abc".to_string()));
                assert_eq!(name, Some("rustdev".to_string()));
                assert_eq!(expose, vec![3001, 7331]);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn accept_needs_a_handle() {
        assert!(parse(&["pai-sho", "accept"]).is_err());
    }

    #[test]
    fn accept_takes_a_handle_and_a_name() {
        let cli = parse(&["pai-sho", "accept", "abc.def", "--as", "buildbox"]).unwrap();
        match cli.command {
            Command::Accept { handle, name } => {
                assert_eq!(handle, "abc.def");
                assert_eq!(name, Some("buildbox".to_string()));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn the_old_names_are_gone() {
        for old in [
            "ticket",
            "grant-token",
            "pin",
            "add-peer",
            "remove-peer",
            "surfaces",
        ] {
            assert!(parse(&["pai-sho", old]).is_err(), "{} still parses", old);
        }
    }

    #[test]
    fn the_daemon_accepts_invitations() {
        let cli = parse(&["pai-sho", "daemon", "--accept", "abc.def", "-e", "3001"]).unwrap();
        match cli.command {
            Command::Daemon { accept, ports, .. } => {
                assert_eq!(accept, vec!["abc.def".to_string()]);
                assert_eq!(ports, vec![3001]);
            }
            _ => panic!("wrong command"),
        }
    }
}
