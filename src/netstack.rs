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

/// Linux: attach to a pre-created tun by name (IFF_TUN | IFF_NO_PI). The device
/// is created and configured by the boot step and owned by this user, so the
/// daemon needs no CAP_NET_ADMIN. Packets have no header (IFF_NO_PI).
#[cfg(target_os = "linux")]
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
    set_nonblocking(fd);
    Ok(fd)
}

/// illumos: create a tunN with the tuntap driver (OmniOS `driver/tuntap`), link
/// it under IP, and configure the address and subnet route as on macOS. Needs
/// root, or `sys_ip_config` and `sys_net_config`. In a zone, `/dev/tun` has to
/// be added to the zone, and the zone needs exclusive-IP networking.
///
/// `--tun tunN` asks for unit N, so the interface name stays the same across
/// restarts and firewall rules can name it. Any other name takes the next free
/// unit.
///
/// The interface is linked with I_LINK rather than the I_PLINK ifconfig uses,
/// so it exists only while the `/dev/udp` stream holding the link is open. That
/// stream stays open for the life of the process. When the process exits, even
/// by crashing, IP unplumbs the interface and drops its route.
#[cfg(target_os = "illumos")]
fn open_tun(name: &str) -> Result<RawFd> {
    use libc::{c_char, c_int};
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

    #[repr(C)]
    struct StrIoctl {
        ic_cmd: c_int,
        ic_timout: c_int,
        ic_len: c_int,
        ic_dp: *mut c_char,
    }
    // From the driver's if_tun.h, and illumos <sys/sockio.h> and <sys/stropts.h>.
    const TUNNEWPPA: c_int = ((b'T' as c_int) << 16) | 0x0001;
    const IF_UNITSEL: c_int = 0x8004_7336_u32 as c_int; // _IOW('s', 54, int)
    const RMSGD: c_int = 0x001;

    fn open(path: &std::ffi::CStr) -> Result<OwnedFd> {
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
        anyhow::ensure!(
            fd >= 0,
            "open {} failed (errno {})",
            path.to_string_lossy(),
            errno()
        );
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    let requested: c_int = name
        .strip_prefix("tun")
        .and_then(|n| n.parse().ok())
        .unwrap_or(-1);

    // The packet end. TUNNEWPPA creates a unit and makes this its control
    // stream: what we write here comes out of tunN, and what IP sends to
    // tunN is read here. Closing it destroys the unit.
    let fd = open(c"/dev/tun").map_err(|e| {
        anyhow::anyhow!("{e}; is driver/tuntap installed, and /dev/tun added to this zone?")
    })?;
    let mut ppa = requested;
    let mut req = StrIoctl {
        ic_cmd: TUNNEWPPA,
        ic_timout: 0,
        ic_len: std::mem::size_of::<c_int>() as c_int,
        ic_dp: &mut ppa as *mut c_int as *mut c_char,
    };
    let unit = unsafe { libc::ioctl(fd.as_raw_fd(), libc::I_STR, &mut req as *mut StrIoctl) };
    anyhow::ensure!(
        unit >= 0,
        "creating a tun unit failed (errno {}){}",
        errno(),
        if requested >= 0 {
            format!("; is tun{} already in use?", requested)
        } else {
            String::new()
        }
    );
    let ifname = format!("tun{}", unit);

    // The IP end: a second stream on the same unit with the ip module pushed,
    // linked under a /dev/udp stream so IP plumbs it as tunN.
    let if_fd = open(c"/dev/tun")?;
    let r = unsafe { libc::ioctl(if_fd.as_raw_fd(), libc::I_PUSH, c"ip".as_ptr()) };
    anyhow::ensure!(
        r >= 0,
        "pushing ip onto {} failed (errno {})",
        ifname,
        errno()
    );
    let r = unsafe { libc::ioctl(if_fd.as_raw_fd(), IF_UNITSEL, &unit as *const c_int) };
    anyhow::ensure!(r >= 0, "selecting unit {} failed (errno {})", unit, errno());
    let ip_fd = open(c"/dev/udp")?;
    let r = unsafe { libc::ioctl(ip_fd.as_raw_fd(), libc::I_LINK, if_fd.as_raw_fd()) };
    anyhow::ensure!(
        r >= 0,
        "linking {} under IP failed (errno {}); need root or sys_ip_config",
        ifname,
        errno()
    );
    drop(if_fd);
    info!("created {}", ifname);

    // Configure the address and subnet route. Same layout as macOS: 10.99.0.1
    // local, 10.99.0.2 the point-to-point peer, and the /16 routed into tunN so
    // packets to any surface address reach the stack. The route is an
    // interface route through our own 10.99.0.1, so IP deletes it along with
    // tunN. A route through the gateway 10.99.0.2 would outlive the interface
    // and make the next start fail with "entry exists". On failure, returning
    // drops ip_fd, and IP removes the interface.
    run(
        "/usr/sbin/ifconfig",
        &[&ifname, "10.99.0.1", "10.99.0.2", "up"],
    )?;
    run(
        "/usr/sbin/route",
        &[
            "-n",
            "add",
            "-interface",
            "-net",
            "10.99.0.0/16",
            "10.99.0.1",
        ],
    )?;

    // By default a STREAMS read can return several queued packets joined
    // together. Message-discard mode returns exactly one per read.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), libc::I_SRDOPT, RMSGD) };
    anyhow::ensure!(r >= 0, "I_SRDOPT on {} failed (errno {})", ifname, errno());

    // Kept open for the life of the process; see above.
    let _ = ip_fd.into_raw_fd();
    let fd = fd.into_raw_fd();
    set_nonblocking(fd);
    Ok(fd)
}

/// macOS: create a utun via the SYSPROTO_CONTROL socket, configure its address
/// and the subnet route, and return the fd. Needs root (utun creation and
/// ifconfig/route both do). The `name` arg is ignored; the kernel assigns the
/// next free utunN. utun frames carry a 4-byte address-family header (handled
/// in tun_read/tun_write).
#[cfg(target_os = "macos")]
fn open_tun(_name: &str) -> Result<RawFd> {
    use std::mem;
    let fd = unsafe { libc::socket(libc::PF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL) };
    anyhow::ensure!(fd >= 0, "utun socket failed (errno {}); need root", errno());

    let mut info: libc::ctl_info = unsafe { mem::zeroed() };
    for (i, c) in b"com.apple.net.utun_control".iter().enumerate() {
        info.ctl_name[i] = *c as libc::c_char;
    }
    let r = unsafe { libc::ioctl(fd, libc::CTLIOCGINFO, &mut info) };
    anyhow::ensure!(r >= 0, "CTLIOCGINFO failed (errno {})", errno());

    let mut addr: libc::sockaddr_ctl = unsafe { mem::zeroed() };
    addr.sc_len = mem::size_of::<libc::sockaddr_ctl>() as u8;
    addr.sc_family = libc::AF_SYSTEM as u8;
    addr.ss_sysaddr = libc::AF_SYS_CONTROL as u16;
    addr.sc_id = info.ctl_id;
    addr.sc_unit = 0; // kernel assigns the next free utun
    let r = unsafe {
        libc::connect(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_ctl>() as libc::socklen_t,
        )
    };
    anyhow::ensure!(r >= 0, "utun connect failed (errno {}); need root", errno());

    // read back the assigned interface name (utunN)
    let mut namebuf = [0u8; 32];
    let mut len = namebuf.len() as libc::socklen_t;
    const UTUN_OPT_IFNAME: libc::c_int = 2;
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::SYSPROTO_CONTROL,
            UTUN_OPT_IFNAME,
            namebuf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
        )
    };
    anyhow::ensure!(r >= 0, "UTUN_OPT_IFNAME failed (errno {})", errno());
    let ifname = std::str::from_utf8(&namebuf[..len.saturating_sub(1) as usize])
        .unwrap_or("utun")
        .to_string();
    info!("created {}", ifname);

    // Configure the address and subnet route (we are root here). Mirrors the
    // Linux boot step's `ip addr add 10.99.0.1/16 dev ps0`: 10.99.0.1 local,
    // 10.99.0.2 the point-to-point peer, and the /16 routed into the utun so
    // packets to any surface address reach the stack.
    run("ifconfig", &[&ifname, "10.99.0.1", "10.99.0.2", "up"])?;
    run(
        "route",
        &[
            "-q",
            "-n",
            "add",
            "-net",
            "10.99.0.0/16",
            "-interface",
            &ifname,
        ],
    )?;

    set_nonblocking(fd);
    Ok(fd)
}

fn set_nonblocking(fd: RawFd) {
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
}

/// Read one IP packet off the tun. On macOS, strip the 4-byte utun
/// address-family header; on Linux (IFF_NO_PI) and illumos there is none.
fn tun_read(fd: RawFd) -> io::Result<Vec<u8>> {
    #[cfg(target_os = "macos")]
    let header = 4usize;
    #[cfg(not(target_os = "macos"))]
    let header = 0usize;

    let mut buf = vec![0u8; MTU + header];
    // libc::read returns isize: >header is a packet, 0..=header is a runt/empty
    // (treat as would-block), negative is an error (EAGAIN maps to WouldBlock).
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n > header as isize {
        Ok(buf[header..n as usize].to_vec())
    } else if n >= 0 {
        Err(io::ErrorKind::WouldBlock.into())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Write one IP packet to the tun. On macOS, prepend the 4-byte utun
/// address-family header (AF_INET / AF_INET6, big-endian); on Linux, none.
fn tun_write(fd: RawFd, pkt: &[u8]) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if pkt.is_empty() {
            return Ok(());
        }
        // 4 = AF_INET, 30 = AF_INET6 on macOS, big-endian
        let af: u32 = if pkt[0] >> 4 == 6 { 30 } else { 2 };
        let mut framed = Vec::with_capacity(4 + pkt.len());
        framed.extend_from_slice(&af.to_be_bytes());
        framed.extend_from_slice(pkt);
        let n = unsafe { libc::write(fd, framed.as_ptr() as *const libc::c_void, framed.len()) };
        if n >= 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let n = unsafe { libc::write(fd, pkt.as_ptr() as *const libc::c_void, pkt.len()) };
        if n >= 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Run a command, erroring on nonzero exit. Used on macOS and illumos to
/// configure the interface.
#[cfg(any(target_os = "macos", target_os = "illumos"))]
fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {}", cmd))?;
    // illumos route(8) reports failures such as "entry exists" on stdout.
    let output = if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    anyhow::ensure!(
        out.status.success(),
        "{} {}: {}",
        cmd,
        args.join(" "),
        String::from_utf8_lossy(output).trim()
    );
    Ok(())
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
        |t: &mut Tun| tun_read(t.fd),
        |t: &mut Tun, pkt: &[u8]| tun_write(t.fd, pkt),
        caps,
    )
    .context("failed to create tun async device")?;

    let mut ifcfg = tokio_smoltcp::smoltcp::iface::Config::new(HardwareAddress::Ip);
    ifcfg.random_seed = rand::random();
    let cfg = NetConfig::new(ifcfg, IpCidr::new(ip4(resolver_ip), 16), vec![]);
    let net = Net::new(device, cfg);
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
