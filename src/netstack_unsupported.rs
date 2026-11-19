//! The TUN backend's interface on platforms without one (Windows). The real
//! backend in `netstack.rs` reads a unix tun fd. Here `NetStack` has no
//! values, so the daemon's `Option<NetStack>` is always `None`, and `spawn`
//! fails. `daemon::run` refuses `--tun` before it gets that far.

use anyhow::Result;
use std::net::Ipv4Addr;
use tokio::sync::mpsc;

pub struct Accept {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub stream: tokio::net::TcpStream,
}

#[derive(Clone)]
pub enum NetStack {}

impl NetStack {
    pub fn add_surface(&self, _ip: Ipv4Addr, _name: Option<String>) {
        match *self {}
    }

    pub fn remove_surface(&self, _ip: Ipv4Addr) {
        match *self {}
    }

    pub fn listen(&self, _ip: Ipv4Addr, _port: u16) {
        match *self {}
    }

    pub fn unlisten(&self, _ip: Ipv4Addr, _port: u16) {
        match *self {}
    }
}

pub fn spawn(
    tun_name: &str,
    _resolver_ip: Ipv4Addr,
) -> Result<(NetStack, mpsc::UnboundedReceiver<Accept>)> {
    anyhow::bail!("tun {tun_name} is not available on this OS")
}
