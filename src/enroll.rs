//! Enrollment - one-time codes minted by the inviter, and pinned peers.
//!
//! `invite` mints a code; the accepter presents it on first connect. A valid
//! claim pins the accepter's key under the name the inviter chose, if any, and
//! consumes the code. Pins persist across restarts. See ADR 0006.

use anyhow::Result;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a minted token stays claimable
pub const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

struct PendingToken {
    name: Option<String>,
    ports: Vec<u16>,
    expires: Instant,
}

/// A successfully claimed token. The name is what the issuer chose to call
/// whoever takes the invitation up, and is optional.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Claimed {
    pub name: Option<String>,
    /// Ports the issuer granted along with the invitation. The grantee is
    /// filled in on claim, so the grant still names one key.
    pub ports: Vec<u16>,
}

/// One-time enrollment tokens, in-memory (a daemon restart voids them)
#[derive(Default)]
pub struct Tokens {
    pending: Mutex<HashMap<String, PendingToken>>,
}

impl Tokens {
    pub fn mint(&self, name: Option<String>, ports: Vec<u16>) -> String {
        self.mint_with_ttl(name, ports, TOKEN_TTL)
    }

    fn mint_with_ttl(&self, name: Option<String>, ports: Vec<u16>, ttl: Duration) -> String {
        let bytes: [u8; 32] = rand::rng().random();
        let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        self.pending.lock().unwrap().insert(
            token.clone(),
            PendingToken {
                name,
                ports,
                expires: Instant::now() + ttl,
            },
        );
        token
    }

    /// Claim a token. Single use: a valid claim consumes it and returns what
    /// the issuer named the peer; reused, expired, or unknown tokens return
    /// None.
    pub fn claim(&self, token: &str) -> Option<Claimed> {
        let mut pending = self.pending.lock().unwrap();
        let now = Instant::now();
        pending.retain(|_, t| t.expires > now);
        pending.remove(token).map(|t| Claimed {
            name: t.name,
            ports: t.ports,
        })
    }
}

/// A peer key admitted at some point, persisted so it survives a restart
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub key: String,
    /// What we call this peer locally. Absent until something names it.
    #[serde(default)]
    pub label: Option<String>,
    /// We took up this peer's invitation, so we dial it. Kept so that after a
    /// restart we call it again instead of waiting for a call that may never
    /// come (an iPhone app restarts on every update).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dial: bool,
}

/// Pinned peers, persisted as JSON next to the daemon key
pub struct Pins {
    file: crate::json_file::JsonFile<Pin>,
}

impl Pins {
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: crate::json_file::JsonFile::new(path),
        }
    }

    pub fn load(&self) -> Result<Vec<Pin>> {
        self.file.load()
    }

    /// Pin a peer that calls us. A peer we already dial keeps dialing.
    pub fn add(&self, key: &str, label: Option<&str>) -> Result<()> {
        self.put(key, label, false)
    }

    /// Pin a peer whose invitation we took up: we dial it, after a restart too.
    pub fn add_dialed(&self, key: &str, label: Option<&str>) -> Result<()> {
        self.put(key, label, true)
    }

    /// Dial a pinned peer after a restart, as if we had taken up its
    /// invitation. Leaves its name alone. A key with no pin is left unpinned.
    pub fn mark_dialed(&self, key: &str) -> Result<()> {
        let mut pins = self.load()?;
        let Some(pin) = pins.iter_mut().find(|p| p.key == key && !p.dial) else {
            return Ok(());
        };
        pin.dial = true;
        self.save(&pins)
    }

    fn put(&self, key: &str, label: Option<&str>, dial: bool) -> Result<()> {
        let mut pins = self.load()?;
        let dial = dial || pins.iter().any(|p| p.key == key && p.dial);
        pins.retain(|p| p.key != key);
        pins.push(Pin {
            key: key.to_string(),
            label: label.map(|l| l.to_string()),
            dial,
        });
        self.save(&pins)
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        let mut pins = self.load()?;
        pins.retain(|p| p.key != key);
        self.save(&pins)
    }

    fn save(&self, pins: &[Pin]) -> Result<()> {
        self.file.save(pins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_single_use() {
        let tokens = Tokens::default();
        let token = tokens.mint(Some("rustdev".to_string()), vec![]);
        assert_eq!(
            tokens.claim(&token),
            Some(Claimed {
                name: Some("rustdev".to_string()),
                ports: vec![],
            })
        );
        assert_eq!(tokens.claim(&token), None);
    }

    #[test]
    fn unknown_token_fails() {
        let tokens = Tokens::default();
        assert_eq!(tokens.claim("nope"), None);
    }

    #[test]
    fn expired_token_fails() {
        let tokens = Tokens::default();
        let token = tokens.mint_with_ttl(Some("rustdev".to_string()), vec![], Duration::ZERO);
        assert_eq!(tokens.claim(&token), None);
    }

    #[test]
    fn pins_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pai-sho-pins-test-{}", std::process::id()));
        let pins = Pins::new(dir.join("peers.json"));

        assert!(pins.load().unwrap().is_empty());
        pins.add("k1", Some("rustdev")).unwrap();
        pins.add("k2", Some("webdev")).unwrap();
        pins.add("k1", Some("rustdev2")).unwrap(); // re-pin replaces

        let loaded = pins.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.iter().find(|p| p.key == "k1").unwrap().label,
            Some("rustdev2".to_string())
        );

        pins.remove("k2").unwrap();
        assert_eq!(pins.load().unwrap().len(), 1);

        // a peer whose invitation we took up stays dialed, even when it is
        // pinned again later as a peer that calls us
        pins.add_dialed("k3", Some("chunker")).unwrap();
        pins.add("k3", Some("chunker")).unwrap();
        let k3 = pins
            .load()
            .unwrap()
            .into_iter()
            .find(|p| p.key == "k3")
            .unwrap();
        assert!(k3.dial);
        // a peer that calls us can be marked dialed, keeping its name
        pins.add("k4", Some("phone")).unwrap();
        pins.mark_dialed("k4").unwrap();
        pins.mark_dialed("nope").unwrap();
        let loaded = pins.load().unwrap();
        let k4 = loaded.iter().find(|p| p.key == "k4").unwrap();
        assert!(k4.dial);
        assert_eq!(k4.label.as_deref(), Some("phone"));
        assert!(!loaded.iter().any(|p| p.key == "nope"));

        // and a pins file from before `dial` existed still loads
        std::fs::write(dir.join("old.json"), r#"[{"key":"k9","label":null}]"#).unwrap();
        let old = Pins::new(dir.join("old.json")).load().unwrap();
        assert!(!old[0].dial);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_token_carries_its_ports() {
        let tokens = Tokens::default();
        let token = tokens.mint(None, vec![8080]);
        assert_eq!(
            tokens.claim(&token),
            Some(Claimed {
                name: None,
                ports: vec![8080],
            })
        );
    }
}
