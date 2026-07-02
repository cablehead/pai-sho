//! Daemon - manages iroh endpoint, peers, and tunnels.

use crate::enroll::{Pins, Tokens};
use crate::grants::Grants;
use crate::peer::PeerManager;
use crate::protocol::{GrantInfo, ListInfo, Request, Response, ALPN};
use anyhow::{anyhow, Context, Result};
use iroh::{Endpoint, EndpointId, SecretKey};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub struct Daemon {
    /// The iroh endpoint
    endpoint: Endpoint,
    /// Directed grants: which port is exposed to which peer
    grants: Arc<RwLock<Grants>>,
    /// Connected peers
    peers: Arc<PeerManager>,
    /// Enrollment tokens minted by grant-token
    tokens: Arc<Tokens>,
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

        let grants = Arc::new(RwLock::new(Grants::default()));
        let tokens = Arc::new(Tokens::default());

        // Pins live next to the key: <key>.peers.json
        let pins = Pins::new(PathBuf::from(format!("{}.peers.json", key_path.display())));
        let pinned = pins.load()?;

        let daemon = Arc::new(Self {
            peers: Arc::new(PeerManager::new(
                endpoint.clone(),
                host,
                grants.clone(),
                tokens.clone(),
                pins,
            )),
            endpoint,
            grants,
            tokens,
        });

        for pin in pinned {
            if let Err(e) = daemon.peers.add_pinned(&pin.key, &pin.label) {
                error!("failed to load pinned peer {}: {}", pin.key, e);
            }
        }

        Ok(daemon)
    }

    pub fn ticket(&self) -> String {
        // TODO: proper ticket serialization
        self.endpoint.id().to_string()
    }

    /// Grant `port` to each peer in `to` and re-announce
    pub async fn expose(&self, port: u16, to: &[EndpointId]) -> Result<()> {
        {
            let mut grants = self.grants.write().await;
            for grantee in to {
                grants.add(port, *grantee);
            }
        }
        self.peers.broadcast_grants().await;
        info!("exposed port {} to {} peer(s)", port, to.len());
        Ok(())
    }

    /// Revoke grants for `port` (all of them, or just `to`) and re-announce
    pub async fn unexpose(&self, port: u16, to: Option<EndpointId>) -> Result<()> {
        self.grants.write().await.remove(port, to);
        self.peers.broadcast_grants().await;
        info!("unexposed port {}", port);
        Ok(())
    }

    pub async fn list(&self) -> ListInfo {
        let grants = self.grants.read().await;
        ListInfo {
            me: self.endpoint.id().to_string(),
            peers: self.peers.list().await,
            i_expose: grants.ports(),
            grants: grants
                .all()
                .into_iter()
                .map(|(port, to)| GrantInfo {
                    port,
                    to: to.to_string(),
                })
                .collect(),
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
            Request::AddPeer { ticket } => match self.peers.add_peer(&ticket, None).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::RemovePeer { ticket } => match self.peers.remove_peer(&ticket).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Expose { port, to } => {
                // Explicit grantees, or every currently known peer
                let grantees: Result<Vec<EndpointId>> = if to.is_empty() {
                    let ids = self.peers.peer_ids();
                    if ids.is_empty() {
                        Err(anyhow!("no peers to grant to; use --to <key>"))
                    } else {
                        Ok(ids)
                    }
                } else {
                    to.iter()
                        .map(|k| k.parse().context("invalid peer key"))
                        .collect()
                };
                match grantees {
                    Ok(grantees) => match self.expose(port, &grantees).await {
                        Ok(()) => Response::Ok,
                        Err(e) => Response::Error(e.to_string()),
                    },
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Unexpose { port, to } => {
                let grantee: Result<Option<EndpointId>> = to
                    .map(|k| k.parse().context("invalid peer key"))
                    .transpose();
                match grantee {
                    Ok(grantee) => match self.unexpose(port, grantee).await {
                        Ok(()) => Response::Ok,
                        Err(e) => Response::Error(e.to_string()),
                    },
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::List => Response::List(self.list().await),
            Request::Ticket => Response::Ticket(self.ticket()),
            Request::GrantToken { label } => Response::Token(self.tokens.mint(label)),
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
    enroll: Option<String>,
) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    let key_path = key_path.unwrap_or_else(default_key_path);
    let daemon = Daemon::new(host, &key_path).await?;

    println!("Ticket: {}", daemon.ticket());
    info!("daemon started, host={}, key={}", host, key_path.display());

    // -e ports are granted to the -a peers: expose these ports to those
    // peers, and to no one else
    let grantees: Vec<EndpointId> = peers.iter().filter_map(|t| t.parse().ok()).collect();
    if !ports.is_empty() && grantees.is_empty() {
        warn!("-e given without -a: ports are granted to no one; use expose --to");
    }
    for &port in &ports {
        daemon.expose(port, &grantees).await?;
    }

    // Add peers specified on command line, presenting the enroll token if given
    for ticket in &peers {
        match daemon.peers.add_peer(ticket, enroll.clone()).await {
            Ok(()) => {
                info!("added peer {}", ticket);
            }
            Err(e) => {
                error!("failed to add peer {}: {}", ticket, e);
            }
        }
    }

    // Announce grants to the newly added peers
    if !peers.is_empty() {
        daemon.peers.broadcast_grants().await;
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
