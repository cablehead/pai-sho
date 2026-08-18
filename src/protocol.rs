//! Protocol definitions for daemon<->client and peer<->peer communication.

use serde::{Deserialize, Serialize};

/// ALPN protocol identifier
pub const ALPN: &[u8] = b"PAI_SHO/1";

// ============================================================================
// Client <-> Daemon (over Unix socket)
// ============================================================================

/// Request from CLI client to daemon
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Extend an invitation. With a key, address it to that key and return
    /// nothing; without one, mint a code and return the invitation.
    Invite {
        key: Option<String>,
        name: Option<String>,
        /// Ports granted to whoever takes the invitation up
        expose: Vec<u16>,
    },
    /// Take up an invitation, or reach a peer known by key
    Accept {
        handle: String,
        name: Option<String>,
    },
    /// Forget a peer
    Forget {
        peer: String,
    },
    /// Grant `port` to `to`, or to every peer known right now when `all`
    Expose {
        port: u16,
        to: Vec<String>,
        all: bool,
    },
    /// Revoke grants for `port`; `to` limits it to one grantee
    Unexpose {
        port: u16,
        to: Option<String>,
    },
    List,
    /// Print this daemon's key
    Key,
    /// Project a peer's surface to a local address. `peer` is a key or label;
    /// `ip` is chosen if given, else allocated; `name` adds a /etc/hosts handle.
    Project {
        peer: String,
        ip: Option<String>,
        name: Option<String>,
    },
    /// Take a peer's surface down: unbind its ports, drop the address and name.
    Unproject {
        peer: String,
    },
}

/// Response from daemon to CLI client
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Key(String),
    List(ListInfo),
    Invite(String),
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListInfo {
    /// This node's own key
    pub me: String,
    pub peers: Vec<PeerInfo>,
    /// Ports this node exposes (distinct granted ports)
    pub i_expose: Vec<u16>,
    /// Who each port is granted to, one row per (port, grantee)
    pub grants: Vec<GrantInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GrantInfo {
    pub port: u16,
    /// Key of the peer this port is granted to
    pub to: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub key: String,
    /// What we call this peer locally, absent until something names it
    pub name: Option<String>,
    pub online: bool,
    /// How this peer came to be admitted: "added", "code", or "key"
    pub admission: String,
    /// Ports this peer exposes to us
    pub they_expose: Vec<u16>,
    /// Local address its ports are bound at, absent when not projected
    pub ip: Option<String>,
    /// Ports bound under that address
    pub bound: Vec<u16>,
}

// ============================================================================
// Peer <-> Peer (over iroh QUIC)
// ============================================================================

/// Message sent between peers over iroh
#[derive(Debug, Serialize, Deserialize)]
pub enum PeerMessage {
    /// Announce exposed ports (sent on connect and when ports change)
    ExposedPorts(Vec<u16>),
    /// Request to connect to a specific port
    Connect { port: u16 },
    /// Present a one-time enrollment token (sent on connect by `--enroll`)
    Enroll { token: String },
    /// Error response
    Error(String),
}
