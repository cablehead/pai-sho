//! TUN owned-network backend (userspace stack). See docs/adr/0005.
//!
//! The operator owns one TUN and a subnet routed to it. This module runs a
//! smoltcp userspace stack on that TUN: it answers `.pai-sho` DNS on the resolver
//! address in-stack (no socket bind, no privilege), and accepts TCP to each
//! projected surface address, handing every connection to the daemon to splice
//! onto the peer's QUIC tunnel.
//!
//! smoltcp is synchronous and poll-driven, so the stack runs on a dedicated
//! thread woken by an eventfd. The daemon drives it with `Cmd`s and consumes
//! `Accept`s over tokio channels; per-connection bytes cross on bounded
//! channels (backpressure maps to the TCP window). The datapath primitives
//! (tun<->smoltcp, DNS on :53, TCP accept/recv/send) are validated on a real
//! tun; see ~/ps-netstack-probe.

use crate::resolver;
use anyhow::{Context, Result};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::info;

const MTU: usize = 1500;
const CONN_CHAN: usize = 32; // per-connection buffered chunks (backpressure)

/// A control command to the stack thread.
enum Cmd {
    AddSurface { ip: Ipv4Addr, name: Option<String> },
    RemoveSurface { ip: Ipv4Addr },
    Listen { ip: Ipv4Addr, port: u16 },
    Unlisten { ip: Ipv4Addr, port: u16 },
}

/// An accepted TCP connection, handed to the daemon to splice onto a tunnel.
/// `from_client` yields bytes the client sent; `to_client` accepts bytes to
/// send back. Dropping either end tears the connection down.
pub struct Accept {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub from_client: mpsc::Receiver<Vec<u8>>,
    pub to_client: mpsc::Sender<Vec<u8>>,
}

/// Wakes the stack thread out of its poll() (eventfd write).
#[derive(Clone)]
struct Wake {
    fd: Arc<OwnedEventFd>,
}
impl Wake {
    fn signal(&self) {
        let one: u64 = 1;
        unsafe { libc::write(self.fd.0, &one as *const u64 as *const libc::c_void, 8) };
    }
}
struct OwnedEventFd(RawFd);
impl Drop for OwnedEventFd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Handle the daemon uses to drive the stack.
#[derive(Clone)]
pub struct NetStack {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    wake: Wake,
}

impl NetStack {
    pub fn add_surface(&self, ip: Ipv4Addr, name: Option<String>) {
        let _ = self.cmd_tx.send(Cmd::AddSurface { ip, name });
        self.wake.signal();
    }
    pub fn remove_surface(&self, ip: Ipv4Addr) {
        let _ = self.cmd_tx.send(Cmd::RemoveSurface { ip });
        self.wake.signal();
    }
    pub fn listen(&self, ip: Ipv4Addr, port: u16) {
        let _ = self.cmd_tx.send(Cmd::Listen { ip, port });
        self.wake.signal();
    }
    pub fn unlisten(&self, ip: Ipv4Addr, port: u16) {
        let _ = self.cmd_tx.send(Cmd::Unlisten { ip, port });
        self.wake.signal();
    }
    /// Nudge the stack to re-poll (a forward task pushed bytes to a client).
    pub fn wake(&self) {
        self.wake.signal();
    }
}

/// Start the stack on `tun_name` (a pre-created, app-owned TUN) with the
/// resolver answering at `resolver_ip:53`. Returns the control handle and the
/// stream of accepted connections.
pub fn spawn(
    tun_name: &str,
    resolver_ip: Ipv4Addr,
) -> Result<(NetStack, mpsc::UnboundedReceiver<Accept>)> {
    let tun_fd = open_tun(tun_name)?;
    let event_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK) };
    anyhow::ensure!(event_fd >= 0, "eventfd failed");
    let wake = Wake {
        fd: Arc::new(OwnedEventFd(event_fd)),
    };

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (accept_tx, accept_rx) = mpsc::unbounded_channel();

    let wake_thread = wake.clone();
    std::thread::Builder::new()
        .name("pai-sho-netstack".into())
        .spawn(move || {
            run(
                tun_fd,
                event_fd,
                resolver_ip,
                cmd_rx,
                accept_tx,
                wake_thread,
            )
        })
        .context("spawn netstack thread")?;

    Ok((NetStack { cmd_tx, wake }, accept_rx))
}

// ---- tun device ------------------------------------------------------------

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
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn now() -> SmolInstant {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    SmolInstant::from_millis(d.as_millis() as i64)
}

struct TunDevice {
    fd: RawFd,
}
struct RxTok {
    buf: Vec<u8>,
}
struct TxTok {
    fd: RawFd,
}
impl Device for TunDevice {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok;
    fn receive(&mut self, _t: SmolInstant) -> Option<(RxTok, TxTok)> {
        let mut buf = vec![0u8; MTU];
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n > 0 {
            buf.truncate(n as usize);
            Some((RxTok { buf }, TxTok { fd: self.fd }))
        } else {
            None
        }
    }
    fn transmit(&mut self, _t: SmolInstant) -> Option<TxTok> {
        Some(TxTok { fd: self.fd })
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = MTU;
        c
    }
}
impl RxToken for RxTok {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R {
        f(&mut self.buf)
    }
}
impl TxToken for TxTok {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        unsafe { libc::write(self.fd, buf.as_ptr() as *const libc::c_void, len) };
        r
    }
}

// ---- the stack thread ------------------------------------------------------

/// Per-connection state held by the stack (the other ends live in the daemon).
struct Conn {
    to_client_rx: mpsc::Receiver<Vec<u8>>, // daemon -> client (QUIC -> tun)
    from_client_tx: mpsc::Sender<Vec<u8>>, // client -> daemon (tun -> QUIC)
    pending: Vec<u8>,                      // not yet written into the tcp socket
}

fn tcp_socket() -> tcp::Socket<'static> {
    let rx = tcp::SocketBuffer::new(vec![0u8; 16 * 1024]);
    let tx = tcp::SocketBuffer::new(vec![0u8; 16 * 1024]);
    tcp::Socket::new(rx, tx)
}

fn run(
    tun_fd: RawFd,
    event_fd: RawFd,
    resolver_ip: Ipv4Addr,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    accept_tx: mpsc::UnboundedSender<Accept>,
    wake: Wake,
) {
    let mut device = TunDevice { fd: tun_fd };
    let config = Config::new(HardwareAddress::Ip);
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(ip4(resolver_ip), 16));
    });

    let mut sockets = SocketSet::new(vec![]);

    // resolver UDP socket on :53
    let udp_rx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 16], vec![0u8; 16 * 1024]);
    let udp_tx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 16], vec![0u8; 16 * 1024]);
    let mut udp_sock = udp::Socket::new(udp_rx, udp_tx);
    udp_sock.bind(53).expect("bind :53");
    let udp_handle = sockets.add(udp_sock);

    let mut names: HashMap<String, Ipv4Addr> = HashMap::new();
    let mut wanted: std::collections::HashSet<(Ipv4Addr, u16)> = Default::default();
    let mut listeners: HashMap<SocketHandle, (Ipv4Addr, u16)> = HashMap::new();
    let mut conns: HashMap<SocketHandle, Conn> = HashMap::new();

    info!("netstack up: resolver {}:53, iface /16", resolver_ip);

    loop {
        // 1. apply control commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::AddSurface { ip, name } => {
                    iface.update_ip_addrs(|addrs| {
                        if !addrs.iter().any(|c| c.address() == ip4(ip)) {
                            let _ = addrs.push(IpCidr::new(ip4(ip), 16));
                        }
                    });
                    if let Some(n) = name {
                        names.insert(n, ip);
                    }
                }
                Cmd::RemoveSurface { ip } => {
                    iface.update_ip_addrs(|addrs| addrs.retain(|c| c.address() != ip4(ip)));
                    names.retain(|_, v| *v != ip);
                }
                Cmd::Listen { ip, port } => {
                    wanted.insert((ip, port));
                }
                Cmd::Unlisten { ip, port } => {
                    wanted.remove(&(ip, port));
                }
            }
        }

        // 2. ensure a listener exists for each wanted (ip, port)
        let armed: std::collections::HashSet<(Ipv4Addr, u16)> =
            listeners.values().copied().collect();
        for &(ip, port) in wanted.iter() {
            if !armed.contains(&(ip, port)) {
                let mut s = tcp_socket();
                if s.listen(IpListenEndpoint {
                    addr: Some(ip4(ip)),
                    port,
                })
                .is_ok()
                {
                    let h = sockets.add(s);
                    listeners.insert(h, (ip, port));
                }
            }
        }

        // 3. poll the stack
        iface.poll(now(), &mut device, &mut sockets);

        // 4. resolver
        {
            let s = sockets.get_mut::<udp::Socket>(udp_handle);
            while s.can_recv() {
                let Ok((data, meta)) = s.recv().map(|(d, m)| (d.to_vec(), m)) else {
                    break;
                };
                if let Some(reply) = resolver::reply(&data, |label| names.get(label).copied()) {
                    let _ = s.send_slice(&reply, meta.endpoint);
                }
            }
        }

        // 5. promote accepted listeners -> conns
        let promote: Vec<SocketHandle> = listeners
            .keys()
            .copied()
            .filter(|h| {
                let s = sockets.get::<tcp::Socket>(*h);
                s.state() != tcp::State::Listen && s.state() != tcp::State::Closed
            })
            .collect();
        for h in promote {
            let (ip, port) = listeners.remove(&h).unwrap();
            let (to_client_tx, to_client_rx) = mpsc::channel::<Vec<u8>>(CONN_CHAN);
            let (from_client_tx, from_client_rx) = mpsc::channel::<Vec<u8>>(CONN_CHAN);
            conns.insert(
                h,
                Conn {
                    to_client_rx,
                    from_client_tx,
                    pending: Vec::new(),
                },
            );
            let _ = accept_tx.send(Accept {
                ip,
                port,
                from_client: from_client_rx,
                to_client: to_client_tx,
            });
            // re-arm a fresh listener for this (ip, port) if still wanted
            if wanted.contains(&(ip, port)) {
                let mut s = tcp_socket();
                if s.listen(IpListenEndpoint {
                    addr: Some(ip4(ip)),
                    port,
                })
                .is_ok()
                {
                    let nh = sockets.add(s);
                    listeners.insert(nh, (ip, port));
                }
            }
        }

        // 6. shuttle connection bytes
        let mut dead: Vec<SocketHandle> = Vec::new();
        for (h, conn) in conns.iter_mut() {
            let s = sockets.get_mut::<tcp::Socket>(*h);

            // client -> daemon (only while the daemon has capacity)
            while s.can_recv() && conn.from_client_tx.capacity() > 0 {
                let chunk = s.recv(|b| (b.len(), b.to_vec())).unwrap_or_default();
                if chunk.is_empty() {
                    break;
                }
                if conn.from_client_tx.try_send(chunk).is_err() {
                    break;
                }
            }

            // daemon -> client
            if conn.pending.is_empty() {
                match conn.to_client_rx.try_recv() {
                    Ok(buf) => conn.pending = buf,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        if s.may_send() {
                            s.close();
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {}
                }
            }
            if !conn.pending.is_empty() && s.can_send() {
                if let Ok(n) = s.send_slice(&conn.pending) {
                    conn.pending.drain(..n);
                }
            }

            // teardown: peer gone (client closed) or socket closed
            if conn.from_client_tx.is_closed() && s.may_send() {
                s.close();
            }
            if s.state() == tcp::State::Closed {
                dead.push(*h);
            }
        }
        for h in dead {
            conns.remove(&h);
            sockets.remove(h);
        }

        // 7. wait for tun readable, a wake (cmd or daemon bytes), or the timer
        let timeout_ms = iface
            .poll_delay(now(), &sockets)
            .map(|d| d.total_millis() as libc::c_int)
            .unwrap_or(1000)
            .clamp(0, 1000);
        let mut fds = [
            libc::pollfd {
                fd: tun_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: event_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
        if fds[1].revents & libc::POLLIN != 0 {
            let mut b = [0u8; 8];
            unsafe { libc::read(event_fd, b.as_mut_ptr() as *mut libc::c_void, 8) };
        }
        // keep `wake` alive for the thread's lifetime (owns the eventfd)
        let _ = &wake;
    }
}

fn ip4(ip: Ipv4Addr) -> IpAddress {
    let o = ip.octets();
    IpAddress::v4(o[0], o[1], o[2], o[3])
}
