//! Peer management - connections, port announcements, auto-binding, reconnection.
//!
//! Access is default deny: an incoming connection is served only if its key
//! is already known (added by ticket, or pinned at enrollment) or it presents
//! a valid enrollment token. Anyone else is refused -- no announcement, no
//! tunnel.

use crate::enroll::{Pins, Tokens};
use crate::grants::Grants;
use crate::protocol::{BindingInfo, PeerInfo, PeerMessage, ALPN};
use crate::tunnel::{self, PeerConnection};
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
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
}

impl PeerManager {
    pub fn new(
        endpoint: Endpoint,
        host: IpAddr,
        grants: Arc<RwLock<Grants>>,
        tokens: Arc<Tokens>,
        pins: Pins,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            endpoint,
            host,
            grants,
            tokens,
            pins,
        }
    }

    /// Add a new peer and connect to it. If `enroll_token` is set, present
    /// it on connect and on every reconnect (the peer ignores it once we
    /// are pinned).
    pub async fn add_peer(&self, ticket: &str, enroll_token: Option<String>) -> Result<()> {
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
    pub fn add_pinned(&self, key: &str, label: &str) -> Result<()> {
        let endpoint_id: EndpointId = key.parse().context("invalid pinned key")?;
        if self.peers.contains_key(&endpoint_id) {
            return Ok(());
        }
        let peer = Peer::new(endpoint_id, Some(label.to_string()), false, None, None);
        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);
        info!("loaded pinned peer {} (\"{}\")", endpoint_id, label);
        Ok(())
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
    fn spawn_connection_loop(&self, peer: Arc<Peer>) {
        let endpoint = self.endpoint.clone();
        let host = self.host;
        let grants = self.grants.clone();
        tokio::spawn(async move {
            Self::peer_connection_loop(endpoint, peer, host, grants).await;
        });
    }

    /// Long-running task managing a peer's connection lifecycle.
    /// Runs the unified connection handler and reconnects with backoff on failure.
    async fn peer_connection_loop(
        endpoint: Endpoint,
        peer: Arc<Peer>,
        host: IpAddr,
        grants: Arc<RwLock<Grants>>,
    ) {
        let mut backoff = BACKOFF_INITIAL;

        loop {
            let has_conn = peer.connection.read().await.is_some();
            if has_conn {
                if let Err(e) = Self::run_connection(&peer, host, &grants).await {
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

                match endpoint.connect(peer.endpoint_id, ALPN).await {
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
                        Self::send_exposed_ports_to_peer(&peer, &grants).await;
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
    async fn run_connection(
        peer: &Arc<Peer>,
        host: IpAddr,
        grants: &Arc<RwLock<Grants>>,
    ) -> Result<()> {
        let conn = {
            let guard = peer.connection.read().await;
            guard.clone().ok_or_else(|| anyhow!("disconnected"))?
        };

        loop {
            tokio::select! {
                result = conn.accept_uni() => {
                    let recv = result?;
                    let peer = peer.clone();
                    tokio::spawn(async move {
                        Self::handle_uni_stream(recv, &peer).await;
                    });
                }
                result = conn.accept_bi() => {
                    let (send, recv) = result?;
                    let peer = peer.clone();
                    let grants = grants.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_bi_stream(host, &grants, &peer, send, recv).await {
                            error!("tunnel error: {}", e);
                        }
                    });
                }
            }
        }
    }

    async fn handle_uni_stream(mut recv: iroh::endpoint::RecvStream, peer: &Arc<Peer>) {
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
                Self::update_peer_ports(peer, ports).await;
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

    /// Update peer's exposed ports and manage bindings
    async fn update_peer_ports(peer: &Arc<Peer>, new_ports: Vec<u16>) {
        let old_ports = peer.exposed_ports.read().await.clone();

        // Stop bindings for removed ports
        for port in &old_ports {
            if !new_ports.contains(port) {
                if let Some((_, handle)) = peer.bindings.remove(port) {
                    handle.abort();
                    info!("removed binding for port {}", port);
                }
            }
        }

        // Create bindings for new ports
        for &port in &new_ports {
            if !old_ports.contains(&port) && !peer.bindings.contains_key(&port) {
                let peer_clone = peer.clone();
                let handle = tokio::spawn(async move {
                    if let Err(e) = tunnel::bind_port(port, &peer_clone).await {
                        error!("binding port {} failed: {}", port, e);
                    }
                });
                peer.bindings.insert(port, handle);
                info!("created binding for port {}", port);
            }
        }

        *peer.exposed_ports.write().await = new_ports;
    }

    /// Remove a peer by ticket
    pub async fn remove_peer(&self, ticket: &str) -> Result<()> {
        let endpoint_id: EndpointId = ticket.parse().context("invalid ticket")?;

        let (_, peer) = self
            .peers
            .remove(&endpoint_id)
            .ok_or_else(|| anyhow!("peer not found"))?;

        // Signal connection loop to exit
        peer.removed.store(true, Ordering::Relaxed);
        peer.conn_notify.notify_one();

        // Close connection
        if let Some(conn) = peer.connection.write().await.take() {
            conn.close(0u32.into(), b"removed");
        }

        // Abort all bindings
        for entry in peer.bindings.iter() {
            entry.value().abort();
        }

        // Drop its pin so it cannot reconnect as a known peer
        self.pins.remove(ticket)?;

        info!("removed peer {}", endpoint_id);
        Ok(())
    }

    /// Handle an incoming connection from a peer. Known peers (added by
    /// ticket or pinned) are reconnected; unknown peers must enroll with a
    /// valid token or are refused.
    pub async fn handle_connection(&self, conn: Connection) -> Result<()> {
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
    /// anything else -- no token, bad token, timeout -- closes the
    /// connection without announcing anything.
    async fn handle_enrollment(&self, conn: Connection) -> Result<Option<Arc<Peer>>> {
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
            Self::update_peer_ports(&peer, ports).await;
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
