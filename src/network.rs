//! Where peers' surfaces live on this machine: real loopback addresses, or the
//! private network the tun backend runs in userspace (ADR 0005).
//!
//! `PeerManager` decides what to bind; this says how. Everything that differs
//! between the two is here, so the rest of the daemon never asks which one it
//! has, and a platform without a tun device (Windows) leaves `Tun` out.

use anyhow::Result;
use std::net::{IpAddr, Ipv4Addr};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

#[cfg(unix)]
use crate::netstack::NetStack;
use crate::surface;

/// A connection a peer's surface accepted, for the daemon to splice onto that
/// peer's tunnel. Only the tun network hands these over: on loopback the
/// daemon accepts on its own listeners.
pub struct Accept {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub stream: Box<dyn Stream>,
}

/// A byte stream an [`Accept`] carries.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

#[derive(Clone)]
pub enum Network {
    /// Each surface is an address on the loopback interface, and each of its
    /// ports a real listening socket.
    Loopback,
    /// Each surface is an address in the userspace stack over a tun device.
    #[cfg(unix)]
    Tun(NetStack),
}

// Without a tun network, the arguments only it reads go unused.
#[cfg_attr(not(unix), allow(unused_variables))]
impl Network {
    /// Bring up the tun network on device `dev`, with its resolver at
    /// `resolver_ip`. Returns the stream of connections its surfaces accept.
    pub fn tun(
        dev: &str,
        resolver_ip: Ipv4Addr,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Accept>)> {
        #[cfg(unix)]
        {
            let (ns, accepts) = crate::netstack::spawn(dev, resolver_ip)?;
            Ok((Network::Tun(ns), accepts))
        }
        #[cfg(not(unix))]
        {
            let _ = resolver_ip;
            anyhow::bail!("--tun {dev}: the tun backend is not available on this OS")
        }
    }

    /// The /24 surface addresses are allocated from.
    pub fn base(&self) -> [u8; 3] {
        match self {
            Network::Loopback => surface::LOOPBACK_BASE,
            #[cfg(unix)]
            Network::Tun(_) => surface::TUN_BASE,
        }
    }

    /// Whether ports listen inside the network itself. If not, the daemon binds
    /// real sockets for them.
    pub fn listens_itself(&self) -> bool {
        !matches!(self, Network::Loopback)
    }

    /// Bring up a surface address, and on the tun network its name.
    pub fn claim(&self, ip: IpAddr, name: Option<String>) -> Result<()> {
        match self {
            Network::Loopback => surface::ensure_addr(ip),
            #[cfg(unix)]
            Network::Tun(ns) => {
                if let IpAddr::V4(v4) = ip {
                    ns.add_surface(v4, name);
                }
                Ok(())
            }
        }
    }

    /// Take a surface address down.
    pub fn unclaim(&self, ip: IpAddr) -> Result<()> {
        match self {
            Network::Loopback => surface::remove_addr(ip),
            #[cfg(unix)]
            Network::Tun(ns) => {
                if let IpAddr::V4(v4) = ip {
                    ns.remove_surface(v4);
                }
                Ok(())
            }
        }
    }

    /// Answer `<name>.pai-sho` with this node's own address `ip`, on the tun
    /// network's in-stack resolver. A no-op on loopback, where the daemon's
    /// resolver looks names up itself.
    pub fn name_self(&self, ip: IpAddr, name: &str) {
        match self {
            Network::Loopback => {}
            #[cfg(unix)]
            Network::Tun(ns) => {
                if let IpAddr::V4(v4) = ip {
                    ns.add_surface(v4, Some(name.to_string()));
                }
            }
        }
    }

    /// Listen on a surface's port, when the network does that itself (see
    /// [`Network::listens_itself`]); a no-op on loopback.
    pub fn listen(&self, ip: IpAddr, port: u16) {
        match self {
            Network::Loopback => {}
            #[cfg(unix)]
            Network::Tun(ns) => {
                if let IpAddr::V4(v4) = ip {
                    ns.listen(v4, port);
                }
            }
        }
    }

    /// Stop listening on a surface's port; a no-op on loopback.
    pub fn unlisten(&self, ip: IpAddr, port: u16) {
        match self {
            Network::Loopback => {}
            #[cfg(unix)]
            Network::Tun(ns) => {
                if let IpAddr::V4(v4) = ip {
                    ns.unlisten(v4, port);
                }
            }
        }
    }
}
