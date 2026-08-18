//! Live tests over loopback: two daemons, real QUIC, no network.
//!
//! Endpoints are built with relays disabled and a `MemoryLookup` seeded with
//! each other's bound sockets, so nothing here reaches a relay, DNS, or the
//! LAN. What is covered is the part the pure core cannot reach: that bytes
//! actually cross a granted tunnel, and that the shell carries out the core's
//! verdicts.
//!
//! The admission and authorization rules themselves are asserted in
//! src/core/session.rs, without any of this machinery.

use crate::daemon::Daemon;
use crate::protocol::{Request, Response, ALPN};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::{presets, PortmapperConfig};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A temp directory that cleans up when the test ends, pass or fail.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pai-sho-test-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn prefix(&self) -> PathBuf {
        self.0.join("key")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An endpoint that cannot leave the machine: no relays, no address lookup
/// beyond what we seed by hand, no port mapping.
async fn test_endpoint(lookup: &MemoryLookup) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .portmapper_config(PortmapperConfig::Disabled)
        .clear_address_lookup()
        .address_lookup(lookup.clone())
        .alpns(vec![ALPN.to_vec()])
        .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind()
        .await
        .unwrap()
}

fn addr_of(ep: &Endpoint) -> EndpointAddr {
    let mut addr = EndpointAddr::new(ep.id());
    for sock in ep.bound_sockets() {
        addr = addr.with_ip_addr(sock);
    }
    addr
}

struct TestDaemon {
    daemon: Arc<Daemon>,
    _router: iroh::protocol::Router,
    _dir: TempDir,
}

impl TestDaemon {
    async fn start(tag: &str, lookup: &MemoryLookup, host: IpAddr) -> Self {
        let dir = TempDir::new(tag);
        let endpoint = test_endpoint(lookup).await;
        let daemon = Daemon::with_endpoint(endpoint, host, &dir.prefix(), None, None)
            .await
            .unwrap();
        let router = daemon.spawn_router();
        Self {
            daemon,
            _router: router,
            _dir: dir,
        }
    }

    fn key(&self) -> String {
        self.daemon.endpoint().id().to_string()
    }

    async fn request(&self, req: Request) -> Response {
        self.daemon.handle_request(req).await
    }
}

/// Two daemons that can find each other, and nothing else.
async fn pair(host: IpAddr) -> (TestDaemon, TestDaemon) {
    let (lookup_a, lookup_b) = (MemoryLookup::new(), MemoryLookup::new());
    let a = TestDaemon::start("a", &lookup_a, host).await;
    let b = TestDaemon::start("b", &lookup_b, host).await;

    lookup_a.add_endpoint_info(addr_of(b.daemon.endpoint()));
    lookup_b.add_endpoint_info(addr_of(a.daemon.endpoint()));

    (a, b)
}

/// Link `b` to `a` the way an operator would: `a` extends an invitation and
/// `b` takes it up.
async fn enroll(a: &TestDaemon, b: &TestDaemon, name: &str) {
    let invite = match a
        .request(Request::Invite {
            key: None,
            name: Some(name.to_string()),
            expose: vec![],
        })
        .await
    {
        Response::Invite(i) => i,
        other => panic!("expected an invitation, got {:?}", other),
    };

    match b
        .request(Request::Accept {
            handle: invite,
            name: Some("a".to_string()),
        })
        .await
    {
        Response::Ok => {}
        other => panic!("accept failed: {:?}", other),
    }
}

/// A TCP echo server on 127.0.0.1, standing in for whatever a daemon forwards
/// to. Returns the port it bound.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if sock.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

/// Poll until `f` returns Some, or fail after `secs`. Nothing here waits on a
/// fixed sleep: the daemons settle at their own pace.
async fn until<T, F, Fut>(secs: u64, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out after {}s", secs);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Where `b` has bound `a`'s `port`, once it has.
async fn bound_addr(b: &TestDaemon, port: u16) -> SocketAddr {
    until(20, || async {
        let peers = b.daemon.peers().list().await;
        let p = peers.iter().find(|p| p.bound.contains(&port))?;
        let ip: IpAddr = p.ip.as_ref()?.parse().ok()?;
        Some(SocketAddr::from((ip, port)))
    })
    .await
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_granted_port_carries_bytes() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;

    let addr = bound_addr(&b, port).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"hello pai-sho").await.unwrap();

    let mut buf = [0u8; 13];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello pai-sho");
}

#[tokio::test]
async fn an_ungranted_port_is_never_announced() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    // Grant, wait for the binding, then revoke.
    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;
    bound_addr(&b, port).await;

    a.request(Request::Unexpose { port, to: None }).await;

    // The revocation reaches b and its binding goes away.
    until(20, || async {
        let peers = b.daemon.peers().list().await;
        peers.iter().all(|p| !p.bound.contains(&port)).then_some(())
    })
    .await;
}

#[tokio::test]
async fn a_peer_with_no_grant_is_told_nothing() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    // Wait for the link, then confirm b was announced no ports at all.
    until(20, || async {
        let peers = b.daemon.peers().list().await;
        peers.iter().find(|p| p.online).map(|_| ())
    })
    .await;

    let peers = b.daemon.peers().list().await;
    let a_seen = peers.iter().find(|p| p.key == a.key()).unwrap();
    assert!(a_seen.they_expose.is_empty());
}

#[tokio::test]
async fn an_unenrolled_peer_is_refused() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;

    // b reaches for a by key alone, with no invitation. a admits nothing.
    b.request(Request::Accept {
        handle: a.key(),
        name: None,
    })
    .await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let peers = a.daemon.peers().list().await;
    assert!(
        peers.is_empty(),
        "a admitted an unenrolled peer: {:?}",
        peers
    );
}

#[tokio::test]
async fn forwarding_honours_the_host_address() {
    // A daemon told to forward to an address with nothing on it still
    // announces the grant; the tunnel just finds no service.
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))).await;
    enroll(&a, &b, "b").await;

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;

    let addr = bound_addr(&b, port).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();

    // 127.0.0.2 has no listener on that port, so the tunnel closes without
    // ever reaching the echo server on 127.0.0.1.
    let mut buf = [0u8; 1];
    let _ = sock.write_all(b"x").await;
    assert_eq!(sock.read(&mut buf).await.unwrap(), 0);
}

#[tokio::test]
async fn a_peer_added_before_its_daemon_is_up_is_still_recorded() {
    let lookup = MemoryLookup::new();
    let b = TestDaemon::start("b", &lookup, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;

    // A key that resolves to nothing: the dial cannot succeed.
    let absent = iroh::SecretKey::from_bytes(&[7u8; 32]).public().to_string();
    b.request(Request::Accept {
        handle: absent.clone(),
        name: None,
    })
    .await;

    let peers = b.daemon.peers().list().await;
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].key, absent);
    assert!(!peers[0].online);
}

#[tokio::test]
async fn a_port_blocked_at_first_still_binds() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    // Occupy the address b will project onto, so its first bind attempt
    // fails. b announces nothing new afterwards, so without a retry the port
    // would stay dark.
    let blocker = TcpListener::bind(SocketAddr::from((
        IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)),
        port,
    )))
    .await
    .unwrap();

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        drop(blocker);
    });

    let addr = bound_addr(&b, port).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"late").await.unwrap();
    let mut buf = [0u8; 4];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"late");
}

#[tokio::test]
async fn an_invitation_can_bring_its_own_grant() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;

    // One command on a: the invitation carries the grant.
    let invite = match a
        .request(Request::Invite {
            key: None,
            name: Some("b".into()),
            expose: vec![port],
        })
        .await
    {
        Response::Invite(i) => i,
        other => panic!("expected an invitation, got {:?}", other),
    };

    // One command on b, and the port is reachable. No expose step.
    b.request(Request::Accept {
        handle: invite,
        name: Some("a".into()),
    })
    .await;

    let addr = bound_addr(&b, port).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"carried").await.unwrap();
    let mut buf = [0u8; 7];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"carried");
}
