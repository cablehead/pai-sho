//! Peer management - connections, port announcements, auto-binding, reconnection.
//!
//! Access is default deny: an incoming connection is served only if its key
//! is already known (added by ticket, or pinned at enrollment) or it presents
//! a valid enrollment token. Anyone else is refused -- no announcement, no
//! tunnel.

use crate::enroll::{Pins, Tokens};
use crate::grants::Grants;
use crate::protocol::{BindingInfo, PeerInfo, PeerMessage, SurfaceInfo, ALPN};
use crate::surface::{self, Surface, SurfaceStore};
use crate::tunnel::{self, PeerConnection};
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock};
use tracing::{error, info, warn};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How long an unknown incoming peer gets to present its enrollment token
const ENROLL_TIMEOUT: Duration = Duration::from_secs(10);

/// Info about a connected peer
struct Peer {
    endpoint_id: EndpointId,
    /// Label assigned at enrollment (None for peers added by ticket)
    label: Option<String>,
    /// Whether we dial this peer to reconnect. True for peers added by
    /// ticket; enrolled/pinned peers dial us, so we just wait.
    dial: bool,
    /// Token to present on connect (workload side, from --enroll)
    enroll_token: Option<String>,
    connection: RwLock<Option<Connection>>,
    /// Ports this peer exposes
    exposed_ports: RwLock<Vec<u16>>,
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
    /// Directed grants: which port is exposed to which peer
    grants: Arc<RwLock<Grants>>,
    /// Host address for forwarding tunnel requests
    host: IpAddr,
    /// Enrollment tokens we minted (operator side)
    tokens: Arc<Tokens>,
    /// Peers pinned at enrollment, persisted across restarts
    pins: Pins,
    /// Projected surfaces, persisted across restarts
    surfaces: SurfaceStore,
    /// Serializes binding creation/teardown and surface changes so a port's
    /// listener state and the address it binds move together
    bind_lock: Mutex<()>,
}

impl PeerManager {
    pub fn new(
        endpoint: Endpoint,
        host: IpAddr,
        grants: Arc<RwLock<Grants>>,
        tokens: Arc<Tokens>,
        pins: Pins,
        surfaces: SurfaceStore,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            endpoint,
            host,
            grants,
            tokens,
            pins,
            surfaces,
            bind_lock: Mutex::new(()),
        }
    }

    /// Add a new peer and connect to it. If `enroll_token` is set, present
    /// it on connect and on every reconnect (the peer ignores it once we
    /// are pinned).
    pub async fn add_peer(
        self: &Arc<Self>,
        ticket: &str,
        enroll_token: Option<String>,
    ) -> Result<()> {
        let endpoint_id: EndpointId = ticket.parse().context("invalid ticket")?;

        // Check if already connected
        if self.peers.contains_key(&endpoint_id) {
            return Err(anyhow!("peer already exists"));
        }

        // Connect to the peer
        let conn = self
            .endpoint
            .connect(endpoint_id, ALPN)
            .await
            .context("failed to connect to peer")?;

        info!("connected to {}", endpoint_id);

        let peer = Peer::new(endpoint_id, None, true, enroll_token, Some(conn.clone()));

        if let Some(token) = &peer.enroll_token {
            let msg = PeerMessage::Enroll {
                token: token.clone(),
            };
            if let Err(e) = Self::send_message(&conn, &msg).await {
                warn!("failed to send enroll token to {}: {}", endpoint_id, e);
            }
        }

        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);

        Ok(())
    }

    /// Register a peer pinned at a previous enrollment (loaded at startup).
    /// We never dial it -- it phones home.
    pub fn add_pinned(self: &Arc<Self>, key: &str, label: &str) -> Result<()> {
        let endpoint_id: EndpointId = key.parse().context("invalid pinned key")?;
        if self.peers.contains_key(&endpoint_id) {
            return Ok(());
        }
        let peer = Peer::new(endpoint_id, Some(label.to_string()), false, None, None);
        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);
        info!("pinned peer {} (\"{}\")", endpoint_id, label);
        Ok(())
    }

    /// Pin a peer by key under a label without a token (host-attested
    /// enrollment): register it live so it is authorized when it phones
    /// home, and persist the pin across restarts. Idempotent.
    pub fn pin_peer(self: &Arc<Self>, key: &str, label: &str) -> Result<()> {
        self.add_pinned(key, label)?;
        self.pins.add(key, label)
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
        let mut backoff = BACKOFF_INITIAL;

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
                            let msg = PeerMessage::Enroll {
                                token: token.clone(),
                            };
                            if let Err(e) = Self::send_message(&conn, &msg).await {
                                warn!("failed to send enroll token: {}", e);
                            }
                        }
                        Self::send_exposed_ports_to_peer(&peer, &manager.grants).await;
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
                    let host = manager.host;
                    let grants = manager.grants.clone();
                    let peer = peer.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_bi_stream(host, &grants, &peer, send, recv).await {
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
            PeerMessage::Enroll { .. } => {
                // Peer is already known; nothing to enroll
            }
            PeerMessage::Error(e) => {
                error!("peer error: {}", e);
            }
        }
    }

    async fn handle_bi_stream(
        host: IpAddr,
        grants: &Arc<RwLock<Grants>>,
        peer: &Arc<Peer>,
        send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
    ) -> Result<()> {
        let mut buf = [0u8; 2];
        recv.read_exact(&mut buf).await?;
        let port = u16::from_be_bytes(buf);
        // A tunnel is served only for a port granted to this specific peer
        if !grants.read().await.allows(port, &peer.endpoint_id) {
            warn!(
                "refused tunnel to ungranted port {} from {}",
                port, peer.endpoint_id
            );
            return Ok(());
        }
        info!("tunnel request for port {}", port);
        tunnel::handle_tunnel(host, port, send, recv).await
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
                Self::release_binding(peer, *port).await;
            }
        }

        // Bind newly announced ports, but only if the peer is projected. A
        // port with no binding is retried on every announce, so projecting a
        // peer later picks up whatever it has already announced.
        let ip = peer.surface.read().await.as_ref().map(|s| s.ip);
        if let Some(ip) = ip {
            for &port in &new_ports {
                if peer.bindings.contains_key(&port) {
                    continue;
                }
                Self::bind_one(peer, ip, port).await;
            }
        }

        *peer.exposed_ports.write().await = new_ports;
    }

    /// Bind one forwarded port under `ip` and record the binding. A failed
    /// bind is logged and leaves no phantom entry, so the next announce retries.
    async fn bind_one(peer: &Arc<Peer>, ip: IpAddr, port: u16) {
        let addr = SocketAddr::from((ip, port));
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
            }
            Err(e) => {
                error!("failed to bind {}: {}", addr, e);
            }
        }
    }

    /// Release one binding, waiting for its task to finish so the
    /// TcpListener is actually dropped before the port is reused
    async fn release_binding(peer: &Arc<Peer>, port: u16) {
        if let Some((_, handle)) = peer.bindings.remove(&port) {
            handle.abort();
            let _ = handle.await;
            info!("removed binding for port {}", port);
        }
    }

    /// Fully evict a peer: close its connection, release every binding it
    /// holds (waiting for the listeners to drop), tear down its surface, clear
    /// grants naming it, and delete its pin. Callers hold the bind lock where
    /// it matters.
    async fn evict(&self, endpoint_id: &EndpointId, reason: &str) {
        let mut had_named_surface = false;
        if let Some((_, peer)) = self.peers.remove(endpoint_id) {
            peer.removed.store(true, Ordering::Relaxed);
            peer.conn_notify.notify_one();

            if let Some(conn) = peer.connection.write().await.take() {
                conn.close(0u32.into(), reason.as_bytes());
            }

            let ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            for port in ports {
                Self::release_binding(&peer, port).await;
            }

            if let Some(surface) = peer.surface.write().await.take() {
                had_named_surface = surface.name.is_some();
                if let Err(e) = surface::remove_addr(surface.ip) {
                    warn!("failed to remove address {}: {}", surface.ip, e);
                }
            }
        }

        self.grants.write().await.revoke_grantee(endpoint_id);
        if let Err(e) = self.pins.remove(&endpoint_id.to_string()) {
            warn!("failed to remove pin for {}: {}", endpoint_id, e);
        }
        if let Err(e) = self.surfaces.remove(&endpoint_id.to_string()) {
            warn!("failed to remove surface record for {}: {}", endpoint_id, e);
        }
        if had_named_surface {
            if let Err(e) = self.sync_hosts().await {
                warn!("failed to sync /etc/hosts after eviction: {}", e);
            }
        }

        info!("evicted peer {} ({})", endpoint_id, reason);
    }

    /// Remove a peer by ticket
    pub async fn remove_peer(&self, ticket: &str) -> Result<()> {
        let endpoint_id: EndpointId = ticket.parse().context("invalid ticket")?;

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

        let peer = if let Some(peer) = self.peers.get(&remote_id) {
            // Known peer reconnecting -- close old connection, install new one
            let mut conn_guard = peer.connection.write().await;
            if let Some(old_conn) = conn_guard.take() {
                old_conn.close(0u32.into(), b"replaced");
            }
            *conn_guard = Some(conn.clone());
            drop(conn_guard);

            peer.conn_notify.notify_one();
            info!("{} reconnected", remote_id);
            peer.clone()
        } else {
            // Unknown peer: enroll with a valid token, or nothing
            match self.handle_enrollment(conn.clone()).await? {
                Some(peer) => peer,
                None => return Ok(()),
            }
        };

        // Send our exposed ports to this (authorized) peer
        Self::send_exposed_ports_to_peer(&peer, &self.grants).await;

        Ok(())
    }

    /// Wait for an unknown incoming peer to present an enrollment token.
    /// A valid claim pins its key under the token's label and admits it;
    /// no token, a bad token, or a timeout closes the connection without
    /// pinning or announcing anything. An enrolled peer binds nothing until
    /// it is projected, so there is no port contention to check here.
    async fn handle_enrollment(self: &Arc<Self>, conn: Connection) -> Result<Option<Arc<Peer>>> {
        let remote_id = conn.remote_id();

        // ExposedPorts can arrive before the Enroll message (separate uni
        // streams); hold on to it and apply after a successful enrollment.
        let mut early_ports: Option<Vec<u16>> = None;

        let claim = tokio::time::timeout(ENROLL_TIMEOUT, async {
            loop {
                let mut recv = conn.accept_uni().await?;
                let data = recv.read_to_end(64 * 1024).await?;
                match serde_json::from_slice::<PeerMessage>(&data) {
                    Ok(PeerMessage::Enroll { token }) => {
                        return Ok::<_, anyhow::Error>(self.tokens.claim(&token));
                    }
                    Ok(PeerMessage::ExposedPorts(ports)) => {
                        early_ports = Some(ports);
                    }
                    _ => {}
                }
            }
        })
        .await;

        let label = match claim {
            Ok(Ok(Some(label))) => label,
            _ => {
                info!("refused unauthorized peer {}", remote_id);
                conn.close(0u32.into(), b"not authorized");
                return Ok(None);
            }
        };

        info!("enrolled {} as \"{}\"", remote_id, label);
        self.pins.add(&remote_id.to_string(), &label)?;

        let peer = Peer::new(remote_id, Some(label), false, None, Some(conn));
        self.peers.insert(remote_id, peer.clone());
        self.spawn_connection_loop(peer.clone());

        if let Some(ports) = early_ports {
            self.update_peer_ports(&peer, ports).await;
        }

        Ok(Some(peer))
    }

    /// Announce to a peer the ports granted to it. Always sent, even when
    /// empty, so a revocation tears down the peer's binding.
    async fn send_exposed_ports_to_peer(peer: &Peer, grants: &Arc<RwLock<Grants>>) {
        let ports = grants.read().await.ports_for(&peer.endpoint_id);
        let msg = PeerMessage::ExposedPorts(ports);

        let conn = peer.connection.read().await;
        if let Some(conn) = conn.as_ref() {
            if let Err(e) = Self::send_message(conn, &msg).await {
                warn!("failed to send ports to {}: {}", peer.endpoint_id, e);
            }
        }
    }

    /// Re-announce grants to every connected peer (each gets its own view)
    pub async fn broadcast_grants(&self) {
        for entry in self.peers.iter() {
            Self::send_exposed_ports_to_peer(entry.value(), &self.grants).await;
        }
    }

    /// Keys of all currently known peers
    pub fn peer_ids(&self) -> Vec<EndpointId> {
        self.peers.iter().map(|e| *e.key()).collect()
    }

    /// List all peers
    pub async fn list(&self) -> Vec<PeerInfo> {
        let mut result = Vec::new();
        for entry in self.peers.iter() {
            let peer = entry.value();
            let connected = {
                let conn = peer.connection.read().await;
                conn.as_ref()
                    .map(|c| c.close_reason().is_none())
                    .unwrap_or(false)
            };
            result.push(PeerInfo {
                key: peer.endpoint_id.to_string(),
                label: peer.label.clone(),
                online: connected,
                they_expose: peer.exposed_ports.read().await.clone(),
            });
        }
        result
    }

    /// List all bindings
    pub async fn list_bindings(&self) -> Vec<BindingInfo> {
        let mut result = Vec::new();
        for entry in self.peers.iter() {
            let peer = entry.value();
            for binding in peer.bindings.iter() {
                result.push(BindingInfo {
                    port: *binding.key(),
                    peer: peer.endpoint_id.to_string(),
                });
            }
        }
        result
    }

    /// Resolve a peer reference (an endpoint key or an enrollment label) to a
    /// key. A key is tried first; a label match is the fallback.
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

    /// A snapshot of the loopback addresses currently in use by surfaces.
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

    /// Rewrite the /etc/hosts managed block from every named surface.
    async fn sync_hosts(&self) -> Result<()> {
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        let mut entries = Vec::new();
        for peer in peers {
            if let Some(surface) = peer.surface.read().await.as_ref() {
                if let Some(name) = &surface.name {
                    entries.push((surface.ip, name.clone()));
                }
            }
        }
        surface::sync_hosts(&entries)
    }

    /// Project a peer's surface: give it a local address (chosen or allocated),
    /// an optional /etc/hosts name, and bind every port it has announced. The
    /// OS-visible effects (address, name) are applied before anything is
    /// recorded, and rolled back if the name cannot be set.
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

        let ip = match ip {
            Some(ip) => ip,
            None => {
                let taken = self.projected_ips().await;
                surface::allocate(&taken)?
            }
        };

        surface::ensure_addr(ip)?;
        *peer.surface.write().await = Some(Surface {
            ip,
            name: name.clone(),
        });

        // A name change touches /etc/hosts (root-only); only do it when named,
        // so an unnamed projection stays unprivileged.
        if name.is_some() {
            if let Err(e) = self.sync_hosts().await {
                *peer.surface.write().await = None;
                let _ = surface::remove_addr(ip);
                return Err(e);
            }
        }

        if let Err(e) = self.surfaces.add(&id.to_string(), ip, name) {
            warn!("failed to persist surface for {}: {}", id, e);
        }

        // Bind whatever the peer has already announced.
        let ports = peer.exposed_ports.read().await.clone();
        for port in ports {
            if !peer.bindings.contains_key(&port) {
                Self::bind_one(&peer, ip, port).await;
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

        let surface = peer.surface.write().await.take();
        let surface = match surface {
            Some(s) => s,
            None => return Err(anyhow!("peer {} is not projected", peer_ref)),
        };

        let ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
        for port in ports {
            Self::release_binding(&peer, port).await;
        }

        surface::remove_addr(surface.ip)?;
        if let Err(e) = self.surfaces.remove(&id.to_string()) {
            warn!("failed to remove surface record for {}: {}", id, e);
        }
        if surface.name.is_some() {
            self.sync_hosts().await?;
        }

        info!("unprojected {}", id);
        Ok(())
    }

    /// List every known peer and its surface (projected or not).
    pub async fn surfaces(&self) -> Vec<SurfaceInfo> {
        let peers: Vec<Arc<Peer>> = self.peers.iter().map(|e| e.value().clone()).collect();
        let mut result = Vec::new();
        for peer in peers {
            let surface = peer.surface.read().await.clone();
            let mut ports: Vec<u16> = peer.bindings.iter().map(|e| *e.key()).collect();
            ports.sort_unstable();
            result.push(SurfaceInfo {
                peer: peer.endpoint_id.to_string(),
                label: peer.label.clone(),
                projected: surface.is_some(),
                ip: surface.as_ref().map(|s| s.ip.to_string()),
                name: surface.as_ref().and_then(|s| s.name.clone()),
                ports,
            });
        }
        result
    }

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

        let mut any_named = false;
        for record in records {
            let id: EndpointId = match record.key.parse() {
                Ok(id) => id,
                Err(_) => continue,
            };
            let Some(peer) = self.peers.get(&id) else {
                continue;
            };
            if let Err(e) = surface::ensure_addr(record.ip) {
                warn!("failed to restore address {}: {}", record.ip, e);
                continue;
            }
            if record.name.is_some() {
                any_named = true;
            }
            *peer.surface.write().await = Some(Surface {
                ip: record.ip,
                name: record.name,
            });
        }

        if any_named {
            if let Err(e) = self.sync_hosts().await {
                warn!("failed to sync /etc/hosts on restore: {}", e);
            }
        }
    }
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
