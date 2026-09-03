//! Peer management - connections, port announcements, auto-binding, reconnection.
//!
//! Access is default deny: an incoming connection is served only if its key
//! is already known (added by ticket, or pinned at enrollment) or it presents
//! a valid enrollment token. Anyone else is refused -- no announcement, no
//! tunnel.

use crate::core::session::{Action, Admission, ConnId, Refusal, Session};
use crate::enroll::Pins;
use crate::netstack::{Accept, NetStack};
use crate::protocol::{PeerInfo, PeerMessage, ALPN};
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

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How long an unknown incoming peer gets to present its enrollment token
const ENROLL_TIMEOUT: Duration = Duration::from_secs(10);
/// How many times to try binding a forwarded port before giving up
const BIND_ATTEMPTS: u32 = 5;
/// Delay between bind attempts
const BIND_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Info about a connected peer
struct Peer {
    endpoint_id: EndpointId,
    /// What we call this peer, from `--as` (None until something names it)
    label: Option<String>,
    /// Whether we dial this peer to reconnect. True for peers we accepted;
    /// peers that accepted our invitation dial us, so we just wait.
    dial: bool,
    /// An invitation's one-time code, presented on connect by the accepter
    enroll_token: Option<String>,
    connection: RwLock<Option<Connection>>,
    /// Ports this peer exposes
    exposed_ports: RwLock<Vec<u16>>,
    /// Crate version the peer last announced. None until it sends one.
    version: RwLock<Option<String>>,
    /// Where this peer's ports are bound locally. None until projected: an
    /// unprojected peer's ports are known but have no listener.
    surface: RwLock<Option<Surface>>,
    /// Active bindings (local port -> task handle)
    bindings: DashMap<u16, tokio::task::JoinHandle<()>>,
    /// Notified when a new connection replaces the current one
    conn_notify: Notify,
    /// Set when peer is removed; signals connection loop to exit
    removed: AtomicBool,
}

impl Peer {
    fn new(
        endpoint_id: EndpointId,
        label: Option<String>,
        dial: bool,
        enroll_token: Option<String>,
        connection: Option<Connection>,
    ) -> Arc<Self> {
        Arc::new(Self {
            endpoint_id,
            label,
            dial,
            enroll_token,
            connection: RwLock::new(connection),
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
    /// Projected surfaces, persisted across restarts
    surfaces: SurfaceStore,
    /// TUN owned-network backend. When present, surfaces bind on the TUN via a
    /// userspace stack instead of loopback (ADR 0005); None = loopback backend.
    netstack: Option<NetStack>,
    /// This node's own name. The owned resolver answers `<self_name>.pai-sho`
    /// with 127.0.0.1, so local traffic uses the same origin peers do.
    self_name: Option<String>,
    /// Serializes binding creation/teardown and surface changes so a port's
    /// listener state and the address it binds move together
    bind_lock: Mutex<()>,
}

impl PeerManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: Endpoint,
        host: IpAddr,
        session: Arc<std::sync::Mutex<Session>>,
        pins: Pins,
        surfaces: SurfaceStore,
        netstack: Option<NetStack>,
        self_name: Option<String>,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            endpoint,
            host,
            session,
            next_conn_id: AtomicU64::new(1),
            pins,
            surfaces,
            netstack,
            self_name,
            bind_lock: Mutex::new(()),
        }
    }

    /// Take up an invitation, or reach a peer known by key. We dial this peer
    /// and keep dialing it. A code, when there is one, is presented on connect
    /// and on every reconnect (the peer ignores it once we are known).
    pub async fn add_peer(
        self: &Arc<Self>,
        endpoint_id: EndpointId,
        name: Option<String>,
        code: Option<String>,
    ) -> Result<()> {
        if self.peers.contains_key(&endpoint_id) {
            return Err(anyhow!("peer already exists"));
        }

        self.session
            .lock()
            .unwrap()
            .admit_known(endpoint_id, name.clone(), Admission::Added);

        // Record the peer before dialing. A peer whose daemon is not up yet is
        // still ours: its connection loop retries with backoff.
        let peer = Peer::new(endpoint_id, name, true, code, None);
        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);

        Ok(())
    }

    /// Register a peer pinned at a previous enrollment (loaded at startup).
    /// We never dial it -- it phones home.
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
        // A peer added but never dialed starts at zero, so its first dial is
        // immediate. Backoff only applies once an attempt has failed.
        let mut backoff = if peer.connection.read().await.is_some() {
            BACKOFF_INITIAL
        } else {
            Duration::ZERO
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

            if !peer.dial {
                // This peer phones home; wait for an incoming connection
                peer.conn_notify.notified().await;
                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }
                info!("{} reconnected via incoming connection", peer.endpoint_id);
                continue;
            }

            // Reconnect with exponential backoff
            loop {
                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }

                info!("reconnecting to {} in {:?}", peer.endpoint_id, backoff);

                // Wait for backoff, but wake early if an incoming connection arrives
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = peer.conn_notify.notified() => {
                        info!("{} reconnected via incoming connection", peer.endpoint_id);
                        backoff = BACKOFF_INITIAL;
                        break;
                    }
                }

                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }

                match manager.endpoint.connect(peer.endpoint_id, ALPN).await {
                    Ok(conn) => {
                        info!("reconnected to {}", peer.endpoint_id);
                        *peer.connection.write().await = Some(conn.clone());
                        // Re-present the enroll token in case the peer never
                        // processed it (it ignores the message once we are pinned)
                        if let Some(token) = &peer.enroll_token {
                            let msg = PeerMessage::enroll(token.clone());
                            if let Err(e) = Self::send_message(&conn, &msg).await {
                                warn!("failed to send enroll token: {}", e);
                            }
                        }
                        manager.announce_to(&peer).await;
                        backoff = BACKOFF_INITIAL;
                        break;
                    }
                    Err(e) => {
                        warn!("reconnect to {} failed: {}", peer.endpoint_id, e);
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
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
        if peer.enroll_token.is_none() {
            Self::send_version(peer).await;
        }

        loop {
            tokio::select! {
                result = conn.accept_uni() => {
                    let recv = result?;
                    let manager = manager.clone();
                    let peer = peer.clone();
                    tokio::spawn(async move {
                        Self::handle_uni_stream(&manager, recv, &peer).await;
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
            PeerMessage::Enroll { version, .. } => {
                // Peer is already known; nothing to enroll. Version, if any,
                // is the reason this message shows up again after admission.
                if let Some(version) = version {
                    info!("{} reports version {}", peer.endpoint_id, version);
                    *peer.version.write().await = Some(version);
                }
            }
            PeerMessage::Error(e) => {
                error!("peer error: {}", e);
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

        let taken = self.projected_ips().await;
        let ip = match surface::allocate(&taken, self.surface_base()) {
            Ok(ip) => ip,
            Err(e) => {
                error!("cannot auto-project {}: {}", peer.endpoint_id, e);
                return None;
            }
        };
        // Every surface answers by name. Nothing named this peer, so it gets
        // a short form of its key until `project --as` renames it.
        let name = Some(
            peer.label
                .clone()
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
                peer.bindings
                    .insert(port, tokio::spawn(std::future::pending::<()>()));
                info!("bound {}:{} (tun)", v4, port);
                return;
            }
        }

        let addr = SocketAddr::from((ip, port));
        let mut last_err = None;
        for attempt in 0..BIND_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(BIND_RETRY_DELAY).await;
            }
            match tunnel::bind_listener(addr).await {
                Ok(listener) => {
                    let peer_clone = peer.clone();
                    let handle = tokio::spawn(async move {
                        if let Err(e) = tunnel::serve_listener(listener, port, &peer_clone).await {
                            error!("serving {} failed: {}", addr, e);
                        }
                    });
                    peer.bindings.insert(port, handle);
                    info!("bound {}", addr);
                    return;
                }
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

    /// Release one binding, waiting for its task to finish so the listener is
    /// actually dropped before the port is reused. On the TUN backend, also
    /// tells the stack to stop listening (reads the peer's surface for the ip).
    async fn release_binding(&self, peer: &Arc<Peer>, port: u16) {
        if let Some((_, handle)) = peer.bindings.remove(&port) {
            handle.abort();
            let _ = handle.await;
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
            if let Action::DropPin { key } = action {
                if let Err(e) = self.pins.remove(&key.to_string()) {
                    warn!("failed to remove pin for {}: {}", key, e);
                }
            }
        }
        if let Err(e) = self.surfaces.remove(&endpoint_id.to_string()) {
            warn!("failed to remove surface record for {}: {}", endpoint_id, e);
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

        // Known peer reconnecting -- close old connection, install new one
        let mut conn_guard = peer.connection.write().await;
        if let Some(old_conn) = conn_guard.take() {
            old_conn.close(0u32.into(), b"replaced");
        }
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

        let peer = Peer::new(remote_id, label, false, None, Some(conn));
        if let Some(version) = early_version {
            *peer.version.write().await = Some(version);
        }
        self.peers.insert(remote_id, peer.clone());
        self.spawn_connection_loop(peer.clone());

        self.apply(&peer, actions).await;
        Ok(())
    }

    /// Tell a peer our crate version on an Enroll they already know how to
    /// read. Token is empty: they are already admitted and ignore Enroll.
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
            let surface = peer.surface.read().await.clone();
            let mut bound: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            bound.sort_unstable();
            let admission = self
                .session
                .lock()
                .unwrap()
                .admission_of(&peer.endpoint_id)
                .map(|a| a.as_str())
                .unwrap_or("unknown");
            result.push(PeerInfo {
                key: peer.endpoint_id.to_string(),
                name: peer.label.clone(),
                online,
                admission: admission.to_string(),
                they_expose: peer.exposed_ports.read().await.clone(),
                version: peer.version.read().await.clone(),
                ip: surface.as_ref().map(|s| s.ip.to_string()),
                bound,
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
            .find(|e| e.value().label.as_deref() == Some(peer_ref))
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

        // Default the name to the peer's label so `project` without --as still
        // yields a resolvable handle.
        let name = name.or_else(|| peer.label.clone());
        let ip = match ip {
            Some(ip) => ip,
            None => {
                let taken = self.projected_ips().await;
                surface::allocate(&taken, self.surface_base())?
            }
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
