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
use crate::protocol::{Request, Response, ALPN, VERSION};
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
    test_endpoint_with(lookup, iroh::SecretKey::generate()).await
}

async fn test_endpoint_with(lookup: &MemoryLookup, key: iroh::SecretKey) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(key)
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
        let at = p.bound_at.iter().find(|b| b.port == port)?;
        at.at.parse().ok()
    })
    .await
}

// ---------------------------------------------------------------------------

/// Write `data: ping`, wait until the caller says so, then write `\n\n`.
/// Stays open. Returns the port and a oneshot that releases the terminator.
async fn sse_split_server() -> (u16, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release_rest, wait_rest) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.set_nodelay(true).ok();
        sock.write_all(b"data: ping").await.unwrap();
        sock.flush().await.ok();
        let _ = wait_rest.await;
        sock.write_all(b"\n\n").await.unwrap();
        sock.flush().await.ok();
        std::future::pending::<()>().await;
    });
    (port, release_rest)
}

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
async fn an_sse_fragment_arrives_before_the_rest_is_sent() {
    let (port, release_rest) = sse_split_server().await;
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
    sock.set_nodelay(true).ok();

    let mut first = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(2), sock.read_exact(&mut first))
        .await
        .expect("first SSE fragment did not arrive before the terminator was sent")
        .unwrap();
    assert_eq!(&first, b"data: ping");

    release_rest.send(()).unwrap();

    let mut rest = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(2), sock.read_exact(&mut rest))
        .await
        .expect("SSE terminator did not arrive")
        .unwrap();
    assert_eq!(&rest, b"\n\n");
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
async fn list_shows_our_version_and_the_peers() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    until(20, || async {
        let Response::List(a_list) = a.request(Request::List).await else {
            return None;
        };
        let Response::List(b_list) = b.request(Request::List).await else {
            return None;
        };
        let a_sees_b = a_list.peers.iter().find(|p| p.key == b.key())?;
        let b_sees_a = b_list.peers.iter().find(|p| p.key == a.key())?;
        (a_list.daemon == VERSION
            && b_list.daemon == VERSION
            && a_sees_b.version.as_deref() == Some(VERSION)
            && b_sees_a.version.as_deref() == Some(VERSION))
        .then_some(())
    })
    .await;
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

/// A device that already knows us takes a fresh invitation, say after its
/// grant was lost. Still connected, it gets the invitation's port.
#[tokio::test]
async fn a_known_peer_taking_a_fresh_invitation_gets_its_grant() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let b_key = b.key();
    until(20, || sees_online(&a, &b_key, "b")).await;

    let invite = match a
        .request(Request::Invite {
            key: None,
            name: None,
            expose: vec![port],
        })
        .await
    {
        Response::Invite(i) => i,
        other => panic!("expected an invitation, got {:?}", other),
    };
    match b
        .request(Request::Accept {
            handle: invite,
            name: None,
        })
        .await
    {
        Response::Ok => {}
        other => panic!("accept failed: {:?}", other),
    }

    let addr = bound_addr(&b, port).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"again").await.unwrap();
    let mut buf = [0u8; 5];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"again");
    // It is still the peer it was.
    until(5, || sees_online(&a, &b_key, "b")).await;
}

#[tokio::test]
async fn an_unnamed_peer_still_gets_a_name() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;

    // Neither side passes --as.
    let invite = match a
        .request(Request::Invite {
            key: None,
            name: None,
            expose: vec![port],
        })
        .await
    {
        Response::Invite(i) => i,
        other => panic!("expected an invitation, got {:?}", other),
    };
    b.request(Request::Accept {
        handle: invite,
        name: None,
    })
    .await;

    bound_addr(&b, port).await;

    let peers = b.daemon.peers().list().await;
    let seen = peers.iter().find(|p| p.key == a.key()).unwrap();
    assert!(seen.name.is_none(), "nothing named it");

    // The surface is still reachable by name: a short form of the key.
    let short: String = a.key().chars().take(8).collect();
    assert!(b.daemon.peers().resolve_name(&short).await.is_some());
}

/// `rename` gives a known peer a new name: `list` and the resolver go by it.
#[tokio::test]
async fn a_peer_can_be_renamed() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let a_key = a.key();
    until(20, || sees_online(&b, &a_key, "a")).await;

    match b
        .request(Request::Rename {
            peer: "a".into(),
            name: "laptop".into(),
        })
        .await
    {
        Response::Ok => {}
        other => panic!("rename failed: {:?}", other),
    }
    until(5, || sees_online(&b, &a_key, "laptop")).await;
    assert!(b.daemon.peers().resolve_name("a").await.is_none());
}

/// `project --as` renames the peer, not only its surface: `list`, `--to` and
/// the resolver all answer to the new name, and the old one is gone.
#[tokio::test]
async fn project_as_renames_the_peer() {
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;
    bound_addr(&b, port).await;

    match b
        .request(Request::Project {
            peer: "a".into(),
            ip: None,
            name: Some("dev".into()),
        })
        .await
    {
        Response::Ok => {}
        other => panic!("project failed: {:?}", other),
    }

    let peers = b.daemon.peers().list().await;
    let seen = peers.iter().find(|p| p.key == a.key()).unwrap();
    assert_eq!(seen.name.as_deref(), Some("dev"));
    assert!(b.daemon.peers().resolve_name("dev").await.is_some());
    assert!(b.daemon.peers().resolve_name("a").await.is_none());

    match b
        .request(Request::Expose {
            port,
            to: vec!["dev".into()],
            all: false,
        })
        .await
    {
        Response::Ok => {}
        other => panic!("expose --to the new name failed: {:?}", other),
    }
}

#[tokio::test]
async fn at_localhost_a_taken_port_listens_elsewhere() {
    // The echo server holds 127.0.0.1:port, so b, binding every peer at
    // 127.0.0.1 as on iOS, cannot have that port and takes another.
    let port = echo_server().await;
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    b.daemon.peers().bind_at_localhost();
    enroll(&a, &b, "b").await;

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;

    let addr = bound_addr(&b, port).await;
    assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(addr.port(), port);

    let peers = b.daemon.peers().list().await;
    assert_eq!(peers[0].ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(peers[0].bound, vec![port]);

    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"elsewhere").await.unwrap();
    let mut buf = [0u8; 9];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"elsewhere");
}

#[tokio::test]
async fn at_localhost_a_free_port_keeps_its_number() {
    // a forwards to 127.0.0.2, so 127.0.0.1:port is free on b's side. Nothing
    // answers on 127.0.0.2; this only checks where b listens.
    let port = {
        let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        probe.local_addr().unwrap().port()
    };
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))).await;
    b.daemon.peers().bind_at_localhost();
    enroll(&a, &b, "b").await;

    a.request(Request::Expose {
        port,
        to: vec![b.key()],
        all: false,
    })
    .await;

    let addr = bound_addr(&b, port).await;
    assert_eq!(addr, SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
}

/// Whether `d` lists the peer `key` as online, under `name`.
async fn sees_online(d: &TestDaemon, key: &str, name: &str) -> Option<()> {
    let Response::List(list) = d.request(Request::List).await else {
        return None;
    };
    let p = list.peers.iter().find(|p| p.key == key)?;
    (p.online && p.name.as_deref() == Some(name)).then_some(())
}

/// Restart `d`: same key, same state directory, a fresh endpoint that can
/// find `peers`. The old daemon's peers and listeners stop first, as they do
/// when the app's runtime goes away.
async fn restart(d: TestDaemon, peers: &[&TestDaemon], at_localhost: bool) -> TestDaemon {
    let TestDaemon {
        daemon,
        _router,
        _dir,
    } = d;
    let secret = daemon.endpoint().secret_key().clone();
    daemon.peers().stop().await;
    _router.shutdown().await.ok();
    daemon.endpoint().close().await;
    drop(daemon);

    let lookup = MemoryLookup::new();
    for peer in peers {
        lookup.add_endpoint_info(addr_of(peer.daemon.endpoint()));
    }
    let endpoint = test_endpoint_with(&lookup, secret).await;
    let host = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let daemon = if at_localhost {
        Daemon::with_endpoint_at_localhost(endpoint, host, &_dir.prefix()).await
    } else {
        Daemon::with_endpoint(endpoint, host, &_dir.prefix(), None, None).await
    }
    .unwrap();
    let router = daemon.spawn_router();
    TestDaemon {
        daemon,
        _router: router,
        _dir,
    }
}

/// An `expose` outlives a restart: the grant is saved next to the key, and
/// the restarted daemon offers the port to its grantee again. `b` grants and
/// restarts, since it's the side that calls the other back.
#[tokio::test]
async fn a_grant_survives_a_restart() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    until(20, || sees_online(&b, &a_key, "a")).await;
    b.request(Request::Expose {
        port: 4321,
        to: vec![a_key.clone()],
        all: false,
    })
    .await;

    let b = restart(b, &[&a], false).await;

    let Response::List(list) = b.request(Request::List).await else {
        panic!("list failed");
    };
    assert!(
        list.grants.iter().any(|g| g.port == 4321 && g.to == a_key),
        "the grant was lost: {:?}",
        list.grants
    );
    until(20, || async {
        let Response::List(list) = a.request(Request::List).await else {
            return None;
        };
        let p = list.peers.iter().find(|p| p.key == b_key)?;
        p.they_expose.contains(&4321).then_some(())
    })
    .await;
}

/// An iPhone app restarts on every update. The side that took up the
/// invitation must still know the peer afterwards, and call it again.
#[tokio::test]
async fn an_accepted_peer_survives_a_restart() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    until(20, || sees_online(&b, &a_key, "a")).await;

    let b = restart(b, &[&a], false).await;

    until(20, || sees_online(&b, &a_key, "a")).await;
    until(20, || sees_online(&a, &b_key, "b")).await;
}

/// Close the link and wait until a write succeeds again.
#[tokio::test]
async fn a_severed_link_comes_back() {
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
    sock.write_all(b"before").await.unwrap();
    let mut buf = [0u8; 6];
    sock.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"before");
    drop(sock);

    let b_key = b.daemon.endpoint().id();
    a.daemon.peers().sever(&b_key).await;

    // The listener stays up while the link is down, so the write is the test.
    until(20, || async {
        let addr = bound_addr(&b, port).await;
        let mut sock = TcpStream::connect(addr).await.ok()?;
        sock.write_all(b"after").await.ok()?;
        let mut buf = [0u8; 5];
        sock.read_exact(&mut buf).await.ok()?;
        (&buf == b"after").then_some(())
    })
    .await;
}

/// Where `b` has bound `peer`'s `port`, once it has.
async fn bound_addr_of(b: &TestDaemon, peer: &str, port: u16) -> SocketAddr {
    until(20, || async {
        let peers = b.daemon.peers().list().await;
        let p = peers.iter().find(|p| p.key == peer)?;
        let at = p.bound_at.iter().find(|b| b.port == port)?;
        at.at.parse().ok()
    })
    .await
}

/// At 127.0.0.1 a page's origin is its local port number, and its storage
/// goes with it. Two peers grant the same port to b, which binds every peer at
/// 127.0.0.1 as on iOS. Each gets its own number, and each keeps that number
/// after b restarts, whichever peer comes back first.
#[tokio::test]
async fn at_localhost_a_port_keeps_its_number_across_a_restart() {
    // a and c forward to 127.0.0.2, so 127.0.0.1:port is free on b's side.
    // Nothing answers there; this only checks where b listens.
    let port = {
        let probe = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        probe.local_addr().unwrap().port()
    };
    let host = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
    let (lookup_a, lookup_b, lookup_c) = (
        MemoryLookup::new(),
        MemoryLookup::new(),
        MemoryLookup::new(),
    );
    let a = TestDaemon::start("a", &lookup_a, host).await;
    let c = TestDaemon::start("c", &lookup_c, host).await;
    let b = TestDaemon::start("b", &lookup_b, host).await;
    b.daemon.peers().bind_at_localhost();
    for (x, lookup_x) in [(&a, &lookup_a), (&c, &lookup_c)] {
        lookup_x.add_endpoint_info(addr_of(b.daemon.endpoint()));
        lookup_b.add_endpoint_info(addr_of(x.daemon.endpoint()));
    }
    enroll(&a, &b, "b").await;
    enroll(&c, &b, "b").await;

    for x in [&a, &c] {
        x.request(Request::Expose {
            port,
            to: vec![b.key()],
            all: false,
        })
        .await;
    }
    let at_a = bound_addr_of(&b, &a.key(), port).await;
    let at_c = bound_addr_of(&b, &c.key(), port).await;
    assert_ne!(at_a, at_c);
    assert!([at_a.port(), at_c.port()].contains(&port));

    let b = restart(b, &[&a, &c], true).await;

    assert_eq!(bound_addr_of(&b, &a.key(), port).await, at_a);
    assert_eq!(bound_addr_of(&b, &c.key(), port).await, at_c);

    // Forgetting a drops its number; c keeps its own.
    b.request(Request::Forget { peer: a.key() }).await;
    let table = std::fs::read_to_string(b._dir.0.join("key.ports.json")).unwrap();
    assert!(!table.contains(&a.key()));
    assert!(table.contains(&c.key()));
}

/// Whether `d` lists the peer `key` as online and awake.
async fn sees_awake(d: &TestDaemon, key: &str) -> Option<()> {
    let Response::List(list) = d.request(Request::List).await else {
        return None;
    };
    let p = list.peers.iter().find(|p| p.key == key)?;
    (p.online && !p.asleep).then_some(())
}

/// A message this daemon cannot parse, as from a newer one, is dropped. The
/// link stays up and the next message still lands.
#[tokio::test]
async fn a_message_from_a_newer_peer_is_dropped() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    a.daemon.peers().bind_at_localhost();
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    until(20, || sees_awake(&b, &a_key)).await;
    until(20, || sees_awake(&a, &b_key)).await;

    let a_id = a.daemon.endpoint().id();
    b.daemon
        .peers()
        .send_raw(&a_id, br#"{"Nap":{"minutes":5}}"#)
        .await
        .unwrap();
    b.daemon
        .peers()
        .send_raw(&a_id, br#"{"ExposedPorts":[4321]}"#)
        .await
        .unwrap();

    until(20, || async {
        let peers = a.daemon.peers().list().await;
        let p = peers.iter().find(|p| p.key == b_key)?;
        (p.online && p.they_expose == vec![4321]).then_some(())
    })
    .await;
}

/// Whether `d` lists the peer `key` as asleep and offline, and is not
/// trying to reach it.
async fn sees_asleep(d: &TestDaemon, key: &str) -> Option<()> {
    let Response::List(list) = d.request(Request::List).await else {
        return None;
    };
    let p = list.peers.iter().find(|p| p.key == key)?;
    (!p.online && p.asleep && !p.dialing).then_some(())
}

/// The iPhone app says goodbye on going to the background. The Mac stops
/// dialing it, and the phone calls when it comes back.
#[tokio::test]
async fn a_peer_that_says_goodbye_is_not_dialed_until_it_calls() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    let (a_id, b_id) = (a.daemon.endpoint().id(), b.daemon.endpoint().id());
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;

    // The app has a few seconds in the background, so Sleep must not hang.
    let started = std::time::Instant::now();
    assert!(matches!(b.request(Request::Sleep).await, Response::Ok));
    assert!(started.elapsed() < Duration::from_secs(1));
    until(5, || sees_asleep(&a, &b_key)).await;

    // A dropped link is redialed after a second. Give it three.
    let (a_dialed, b_dialed) = (
        a.daemon.peers().dial_attempts(&b_id),
        b.daemon.peers().dial_attempts(&a_id),
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(a.daemon.peers().dial_attempts(&b_id), a_dialed);
    assert_eq!(b.daemon.peers().dial_attempts(&a_id), b_dialed);
    assert!(sees_asleep(&a, &b_key).await.is_some());

    let b = restart(b, &[&a], false).await;
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;
}

/// A peer that went to sleep calls back even when it was the inviter, since
/// the other end is waiting for it.
#[tokio::test]
async fn an_inviter_that_says_goodbye_calls_back() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    // b invites, a accepts: b would wait for a's call after a restart.
    enroll(&b, &a, "a").await;
    let (a_key, b_key) = (a.key(), b.key());
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;

    assert!(matches!(b.request(Request::Sleep).await, Response::Ok));
    until(5, || sees_asleep(&a, &b_key)).await;

    let b = restart(b, &[&a], false).await;
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;
}

/// Stopping or restarting a daemon says no goodbye. Its peers keep dialing
/// it, because a daemon that restarts waits for a call from every peer it
/// invited.
#[tokio::test]
async fn a_plain_stop_says_no_goodbye() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    let b_id = b.daemon.endpoint().id();
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;

    let a_dialed = a.daemon.peers().dial_attempts(&b_id);
    b.daemon.peers().stop().await;
    until(10, || async {
        (a.daemon.peers().dial_attempts(&b_id) > a_dialed).then_some(())
    })
    .await;
    let Response::List(list) = a.request(Request::List).await else {
        panic!("no list");
    };
    assert!(!list.peers.iter().find(|p| p.key == b_key).unwrap().asleep);

    let b = restart(b, &[&a], false).await;
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;
}

/// A peer that is gone and being redialed lists as dialing, so the phone can
/// say it is connecting rather than just offline.
#[tokio::test]
async fn a_peer_being_redialed_lists_as_dialing() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    enroll(&a, &b, "b").await;
    let (a_key, b_key) = (a.key(), b.key());
    until(20, || sees_awake(&a, &b_key)).await;
    until(20, || sees_awake(&b, &a_key)).await;
    let Response::List(list) = a.request(Request::List).await else {
        panic!("no list");
    };
    assert!(!list.peers.iter().find(|p| p.key == b_key).unwrap().dialing);

    let TestDaemon {
        daemon, _router, ..
    } = b;
    daemon.peers().stop().await;
    _router.shutdown().await.ok();
    daemon.endpoint().close().await;

    until(10, || async {
        let Response::List(list) = a.request(Request::List).await else {
            return None;
        };
        let p = list.peers.iter().find(|p| p.key == b_key)?;
        (!p.online && p.dialing).then_some(())
    })
    .await;
}

/// An invitation that expired before it was taken up leaves the peer dialing
/// with a code the inviter refuses. Taking up a fresh invitation from the same
/// machine replaces the code, instead of failing because the peer is known,
/// and the name given the second time replaces the first.
#[tokio::test]
async fn a_fresh_invitation_replaces_a_refused_one() {
    let (a, b) = pair(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    let (a_key, b_key) = (a.key(), b.key());

    // A code `a` never issued stands in for one that expired.
    let stale = format!("{}.{}", a_key, "00".repeat(16));
    match b
        .request(Request::Accept {
            handle: stale,
            name: Some("old".to_string()),
        })
        .await
    {
        Response::Ok => {}
        other => panic!("accept failed: {:?}", other),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(sees_online(&a, &b_key, "b").await.is_none());

    enroll(&a, &b, "b").await;
    until(20, || sees_online(&b, &a_key, "a")).await;
    until(20, || sees_online(&a, &b_key, "b")).await;
}
