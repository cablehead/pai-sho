//! Local port numbers for peers' ports, kept across restarts.
//!
//! When every peer binds at 127.0.0.1 (iOS), two peers can grant the same port
//! and only one of them can have that number locally. The other listens on a
//! number the OS picks. A browser keys a page's storage by host and port, so
//! that number has to stay the same after a reconnect or a restart, or the
//! page loses its storage. This table remembers which local number each
//! `(peer key, peer port)` got, as `<key>.ports.json` next to the daemon key.
//!
//! A surface address per peer (Linux, macOS) never needs this: each port keeps
//! its own number at its peer's address.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One of a peer's ports and the local number it listens on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortRecord {
    /// The peer's endpoint key
    pub key: String,
    /// The peer's port
    pub port: u16,
    /// The number it listens on at 127.0.0.1
    pub local: u16,
}

/// The number stored for this peer's port, if it has bound before.
pub fn stored(records: &[PortRecord], key: &str, port: u16) -> Option<u16> {
    records
        .iter()
        .find(|r| r.key == key && r.port == port)
        .map(|r| r.local)
}

/// Whether `local` belongs to some other peer's port. A port binding for the
/// first time does not take a number another port will come back for.
pub fn held_by_another(records: &[PortRecord], key: &str, port: u16, local: u16) -> bool {
    records
        .iter()
        .any(|r| r.local == local && !(r.key == key && r.port == port))
}

/// The number to try first for this peer's port: the one it had before, else
/// the peer's own port number unless another port holds it. None means let
/// the OS pick.
pub fn first_choice(records: &[PortRecord], key: &str, port: u16) -> Option<u16> {
    if let Some(local) = stored(records, key, port) {
        return Some(local);
    }
    (!held_by_another(records, key, port, port)).then_some(port)
}

/// The table, persisted as JSON next to the daemon key
pub struct PortStore {
    file: crate::json_file::JsonFile<PortRecord>,
}

impl PortStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: crate::json_file::JsonFile::new(path),
        }
    }

    pub fn load(&self) -> Result<Vec<PortRecord>> {
        self.file.load()
    }

    /// Record where this peer's port listens, replacing what was there.
    pub fn set(&self, key: &str, port: u16, local: u16) -> Result<()> {
        let mut records = self.load()?;
        records.retain(|r| !(r.key == key && r.port == port));
        records.push(PortRecord {
            key: key.to_string(),
            port,
            local,
        });
        self.save(&records)
    }

    /// Drop every port of a forgotten peer. Writes nothing when it had none,
    /// so a daemon that never binds at 127.0.0.1 never creates the file.
    pub fn remove_peer(&self, key: &str) -> Result<()> {
        let mut records = self.load()?;
        let before = records.len();
        records.retain(|r| r.key != key);
        if records.len() == before {
            return Ok(());
        }
        self.save(&records)
    }

    fn save(&self, records: &[PortRecord]) -> Result<()> {
        self.file.save(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, port: u16, local: u16) -> PortRecord {
        PortRecord {
            key: key.to_string(),
            port,
            local,
        }
    }

    #[test]
    fn a_port_binding_for_the_first_time_asks_for_its_own_number() {
        assert_eq!(first_choice(&[], "k1", 8080), Some(8080));
    }

    #[test]
    fn a_port_that_bound_before_asks_for_the_same_number() {
        let records = vec![record("k1", 8080, 52311)];
        assert_eq!(first_choice(&records, "k1", 8080), Some(52311));
    }

    #[test]
    fn a_number_another_peer_holds_is_not_asked_for() {
        // k1 had 8080 first. k2 granting 8080 too lets the OS pick, even while
        // k1 is offline and 8080 is free, so k1 gets its number back later.
        let records = vec![record("k1", 8080, 8080)];
        assert_eq!(first_choice(&records, "k2", 8080), None);
        assert!(held_by_another(&records, "k2", 8080, 8080));
        assert!(!held_by_another(&records, "k1", 8080, 8080));
    }

    #[test]
    fn a_number_another_port_of_the_same_peer_holds_is_not_asked_for() {
        let records = vec![record("k1", 3000, 8080)];
        assert_eq!(first_choice(&records, "k1", 8080), None);
    }

    #[test]
    fn the_store_survives_a_reload_and_forgets_a_peer() {
        let dir = std::env::temp_dir().join(format!("pai-sho-ports-test-{}", std::process::id()));
        let path = dir.join("key.ports.json");
        let store = PortStore::new(path.clone());

        // Forgetting a peer with no ports writes nothing.
        store.remove_peer("k1").unwrap();
        assert!(!path.exists());

        store.set("k1", 8080, 8080).unwrap();
        store.set("k2", 8080, 52311).unwrap();
        store.set("k2", 3000, 3000).unwrap();
        store.set("k2", 8080, 52400).unwrap(); // moved: replaces

        let reloaded = PortStore::new(path).load().unwrap();
        assert_eq!(stored(&reloaded, "k1", 8080), Some(8080));
        assert_eq!(stored(&reloaded, "k2", 8080), Some(52400));
        assert_eq!(stored(&reloaded, "k2", 3000), Some(3000));
        assert_eq!(reloaded.len(), 3);

        store.remove_peer("k2").unwrap();
        assert_eq!(store.load().unwrap(), vec![record("k1", 8080, 8080)]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
