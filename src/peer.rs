//! Peer management - connections, port announcements, auto-binding, reconnection.

use crate::protocol::{BindingInfo, PeerInfo, PeerMessage, ALPN};
use crate::tunnel::{self, PeerConnection};
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tracing::{error, info, warn};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Info about a connected peer
struct Peer {
    endpoint_id: EndpointId,
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

pub struct PeerManager {
    /// Peers by endpoint ID
    peers: DashMap<EndpointId, Arc<Peer>>,
    /// Our endpoint (for outbound reconnection)
    endpoint: Endpoint,
    /// Shared exposed ports (for re-announcing after reconnect)
    exposed_ports: Arc<RwLock<HashSet<u16>>>,
    /// Host address for forwarding tunnel requests
    host: IpAddr,
}

impl PeerManager {
    pub fn new(endpoint: Endpoint, host: IpAddr, exposed_ports: Arc<RwLock<HashSet<u16>>>) -> Self {
        Self {
            peers: DashMap::new(),
            endpoint,
            host,
            exposed_ports,
        }
    }

    /// Add a new peer and connect to it
    pub async fn add_peer(&self, ticket: &str) -> Result<()> {
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

        let peer = Arc::new(Peer {
            endpoint_id,
            connection: RwLock::new(Some(conn)),
            exposed_ports: RwLock::new(Vec::new()),
            bindings: DashMap::new(),
            conn_notify: Notify::new(),
            removed: AtomicBool::new(false),
        });

        self.peers.insert(endpoint_id, peer.clone());
        self.spawn_connection_loop(peer);

        Ok(())
    }

    /// Spawn the connection management loop for a peer
    fn spawn_connection_loop(&self, peer: Arc<Peer>) {
        let endpoint = self.endpoint.clone();
        let host = self.host;
        let exposed_ports = self.exposed_ports.clone();
        tokio::spawn(async move {
            Self::peer_connection_loop(endpoint, peer, host, exposed_ports).await;
        });
    }

    /// Long-running task managing a peer's connection lifecycle.
    /// Runs the unified connection handler and reconnects with backoff on failure.
    async fn peer_connection_loop(
        endpoint: Endpoint,
        peer: Arc<Peer>,
        host: IpAddr,
        exposed_ports: Arc<RwLock<HashSet<u16>>>,
    ) {
        let mut backoff = BACKOFF_INITIAL;

        loop {
            if let Err(e) = Self::run_connection(&peer, host).await {
                if peer.removed.load(Ordering::Relaxed) {
                    return;
                }
                warn!("{} disconnected: {}", peer.endpoint_id, e);
            }

            if peer.removed.load(Ordering::Relaxed) {
                return;
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
                        *peer.connection.write().await = Some(conn);
                        Self::send_exposed_ports_to_peer(&peer, &exposed_ports).await;
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
    async fn run_connection(peer: &Arc<Peer>, host: IpAddr) -> Result<()> {
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
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_bi_stream(host, send, recv).await {
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
            PeerMessage::Error(e) => {
                error!("peer error: {}", e);
            }
        }
    }

    async fn handle_bi_stream(
        host: IpAddr,
        send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
    ) -> Result<()> {
        let mut buf = [0u8; 2];
        recv.read_exact(&mut buf).await?;
        let port = u16::from_be_bytes(buf);
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

        info!("removed peer {}", endpoint_id);
        Ok(())
    }

    /// Handle an incoming connection from a peer
    pub async fn handle_connection(&self, conn: Connection) -> Result<()> {
        let remote_id = conn.remote_id();

        if let Some(peer) = self.peers.get(&remote_id) {
            // Known peer reconnecting -- close old connection, install new one
            let mut conn_guard = peer.connection.write().await;
            if let Some(old_conn) = conn_guard.take() {
                old_conn.close(0u32.into(), b"replaced");
            }
            *conn_guard = Some(conn.clone());
            drop(conn_guard);

            peer.conn_notify.notify_one();
            info!("{} reconnected", remote_id);
        } else {
            // New incoming peer
            info!("accepted connection from {}", remote_id);

            let peer = Arc::new(Peer {
                endpoint_id: remote_id,
                connection: RwLock::new(Some(conn.clone())),
                exposed_ports: RwLock::new(Vec::new()),
                bindings: DashMap::new(),
                conn_notify: Notify::new(),
                removed: AtomicBool::new(false),
            });

            self.peers.insert(remote_id, peer.clone());
            self.spawn_connection_loop(peer);
        }

        // Send our exposed ports to this peer
        let ports: Vec<u16> = self.exposed_ports.read().await.iter().copied().collect();
        if !ports.is_empty() {
            let msg = PeerMessage::ExposedPorts(ports);
            let data = serde_json::to_vec(&msg).unwrap();
            match conn.open_uni().await {
                Ok(mut send) => {
                    if let Err(e) = send.write_all(&data).await {
                        warn!("failed to send exposed ports to {}: {}", remote_id, e);
                    }
                    let _ = send.finish();
                }
                Err(e) => {
                    warn!("failed to open stream to {}: {}", remote_id, e);
                }
            }
        }

        Ok(())
    }

    /// Send our exposed ports to a specific peer
    async fn send_exposed_ports_to_peer(peer: &Peer, exposed_ports: &Arc<RwLock<HashSet<u16>>>) {
        let ports: Vec<u16> = exposed_ports.read().await.iter().copied().collect();
        if ports.is_empty() {
            return;
        }

        let msg = PeerMessage::ExposedPorts(ports);
        let data = serde_json::to_vec(&msg).unwrap();

        let conn = peer.connection.read().await;
        if let Some(conn) = conn.as_ref() {
            match conn.open_uni().await {
                Ok(mut send) => {
                    if let Err(e) = send.write_all(&data).await {
                        warn!("failed to send ports to {}: {}", peer.endpoint_id, e);
                    }
                    let _ = send.finish();
                }
                Err(e) => {
                    warn!("failed to open stream to {}: {}", peer.endpoint_id, e);
                }
            }
        }
    }

    /// Broadcast our exposed ports to all connected peers
    pub async fn broadcast_exposed_ports(&self, ports: Vec<u16>) {
        let msg = PeerMessage::ExposedPorts(ports);
        let data = serde_json::to_vec(&msg).unwrap();

        for entry in self.peers.iter() {
            let peer = entry.value();
            let conn = peer.connection.read().await;
            if let Some(conn) = conn.as_ref() {
                match conn.open_uni().await {
                    Ok(mut send) => {
                        if let Err(e) = send.write_all(&data).await {
                            warn!("failed to send to {}: {}", peer.endpoint_id, e);
                        }
                        let _ = send.finish();
                    }
                    Err(e) => {
                        warn!("failed to open stream to {}: {}", peer.endpoint_id, e);
                    }
                }
            }
        }
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
