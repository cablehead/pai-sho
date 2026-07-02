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
    AddPeer {
        ticket: String,
    },
    RemovePeer {
        ticket: String,
    },
    /// Grant `port` to `to`; empty `to` grants to all currently known peers
    Expose {
        port: u16,
        to: Vec<String>,
    },
    /// Revoke grants for `port`; `to` limits it to one grantee
    Unexpose {
        port: u16,
        to: Option<String>,
    },
    List,
    Ticket,
    GrantToken {
        label: String,
    },
}

/// Response from daemon to CLI client
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Ticket(String),
    List(ListInfo),
    Token(String),
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListInfo {
    /// This node's own key (its ticket)
    pub me: String,
    pub peers: Vec<PeerInfo>,
    /// Ports this node exposes (distinct granted ports)
    pub i_expose: Vec<u16>,
    /// Who each port is granted to, one row per (port, grantee)
    pub grants: Vec<GrantInfo>,
    pub bindings: Vec<BindingInfo>,
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
    /// Label assigned at enrollment (absent for peers added by ticket)
    pub label: Option<String>,
    pub online: bool,
    /// Ports this peer exposes to us
    pub they_expose: Vec<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BindingInfo {
    pub port: u16,
    /// Key of the peer this local port tunnels to
    pub peer: String,
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
