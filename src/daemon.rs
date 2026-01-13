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
    /// Host to forward exposed ports to
    host: IpAddr,
    /// Ports we expose to peers
    exposed_ports: RwLock<HashSet<u16>>,
    /// Connected peers
    peers: PeerManager,
}

impl Daemon {
    pub async fn new(host: IpAddr) -> Result<Arc<Self>> {
        let secret_key = SecretKey::generate(&mut rand::rng());

        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .context("failed to create iroh endpoint")?;

        Ok(Arc::new(Self {
            endpoint,
            host,
            exposed_ports: RwLock::new(HashSet::new()),
            peers: PeerManager::new(),
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
            peers: self.peers.list().await,
            exposed_ports: self.get_exposed_ports().await,
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
        let remote_id = conn.remote_id()?;
        info!("incoming connection from {}", remote_id);

        // Handle peer protocol (port announcements, tunnel requests)
        let exposed = self.get_exposed_ports().await;
        self.peers.handle_connection(conn, self.host, exposed).await
    }

    /// Handle a request from the CLI client
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        match request {
            Request::AddPeer { ticket } => match self.peers.add_peer(&self.endpoint, &ticket).await
            {
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
pub async fn run(host: IpAddr, socket_path: &Path) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    let daemon = Daemon::new(host).await?;

    println!("Ticket: {}", daemon.ticket());
    info!("daemon started, host={}", host);

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
