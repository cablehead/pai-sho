//! Surfaces - a peer's ports projected to a dedicated local address.
//!
//! A surface binds a peer's granted ports at one IP you choose or that is
//! allocated from `127.0.1.0/24`, and carries an optional name that the owned
//! resolver serves under `*.ps`. See docs/adr/0004-peer-surfaces.md.
//!
//! Claiming the address is behind one helper so the rest of the daemon never
//! shells out. `ensure_addr` / `remove_addr` is a no-op on Linux, where all of
//! `127.0.0.0/8` already routes to `lo`, and an `ifconfig lo0 alias` on macOS,
//! where extra loopback addresses must be added. The TUN-backed backend that
//! claims a whole subnet in one privileged step replaces this seam later.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

/// A projected surface: where a peer's ports are bound locally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Surface {
    pub ip: IpAddr,
    pub name: Option<String>,
}

/// Allocate a loopback address not already in `taken`, from `127.0.1.2`
/// upward. `.0` is the network, `.1` is squatted by Debian for the hostname,
/// and `.255` is broadcast, so the usable window is `.2 ..= .254`.
pub fn allocate(taken: &[IpAddr]) -> Result<IpAddr> {
    for host in 2u8..=254 {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, host));
        if !taken.contains(&ip) {
            return Ok(ip);
        }
    }
    anyhow::bail!("no free address in 127.0.1.0/24; unproject a surface first")
}

/// Make `ip` usable as a local bind address. Idempotent.
pub fn ensure_addr(ip: IpAddr) -> Result<()> {
    if is_stock_loopback(ip) {
        return Ok(());
    }
    add_addr(ip)
}

/// Remove a local bind address added by `ensure_addr`. Idempotent; never
/// touches the stock `127.0.0.1`.
pub fn remove_addr(ip: IpAddr) -> Result<()> {
    if is_stock_loopback(ip) {
        return Ok(());
    }
    del_addr(ip)
}

fn is_stock_loopback(ip: IpAddr) -> bool {
    ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[cfg(target_os = "linux")]
fn add_addr(_ip: IpAddr) -> Result<()> {
    // 127.0.0.0/8 already routes to lo, so a listener can bind any address in
    // it with no setup and no privilege. Nothing to add.
    Ok(())
}

#[cfg(target_os = "linux")]
fn del_addr(_ip: IpAddr) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn add_addr(ip: IpAddr) -> Result<()> {
    run("ifconfig", &["lo0", "alias", &ip.to_string(), "up"])
}

#[cfg(target_os = "macos")]
fn del_addr(ip: IpAddr) -> Result<()> {
    run("ifconfig", &["lo0", "-alias", &ip.to_string()])
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn add_addr(ip: IpAddr) -> Result<()> {
    anyhow::bail!(
        "projecting {} needs a loopback alias, unsupported on this OS (use --ip 127.0.0.1)",
        ip
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn del_addr(_ip: IpAddr) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {}", cmd))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("{} {}: {}", cmd, args.join(" "), err.trim());
    }
    Ok(())
}

/// A projected surface, persisted as JSON next to the daemon key so it
/// survives a restart. Keyed by the peer's endpoint id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceRecord {
    pub key: String,
    pub ip: IpAddr,
    pub name: Option<String>,
}

/// Persisted surfaces, stored alongside the pins as `<key>.surfaces.json`.
pub struct SurfaceStore {
    path: PathBuf,
}

impl SurfaceStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Vec<SurfaceRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        serde_json::from_slice(&data)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub fn add(&self, key: &str, ip: IpAddr, name: Option<String>) -> Result<()> {
        let mut records = self.load()?;
        records.retain(|r| r.key != key);
        records.push(SurfaceRecord {
            key: key.to_string(),
            ip,
            name,
        });
        self.save(&records)
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        let mut records = self.load()?;
        records.retain(|r| r.key != key);
        self.save(&records)
    }

    fn save(&self, records: &[SurfaceRecord]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let data = serde_json::to_vec_pretty(records)?;
        std::fs::write(&self.path, data)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(host: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 1, host))
    }

    #[test]
    fn allocate_skips_taken() {
        let taken = vec![ip(2), ip(3), ip(5)];
        assert_eq!(allocate(&taken).unwrap(), ip(4));
    }

    #[test]
    fn allocate_from_empty_starts_at_two() {
        assert_eq!(allocate(&[]).unwrap(), ip(2));
    }

    #[test]
    fn allocate_errors_when_range_is_full() {
        let taken: Vec<IpAddr> = (2u8..=254).map(ip).collect();
        assert!(allocate(&taken).is_err());
    }

    #[test]
    fn stock_loopback_is_never_touched() {
        // ensure/remove on 127.0.0.1 are no-ops that always succeed, with no
        // privilege and no ifconfig call.
        let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
        ensure_addr(lo).unwrap();
        remove_addr(lo).unwrap();
    }
}
