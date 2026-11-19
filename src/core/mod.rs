//! The pure core: decisions with no IO.
//!
//! Everything here is decidable from in-memory state. The shell in `peer.rs`
//! feeds it events and carries out the actions it returns.

pub mod accept;
pub mod backoff;
pub mod dial;
pub mod grants;
pub mod invite;
pub mod refusals;
pub mod session;
