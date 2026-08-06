//! Surfaces - a peer's ports projected to a dedicated local address.
//!
//! A surface binds a peer's granted ports at one IP you choose or that is
//! allocated from `127.0.1.0/24`, optionally with a `/etc/hosts` name. See
//! docs/adr/0004-peer-surfaces.md.
//!
//! Two OS-visible effects live here, behind small helpers so the rest of the
//! daemon never shells out or touches system files:
//!
//! - the local address (`ensure_addr` / `remove_addr`): a no-op on Linux,
//!   where all of `127.0.0.0/8` already routes to `lo`; an `ifconfig lo0
//!   alias` on macOS, where extra loopback addresses must be added.
//! - the DNS handle (`sync_hosts`): a managed block in `/etc/hosts`, the same
//!   on both platforms.

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

const HOSTS_PATH: &str = "/etc/hosts";
const HOSTS_BEGIN: &str = "# BEGIN pai-sho (managed, do not edit)";
const HOSTS_END: &str = "# END pai-sho";

/// Rewrite the pai-sho managed block in `/etc/hosts` to exactly `entries`.
/// Removes the block entirely when `entries` is empty, which is how stale
/// names from a crashed run get cleaned up on the next sync.
pub fn sync_hosts(entries: &[(IpAddr, String)]) -> Result<()> {
    sync_hosts_at(HOSTS_PATH, entries)
}

fn sync_hosts_at(path: &str, entries: &[(IpAddr, String)]) -> Result<()> {
    let current =
        std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path))?;
    let stripped = strip_block(&current);

    let mut out = stripped;
    if !entries.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(HOSTS_BEGIN);
        out.push('\n');
        for (ip, name) in entries {
            out.push_str(&format!("{}\t{}\n", ip, name));
        }
        out.push_str(HOSTS_END);
        out.push('\n');
    }

    std::fs::write(path, out).with_context(|| {
        format!(
            "failed to write {} (a DNS handle needs write access to it)",
            path
        )
    })
}

/// Return `text` with the pai-sho managed block (and its markers) removed.
fn strip_block(text: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.trim() == HOSTS_BEGIN {
            in_block = true;
            continue;
        }
        if in_block {
            if line.trim() == HOSTS_END {
                in_block = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
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

    #[test]
    fn hosts_block_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pai-sho-hosts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hosts");
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();

        sync_hosts_at(
            path_str,
            &[(ip(2), "broker".into()), (ip(3), "ndyg".into())],
        )
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("127.0.0.1 localhost"));
        assert!(after.contains("127.0.1.2\tbroker"));
        assert!(after.contains("127.0.1.3\tndyg"));

        // A resync replaces the block rather than stacking a second one.
        sync_hosts_at(path_str, &[(ip(2), "broker".into())]).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.matches(HOSTS_BEGIN).count(), 1);
        assert!(after.contains("127.0.1.2\tbroker"));
        assert!(!after.contains("ndyg"));

        // Emptying removes the block and leaves the rest intact.
        sync_hosts_at(path_str, &[]).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains(HOSTS_BEGIN));
        assert!(after.contains("127.0.0.1 localhost"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
