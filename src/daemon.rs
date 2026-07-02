//! Daemon - manages iroh endpoint, peers, and tunnels.

use crate::peer::PeerManager;
use crate::protocol::{ListInfo, Request, Response, ALPN};
use anyhow::{Context, Result};
use iroh::{Endpoint, SecretKey};
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tracing::{error, info};

pub struct Daemon {
    /// The iroh endpoint
    endpoint: Endpoint,
    /// Ports we expose to peers (shared with PeerManager for reconnect re-announce)
    exposed_ports: Arc<RwLock<HashSet<u16>>>,
    /// Connected peers
    peers: PeerManager,
}

impl Daemon {
    pub async fn new(host: IpAddr) -> Result<Arc<Self>> {
        let secret_key = SecretKey::generate(&mut rand::rng());

        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .context("failed to create iroh endpoint")?;

        let exposed_ports = Arc::new(RwLock::new(HashSet::new()));

        Ok(Arc::new(Self {
            peers: PeerManager::new(endpoint.clone(), host, exposed_ports.clone()),
            endpoint,
            exposed_ports,
        }))
    }

    pub fn ticket(&self) -> String {
        // TODO: proper ticket serialization
        self.endpoint.id().to_string()
    }

    pub async fn expose(&self, port: u16) -> Result<()> {
        self.exposed_ports.write().await.insert(port);
        self.peers
            .broadcast_exposed_ports(self.get_exposed_ports().await)
            .await;
        info!("exposed port {}", port);
        Ok(())
    }

    pub async fn unexpose(&self, port: u16) -> Result<()> {
        self.exposed_ports.write().await.remove(&port);
        self.peers
            .broadcast_exposed_ports(self.get_exposed_ports().await)
            .await;
        info!("unexposed port {}", port);
        Ok(())
    }

    pub async fn get_exposed_ports(&self) -> Vec<u16> {
        self.exposed_ports.read().await.iter().copied().collect()
    }

    pub async fn list(&self) -> ListInfo {
        ListInfo {
            me: self.endpoint.id().to_string(),
            peers: self.peers.list().await,
            i_expose: self.get_exposed_ports().await,
            bindings: self.peers.list_bindings().await,
        }
    }

    /// Accept incoming peer connections
    pub async fn accept_loop(self: Arc<Self>) {
        loop {
            match self.endpoint.accept().await {
                Some(incoming) => {
                    let this = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = this.handle_incoming(incoming).await {
                            error!("error handling incoming connection: {}", e);
                        }
                    });
                }
                None => {
                    info!("endpoint closed");
                    break;
                }
            }
        }
    }

    async fn handle_incoming(&self, incoming: iroh::endpoint::Incoming) -> Result<()> {
        let conn = incoming.accept()?.await?;
        self.peers.handle_connection(conn).await
    }

    /// Handle a request from the CLI client
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        match request {
            Request::AddPeer { ticket } => match self.peers.add_peer(&ticket).await {
                Ok(()) => {
                    // Send our exposed ports to the new peer
                    let ports = self.get_exposed_ports().await;
                    self.peers.broadcast_exposed_ports(ports).await;
                    Response::Ok
                }
                Err(e) => Response::Error(e.to_string()),
            },
            Request::RemovePeer { ticket } => match self.peers.remove_peer(&ticket).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Expose { port } => match self.expose(port).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Unexpose { port } => match self.unexpose(port).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::List => Response::List(self.list().await),
            Request::Ticket => Response::Ticket(self.ticket()),
        }
    }
}

/// Run the daemon
pub async fn run(
    host: IpAddr,
    socket_path: &Path,
    peers: Vec<String>,
    ports: Vec<u16>,
) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    let daemon = Daemon::new(host).await?;

    println!("Ticket: {}", daemon.ticket());
    info!("daemon started, host={}", host);

    // Expose ports specified on command line
    for port in ports {
        daemon.expose(port).await?;
    }

    // Add peers specified on command line
    for ticket in &peers {
        match daemon.peers.add_peer(ticket).await {
            Ok(()) => {
                info!("added peer {}", ticket);
            }
            Err(e) => {
                error!("failed to add peer {}: {}", ticket, e);
            }
        }
    }

    // Broadcast exposed ports to newly added peers
    if !peers.is_empty() {
        let ports = daemon.get_exposed_ports().await;
        daemon.peers.broadcast_exposed_ports(ports).await;
    }

    // Start accepting peer connections
    let accept_daemon = daemon.clone();
    tokio::spawn(async move {
        accept_daemon.accept_loop().await;
    });

    // Listen for CLI commands on Unix socket
    let listener = UnixListener::bind(socket_path).context("failed to bind Unix socket")?;

    info!("listening on {:?}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, daemon).await {
                error!("client error: {}", e);
            }
        });
    }
}

async fn handle_client(stream: UnixStream, daemon: Arc<Daemon>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    reader.read_line(&mut line).await?;
    let request: Request = serde_json::from_str(&line)?;

    let response = daemon.handle_request(request).await;
    let response_json = serde_json::to_string(&response)?;

    writer.write_all(response_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    Ok(())
}
