//! The admission and authorization core.
//!
//! `Session` decides who may talk to this daemon and what they may reach. It
//! owns the grant table, the enrollment tokens, and the set of admitted peers.
//! It performs no IO: every method takes an event and returns actions for the
//! shell in `peer.rs` to carry out.
//!
//! The four decisions that make up the security model live here:
//!
//! - admission of an inbound connection (`on_inbound`)
//! - the verdict on an enrollment claim (`on_unadmitted`, `on_enroll_timeout`)
//! - authorization of a tunnel request (`on_tunnel`)
//! - what ports a peer is told about (`announce`)
//!
//! See docs/scenarios.md for the invariants these are meant to hold.

use crate::enroll::Tokens;
use crate::grants::Grants;
use crate::protocol::PeerMessage;
use iroh::EndpointId;
use std::collections::BTreeMap;

/// A connection the shell holds. The core never sees an iroh `Connection`, it
/// addresses one by id.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct ConnId(pub u64);

/// How a peer came to be admitted. Recorded because the two carry different
/// weight when auditing: a code was held, a key was vouched for.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Admission {
    /// The operator added it by key (`add-peer`).
    Added,
    /// It presented a valid one-time enrollment code.
    Code,
    /// Its key was pinned ahead of time, no secret involved.
    Key,
}

/// Why a connection, tunnel, or claim was turned away. These are the refusal
/// paths that were previously only log strings.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Unknown key, and no valid code presented.
    NotAuthorized,
    /// The enrollment window elapsed before a claim arrived.
    EnrollTimeout,
    /// The port is not granted to this specific peer.
    NotGranted,
}

/// How much of a key to use when nothing named the peer. Long enough to read
/// out loud and to stay unique in any realistic peer list.
const SHORT_KEY: usize = 8;

/// What to call a peer when neither side passed `--as`. A truncated key is
/// ugly but stable, and `project --as` renames it.
pub fn default_name(key: &EndpointId) -> String {
    key.to_string().chars().take(SHORT_KEY).collect()
}

impl Admission {
    /// Name used in `list` output.
    pub fn as_str(self) -> &'static str {
        match self {
            Admission::Added => "added",
            Admission::Code => "code",
            Admission::Key => "key",
        }
    }
}

impl Refusal {
    /// Text for the QUIC close reason and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::NotAuthorized => "not authorized",
            Refusal::EnrollTimeout => "enroll timeout",
            Refusal::NotGranted => "not granted",
        }
    }
}

/// What the shell should do next.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// Install this connection as the peer's live one. `replacing` means an
    /// older connection is present and must be closed first.
    Admit {
        conn: ConnId,
        peer: EndpointId,
        replacing: bool,
    },
    /// Close the connection without admitting it.
    Refuse { conn: ConnId, reason: Refusal },
    /// Tell `peer` exactly which of our ports it may reach. Sent even when
    /// empty, so a revocation tears down the peer's binding.
    Announce { peer: EndpointId, ports: Vec<u16> },
    /// Ports this peer announced before it was admitted, to apply now that it
    /// is.
    ApplyPorts { peer: EndpointId, ports: Vec<u16> },
    /// Serve a tunnel for this port.
    ServeTunnel { port: u16 },
    /// Refuse a tunnel.
    RejectTunnel { reason: Refusal },
    /// Write this pin to disk.
    PersistPin {
        key: EndpointId,
        label: Option<String>,
    },
    /// Drop this pin from disk.
    DropPin { key: EndpointId },
}

/// An admitted peer, as far as admission and access are concerned. Connection
/// state, surfaces, and bindings stay in the shell for now.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Admitted {
    pub label: Option<String>,
    pub admission: Admission,
}

/// A connection accepted but not yet admitted. `early_ports` holds an
/// `ExposedPorts` message that arrived before the enrollment claim: they are
/// separate uni streams, so either order is possible.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct Pending {
    early_ports: Option<Vec<u16>>,
}

#[derive(Default)]
pub struct Session {
    grants: Grants,
    tokens: Tokens,
    peers: BTreeMap<EndpointId, Admitted>,
    pending: BTreeMap<ConnId, Pending>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------- admission

    /// Record a peer the operator named directly, by `add-peer` or by pin.
    /// Idempotent: re-adding a known peer leaves it as it was.
    pub fn admit_known(
        &mut self,
        key: EndpointId,
        label: Option<String>,
        admission: Admission,
    ) -> bool {
        if self.peers.contains_key(&key) {
            return false;
        }
        self.peers.insert(key, Admitted { label, admission });
        true
    }

    /// An inbound connection arrived. A known key is admitted with no claim; an
    /// unknown one is held pending until it presents a code.
    pub fn on_inbound(&mut self, conn: ConnId, remote: EndpointId) -> Vec<Action> {
        if self.peers.contains_key(&remote) {
            return vec![
                Action::Admit {
                    conn,
                    peer: remote,
                    replacing: true,
                },
                self.announce(remote),
            ];
        }
        self.pending.insert(conn, Pending::default());
        Vec::new()
    }

    /// A control message on a connection that is not yet admitted. Only an
    /// `Enroll` carrying a live code admits it. Anything else is buffered or
    /// ignored, and the connection stays pending until it claims or times out.
    pub fn on_unadmitted(
        &mut self,
        conn: ConnId,
        remote: EndpointId,
        msg: PeerMessage,
    ) -> Vec<Action> {
        match msg {
            PeerMessage::Enroll { token } => match self.tokens.claim(&token) {
                Some(claimed) => {
                    let early = self
                        .pending
                        .remove(&conn)
                        .and_then(|p| p.early_ports)
                        .unwrap_or_default();
                    self.peers.insert(
                        remote,
                        Admitted {
                            label: claimed.name.clone(),
                            admission: Admission::Code,
                        },
                    );
                    for port in &claimed.ports {
                        self.grants.add(*port, remote);
                    }
                    let mut actions = vec![
                        Action::Admit {
                            conn,
                            peer: remote,
                            replacing: false,
                        },
                        Action::PersistPin {
                            key: remote,
                            label: claimed.name,
                        },
                    ];
                    if !early.is_empty() {
                        actions.push(Action::ApplyPorts {
                            peer: remote,
                            ports: early,
                        });
                    }
                    actions.push(self.announce(remote));
                    actions
                }
                None => {
                    self.pending.remove(&conn);
                    vec![Action::Refuse {
                        conn,
                        reason: Refusal::NotAuthorized,
                    }]
                }
            },
            PeerMessage::ExposedPorts(ports) => {
                if let Some(p) = self.pending.get_mut(&conn) {
                    p.early_ports = Some(ports);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The enrollment window elapsed with no valid claim.
    pub fn on_enroll_timeout(&mut self, conn: ConnId) -> Vec<Action> {
        if self.pending.remove(&conn).is_none() {
            return Vec::new();
        }
        vec![Action::Refuse {
            conn,
            reason: Refusal::EnrollTimeout,
        }]
    }

    /// Forget a peer: it loses admission, and every grant naming it is revoked.
    pub fn evict(&mut self, peer: &EndpointId) -> Vec<Action> {
        if self.peers.remove(peer).is_none() {
            return Vec::new();
        }
        self.grants.revoke_grantee(peer);
        vec![Action::DropPin { key: *peer }]
    }

    // ------------------------------------------------------------------- access

    /// The security perimeter: a tunnel is served only for a port granted to
    /// this specific peer.
    pub fn on_tunnel(&self, peer: &EndpointId, port: u16) -> Action {
        if self.grants.allows(port, peer) {
            Action::ServeTunnel { port }
        } else {
            Action::RejectTunnel {
                reason: Refusal::NotGranted,
            }
        }
    }

    /// What this peer is allowed to know about: its granted ports, and nothing
    /// else.
    pub fn announce(&self, peer: EndpointId) -> Action {
        Action::Announce {
            peer,
            ports: self.grants.ports_for(&peer),
        }
    }

    /// Grant `port` to each of `to`, and re-announce to everyone, so a peer
    /// that lost nothing still hears an unchanged list.
    pub fn expose(&mut self, port: u16, to: &[EndpointId]) -> Vec<Action> {
        for grantee in to {
            self.grants.add(port, *grantee);
        }
        self.announce_all()
    }

    /// Revoke grants for `port`: one grantee's, or every one of them.
    pub fn unexpose(&mut self, port: u16, to: Option<EndpointId>) -> Vec<Action> {
        self.grants.remove(port, to);
        self.announce_all()
    }

    fn announce_all(&self) -> Vec<Action> {
        self.peers.keys().map(|k| self.announce(*k)).collect()
    }

    // ------------------------------------------------------------------ queries

    /// Every peer admitted right now.
    pub fn peer_keys(&self) -> Vec<EndpointId> {
        self.peers.keys().copied().collect()
    }

    pub fn admission_of(&self, peer: &EndpointId) -> Option<Admission> {
        self.peers.get(peer).map(|p| p.admission)
    }

    pub fn label_of(&self, peer: &EndpointId) -> Option<String> {
        self.peers.get(peer).and_then(|p| p.label.clone())
    }

    /// Mint a one-time code. `ports` are granted to whoever claims it, so a
    /// grant still names exactly one key: the key is filled in on claim.
    pub fn mint_token(&self, name: Option<String>, ports: Vec<u16>) -> String {
        self.tokens.mint(name, ports)
    }

    pub fn granted_ports(&self) -> Vec<u16> {
        self.grants.ports()
    }

    pub fn all_grants(&self) -> Vec<(u16, EndpointId)> {
        self.grants.all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> EndpointId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        iroh::SecretKey::from_bytes(&bytes).public()
    }

    fn session_with(peer: EndpointId) -> Session {
        let mut s = Session::new();
        s.admit_known(peer, Some("peer".into()), Admission::Added);
        s
    }

    // -- authorization -----------------------------------------------------

    #[test]
    fn tunnel_needs_a_grant() {
        let a = key(1);
        let s = session_with(a);
        assert_eq!(
            s.on_tunnel(&a, 4000),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
    }

    #[test]
    fn tunnel_is_served_once_granted() {
        let a = key(1);
        let mut s = session_with(a);
        s.expose(4000, &[a]);
        assert_eq!(s.on_tunnel(&a, 4000), Action::ServeTunnel { port: 4000 });
    }

    #[test]
    fn a_grant_to_one_peer_is_not_usable_by_another() {
        let (a, b) = (key(1), key(2));
        let mut s = session_with(a);
        s.admit_known(b, None, Admission::Added);
        s.expose(4000, &[a]);
        assert_eq!(
            s.on_tunnel(&b, 4000),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
    }

    #[test]
    fn a_grant_covers_only_the_port_named() {
        let a = key(1);
        let mut s = session_with(a);
        s.expose(4000, &[a]);
        assert_eq!(
            s.on_tunnel(&a, 4001),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
    }

    // -- announcement ------------------------------------------------------

    #[test]
    fn a_peer_is_told_only_its_own_grants() {
        let (a, b) = (key(1), key(2));
        let mut s = session_with(a);
        s.admit_known(b, None, Admission::Added);
        s.expose(4000, &[a]);
        s.expose(4001, &[b]);
        assert_eq!(
            s.announce(a),
            Action::Announce {
                peer: a,
                ports: vec![4000]
            }
        );
    }

    #[test]
    fn revocation_announces_an_empty_list() {
        let a = key(1);
        let mut s = session_with(a);
        s.expose(4000, &[a]);
        assert_eq!(
            s.unexpose(4000, None),
            vec![Action::Announce {
                peer: a,
                ports: vec![]
            }]
        );
    }

    #[test]
    fn revoking_one_grantee_leaves_the_other() {
        let (a, b) = (key(1), key(2));
        let mut s = session_with(a);
        s.admit_known(b, None, Admission::Added);
        s.expose(4000, &[a, b]);
        s.unexpose(4000, Some(a));
        assert_eq!(
            s.on_tunnel(&a, 4000),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
        assert_eq!(s.on_tunnel(&b, 4000), Action::ServeTunnel { port: 4000 });
    }

    // -- admission ---------------------------------------------------------

    #[test]
    fn an_unknown_peer_is_not_admitted_on_arrival() {
        let mut s = Session::new();
        assert_eq!(s.on_inbound(ConnId(1), key(9)), vec![]);
    }

    #[test]
    fn an_unknown_peer_with_no_claim_is_refused_on_timeout() {
        let mut s = Session::new();
        let conn = ConnId(1);
        s.on_inbound(conn, key(9));
        assert_eq!(
            s.on_enroll_timeout(conn),
            vec![Action::Refuse {
                conn,
                reason: Refusal::EnrollTimeout
            }]
        );
        assert!(s.admission_of(&key(9)).is_none());
    }

    #[test]
    fn a_bad_code_is_refused() {
        let mut s = Session::new();
        let (conn, k) = (ConnId(1), key(9));
        s.on_inbound(conn, k);
        let actions = s.on_unadmitted(
            conn,
            k,
            PeerMessage::Enroll {
                token: "nope".into(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Refuse {
                conn,
                reason: Refusal::NotAuthorized
            }]
        );
        assert!(s.admission_of(&k).is_none());
    }

    #[test]
    fn a_valid_code_admits_and_pins() {
        let mut s = Session::new();
        let (conn, k) = (ConnId(1), key(9));
        let token = s.mint_token(Some("rustdev".into()), vec![]);
        s.on_inbound(conn, k);

        let actions = s.on_unadmitted(conn, k, PeerMessage::Enroll { token });
        assert_eq!(
            actions,
            vec![
                Action::Admit {
                    conn,
                    peer: k,
                    replacing: false
                },
                Action::PersistPin {
                    key: k,
                    label: Some("rustdev".into())
                },
                Action::Announce {
                    peer: k,
                    ports: vec![]
                },
            ]
        );
        assert_eq!(s.admission_of(&k), Some(Admission::Code));
        assert_eq!(s.label_of(&k), Some("rustdev".into()));
    }

    #[test]
    fn a_code_is_single_use() {
        let mut s = Session::new();
        let token = s.mint_token(Some("rustdev".into()), vec![]);
        let first = key(1);
        s.on_inbound(ConnId(1), first);
        s.on_unadmitted(
            ConnId(1),
            first,
            PeerMessage::Enroll {
                token: token.clone(),
            },
        );

        let second = key(2);
        s.on_inbound(ConnId(2), second);
        let actions = s.on_unadmitted(ConnId(2), second, PeerMessage::Enroll { token });
        assert_eq!(
            actions,
            vec![Action::Refuse {
                conn: ConnId(2),
                reason: Refusal::NotAuthorized
            }]
        );
        assert!(s.admission_of(&second).is_none());
    }

    #[test]
    fn an_unadmitted_peer_announcing_ports_is_not_admitted_by_it() {
        let mut s = Session::new();
        let (conn, k) = (ConnId(1), key(9));
        s.on_inbound(conn, k);
        assert_eq!(
            s.on_unadmitted(conn, k, PeerMessage::ExposedPorts(vec![4000])),
            vec![]
        );
        assert!(s.admission_of(&k).is_none());
    }

    #[test]
    fn ports_announced_before_the_claim_are_applied_after_it() {
        let mut s = Session::new();
        let (conn, k) = (ConnId(1), key(9));
        let token = s.mint_token(Some("rustdev".into()), vec![]);
        s.on_inbound(conn, k);
        s.on_unadmitted(conn, k, PeerMessage::ExposedPorts(vec![4000, 4001]));

        let actions = s.on_unadmitted(conn, k, PeerMessage::Enroll { token });
        assert!(actions.contains(&Action::ApplyPorts {
            peer: k,
            ports: vec![4000, 4001]
        }));
    }

    #[test]
    fn a_known_peer_is_admitted_with_no_claim() {
        let a = key(1);
        let mut s = session_with(a);
        let conn = ConnId(1);
        assert_eq!(
            s.on_inbound(conn, a),
            vec![
                Action::Admit {
                    conn,
                    peer: a,
                    replacing: true
                },
                Action::Announce {
                    peer: a,
                    ports: vec![]
                },
            ]
        );
    }

    #[test]
    fn admitting_a_known_peer_twice_changes_nothing() {
        let a = key(1);
        let mut s = session_with(a);
        assert!(!s.admit_known(a, Some("other".into()), Admission::Key));
        assert_eq!(s.label_of(&a), Some("peer".into()));
        assert_eq!(s.admission_of(&a), Some(Admission::Added));
    }

    #[test]
    fn admission_records_how_the_peer_arrived() {
        let mut s = Session::new();
        s.admit_known(key(1), None, Admission::Key);
        assert_eq!(s.admission_of(&key(1)), Some(Admission::Key));
    }

    // -- eviction ----------------------------------------------------------

    #[test]
    fn eviction_revokes_every_grant_naming_the_peer() {
        let (a, b) = (key(1), key(2));
        let mut s = session_with(a);
        s.admit_known(b, None, Admission::Added);
        s.expose(4000, &[a, b]);
        s.expose(4001, &[a]);

        s.evict(&a);

        assert!(s.admission_of(&a).is_none());
        assert_eq!(
            s.on_tunnel(&a, 4000),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
        assert_eq!(s.granted_ports(), vec![4000]);
    }

    #[test]
    fn evicting_an_unknown_peer_does_nothing() {
        let mut s = Session::new();
        assert_eq!(s.evict(&key(1)), vec![]);
    }

    #[test]
    fn an_evicted_peer_is_no_longer_admitted_on_arrival() {
        let a = key(1);
        let mut s = session_with(a);
        s.evict(&a);
        assert_eq!(s.on_inbound(ConnId(1), a), vec![]);
    }

    #[test]
    fn an_invitation_can_carry_a_grant() {
        let mut s = Session::new();
        let (conn, k) = (ConnId(1), key(9));
        let token = s.mint_token(None, vec![8080]);
        s.on_inbound(conn, k);

        let actions = s.on_unadmitted(conn, k, PeerMessage::Enroll { token });
        assert!(actions.contains(&Action::Announce {
            peer: k,
            ports: vec![8080]
        }));
        assert_eq!(s.on_tunnel(&k, 8080), Action::ServeTunnel { port: 8080 });
    }

    #[test]
    fn a_carried_grant_names_only_the_claimer() {
        let (a, b) = (key(1), key(2));
        let mut s = session_with(a);
        let token = s.mint_token(None, vec![8080]);
        s.on_inbound(ConnId(1), b);
        s.on_unadmitted(ConnId(1), b, PeerMessage::Enroll { token });

        assert_eq!(s.on_tunnel(&b, 8080), Action::ServeTunnel { port: 8080 });
        assert_eq!(
            s.on_tunnel(&a, 8080),
            Action::RejectTunnel {
                reason: Refusal::NotGranted
            }
        );
    }

    #[test]
    fn an_unnamed_peer_falls_back_to_a_short_key() {
        let k = key(1);
        let name = default_name(&k);
        assert_eq!(name.len(), SHORT_KEY);
        assert!(k.to_string().starts_with(&name));
    }

    #[test]
    fn short_names_differ_between_peers() {
        assert_ne!(default_name(&key(1)), default_name(&key(2)));
    }
}
