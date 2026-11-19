//! Peer management - connections, port announcements, auto-binding, reconnection.
//!
//! Access is default deny: an incoming connection is served only if its key
//! is already known (added by ticket, or pinned at enrollment) or it presents
//! a valid enrollment token. Anyone else is refused -- no announcement, no
//! tunnel.

use crate::core::backoff::Backoff;
use crate::core::dial::{self, Keep, Reach};
use crate::core::session::{Action, Admission, ConnId, Refusal, Session};
use crate::enroll::Pins;
use crate::netstack::{Accept, NetStack};
use crate::ports::{self, PortRecord, PortStore};
use crate::protocol::{BoundAt, PeerInfo, PeerMessage, ALPN};
use crate::surface::{self, Surface, SurfaceStore};
use crate::tunnel::{self, PeerConnection};
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tracing::{error, info, warn};

/// How long to let a dial run before giving up and backing off
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an unknown incoming peer gets to present its enrollment token
const ENROLL_TIMEOUT: Duration = Duration::from_secs(10);
/// How many times to try binding a forwarded port before giving up
const BIND_ATTEMPTS: u32 = 5;
/// Delay between bind attempts
const BIND_RETRY_DELAY: Duration = Duration::from_millis(200);
/// How long `sleep` waits for peers to read its goodbye. They are told in
/// parallel, so this bounds the whole request.
const GOODBYE_WAIT: Duration = Duration::from_millis(800);

/// Info about a connected peer
/// Marks a peer as being dialed for as long as it lives.
struct Dialing<'a>(&'a AtomicBool);

impl<'a> Dialing<'a> {
    fn start(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self(flag)
    }
}

impl Drop for Dialing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

struct Peer {
    endpoint_id: EndpointId,
    /// What we call this peer, from `--as` (None until something names it).
    /// `project --as` replaces it, so it sits behind a lock.
    label: std::sync::RwLock<Option<String>>,
    /// Whether we dial this peer or wait for its call: we wait while we are
    /// the inviter and it has not called home yet, or after it said goodbye.
    reach: std::sync::Mutex<Reach>,
    /// Dials started, for the tests to watch
    dial_attempts: AtomicU64,
    /// Its connection loop is trying to reach it: waiting out the backoff,
    /// or dialing.
    dialing: AtomicBool,
    /// An invitation's one-time code, presented on connect by the accepter.
    /// A fresh invitation from the same peer replaces it.
    enroll_token: std::sync::Mutex<Option<String>>,
    /// Wakes the connection loop to dial now, when a new code arrives.
    redial: Notify,
    connection: RwLock<Option<Connection>>,
    /// Who opened the connection above. Written under its write lock, so the
    /// two move together.
    conn_dialed_by_us: AtomicBool,
    /// Ports this peer exposes
    exposed_ports: RwLock<Vec<u16>>,
    /// Crate version the peer last announced. None until it sends one.
    version: RwLock<Option<String>>,
    /// Where this peer's ports are bound locally. None until projected: an
    /// unprojected peer's ports are known but have no listener.
    surface: RwLock<Option<Surface>>,
    /// Active bindings, by the peer's port
    bindings: DashMap<u16, Binding>,
    /// Notified when a new connection replaces the current one
    conn_notify: Notify,
    /// Set when peer is removed; signals connection loop to exit
    removed: AtomicBool,
}

/// A listener for one of a peer's ports.
struct Binding {
    task: tokio::task::JoinHandle<()>,
    /// Where it listens. The port differs from the peer's when this daemon
    /// binds every peer at 127.0.0.1 and the peer's port number was taken.
    at: SocketAddr,
}

impl Peer {
    fn label(&self) -> Option<String> {
        self.label.read().unwrap().clone()
    }

    fn new(
        endpoint_id: EndpointId,
        label: Option<String>,
        dial: bool,
        enroll_token: Option<String>,
        connection: Option<Connection>,
    ) -> Arc<Self> {
        Arc::new(Self {
            endpoint_id,
            label: std::sync::RwLock::new(label),
            reach: std::sync::Mutex::new(if dial {
                Reach::dialed()
            } else {
                Reach::invited()
            }),
            dial_attempts: AtomicU64::new(0),
            dialing: AtomicBool::new(false),
            enroll_token: std::sync::Mutex::new(enroll_token),
            redial: Notify::new(),
            connection: RwLock::new(connection),
            conn_dialed_by_us: AtomicBool::new(false),
            exposed_ports: RwLock::new(Vec::new()),
            version: RwLock::new(None),
            surface: RwLock::new(None),
            bindings: DashMap::new(),
            conn_notify: Notify::new(),
            removed: AtomicBool::new(false),
        })
    }
}

pub struct PeerManager {
    /// Peers by endpoint ID
    peers: DashMap<EndpointId, Arc<Peer>>,
    /// Our endpoint (for outbound reconnection)
    endpoint: Endpoint,
    /// Admission and access decisions. Pure; see src/core/session.rs.
    session: Arc<std::sync::Mutex<Session>>,
    /// Host address for forwarding tunnel requests
    host: IpAddr,
    /// Ids handed to the core for connections it has not admitted yet
    next_conn_id: AtomicU64,
    /// Peers pinned at enrollment, persisted across restarts
    pins: Pins,
    /// `expose` grants, persisted across restarts
    grants: crate::grant_store::GrantStore,
    /// Projected surfaces, persisted across restarts
    surfaces: SurfaceStore,
    /// The local number each peer port got at 127.0.0.1, persisted across
    /// restarts. Used only when every peer binds there.
    ports: PortStore,
    /// TUN owned-network backend. When present, surfaces bind on the TUN via a
    /// userspace stack instead of loopback (ADR 0005); None = loopback backend.
    netstack: Option<NetStack>,
    /// This node's own name. The owned resolver answers `<self_name>.pai-sho`
    /// with 127.0.0.1, so local traffic uses the same origin peers do.
    self_name: Option<String>,
    /// Serializes binding creation/teardown and surface changes so a port's
    /// listener state and the address it binds move together
    bind_lock: Mutex<()>,
    /// Bind every peer at 127.0.0.1 instead of a surface address of its own.
    /// Each of a peer's ports then listens on the number it had before, else
    /// its own number if free, else one the OS picks (see src/ports.rs). True
    /// on iOS, where an app can add no address and can bind only 127.0.0.1.
    localhost_only: AtomicBool,
    /// Set by `sleep`: we said goodbye and dial no one until restarted
    sleeping: AtomicBool,
}

impl PeerManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: Endpoint,
        host: IpAddr,
        session: Arc<std::sync::Mutex<Session>>,
        pins: Pins,
        grants: crate::grant_store::GrantStore,
        surfaces: SurfaceStore,
        ports: PortStore,
        netstack: Option<NetStack>,
        self_name: Option<String>,
        localhost_only: bool,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            endpoint,
            host,
            session,
            next_conn_id: AtomicU64::new(1),
            pins,
            grants,
            surfaces,
            ports,
            netstack,
            self_name,
            bind_lock: Mutex::new(()),
            localhost_only: AtomicBool::new(localhost_only),
            sleeping: AtomicBool::new(false),
        }
    }

    /// Bind every peer at 127.0.0.1, as on iOS. Lets the tests on other
    /// systems take that path.
    #[cfg(test)]
    pub fn bind_at_localhost(&self) {
        self.localhost_only.store(true, Ordering::SeqCst);
    }

    fn is_localhost_only(&self) -> bool {
        self.localhost_only.load(Ordering::SeqCst)
    }

    /// An address for a new surface. Allocated from the backend's range, or
    /// 127.0.0.1 when every peer shares it.
    async fn new_surface_ip(&self) -> Result<IpAddr> {
        if self.is_localhost_only() {
            return Ok(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        }
        let taken = self.projected_ips().await;
        surface::allocate(&taken, self.surface_base())
    }

    /// Call a peer by a new name, here and in its pin, so `list`, `--to` and
    /// a restart all go by it. Its surface keeps its old name until projected.
    async fn rename(self: &Arc<Self>, peer: &Arc<Peer>, name: String) {
        *peer.label.write().unwrap() = Some(name.clone());
        let actions = self.session.lock().unwrap().rename(&peer.endpoint_id, name);
        self.apply(peer, actions).await;
    }

    /// Call a peer, by key or its current name, by a new name. A projected
    /// peer projects again at the same address, so its surface answers to the
    /// new name too.
    pub async fn rename_peer(self: &Arc<Self>, peer_ref: &str, name: String) -> Result<()> {
        let id = self
            .resolve_peer(peer_ref)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?;
        let peer = self
            .peers
            .get(&id)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?
            .clone();
        let surface = peer.surface.read().await.clone();
        match surface {
            Some(s) => self.project(&id.to_string(), Some(s.ip), Some(name)).await,
            None => {
                self.rename(&peer, name).await;
                Ok(())
            }
        }
    }

    /// Take up an invitation, or reach a peer known by key. We dial this peer
    /// and keep dialing it. A code, when there is one, is presented on connect
    /// and on every reconnect. A peer that already knows us still takes a live
    /// code's ports, and ignores a used one.
    pub async fn add_peer(
        self: &Arc<Self>,
        endpoint_id: EndpointId,
        name: Option<String>,
        code: Option<String>,
    ) -> Result<()> {
        if let Some(peer) = self.peers.get(&endpoint_id).map(|p| p.clone()) {
            // Taken up again, say because the first invitation expired before
            // it reached the peer, which then refuses it on every dial. Present
            // the new code from now on, and dial now rather than after the
            // backoff. Already connected, present it now too: the peer grants
            // a live code's ports even to a key it knows.
            if let Some(code) = code {
                *peer.enroll_token.lock().unwrap() = Some(code.clone());
                peer.redial.notify_one();
                let live = peer
                    .connection
                    .read()
                    .await
                    .clone()
                    .filter(|c| c.close_reason().is_none());
                if let Some(conn) = live {
                    if let Err(e) = Self::send_message(&conn, &PeerMessage::enroll(code)).await {
                        warn!("failed to send enroll token: {}", e);
                    }
                }
            }
            // The name given this time wins.
            if let Some(name) = name.filter(|n| peer.label().as_ref() != Some(n)) {
                self.rename_peer(&endpoint_id.to_string(), name).await?;
            }
            return Ok(());
        }
        // Pin it first, so a restart dials it again rather than forgetting it.
        self.pins
            .add_dialed(&endpoint_id.to_string(), name.as_deref())?;
        self.start_dialing(endpoint_id, name, code);
        Ok(())
    }

    /// A peer whose invitation we took up before a restart: dial it again.
    pub fn add_dialed_pin(self: &Arc<Self>, key: &str, name: Option<&str>) -> Result<()> {
        let endpoint_id: EndpointId = key.parse().context("invalid key")?;
        if self.peers.contains_key(&endpoint_id) {
            return Ok(());
        }
        self.start_dialing(endpoint_id, name.map(|n| n.to_string()), None);
        info!("dialing pinned peer {}", endpoint_id);
        Ok(())
    }

    fn start_dialing(
        self: &Arc<Self>,
        endpoint_id: EndpointId,
        name: Option<String>,
        code: Option<String>,
    ) {
        self.session
            .lock()
            .unwrap()
            .admit_known(endpoint_id, name.clone(), Admission::Added);

        // Record the peer before dialing. A peer whose daemon is not up yet is
        // still ours: its connection loop retries with backoff.
        let peer = Peer::new(endpoint_id, name, true, code, None);
        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);
    }

    /// A peer loaded from a pin. Wait for them to call. After first contact
    /// we dial them too.
    pub fn add_pinned(self: &Arc<Self>, key: &str, name: Option<&str>) -> Result<()> {
        let endpoint_id: EndpointId = key.parse().context("invalid key")?;
        if self.peers.contains_key(&endpoint_id) {
            return Ok(());
        }
        let name = name.map(|n| n.to_string());
        self.session
            .lock()
            .unwrap()
            .admit_known(endpoint_id, name.clone(), Admission::Key);
        let peer = Peer::new(endpoint_id, name, false, None, None);
        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);
        info!("invited peer {}", endpoint_id);
        Ok(())
    }

    /// Pin a peer by key under a label without a token (host-attested
    /// enrollment): register it live so it is authorized when it phones
    /// home, and persist the pin across restarts. Idempotent.
    pub fn pin_peer(self: &Arc<Self>, key: &str, name: Option<&str>) -> Result<()> {
        self.add_pinned(key, name)?;
        self.pins.add(key, name)
    }

    /// Send a control message on a new uni stream
    async fn send_message(conn: &Connection, msg: &PeerMessage) -> Result<()> {
        let data = serde_json::to_vec(msg)?;
        let mut send = conn.open_uni().await.context("failed to open stream")?;
        send.write_all(&data).await?;
        send.finish()?;
        Ok(())
    }

    /// Whether `peer`'s loop should dial it now
    fn dials(&self, peer: &Peer) -> bool {
        let we_sleep = self.sleeping.load(Ordering::SeqCst);
        peer.reach.lock().unwrap().dials(we_sleep)
    }

    /// Spawn the connection management loop for a peer
    fn spawn_connection_loop(self: &Arc<Self>, peer: Arc<Peer>) {
        let manager = self.clone();
        tokio::spawn(async move {
            Self::peer_connection_loop(manager, peer).await;
        });
    }

    /// Long-running task managing a peer's connection lifecycle.
    /// Runs the unified connection handler and reconnects with backoff on failure.
    async fn peer_connection_loop(manager: Arc<PeerManager>, peer: Arc<Peer>) {
        let mut backoff = if peer.connection.read().await.is_some() {
            Backoff::connected()
        } else {
            Backoff::unreached()
        };

        loop {
            let has_conn = peer.connection.read().await.is_some();
            if has_conn {
                if let Err(e) = Self::run_connection(&manager, &peer).await {
                    if peer.removed.load(Ordering::Relaxed) {
                        return;
                    }
                    warn!("{} disconnected: {}", peer.endpoint_id, e);
                }
            }

            if peer.removed.load(Ordering::Relaxed) {
                return;
            }

            if !manager.dials(&peer) {
                // Inviter, not yet reached, or they said goodbye. They call us.
                if !Self::wait_for_call(&peer).await {
                    return;
                }
                continue;
            }

            // Reconnect with exponential backoff
            let _dialing = Dialing::start(&peer.dialing);
            loop {
                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }

                info!(
                    "reconnecting to {} in {:?}",
                    peer.endpoint_id,
                    backoff.delay()
                );

                // Wait for backoff, but wake early if an incoming connection arrives
                tokio::select! {
                    _ = tokio::time::sleep(backoff.delay()) => {}
                    _ = peer.redial.notified() => {
                        info!("{} has a new invitation code, dialing now", peer.endpoint_id);
                        backoff.reset();
                    }
                    _ = peer.conn_notify.notified() => {
                        info!("{} reconnected via incoming connection", peer.endpoint_id);
                        backoff.reset();
                        break;
                    }
                }

                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }

                // A goodbye can land while we wait. Then they call us.
                if !manager.dials(&peer) {
                    if !Self::wait_for_call(&peer).await {
                        return;
                    }
                    backoff.reset();
                    break;
                }

                peer.dial_attempts.fetch_add(1, Ordering::Relaxed);
                // iroh's own dial timeout only applies once it has an address
                // to try, so bound the call ourselves rather than let the loop
                // stall on it.
                let dial = tokio::time::timeout(
                    DIAL_TIMEOUT,
                    manager.endpoint.connect(peer.endpoint_id, ALPN),
                )
                .await;

                let conn = match dial {
                    Ok(Ok(conn)) => conn,
                    Ok(Err(e)) => {
                        warn!("reconnect to {} failed: {}", peer.endpoint_id, e);
                        backoff.failed();
                        continue;
                    }
                    Err(_) => {
                        warn!(
                            "reconnect to {} timed out after {:?}",
                            peer.endpoint_id, DIAL_TIMEOUT
                        );
                        backoff.failed();
                        continue;
                    }
                };

                info!("reconnected to {}", peer.endpoint_id);
                {
                    let mut guard = peer.connection.write().await;
                    let held_is_live = guard
                        .as_ref()
                        .is_some_and(|held| held.close_reason().is_none());
                    let held_dialed_by_us = peer.conn_dialed_by_us.load(Ordering::Relaxed);
                    if dial::resolve(
                        &manager.endpoint.id(),
                        &peer.endpoint_id,
                        held_is_live,
                        held_dialed_by_us,
                        true,
                    ) == Keep::Held
                    {
                        conn.close(0u32.into(), b"kept ours");
                        drop(guard);
                        backoff.reset();
                        break;
                    }
                    if let Some(old_conn) = guard.take() {
                        old_conn.close(0u32.into(), b"replaced");
                    }
                    peer.conn_dialed_by_us.store(true, Ordering::Relaxed);
                    *guard = Some(conn.clone());
                }
                // Re-present the enroll token in case the peer never processed
                // it. Once used, the peer ignores it.
                let token = peer.enroll_token.lock().unwrap().clone();
                if let Some(token) = token {
                    let msg = PeerMessage::enroll(token);
                    if let Err(e) = Self::send_message(&conn, &msg).await {
                        warn!("failed to send enroll token: {}", e);
                    }
                }
                manager.announce_to(&peer).await;
                backoff.reset();
                break;
            }
        }
    }

    /// Wait for the peer to connect to us. A notification left from an earlier
    /// call, whose connection has since closed, is passed over. False once
    /// the peer is removed.
    async fn wait_for_call(peer: &Peer) -> bool {
        loop {
            peer.conn_notify.notified().await;
            if peer.removed.load(Ordering::Relaxed) {
                return false;
            }
            let live = peer
                .connection
                .read()
                .await
                .as_ref()
                .is_some_and(|c| c.close_reason().is_none());
            if live {
                info!("{} reconnected via incoming connection", peer.endpoint_id);
                return true;
            }
        }
    }

    /// Unified connection handler: accepts both uni streams (control messages)
    /// and bi streams (tunnel requests) on the current connection.
    async fn run_connection(manager: &Arc<PeerManager>, peer: &Arc<Peer>) -> Result<()> {
        let conn = {
            let guard = peer.connection.read().await;
            guard.clone().ok_or_else(|| anyhow!("disconnected"))?
        };

        // The dialer already announced version on its Enroll. Sending another
        // Enroll with an empty token would race that claim on the other side.
        if peer.enroll_token.lock().unwrap().is_none() {
            Self::send_version(peer).await;
        }

        loop {
            tokio::select! {
                result = conn.accept_uni() => {
                    let recv = result?;
                    let manager = manager.clone();
                    let peer = peer.clone();
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        Self::handle_uni_stream(&manager, &conn, recv, &peer).await;
                    });
                }
                result = conn.accept_bi() => {
                    let (send, recv) = result?;
                    let manager = manager.clone();
                    let peer = peer.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_bi_stream(&manager, &peer, send, recv).await {
                            error!("tunnel error: {}", e);
                        }
                    });
                }
            }
        }
    }

    async fn handle_uni_stream(
        manager: &Arc<PeerManager>,
        conn: &Connection,
        mut recv: iroh::endpoint::RecvStream,
        peer: &Arc<Peer>,
    ) {
        let data = match recv.read_to_end(64 * 1024).await {
            Ok(data) => data,
            Err(e) => {
                warn!("uni stream read error: {}", e);
                return;
            }
        };
        let msg: PeerMessage = match serde_json::from_slice(&data) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("uni stream parse error: {}", e);
                return;
            }
        };
        match msg {
            PeerMessage::ExposedPorts(ports) => {
                info!("{} exposed ports: {:?}", peer.endpoint_id, ports);
                manager.update_peer_ports(peer, ports).await;
            }
            PeerMessage::Connect { port: _ } => {
                warn!("unexpected Connect message on control stream");
            }
            PeerMessage::Enroll { token, version } => {
                // Peer is already known. It sends Enroll again to report its
                // version, and to present a code from a fresh invitation: a
                // live one still grants its ports.
                if let Some(version) = version {
                    info!("{} reports version {}", peer.endpoint_id, version);
                    *peer.version.write().await = Some(version);
                }
                if !token.is_empty() {
                    let actions = manager
                        .session
                        .lock()
                        .unwrap()
                        .on_admitted_enroll(peer.endpoint_id, &token);
                    if !actions.is_empty() {
                        info!("{} took up a fresh invitation", peer.endpoint_id);
                    }
                    manager.apply(peer, actions).await;
                }
            }
            PeerMessage::Error(e) => {
                error!("peer error: {}", e);
            }
            PeerMessage::Goodbye => {
                // Mark it asleep before closing, so the connection loop sees
                // the close and waits instead of redialing. The sender waits
                // for this close before it closes its end.
                info!("{} said goodbye; waiting for it to call", peer.endpoint_id);
                peer.reach.lock().unwrap().said_goodbye();
                conn.close(0u32.into(), b"goodbye");
            }
        }
    }

    async fn handle_bi_stream(
        manager: &Arc<PeerManager>,
        peer: &Arc<Peer>,
        send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
    ) -> Result<()> {
        let mut buf = [0u8; 2];
        recv.read_exact(&mut buf).await?;
        let port = u16::from_be_bytes(buf);

        let verdict = manager
            .session
            .lock()
            .unwrap()
            .on_tunnel(&peer.endpoint_id, port);

        match verdict {
            Action::ServeTunnel { port } => {
                info!("tunnel request for port {}", port);
                tunnel::handle_tunnel(manager.host, port, send, recv).await
            }
            Action::RejectTunnel { reason } => {
                warn!(
                    "refused tunnel to port {} from {}: {}",
                    port,
                    peer.endpoint_id,
                    reason.as_str()
                );
                Ok(())
            }
            other => Err(anyhow!("unexpected tunnel verdict: {:?}", other)),
        }
    }

    /// Update a peer's announced ports and reconcile its bindings.
    ///
    /// Bindings exist only while the peer is projected: with a surface, each
    /// announced port binds under the surface's IP; without one, the ports are
    /// recorded but nothing is bound. A unique IP per surface means no two
    /// peers ever contend for an address, so there is no collision to arbitrate.
    async fn update_peer_ports(self: &Arc<Self>, peer: &Arc<Peer>, new_ports: Vec<u16>) {
        let _guard = self.bind_lock.lock().await;

        // Stop bindings for ports no longer announced
        let old_ports = peer.exposed_ports.read().await.clone();
        for port in &old_ports {
            if !new_ports.contains(port) {
                self.release_binding(peer, *port).await;
            }
        }

        // Bind newly announced ports. A peer with no surface is auto-projected
        // on its first announced port: allocate an address, name it after the
        // peer's label, and bind there. This is the default path, so reach is
        // automatic again; explicit `project` only overrides the address/name.
        let ip = match self.surface_ip(peer, &new_ports).await {
            Some(ip) => ip,
            None => {
                *peer.exposed_ports.write().await = new_ports;
                return;
            }
        };
        for &port in &new_ports {
            if peer.bindings.contains_key(&port) {
                continue;
            }
            self.bind_one(peer, ip, port).await;
        }

        *peer.exposed_ports.write().await = new_ports;
    }

    /// The address to bind this peer's ports at: its existing surface, or a
    /// freshly auto-projected one when it has ports to bind and none yet.
    /// Returns None if there is nothing to bind. Caller holds the bind lock.
    async fn surface_ip(self: &Arc<Self>, peer: &Arc<Peer>, ports: &[u16]) -> Option<IpAddr> {
        if let Some(surface) = peer.surface.read().await.as_ref() {
            return Some(surface.ip);
        }
        if ports.is_empty() {
            return None;
        }

        let ip = match self.new_surface_ip().await {
            Ok(ip) => ip,
            Err(e) => {
                error!("cannot auto-project {}: {}", peer.endpoint_id, e);
                return None;
            }
        };
        // Every surface answers by name. Nothing named this peer, so it gets
        // a short form of its key until `project --as` renames it.
        let name = Some(
            peer.label()
                .unwrap_or_else(|| crate::core::session::default_name(&peer.endpoint_id)),
        );
        if let Err(e) = self.claim_addr(ip, name.clone()) {
            error!("cannot claim {} for {}: {}", ip, peer.endpoint_id, e);
            return None;
        }

        *peer.surface.write().await = Some(Surface {
            ip,
            name: name.clone(),
        });
        if let Err(e) = self.surfaces.add(&peer.endpoint_id.to_string(), ip, name) {
            warn!("failed to persist surface for {}: {}", peer.endpoint_id, e);
        }
        info!("auto-projected {} to {}", peer.endpoint_id, ip);
        Some(ip)
    }

    /// Bind one forwarded port under `ip` and record the binding.
    ///
    /// A bind can fail transiently: the surface address was just claimed, or
    /// the previous listener on that port has not finished closing. Retry a
    /// few times before giving up, because there is no announce to fall back
    /// on. A peer whose port set does not change never re-announces, so a port
    /// dropped here would stay dark for the life of the connection.
    async fn bind_one(&self, peer: &Arc<Peer>, ip: IpAddr, port: u16) {
        // TUN backend: register a listener with the userspace stack. The
        // accept -> QUIC splice happens in run_netstack_accepts. A resident
        // placeholder task stands in for the binding so bindings tracking and
        // release work uniformly with the loopback path.
        if let Some(ns) = &self.netstack {
            if let IpAddr::V4(v4) = ip {
                ns.listen(v4, port);
                peer.bindings.insert(
                    port,
                    Binding {
                        task: tokio::spawn(std::future::pending::<()>()),
                        at: SocketAddr::from((v4, port)),
                    },
                );
                info!("bound {}:{} (tun)", v4, port);
                return;
            }
        }

        if self.is_localhost_only() {
            return self.bind_at_localhost_port(peer, ip, port).await;
        }

        let addr = SocketAddr::from((ip, port));
        let mut last_err = None;
        for attempt in 0..BIND_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(BIND_RETRY_DELAY).await;
            }
            match tunnel::bind_listener(addr).await {
                Ok(listener) => return Self::serve(peer, port, listener, addr),
                Err(e) => last_err = Some(e),
            }
        }
        error!(
            "failed to bind {} after {} attempts: {}",
            addr,
            BIND_ATTEMPTS,
            last_err.map(|e| e.to_string()).unwrap_or_default()
        );
    }

    /// Bind one of a peer's ports at 127.0.0.1, where every peer shares the
    /// address. The port listens on the number it had before, so a page keeps
    /// its origin and with it its storage. That number is retried like any
    /// other bind, since the last listener may still be closing, and only once
    /// it stays taken does the port move to a number the OS picks. A port
    /// binding for the first time tries its own number once, then moves.
    async fn bind_at_localhost_port(&self, peer: &Arc<Peer>, ip: IpAddr, port: u16) {
        let key = peer.endpoint_id.to_string();
        let records = self.ports.load().unwrap_or_else(|e| {
            warn!("failed to load ports: {}", e);
            Vec::new()
        });
        let stored = ports::stored(&records, &key, port);
        let wanted = ports::first_choice(&records, &key, port);

        let mut last_err = None;
        for attempt in 0..BIND_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(BIND_RETRY_DELAY).await;
            }
            if let Some(local) = wanted {
                match tunnel::bind_listener(SocketAddr::from((ip, local))).await {
                    Ok(listener) => return self.keep_local(peer, port, stored, listener),
                    Err(e) => last_err = Some(e),
                }
            }
            if stored.is_none() || attempt + 1 == BIND_ATTEMPTS {
                match Self::bind_unheld(ip, &records, &key, port).await {
                    Ok(listener) => return self.keep_local(peer, port, stored, listener),
                    Err(e) => last_err = Some(e),
                }
            }
        }
        error!(
            "failed to bind port {} of {} at {} after {} attempts: {}",
            port,
            peer.endpoint_id,
            ip,
            BIND_ATTEMPTS,
            last_err.map(|e| e.to_string()).unwrap_or_default()
        );
    }

    /// Serve a port bound at 127.0.0.1 and record its number when it is new.
    fn keep_local(
        &self,
        peer: &Arc<Peer>,
        port: u16,
        stored: Option<u16>,
        listener: tokio::net::TcpListener,
    ) {
        let at = match listener.local_addr() {
            Ok(at) => at,
            Err(e) => {
                error!(
                    "bound port {} of {} has no address: {}",
                    port, peer.endpoint_id, e
                );
                return;
            }
        };
        if stored != Some(at.port()) {
            if let Some(before) = stored {
                warn!(
                    "port {} of {} moved from {} to {}",
                    port,
                    peer.endpoint_id,
                    before,
                    at.port()
                );
            }
            let key = peer.endpoint_id.to_string();
            if let Err(e) = self.ports.set(&key, port, at.port()) {
                warn!("failed to persist port {} of {}: {}", port, key, e);
            }
        }
        Self::serve(peer, port, listener, at);
    }

    /// A listener at `ip` on a number the OS picks that no other peer port
    /// holds in the table. A number another port holds is passed over (and
    /// kept open until the end, so the OS does not hand it out again).
    async fn bind_unheld(
        ip: IpAddr,
        records: &[PortRecord],
        key: &str,
        port: u16,
    ) -> Result<tokio::net::TcpListener> {
        let mut passed_over = Vec::new();
        for _ in 0..16 {
            let listener = tunnel::bind_listener(SocketAddr::from((ip, 0))).await?;
            let local = listener.local_addr()?.port();
            if !ports::held_by_another(records, key, port, local) {
                return Ok(listener);
            }
            passed_over.push(listener);
        }
        Err(anyhow!(
            "every number the OS offered is held by another port"
        ))
    }

    /// Serve a bound listener for the peer's `port` and record the binding.
    fn serve(peer: &Arc<Peer>, port: u16, listener: tokio::net::TcpListener, at: SocketAddr) {
        let peer_clone = peer.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = tunnel::serve_listener(listener, port, &peer_clone).await {
                error!("serving {} failed: {}", at, e);
            }
        });
        peer.bindings.insert(port, Binding { task, at });
        info!("bound {} for port {}", at, port);
    }

    /// Release one binding, waiting for its task to finish so the listener is
    /// actually dropped before the port is reused. On the TUN backend, also
    /// tells the stack to stop listening (reads the peer's surface for the ip).
    async fn release_binding(&self, peer: &Arc<Peer>, port: u16) {
        if let Some((_, binding)) = peer.bindings.remove(&port) {
            binding.task.abort();
            let _ = binding.task.await;
            info!("removed binding for port {}", port);
        }
        if let Some(ns) = &self.netstack {
            if let Some(IpAddr::V4(v4)) = peer.surface.read().await.as_ref().map(|s| s.ip) {
                ns.unlisten(v4, port);
            }
        }
    }

    /// Claim a surface address: on the TUN backend, add it (and its name) to the
    /// userspace stack; on loopback, add the OS-level address.
    fn claim_addr(&self, ip: IpAddr, name: Option<String>) -> Result<()> {
        if let Some(ns) = &self.netstack {
            if let IpAddr::V4(v4) = ip {
                ns.add_surface(v4, name);
            }
            return Ok(());
        }
        surface::ensure_addr(ip)
    }

    /// Reverse of `claim_addr`.
    fn unclaim_addr(&self, ip: IpAddr) -> Result<()> {
        if let Some(ns) = &self.netstack {
            if let IpAddr::V4(v4) = ip {
                ns.remove_surface(v4);
            }
            return Ok(());
        }
        surface::remove_addr(ip)
    }

    /// The peer whose surface is bound at `ip` (for routing accepted TUN
    /// connections back to the owning peer).
    async fn peer_for_ip(&self, ip: IpAddr) -> Option<Arc<Peer>> {
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        for peer in peers {
            if peer.surface.read().await.as_ref().map(|s| s.ip) == Some(ip) {
                return Some(peer);
            }
        }
        None
    }

    /// Consume accepted TUN connections and splice each onto its peer's tunnel.
    pub async fn run_netstack_accepts(
        self: Arc<Self>,
        mut accepts: mpsc::UnboundedReceiver<Accept>,
    ) {
        if self.netstack.is_none() {
            return;
        }
        while let Some(acc) = accepts.recv().await {
            let ip = IpAddr::V4(acc.ip);
            let Some(peer) = self.peer_for_ip(ip).await else {
                warn!(
                    "accepted {}:{} but no peer owns that surface",
                    acc.ip, acc.port
                );
                continue;
            };
            tokio::spawn(async move {
                if let Err(e) = bridge(peer, acc).await {
                    warn!("tun bridge ended: {}", e);
                }
            });
        }
    }

    /// Fully evict a peer: close its connection, release every binding it
    /// holds (waiting for the listeners to drop), tear down its surface, clear
    /// grants naming it, and delete its pin. Callers hold the bind lock where
    /// it matters.
    async fn evict(&self, endpoint_id: &EndpointId, reason: &str) {
        if let Some((_, peer)) = self.peers.remove(endpoint_id) {
            peer.removed.store(true, Ordering::Relaxed);
            peer.conn_notify.notify_one();

            if let Some(conn) = peer.connection.write().await.take() {
                conn.close(0u32.into(), reason.as_bytes());
            }

            // release while the surface is still set so the tun backend can
            // unlisten by ip
            let ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            for port in ports {
                self.release_binding(&peer, port).await;
            }

            if let Some(surface) = peer.surface.write().await.take() {
                if let Err(e) = self.unclaim_addr(surface.ip) {
                    warn!("failed to remove address {}: {}", surface.ip, e);
                }
            }
        }

        let actions = self.session.lock().unwrap().evict(endpoint_id);
        for action in actions {
            match action {
                Action::DropPin { key } => {
                    if let Err(e) = self.pins.remove(&key.to_string()) {
                        warn!("failed to remove pin for {}: {}", key, e);
                    }
                }
                Action::PersistGrants => self.save_grants(),
                _ => {}
            }
        }
        if let Err(e) = self.surfaces.remove(&endpoint_id.to_string()) {
            warn!("failed to remove surface record for {}: {}", endpoint_id, e);
        }
        if let Err(e) = self.ports.remove_peer(&endpoint_id.to_string()) {
            warn!("failed to remove port records for {}: {}", endpoint_id, e);
        }

        info!("evicted peer {} ({})", endpoint_id, reason);
    }

    /// Forget a peer, named by key or by the name we gave it.
    pub async fn remove_peer(&self, peer_ref: &str) -> Result<()> {
        let endpoint_id = self
            .resolve_peer(peer_ref)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?;

        if !self.peers.contains_key(&endpoint_id) {
            return Err(anyhow!("peer not found"));
        }

        let _guard = self.bind_lock.lock().await;
        self.evict(&endpoint_id, "removed").await;
        Ok(())
    }

    /// Handle an incoming connection from a peer. Known peers (added by
    /// ticket or pinned) are reconnected; unknown peers must enroll with a
    /// valid token or are refused.
    pub async fn handle_connection(self: &Arc<Self>, conn: Connection) -> Result<()> {
        let remote_id = conn.remote_id();
        let conn_id = ConnId(self.next_conn_id.fetch_add(1, Ordering::Relaxed));

        let admission = self.session.lock().unwrap().on_inbound(conn_id, remote_id);

        // No verdict yet: the core is holding this connection pending a claim.
        if admission.is_empty() {
            return self.handle_enrollment(conn_id, conn).await;
        }

        let peer = match self.peers.get(&remote_id) {
            Some(peer) => peer.clone(),
            None => return Ok(()),
        };

        // It called, so it is awake, and from now on we dial it too.
        peer.reach.lock().unwrap().connected_in();

        let mut conn_guard = peer.connection.write().await;
        let held_is_live = conn_guard
            .as_ref()
            .is_some_and(|held| held.close_reason().is_none());
        let held_dialed_by_us = peer.conn_dialed_by_us.load(Ordering::Relaxed);
        if dial::resolve(
            &self.endpoint.id(),
            &remote_id,
            held_is_live,
            held_dialed_by_us,
            false,
        ) == Keep::Held
        {
            info!(
                "{} dialed us while we were dialing it; keeping ours",
                remote_id
            );
            conn.close(0u32.into(), b"kept ours");
            return Ok(());
        }
        if let Some(old_conn) = conn_guard.take() {
            old_conn.close(0u32.into(), b"replaced");
        }
        peer.conn_dialed_by_us.store(false, Ordering::Relaxed);
        *conn_guard = Some(conn.clone());
        drop(conn_guard);

        peer.conn_notify.notify_one();
        info!("{} reconnected", remote_id);

        self.apply(&peer, admission).await;
        Ok(())
    }

    /// Carry out the core's actions for a peer that has a live connection.
    async fn apply(self: &Arc<Self>, peer: &Arc<Peer>, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Announce { ports, .. } => {
                    Self::send_ports(peer, ports).await;
                }
                Action::ApplyPorts { ports, .. } => {
                    self.update_peer_ports(peer, ports).await;
                }
                Action::PersistPin { key, label } => {
                    if let Err(e) = self.pins.add(&key.to_string(), label.as_deref()) {
                        error!("failed to persist pin for {}: {}", key, e);
                    }
                }
                Action::DropPin { key } => {
                    if let Err(e) = self.pins.remove(&key.to_string()) {
                        error!("failed to remove pin for {}: {}", key, e);
                    }
                }
                Action::PersistGrants => self.save_grants(),
                Action::Admit { .. } | Action::Refuse { .. } => {}
                Action::ServeTunnel { .. } | Action::RejectTunnel { .. } => {}
            }
        }
    }

    /// Wait for an unknown incoming peer to present an enrollment token.
    /// A valid claim pins its key under the token's label and admits it;
    /// no token, a bad token, or a timeout closes the connection without
    /// pinning or announcing anything. An enrolled peer binds nothing until
    /// it is projected, so there is no port contention to check here.
    async fn handle_enrollment(self: &Arc<Self>, conn_id: ConnId, conn: Connection) -> Result<()> {
        let remote_id = conn.remote_id();

        // Feed the core every control message until it admits or refuses. The
        // ExposedPorts / Enroll order is not fixed: they arrive on separate uni
        // streams, and the core buffers whichever lands first. Version rides on
        // Enroll as an extra field; the shell copies it, the core ignores it.
        let mut early_version = None;
        let verdict = tokio::time::timeout(ENROLL_TIMEOUT, async {
            loop {
                let mut recv = conn.accept_uni().await?;
                let data = recv.read_to_end(64 * 1024).await?;
                let msg: PeerMessage = match serde_json::from_slice(&data) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };
                if let PeerMessage::Enroll {
                    version: Some(version),
                    ..
                } = &msg
                {
                    early_version = Some(version.clone());
                }
                let actions = self
                    .session
                    .lock()
                    .unwrap()
                    .on_unadmitted(conn_id, remote_id, msg);
                if !actions.is_empty() {
                    return Ok::<_, anyhow::Error>(actions);
                }
            }
        })
        .await;

        let actions = match verdict {
            Ok(Ok(actions)) => actions,
            // Timed out or the stream failed: the core still holds the pending
            // connection, so ask it for the timeout verdict.
            _ => self.session.lock().unwrap().on_enroll_timeout(conn_id),
        };

        if let Some(Action::Refuse { reason, .. }) =
            actions.iter().find(|a| matches!(a, Action::Refuse { .. }))
        {
            info!("refused peer {}: {}", remote_id, reason.as_str());
            conn.close(0u32.into(), reason.as_str().as_bytes());
            return Ok(());
        }

        if !actions.iter().any(|a| matches!(a, Action::Admit { .. })) {
            conn.close(0u32.into(), Refusal::NotAuthorized.as_str().as_bytes());
            return Ok(());
        }

        let label = self.session.lock().unwrap().label_of(&remote_id);
        info!(
            "enrolled {} as \"{}\"",
            remote_id,
            label.clone().unwrap_or_default()
        );

        let peer = Peer::new(remote_id, label, true, None, Some(conn));
        if let Some(version) = early_version {
            *peer.version.write().await = Some(version);
        }
        self.peers.insert(remote_id, peer.clone());
        self.spawn_connection_loop(peer.clone());

        self.apply(&peer, actions).await;
        Ok(())
    }

    /// Tell a peer our crate version on an Enroll they already know how to
    /// read. Token is empty: they are already admitted, and there is no code.
    async fn send_version(peer: &Peer) {
        let msg = PeerMessage::enroll("");

        let conn = peer.connection.read().await;
        if let Some(conn) = conn.as_ref() {
            if let Err(e) = Self::send_message(conn, &msg).await {
                warn!("failed to send version to {}: {}", peer.endpoint_id, e);
            }
        }
    }

    /// Send a peer its port list. Always sent, even when empty, so a
    /// revocation tears down the peer's binding.
    async fn send_ports(peer: &Peer, ports: Vec<u16>) {
        let msg = PeerMessage::ExposedPorts(ports);

        let conn = peer.connection.read().await;
        if let Some(conn) = conn.as_ref() {
            if let Err(e) = Self::send_message(conn, &msg).await {
                warn!("failed to send ports to {}: {}", peer.endpoint_id, e);
            }
        }
    }

    /// Announce to one peer the ports granted to it.
    async fn announce_to(&self, peer: &Peer) {
        let ports = match self.session.lock().unwrap().announce(peer.endpoint_id) {
            Action::Announce { ports, .. } => ports,
            _ => return,
        };
        Self::send_ports(peer, ports).await;
    }

    /// Re-announce grants to every connected peer (each gets its own view)
    /// Write every grant to disk, as the session has them now.
    pub fn save_grants(&self) {
        let grants = self.session.lock().unwrap().all_grants();
        if let Err(e) = self.grants.save(&grants) {
            error!("failed to save grants: {}", e);
        }
    }

    pub async fn broadcast_grants(&self) {
        for entry in self.peers.iter() {
            self.announce_to(entry.value()).await;
        }
    }

    /// List all peers
    pub async fn list(&self) -> Vec<PeerInfo> {
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        let mut result = Vec::new();
        for peer in peers {
            let online = {
                let conn = peer.connection.read().await;
                conn.as_ref()
                    .map(|c| c.close_reason().is_none())
                    .unwrap_or(false)
            };
            let asleep = peer.reach.lock().unwrap().asleep();
            let dialing = !online && peer.dialing.load(Ordering::Relaxed);
            let surface = peer.surface.read().await.clone();
            let mut bound_at: Vec<BoundAt> = peer
                .bindings
                .iter()
                .map(|e| BoundAt {
                    port: *e.key(),
                    at: e.value().at.to_string(),
                })
                .collect();
            bound_at.sort_unstable_by_key(|b| b.port);
            let bound: Vec<u16> = bound_at.iter().map(|b| b.port).collect();
            let admission = self
                .session
                .lock()
                .unwrap()
                .admission_of(&peer.endpoint_id)
                .map(|a| a.as_str())
                .unwrap_or("unknown");
            result.push(PeerInfo {
                key: peer.endpoint_id.to_string(),
                name: peer.label(),
                online,
                asleep,
                dialing,
                admission: admission.to_string(),
                they_expose: peer.exposed_ports.read().await.clone(),
                version: peer.version.read().await.clone(),
                ip: surface.as_ref().map(|s| s.ip.to_string()),
                bound,
                bound_at,
            });
        }
        result
    }

    /// Resolve a peer reference (a key, or the name we gave it) to a key.
    /// A key is tried first; a name match is the fallback.
    fn resolve_peer(&self, peer_ref: &str) -> Option<EndpointId> {
        if let Ok(id) = peer_ref.parse::<EndpointId>() {
            if self.peers.contains_key(&id) {
                return Some(id);
            }
        }
        self.peers
            .iter()
            .find(|e| e.value().label().as_deref() == Some(peer_ref))
            .map(|e| *e.key())
    }

    /// The /24 to allocate surface addresses from: the TUN subnet when the
    /// tun backend is active, else the loopback range.
    fn surface_base(&self) -> [u8; 3] {
        if self.netstack.is_some() {
            surface::TUN_BASE
        } else {
            surface::LOOPBACK_BASE
        }
    }

    /// A snapshot of the addresses currently in use by surfaces.
    async fn projected_ips(&self) -> Vec<IpAddr> {
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        let mut ips = Vec::new();
        for peer in peers {
            if let Some(surface) = peer.surface.read().await.as_ref() {
                ips.push(surface.ip);
            }
        }
        ips
    }

    /// Resolve a surface name to its address, for the owned resolver. Matches
    /// the name a surface was projected under, from `--as` or a truncated key.
    pub async fn resolve_name(&self, label: &str) -> Option<IpAddr> {
        // Our own name resolves to where our services are: the --host forward
        // address (default 127.0.0.1). The TUN backend answers self-name the
        // same way, from its in-stack name map.
        if self.self_name.as_deref() == Some(label) {
            return Some(self.host);
        }
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        for peer in peers {
            if let Some(surface) = peer.surface.read().await.as_ref() {
                if surface.name.as_deref() == Some(label) {
                    return Some(surface.ip);
                }
            }
        }
        None
    }

    /// Project a peer's surface explicitly: pin an address (chosen or
    /// allocated) and a name, overriding whatever auto-project set. The name is
    /// served by the owned resolver, so it needs no /etc/hosts write. Ports
    /// already announced are rebound at the new address.
    pub async fn project(
        self: &Arc<Self>,
        peer_ref: &str,
        ip: Option<IpAddr>,
        name: Option<String>,
    ) -> Result<()> {
        let id = self
            .resolve_peer(peer_ref)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?;
        let peer = self
            .peers
            .get(&id)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?
            .clone();

        let _guard = self.bind_lock.lock().await;

        // Tear down any existing surface first so a re-project moves cleanly.
        // Release while the old surface is still set (the tun backend unlistens
        // by ip), then clear it and drop the address.
        let old = peer.surface.read().await.clone();
        if let Some(old) = old {
            let held: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            for port in held {
                self.release_binding(&peer, port).await;
            }
            *peer.surface.write().await = None;
            let _ = self.unclaim_addr(old.ip);
        }

        // An explicit --as renames the peer, not just its surface, so `list`,
        // `--to` and the resolver all go by the new name. Without --as the
        // surface takes the peer's current label, so it still has a handle.
        if let Some(name) = &name {
            self.rename(&peer, name.clone()).await;
        }
        let name = name.or_else(|| peer.label());
        let ip = match ip {
            Some(ip) => ip,
            None => self.new_surface_ip().await?,
        };

        self.claim_addr(ip, name.clone())?;
        *peer.surface.write().await = Some(Surface {
            ip,
            name: name.clone(),
        });
        if let Err(e) = self.surfaces.add(&id.to_string(), ip, name) {
            warn!("failed to persist surface for {}: {}", id, e);
        }

        let ports = peer.exposed_ports.read().await.clone();
        for port in ports {
            if !peer.bindings.contains_key(&port) {
                self.bind_one(&peer, ip, port).await;
            }
        }

        info!("projected {} to {}", id, ip);
        Ok(())
    }

    /// Unproject a peer's surface: release every binding, drop the address and
    /// name. The peer stays connected; only its local reach is torn down.
    pub async fn unproject(self: &Arc<Self>, peer_ref: &str) -> Result<()> {
        let id = self
            .resolve_peer(peer_ref)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?;
        let peer = self
            .peers
            .get(&id)
            .ok_or_else(|| anyhow!("no such peer: {}", peer_ref))?
            .clone();

        let _guard = self.bind_lock.lock().await;

        let surface = peer.surface.read().await.clone();
        let surface = match surface {
            Some(s) => s,
            None => return Err(anyhow!("peer {} is not projected", peer_ref)),
        };

        // release while the surface is still set (tun backend unlistens by ip)
        let ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
        for port in ports {
            self.release_binding(&peer, port).await;
        }
        *peer.surface.write().await = None;

        self.unclaim_addr(surface.ip)?;
        if let Err(e) = self.surfaces.remove(&id.to_string()) {
            warn!("failed to remove surface record for {}: {}", id, e);
        }

        info!("unprojected {}", id);
        Ok(())
    }

    /// List every known peer and its surface (projected or not).
    /// Re-apply persisted surfaces at startup: restore each address so a
    /// projected peer keeps its IP and name across a restart. Ports rebind
    /// when the peer reconnects and re-announces.
    pub async fn restore_surfaces(self: &Arc<Self>) {
        let records = match self.surfaces.load() {
            Ok(records) => records,
            Err(e) => {
                warn!("failed to load surfaces: {}", e);
                return;
            }
        };

        for record in records {
            let id: EndpointId = match record.key.parse() {
                Ok(id) => id,
                Err(_) => continue,
            };
            let Some(peer) = self.peers.get(&id) else {
                continue;
            };
            if let Err(e) = self.claim_addr(record.ip, record.name.clone()) {
                warn!("failed to restore address {}: {}", record.ip, e);
                continue;
            }
            *peer.surface.write().await = Some(Surface {
                ip: record.ip,
                name: record.name,
            });
        }
    }

    /// Say goodbye to every connected peer and stop dialing: this daemon is
    /// about to be suspended. A peer that reads the goodbye waits for us to
    /// call, so we must: each one is pinned to be dialed after a restart, even
    /// one we invited. Returns once each peer has closed its end, or after
    /// `GOODBYE_WAIT`. A daemon older than Goodbye drops it and never closes,
    /// so it costs the whole wait.
    ///
    /// Only an explicit `Sleep` reaches this. A daemon stopping or restarting
    /// says nothing, because a peer told goodbye stops dialing, and a daemon
    /// that restarts waits for a call from every peer it invited (ADR 0009).
    pub async fn sleep(&self) {
        self.sleeping.store(true, Ordering::SeqCst);
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        let mut goodbyes = tokio::task::JoinSet::new();
        for peer in peers {
            let held = peer.connection.read().await.clone();
            let Some(conn) = held.filter(|c| c.close_reason().is_none()) else {
                continue;
            };
            let key = peer.endpoint_id.to_string();
            if let Err(e) = self.pins.mark_dialed(&key) {
                warn!("failed to pin {} to be dialed: {}", key, e);
            }
            goodbyes.spawn(async move {
                let said = tokio::time::timeout(GOODBYE_WAIT, async {
                    Self::send_message(&conn, &PeerMessage::Goodbye).await?;
                    conn.closed().await;
                    Ok::<_, anyhow::Error>(())
                })
                .await;
                match said {
                    Ok(Ok(())) => info!("said goodbye to {}", key),
                    Ok(Err(e)) => warn!("failed to say goodbye to {}: {}", key, e),
                    Err(_) => info!("{} did not answer goodbye; closing", key),
                }
                conn.close(0u32.into(), b"goodbye");
            });
        }
        while goodbyes.join_next().await.is_some() {}
    }

    /// Stop every peer's loop and close its listeners, leaving pins, surfaces
    /// and ports on disk. A test restarting a daemon in the same process does
    /// this first, the way the app's runtime going away does on iOS.
    #[cfg(test)]
    pub async fn stop(&self) {
        let _guard = self.bind_lock.lock().await;
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        for peer in peers {
            peer.removed.store(true, Ordering::Relaxed);
            peer.conn_notify.notify_one();
            if let Some(conn) = peer.connection.write().await.take() {
                conn.close(0u32.into(), b"stopped");
            }
            let ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            for port in ports {
                self.release_binding(&peer, port).await;
            }
        }
    }

    /// Send `data` to a peer as a control message, whatever it holds. Lets a
    /// test send what a newer daemon would.
    #[cfg(test)]
    pub async fn send_raw(&self, endpoint_id: &EndpointId, data: &[u8]) -> Result<()> {
        let peer = self
            .peers
            .get(endpoint_id)
            .map(|p| p.clone())
            .ok_or_else(|| anyhow!("no such peer"))?;
        let conn = peer
            .connection
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("disconnected"))?;
        let mut send = conn.open_uni().await?;
        send.write_all(data).await?;
        send.finish()?;
        Ok(())
    }

    /// How many dials `endpoint_id`'s loop has started
    #[cfg(test)]
    pub fn dial_attempts(&self, endpoint_id: &EndpointId) -> u64 {
        self.peers
            .get(endpoint_id)
            .map(|p| p.dial_attempts.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Close the connection and leave the peer in place, like a laptop
    /// suspending. Both ends have to reconnect.
    #[cfg(test)]
    pub async fn sever(&self, endpoint_id: &EndpointId) {
        let Some(peer) = self.peers.get(endpoint_id).map(|p| p.clone()) else {
            return;
        };
        let held = peer.connection.write().await.take();
        if let Some(conn) = held {
            conn.close(0u32.into(), b"severed");
        }
    }
}

/// Splice one accepted TUN connection onto the peer's QUIC tunnel: client
/// bytes to the tunnel, tunnel bytes back to the client. Ends when either side
/// closes.
async fn bridge(peer: Arc<Peer>, acc: Accept) -> Result<()> {
    let (mut qs, mut qr) = peer.open_tunnel(acc.port).await?;
    let (mut client_r, mut client_w) = tokio::io::split(acc.stream);

    let client_to_quic = async {
        let _ = tunnel::copy_flush(&mut client_r, &mut qs).await;
        let _ = qs.finish();
    };
    let quic_to_client = async {
        let _ = tunnel::copy_flush(&mut qr, &mut client_w).await;
        let _ = client_w.shutdown().await;
    };
    tokio::select! {
        _ = client_to_quic => {}
        _ = quic_to_client => {}
    }
    Ok(())
}

impl PeerConnection for Arc<Peer> {
    async fn open_tunnel(
        &self,
        port: u16,
    ) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
        let conn = self.connection.read().await;
        let conn = conn.as_ref().ok_or_else(|| anyhow!("peer disconnected"))?;

        let (mut send, recv) = conn.open_bi().await.context("failed to open stream")?;

        // Send the port number as first 2 bytes
        send.write_all(&port.to_be_bytes()).await?;

        Ok((send, recv))
    }
}
