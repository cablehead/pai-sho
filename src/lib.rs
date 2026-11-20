//! The pai-sho process as a library. The binary in `main.rs` is the CLI.
//! An app links this crate and calls `daemon::run` in-process. The socket
//! stays the way other tools talk to that process.

pub mod client;
pub mod command;
pub mod core;
pub mod daemon;
pub mod enroll;
pub mod ffi;
pub mod grant_store;
pub mod json_file;
#[cfg(unix)]
pub mod launchd;
#[cfg(not(unix))]
#[path = "launchd_unsupported.rs"]
pub mod launchd;
#[cfg(test)]
mod live_tests;
#[cfg(unix)]
pub mod netstack;
pub mod network;
pub mod peer;
pub mod ports;
pub mod protocol;
pub mod qr;
pub mod resolver;
pub mod surface;
pub mod tunnel;
#[cfg(windows)]
pub mod uds;

pub use command::{Command, DaemonService};
