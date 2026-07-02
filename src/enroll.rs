//! Enrollment - one-time tokens minted by the operator, and pinned peers.
//!
//! `grant-token --label <name>` mints a token; a workload presents it on
//! first connect (`--enroll TOKEN`). A valid claim pins the workload's key
//! under that label and consumes the token. Pins persist across restarts.

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a minted token stays claimable
pub const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

struct PendingToken {
    label: String,
    expires: Instant,
}

/// One-time enrollment tokens, in-memory (a daemon restart voids them)
#[derive(Default)]
pub struct Tokens {
    pending: Mutex<HashMap<String, PendingToken>>,
}

impl Tokens {
    pub fn mint(&self, label: String) -> String {
        self.mint_with_ttl(label, TOKEN_TTL)
    }

    fn mint_with_ttl(&self, label: String, ttl: Duration) -> String {
        let bytes: [u8; 32] = rand::rng().random();
        let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        self.pending.lock().unwrap().insert(
            token.clone(),
            PendingToken {
                label,
                expires: Instant::now() + ttl,
            },
        );
        token
    }

    /// Claim a token. Single use: a valid claim consumes it and returns its
    /// label; reused, expired, or unknown tokens return None.
    pub fn claim(&self, token: &str) -> Option<String> {
        let mut pending = self.pending.lock().unwrap();
        let now = Instant::now();
        pending.retain(|_, t| t.expires > now);
        pending.remove(token).map(|t| t.label)
    }
}

/// A peer key pinned at enrollment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub key: String,
    pub label: String,
}

/// Pinned peers, persisted as JSON next to the daemon key
pub struct Pins {
    path: PathBuf,
}

impl Pins {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Vec<Pin>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        serde_json::from_slice(&data)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub fn add(&self, key: &str, label: &str) -> Result<()> {
        let mut pins = self.load()?;
        pins.retain(|p| p.key != key);
        pins.push(Pin {
            key: key.to_string(),
            label: label.to_string(),
        });
        self.save(&pins)
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        let mut pins = self.load()?;
        pins.retain(|p| p.key != key);
        self.save(&pins)
    }

    fn save(&self, pins: &[Pin]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let data = serde_json::to_vec_pretty(pins)?;
        std::fs::write(&self.path, data)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_single_use() {
        let tokens = Tokens::default();
        let token = tokens.mint("rustdev".to_string());
        assert_eq!(tokens.claim(&token), Some("rustdev".to_string()));
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
        let token = tokens.mint_with_ttl("rustdev".to_string(), Duration::ZERO);
        assert_eq!(tokens.claim(&token), None);
    }

    #[test]
    fn pins_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pai-sho-pins-test-{}", std::process::id()));
        let pins = Pins::new(dir.join("peers.json"));

        assert!(pins.load().unwrap().is_empty());
        pins.add("k1", "rustdev").unwrap();
        pins.add("k2", "webdev").unwrap();
        pins.add("k1", "rustdev2").unwrap(); // re-pin replaces

        let loaded = pins.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.iter().find(|p| p.key == "k1").unwrap().label,
            "rustdev2"
        );

        pins.remove("k2").unwrap();
        assert_eq!(pins.load().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
