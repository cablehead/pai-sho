//! Protocol definitions for daemon<->client and peer<->peer communication.

use serde::{Deserialize, Serialize};

/// ALPN protocol identifier
pub const ALPN: &[u8] = b"PAI_SHO/1";

/// This crate's version, announced to peers and shown by `list`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Project a peer's surface to a local address. `peer` is a key or the name
    /// it was given; `ip` is chosen if given, else allocated; `name` renames the
    /// surface, which is what the resolver answers `<name>.pai-sho` with.
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
    /// The `pai-sho` binary that issued `list`. Empty on the daemon's own
    /// response; the CLI fills it in before printing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli: String,
    /// The running daemon's crate version. Empty when that daemon is older
    /// than this field and omitted it.
    #[serde(default)]
    pub daemon: String,
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
    /// The peer's crate version, absent until it announces one
    #[serde(default)]
    pub version: Option<String>,
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
    /// Present an invitation's one-time code (sent on connect by `accept`).
    /// `version` is this daemon's crate version. Older daemons ignore unknown
    /// fields, so the claim still works; they just never send one themselves.
    Enroll {
        token: String,
        #[serde(default)]
        version: Option<String>,
    },
    /// Error response
    Error(String),
}

impl PeerMessage {
    /// An Enroll that also announces our crate version.
    pub fn enroll(token: impl Into<String>) -> Self {
        Self::Enroll {
            token: token.into(),
            version: Some(VERSION.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_enroll_still_parses() {
        let msg: PeerMessage = serde_json::from_str(r#"{"Enroll":{"token":"abc"}}"#).unwrap();
        match msg {
            PeerMessage::Enroll { token, version } => {
                assert_eq!(token, "abc");
                assert!(version.is_none());
            }
            other => panic!("expected Enroll, got {other:?}"),
        }
    }

    #[test]
    fn an_old_peer_still_parses_an_enroll_that_carries_version() {
        // 0.5.0/0.5.1 Enroll only has `token`. serde ignores unknown fields, so
        // the claim still lands. This is that old shape.
        #[derive(Deserialize)]
        struct OldEnroll {
            token: String,
        }
        #[derive(Deserialize)]
        enum OldMessage {
            Enroll(OldEnroll),
        }

        let json = serde_json::to_string(&PeerMessage::enroll("abc")).unwrap();
        let old: OldMessage = serde_json::from_str(&json).unwrap();
        match old {
            OldMessage::Enroll(e) => assert_eq!(e.token, "abc"),
        }
    }

    #[test]
    fn an_old_list_has_an_empty_daemon_version() {
        let info: ListInfo =
            serde_json::from_str(r#"{"me":"abc","peers":[],"i_expose":[],"grants":[]}"#).unwrap();
        assert!(info.cli.is_empty());
        assert!(info.daemon.is_empty());
    }
}
