//! `daemon install`, `up`, `down`, `remove`, and `status` drive the macOS
//! launchd service in `launchd.rs`, which builds on unix only. Elsewhere each
//! one fails with the same message.

use anyhow::{bail, Result};

fn unsupported() -> Result<()> {
    bail!("daemon install, up, down, remove, and status run on macOS, not on Windows");
}

pub fn install() -> Result<()> {
    unsupported()
}

pub fn up() -> Result<()> {
    unsupported()
}

pub fn down() -> Result<()> {
    unsupported()
}

pub fn remove() -> Result<()> {
    unsupported()
}

pub fn status() -> Result<()> {
    unsupported()
}
