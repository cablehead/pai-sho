//! TUN owned-network backend (tokio-smoltcp userspace stack). See ADR 0005.
//!
//! The operator owns one TUN and a subnet routed to it. `tokio-smoltcp` runs a
//! userspace stack over it and gives real async sockets: the `.pai-sho`
//! resolver is a `UdpSocket` on `10.99.0.53:53`, and each projected surface
//! port is a `TcpListener` whose accepted `TcpStream`s the daemon splices onto
//! the peer's QUIC tunnel with `tokio::io::copy`. `set_any_ip` lets us bind any
//! surface address in the subnet without adding each to the interface.
//!
//! Datapath (DNS on :53, TCP accept via any_ip) is validated on a real tun as
//! an unprivileged process; see ~/ps-netstack-probe. The daemon opens a
//! pre-created, app-owned tun, so it holds no capabilities at runtime.

use crate::resolver;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_smoltcp::device::AsyncCapture;
use tokio_smoltcp::smoltcp::{
    phy::{DeviceCapabilities, Medium},
    wire::{HardwareAddress, IpAddress, IpCidr},
};
use tokio_smoltcp::{Net, NetConfig, TcpStream};
use tracing::{error, info, warn};

const MTU: usize = 1500;

/// Live listener accept-loops, keyed by the surface (ip, port) they serve.
type Listeners = Arc<Mutex<HashMap<(Ipv4Addr, u16), JoinHandle<()>>>>;

/// An accepted TCP connection on a surface, handed to the daemon to splice onto
/// the owning peer's tunnel. `stream` is a real async TCP stream.
pub struct Accept {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub stream: TcpStream,
}

/// Handle the daemon uses to drive the stack.
#[derive(Clone)]
pub struct NetStack {
    net: Arc<Net>,
    /// surface name -> address, for the in-stack resolver
    names: Arc<Mutex<HashMap<String, Ipv4Addr>>>,
    /// live listener accept-loops, one per (ip, port)
    listeners: Listeners,
    accept_tx: mpsc::UnboundedSender<Accept>,
}

impl NetStack {
    /// Record a surface's name for the resolver. With `any_ip` the address needs
    /// no interface change, so this is just the name map.
    pub fn add_surface(&self, ip: Ipv4Addr, name: Option<String>) {
        if let Some(name) = name {
            self.names.lock().unwrap().insert(name, ip);
        }
    }

    pub fn remove_surface(&self, ip: Ipv4Addr) {
        self.names.lock().unwrap().retain(|_, v| *v != ip);
    }

    /// Start accepting on `ip:port`, feeding each connection to the daemon.
    /// Idempotent per (ip, port).
    pub fn listen(&self, ip: Ipv4Addr, port: u16) {
        let key = (ip, port);
        let mut listeners = self.listeners.lock().unwrap();
        if listeners.contains_key(&key) {
            return;
        }
        let net = self.net.clone();
        let tx = self.accept_tx.clone();
        let handle = tokio::spawn(async move {
            let mut listener = match net.tcp_bind(SocketAddr::from((ip, port))).await {
                Ok(l) => l,
                Err(e) => {
                    error!("tun tcp_bind {}:{} failed: {}", ip, port, e);
                    return;
                }
            };
            info!("tun listening {}:{}", ip, port);
            loop {
                match listener.accept().await {
                    Ok((stream, _from)) => {
                        if tx.send(Accept { ip, port, stream }).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("tun accept {}:{} error: {}", ip, port, e);
                        break;
                    }
                }
            }
        });
        listeners.insert(key, handle);
    }

    /// Stop accepting on `ip:port`.
    pub fn unlisten(&self, ip: Ipv4Addr, port: u16) {
        if let Some(handle) = self.listeners.lock().unwrap().remove(&(ip, port)) {
            handle.abort();
        }
    }
}

struct Tun {
    fd: RawFd,
}
impl AsRawFd for Tun {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

/// Attach to a pre-created tun by name (IFF_TUN | IFF_NO_PI), nonblocking. The
/// device must already exist and be owned by this user, so no CAP_NET_ADMIN.
fn open_tun(name: &str) -> Result<RawFd> {
    let fd = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR) };
    anyhow::ensure!(fd >= 0, "open /dev/net/tun failed (errno {})", errno());
    let mut ifr = [0u8; 40];
    let nb = name.as_bytes();
    anyhow::ensure!(nb.len() < 16, "tun name too long");
    ifr[..nb.len()].copy_from_slice(nb);
    let flags: libc::c_short = (libc::IFF_TUN | libc::IFF_NO_PI) as libc::c_short;
    ifr[16..18].copy_from_slice(&flags.to_ne_bytes());
    const TUNSETIFF: libc::Ioctl = 0x400454ca;
    let r = unsafe { libc::ioctl(fd, TUNSETIFF, ifr.as_ptr()) };
    anyhow::ensure!(
        r >= 0,
        "TUNSETIFF {} failed (errno {}); is the tun pre-created and owned by this user?",
        name,
        errno()
    );
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
    Ok(fd)
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn ip4(ip: Ipv4Addr) -> IpAddress {
    let o = ip.octets();
    IpAddress::v4(o[0], o[1], o[2], o[3])
}

/// Bring up the stack on `tun_name` with the resolver at `resolver_ip:53`.
/// Must be called within a tokio runtime (Net spawns its reactor).
pub fn spawn(
    tun_name: &str,
    resolver_ip: Ipv4Addr,
) -> Result<(NetStack, mpsc::UnboundedReceiver<Accept>)> {
    let tun = Tun {
        fd: open_tun(tun_name)?,
    };

    let mut caps = DeviceCapabilities::default();
    caps.medium = Medium::Ip;
    caps.max_transmission_unit = MTU;
    caps.max_burst_size = Some(tokio_smoltcp::device::DEFAULT_MAX_BURST_SIZE);

    let device = AsyncCapture::new(
        tun,
        |t: &mut Tun| {
            let mut buf = vec![0u8; MTU];
            let n = unsafe { libc::read(t.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n > 0 {
                buf.truncate(n as usize);
                Ok(buf)
            } else {
                Err(io::Error::last_os_error())
            }
        },
        |t: &mut Tun, pkt: &[u8]| {
            let n = unsafe { libc::write(t.fd, pkt.as_ptr() as *const libc::c_void, pkt.len()) };
            if n >= 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        },
        caps,
    )
    .context("failed to create tun async device")?;

    let mut ifcfg = tokio_smoltcp::smoltcp::iface::Config::new(HardwareAddress::Ip);
    ifcfg.random_seed = rand::random();
    let net = Net::new(
        device,
        NetConfig::new(ifcfg, IpCidr::new(ip4(resolver_ip), 16), vec![]),
    );
    net.set_any_ip(true);
    let net = Arc::new(net);

    let names: Arc<Mutex<HashMap<String, Ipv4Addr>>> = Arc::new(Mutex::new(HashMap::new()));
    let (accept_tx, accept_rx) = mpsc::unbounded_channel();

    // In-stack resolver on <resolver_ip>:53, answering .pai-sho from the name map.
    {
        let net = net.clone();
        let names = names.clone();
        tokio::spawn(async move {
            let sock = match net.udp_bind(SocketAddr::from((resolver_ip, 53))).await {
                Ok(s) => s,
                Err(e) => {
                    error!("resolver bind {}:53 failed: {}", resolver_ip, e);
                    return;
                }
            };
            info!(
                "tun resolver on {}:53 for *.{}",
                resolver_ip,
                resolver::SUFFIX
            );
            let mut buf = [0u8; 1500];
            loop {
                match sock.recv_from(&mut buf).await {
                    Ok((n, from)) => {
                        let reply = resolver::reply(&buf[..n], |label| {
                            names.lock().unwrap().get(label).copied()
                        });
                        if let Some(reply) = reply {
                            let _ = sock.send_to(&reply, from).await;
                        }
                    }
                    Err(e) => {
                        warn!("resolver recv error: {}", e);
                        break;
                    }
                }
            }
        });
    }

    let stack = NetStack {
        net,
        names,
        listeners: Arc::new(Mutex::new(HashMap::new())),
        accept_tx,
    };
    Ok((stack, accept_rx))
}
