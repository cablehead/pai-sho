//! Daemon - manages iroh endpoint, peers, and tunnels.

use crate::peer::PeerManager;
use crate::protocol::{ListInfo, Request, Response, ALPN};
use anyhow::{anyhow, Context, Result};
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
    /// Ports we expose to peers (shared with PeerManager for reconnect re-announce)
    exposed_ports: Arc<RwLock<HashSet<u16>>>,
    /// Connected peers
    peers: PeerManager,
}

/// Default key location: $XDG_STATE_HOME/pai-sho/key (~/.local/state/pai-sho/key)
fn default_key_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("pai-sho").join("key")
}

/// Load the secret key from `path`, or generate one and persist it there.
/// The key file is 32 raw bytes, created with mode 0600.
fn load_or_create_key(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read key file {}", path.display()))?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("key file {} is not 32 bytes", path.display()))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }

    let key = SecretKey::generate(&mut rand::rng());

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to create key file {}", path.display()))?;
    file.write_all(&key.to_bytes())
        .with_context(|| format!("failed to write key file {}", path.display()))?;

    info!("generated new key at {}", path.display());
    Ok(key)
}

impl Daemon {
    pub async fn new(host: IpAddr, key_path: &Path) -> Result<Arc<Self>> {
        let secret_key = load_or_create_key(key_path)?;

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
    key_path: Option<PathBuf>,
) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    let key_path = key_path.unwrap_or_else(default_key_path);
    let daemon = Daemon::new(host, &key_path).await?;

    println!("Ticket: {}", daemon.ticket());
    info!("daemon started, host={}, key={}", host, key_path.display());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_persists_across_loads() {
        let dir = std::env::temp_dir().join(format!("pai-sho-key-test-{}", std::process::id()));
        let path = dir.join("key");

        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first.to_bytes(), second.to_bytes());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_malformed_key_file() {
        let dir = std::env::temp_dir().join(format!("pai-sho-badkey-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key");
        std::fs::write(&path, b"too short").unwrap();

        assert!(load_or_create_key(&path).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
