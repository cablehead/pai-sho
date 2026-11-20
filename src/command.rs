use clap::{ArgGroup, Subcommand};
use std::net::{IpAddr, SocketAddr};

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonService {
    /// Install and start the launchd service. Copies this binary to
    /// /usr/local/libexec/pai-sho. The socket is /tmp/pai-sho.sock, the key
    /// is /usr/local/var/pai-sho/op.key, and .pai-sho resolves at 10.99.0.53.
    Install,
    /// Start the launchd service. Restarts it when it is already loaded, so
    /// the socket is chowned to the user logged in now.
    Up,
    /// Stop the launchd service. It stays stopped across a reboot.
    Down,
    /// Unload the launchd service and delete its files. The key is kept.
    Remove,
    /// Print the launchd service state as JSON.
    Status,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon, or manage its macOS launchd service
    #[command(args_conflicts_with_subcommands = true)]
    Daemon {
        #[command(subcommand)]
        service: Option<DaemonService>,
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
        /// Defaults to $XDG_STATE_HOME/pai-sho/key (~/.local/state/pai-sho/key),
        /// or %LOCALAPPDATA%\pai-sho\key on Windows
        #[arg(long = "key")]
        key_path: Option<std::path::PathBuf>,
        /// Serve the owned `*.pai-sho` resolver on this UDP address (e.g.
        /// 127.0.0.1:5353). Off when omitted.
        #[arg(long)]
        resolver: Option<SocketAddr>,
        /// Put peers on a private network over this pre-created TUN device (e.g.
        /// `ps0`). Their ports bind there via a userspace stack, and the
        /// `.pai-sho` resolver answers at 10.99.0.53:53. Loopback when omitted.
        #[arg(long)]
        tun: Option<String>,
        /// Username to own the control socket, chowned right after bind (before
        /// accept), so the CLI needs no sudo when the daemon runs as root.
        #[arg(long = "socket-owner")]
        socket_owner: Option<String>,
        /// Octal mode for the control socket, e.g. `660` (chmod'd after bind).
        #[arg(long = "socket-mode")]
        socket_mode: Option<String>,
        /// This node's own name. The resolver answers `<name>.pai-sho` with
        /// 127.0.0.1, so a service has the same origin locally and from a peer.
        #[arg(long)]
        name: Option<String>,
    },

    /// Invite a peer. Without a key, prints a one-time invitation valid 5
    /// minutes. With a key, authorizes that key alone and creates no secret.
    Invite {
        /// Peer's key, for when no secret can safely be sent to it.
        /// See docs/adr/0003-host-attested-enrollment.md
        key: Option<String>,
        /// What to call the peer that takes this up
        #[arg(long = "as")]
        name: Option<String>,
        /// Port(s) to grant along with the invitation (repeat or comma-separate)
        #[arg(long = "expose", value_delimiter = ',')]
        expose: Vec<u16>,
        /// Also draw the invitation as a QR code, for a phone to scan off the
        /// screen. Drawn for a dark terminal
        #[arg(long)]
        qr: bool,
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

    /// Call a peer by a new name, in `list`, `--to` and `<name>.pai-sho`
    Rename {
        /// Peer to rename (a key or a name)
        peer: String,
        /// Its new name
        name: String,
    },

    /// Grant a local port to specific peers. Nothing is reachable without a grant.
    #[command(group = ArgGroup::new("grantees").required(true).args(["to", "all"]))]
    Expose {
        /// Local port to grant
        port: u16,
        /// Peer key(s) or name(s) to grant the port to
        #[arg(long = "to")]
        to: Vec<String>,
        /// Grant to every peer known right now. A peer admitted later gets
        /// nothing.
        #[arg(long = "all")]
        all: bool,
    },

    /// Revoke grants for a port. Without --to, every grant for that port.
    Unexpose {
        /// Local port to revoke
        port: u16,
        /// Revoke only this peer's grant (key or name); defaults to every
        /// grant for the port
        #[arg(long = "to")]
        to: Option<String>,
    },

    /// List peers, their grants, and where their ports are bound
    List,

    /// Print this daemon's key
    Key,

    /// Override where a peer's ports are bound. Peers get an address and a name
    /// automatically, so this is only needed to pin one or rename it.
    /// See docs/adr/0004-peer-surfaces.md.
    Project {
        /// Peer to project (a key or the name you gave it)
        peer: String,
        /// Local address to bind at; allocated from 127.0.1.0/24 if omitted
        #[arg(long)]
        ip: Option<IpAddr>,
        /// Rename this peer (e.g. `broker`)
        #[arg(long = "as")]
        name: Option<String>,
    },

    /// Take a peer's surface down: unbind its ports, drop its address and name
    Unproject {
        /// Peer to unproject (a key or the name you gave it)
        peer: String,
    },
}
