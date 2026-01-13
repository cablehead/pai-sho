//! Daemon - manages iroh endpoint, peers, and tunnels.

use crate::peer::PeerManager;
use crate::protocol::{ListInfo, Request, Response, ALPN};
use anyhow::{Context, Result};
use iroh::{Endpoint, SecretKey};
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
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
    pub async fn new(host: IpAddr, data_dir: Option<PathBuf>) -> Result<Arc<Self>> {
        let secret_key = load_or_create_key(data_dir)?;

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
        self.peers.handle_connection(conn, self.host).await
    }

    /// Handle a request from the CLI client
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        match request {
            Request::AddPeer { ticket, name, ip } => {
                match self
                    .peers
                    .add_peer(&self.endpoint, &ticket, name, ip, self.host)
                    .await
                {
                    Ok(()) => {
                        // Send our exposed ports to the new peer
                        let ports = self.get_exposed_ports().await;
                        self.peers.broadcast_exposed_ports(ports).await;
                        Response::Ok
                    }
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::RemovePeer { name } => match self.peers.remove_peer(&name).await {
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

/// Load or create a persistent secret key
fn load_or_create_key(data_dir: Option<PathBuf>) -> Result<SecretKey> {
    let data_dir = match data_dir {
        Some(dir) => dir,
        None => {
            let home = std::env::var("HOME").context("HOME not set")?;
            PathBuf::from(home).join(".pai-sho")
        }
    };

    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create data dir: {}", data_dir.display()))?;

    let key_path = data_dir.join("secret.key");

    if key_path.exists() {
        let key_hex = std::fs::read_to_string(&key_path)
            .with_context(|| format!("failed to read key from {}", key_path.display()))?;
        let key_hex = key_hex.trim();
        let key_bytes: [u8; 32] = hex::decode(key_hex)
            .context("invalid key hex")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid key length"))?;
        info!("loaded existing secret key from {}", key_path.display());
        Ok(SecretKey::from_bytes(&key_bytes))
    } else {
        let secret_key = SecretKey::generate(&mut rand::rng());
        let key_hex = hex::encode(secret_key.to_bytes());
        std::fs::write(&key_path, &key_hex)
            .with_context(|| format!("failed to write key to {}", key_path.display()))?;
        info!("created new secret key at {}", key_path.display());
        Ok(secret_key)
    }
}

/// Run the daemon
pub async fn run(host: IpAddr, socket_path: &Path, data_dir: Option<PathBuf>) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    let daemon = Daemon::new(host, data_dir).await?;

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
