//! The pai-sho process as a library. The binary in `main.rs` is the CLI.
//! An app links this crate and calls `daemon::run` in-process. The socket
//! stays the way other tools talk to that process.

pub mod client;
pub mod command;
pub mod core;
pub mod daemon;
pub mod enroll;
pub mod launchd;
#[cfg(test)]
mod live_tests;
pub mod netstack;
pub mod peer;
pub mod protocol;
pub mod resolver;
pub mod surface;
pub mod tunnel;

pub use command::{Command, DaemonService};
