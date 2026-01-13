//! Peer management - connections, port announcements, auto-binding.

use crate::protocol::{BindingInfo, PeerInfo, PeerMessage, ALPN};
use crate::tunnel::{self, PeerConnection};
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Info about a connected peer
struct Peer {
    name: String,
    ip: Ipv4Addr,
    endpoint_id: EndpointId,
    connection: RwLock<Option<Connection>>,
    /// Ports this peer exposes
    exposed_ports: RwLock<Vec<u16>>,
    /// Active bindings (local port -> task handle)
    bindings: DashMap<u16, tokio::task::JoinHandle<()>>,
}

pub struct PeerManager {
    /// Peers by name
    peers: DashMap<String, Arc<Peer>>,
    /// Endpoint ID -> peer name mapping
    id_to_name: DashMap<EndpointId, String>,
}

impl PeerManager {
    pub fn new() -> Self {
        Self {
            peers: DashMap::new(),
            id_to_name: DashMap::new(),
        }
    }

    /// Add a new peer and connect to it
    pub async fn add_peer(
        &self,
        endpoint: &Endpoint,
        ticket: &str,
        name: String,
        ip: Ipv4Addr,
        _host: IpAddr,
    ) -> Result<()> {
        // Parse the ticket (endpoint ID)
        let endpoint_id: EndpointId = ticket.parse().context("invalid ticket")?;

        // Check for duplicates
        if self.peers.contains_key(&name) {
            return Err(anyhow!("peer '{}' already exists", name));
        }

        // Connect to the peer
        let conn = endpoint
            .connect(endpoint_id, ALPN)
            .await
            .context("failed to connect to peer")?;

        info!("connected to peer '{}' ({})", name, endpoint_id);

        let peer = Arc::new(Peer {
            name: name.clone(),
            ip,
            endpoint_id,
            connection: RwLock::new(Some(conn.clone())),
            exposed_ports: RwLock::new(Vec::new()),
            bindings: DashMap::new(),
        });

        self.peers.insert(name.clone(), peer.clone());
        self.id_to_name.insert(endpoint_id, name.clone());

        // Spawn task to handle incoming messages from this peer
        let peer_clone = peer.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::peer_recv_loop(peer_clone).await {
                warn!("peer recv loop ended: {}", e);
            }
        });

        Ok(())
    }

    /// Handle messages from a peer
    async fn peer_recv_loop(peer: Arc<Peer>) -> Result<()> {
        loop {
            let mut recv = {
                let conn_guard = peer.connection.read().await;
                let conn = conn_guard.as_ref().ok_or_else(|| anyhow!("disconnected"))?;
                conn.accept_uni().await?
            };

            let data = recv.read_to_end(64 * 1024).await?;
            let msg: PeerMessage = serde_json::from_slice(&data)?;

            match msg {
                PeerMessage::ExposedPorts(ports) => {
                    info!("peer '{}' exposed ports: {:?}", peer.name, ports);
                    Self::update_peer_ports(&peer, ports).await;
                }
                PeerMessage::Connect { port: _ } => {
                    warn!("unexpected Connect message on control stream");
                }
                PeerMessage::Error(e) => {
                    error!("peer error: {}", e);
                }
            }
        }
    }

    /// Update peer's exposed ports and manage bindings
    async fn update_peer_ports(peer: &Arc<Peer>, new_ports: Vec<u16>) {
        let old_ports = peer.exposed_ports.read().await.clone();

        // Stop bindings for removed ports
        for port in &old_ports {
            if !new_ports.contains(port) {
                if let Some((_, handle)) = peer.bindings.remove(port) {
                    handle.abort();
                    info!("removed binding {}:{}", peer.ip, port);
                }
            }
        }

        // Create bindings for new ports
        for &port in &new_ports {
            if !old_ports.contains(&port) && !peer.bindings.contains_key(&port) {
                let peer_clone = peer.clone();
                let handle = tokio::spawn(async move {
                    if let Err(e) = tunnel::bind_port(peer_clone.ip, port, &peer_clone).await {
                        error!("binding {}:{} failed: {}", peer_clone.ip, port, e);
                    }
                });
                peer.bindings.insert(port, handle);
                info!("created binding {}:{}", peer.ip, port);
            }
        }

        *peer.exposed_ports.write().await = new_ports;
    }

    /// Remove a peer
    pub async fn remove_peer(&self, name: &str) -> Result<()> {
        let (_, peer) = self
            .peers
            .remove(name)
            .ok_or_else(|| anyhow!("peer '{}' not found", name))?;

        self.id_to_name.remove(&peer.endpoint_id);

        // Close connection
        if let Some(conn) = peer.connection.write().await.take() {
            conn.close(0u32.into(), b"removed");
        }

        // Abort all bindings
        for entry in peer.bindings.iter() {
            entry.value().abort();
        }

        info!("removed peer '{}'", name);
        Ok(())
    }

    /// Handle an incoming connection from a peer
    pub async fn handle_connection(&self, conn: Connection, host: IpAddr) -> Result<()> {
        let remote_id = conn.remote_id()?;

        // Find the peer by endpoint ID
        let peer_name = self.id_to_name.get(&remote_id);

        if peer_name.is_none() {
            warn!("connection from unknown peer {}", remote_id);
            conn.close(1u32.into(), b"unknown peer");
            return Ok(());
        }

        let peer_name = peer_name.unwrap().clone();
        let peer = self.peers.get(&peer_name).unwrap().clone();

        // Update connection
        *peer.connection.write().await = Some(conn.clone());
        info!("peer '{}' reconnected", peer_name);

        // Handle tunnel requests (bidirectional streams)
        loop {
            let stream = conn.accept_bi().await;
            match stream {
                Ok((send, mut recv)) => {
                    // Read the port request
                    let mut buf = [0u8; 2];
                    recv.read_exact(&mut buf).await?;
                    let port = u16::from_be_bytes(buf);

                    info!("tunnel request from '{}' for port {}", peer_name, port);

                    // Spawn tunnel handler
                    tokio::spawn(async move {
                        if let Err(e) = tunnel::handle_tunnel(host, port, send, recv).await {
                            error!("tunnel error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    warn!("accept_bi failed: {}", e);
                    break;
                }
            }
        }

        Ok(())
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
                            warn!("failed to send to '{}': {}", peer.name, e);
                        }
                        let _ = send.finish();
                    }
                    Err(e) => {
                        warn!("failed to open stream to '{}': {}", peer.name, e);
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
            let connected = peer.connection.read().await.is_some();
            result.push(PeerInfo {
                name: peer.name.clone(),
                ip: peer.ip,
                connected,
                exposed_ports: peer.exposed_ports.read().await.clone(),
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
                    local_addr: format!("{}:{}", peer.ip, binding.key()),
                    peer_name: peer.name.clone(),
                    peer_port: *binding.key(),
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
