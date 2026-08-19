//! Directed grants - (port) -> grantee, per ADR 0001.
//!
//! A grant says: this daemon exposes `port` to `grantee`, and to no one
//! else. Default deny -- no grant, no access. Each peer is announced only
//! the ports granted to it, and a tunnel is opened only for a port granted
//! to the requesting peer.

use iroh::EndpointId;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Grants {
    by_port: HashMap<u16, HashSet<EndpointId>>,
}

impl Grants {
    /// Grant `port` to `grantee`
    pub fn add(&mut self, port: u16, grantee: EndpointId) {
        self.by_port.entry(port).or_default().insert(grantee);
    }

    /// Revoke a grant. With a grantee, remove just that grant; without,
    /// remove every grant for the port.
    pub fn remove(&mut self, port: u16, grantee: Option<EndpointId>) {
        match grantee {
            Some(grantee) => {
                if let Some(grantees) = self.by_port.get_mut(&port) {
                    grantees.remove(&grantee);
                    if grantees.is_empty() {
                        self.by_port.remove(&port);
                    }
                }
            }
            None => {
                self.by_port.remove(&port);
            }
        }
    }

    /// Drop every grant naming `peer` (peer evicted)
    pub fn revoke_grantee(&mut self, peer: &EndpointId) {
        self.by_port.retain(|_, grantees| {
            grantees.remove(peer);
            !grantees.is_empty()
        });
    }

    /// Is `port` granted to `peer`?
    pub fn allows(&self, port: u16, peer: &EndpointId) -> bool {
        self.by_port
            .get(&port)
            .map(|g| g.contains(peer))
            .unwrap_or(false)
    }

    /// Ports granted to `peer` (what we announce to it)
    pub fn ports_for(&self, peer: &EndpointId) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .by_port
            .iter()
            .filter(|(_, g)| g.contains(peer))
            .map(|(port, _)| *port)
            .collect();
        ports.sort_unstable();
        ports
    }

    /// Distinct granted ports
    pub fn ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self.by_port.keys().copied().collect();
        ports.sort_unstable();
        ports
    }

    /// Every (port, grantee) pair, sorted by port
    pub fn all(&self) -> Vec<(u16, EndpointId)> {
        let mut rows: Vec<(u16, EndpointId)> = self
            .by_port
            .iter()
            .flat_map(|(port, grantees)| grantees.iter().map(|g| (*port, *g)))
            .collect();
        rows.sort_unstable_by_key(|(port, _)| *port);
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn key(n: u8) -> EndpointId {
        SecretKey::from_bytes(&[n; 32]).public()
    }

    #[test]
    fn default_deny() {
        let grants = Grants::default();
        assert!(!grants.allows(4000, &key(1)));
        assert!(grants.ports_for(&key(1)).is_empty());
    }

    #[test]
    fn grant_is_directed() {
        let (a, b) = (key(1), key(2));
        let mut grants = Grants::default();
        grants.add(4000, a);

        assert!(grants.allows(4000, &a));
        assert!(!grants.allows(4000, &b));
        assert!(!grants.allows(4001, &a));
        assert_eq!(grants.ports_for(&a), vec![4000]);
        assert!(grants.ports_for(&b).is_empty());
    }

    #[test]
    fn revoke_grantee_clears_all_their_grants() {
        let (a, b) = (key(1), key(2));
        let mut grants = Grants::default();
        grants.add(4000, a);
        grants.add(4001, a);
        grants.add(4000, b);

        grants.revoke_grantee(&a);
        assert!(!grants.allows(4000, &a));
        assert!(!grants.allows(4001, &a));
        assert!(grants.allows(4000, &b));
        assert_eq!(grants.ports(), vec![4000]);
    }

    #[test]
    fn remove_one_grantee_keeps_others() {
        let (a, b) = (key(1), key(2));
        let mut grants = Grants::default();
        grants.add(4000, a);
        grants.add(4000, b);

        grants.remove(4000, Some(a));
        assert!(!grants.allows(4000, &a));
        assert!(grants.allows(4000, &b));

        grants.remove(4000, None);
        assert!(!grants.allows(4000, &b));
        assert!(grants.ports().is_empty());
    }
}
