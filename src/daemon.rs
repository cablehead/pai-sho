//! Daemon - manages iroh endpoint, peers, and tunnels.

use crate::enroll::{Pins, Tokens};
use crate::grants::Grants;
use crate::peer::PeerManager;
use crate::protocol::{GrantInfo, ListInfo, Request, Response, ALPN};
use crate::surface::SurfaceStore;
use anyhow::{anyhow, Context, Result};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
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

    let key = SecretKey::generate();

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
    /// Build a daemon on a production endpoint: the n0 preset, which uses
    /// public relays and n0 DNS discovery.
    pub async fn new(
        host: IpAddr,
        key_path: &Path,
        netstack: Option<crate::netstack::NetStack>,
        name: Option<String>,
    ) -> Result<Arc<Self>> {
        let secret_key = load_or_create_key(key_path)?;

        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .context("failed to create iroh endpoint")?;

        Self::with_endpoint(endpoint, host, key_path, netstack, name).await
    }

    /// Build a daemon on an endpoint the caller already bound. Tests use this
    /// to supply a loopback endpoint with relays disabled, so no code path
    /// reaches the network. `state_prefix` is where pins and surfaces are
    /// persisted: `<prefix>.peers.json` and `<prefix>.surfaces.json`.
    pub async fn with_endpoint(
        endpoint: Endpoint,
        host: IpAddr,
        state_prefix: &Path,
        netstack: Option<crate::netstack::NetStack>,
        name: Option<String>,
    ) -> Result<Arc<Self>> {
        let key_path = state_prefix;
        let grants = Arc::new(RwLock::new(Grants::default()));
        let tokens = Arc::new(Tokens::default());

        // Pins and surfaces live next to the key: <key>.peers.json,
        // <key>.surfaces.json
        let pins = Pins::new(PathBuf::from(format!("{}.peers.json", key_path.display())));
        let surfaces = SurfaceStore::new(PathBuf::from(format!(
            "{}.surfaces.json",
            key_path.display()
        )));
        let pinned = pins.load()?;

        let daemon = Arc::new(Self {
            peers: Arc::new(PeerManager::new(
                endpoint.clone(),
                host,
                grants.clone(),
                tokens.clone(),
                pins,
                surfaces,
                netstack,
                name,
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

        // Restore projected surfaces onto the pinned peers just loaded.
        daemon.peers.restore_surfaces().await;

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
            Request::Pin { key, label } => match self.peers.pin_peer(&key, &label) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Project { peer, ip, name } => {
                let ip = match ip.map(|s| s.parse()).transpose() {
                    Ok(ip) => ip,
                    Err(_) => return Response::Error("invalid --ip address".to_string()),
                };
                match self.peers.project(&peer, ip, name).await {
                    Ok(()) => Response::Ok,
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Unproject { peer } => match self.peers.unproject(&peer).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            },
            Request::Surfaces => Response::Surfaces(self.peers.surfaces().await),
        }
    }
}

/// Run the daemon
/// Routes incoming pai-sho connections to the peer manager. Handed to iroh's
/// `Router`, which runs the accept loop and drains connections on shutdown.
/// `handle_connection` installs the connection into its `Peer` and returns; the
/// peer's own loop drives it, and iroh keeps the connection alive because that
/// stored clone outlives this handler's copy.
#[derive(Clone)]
struct PaiShoProtocol {
    peers: Arc<PeerManager>,
}

impl std::fmt::Debug for PaiShoProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaiShoProtocol")
    }
}

impl ProtocolHandler for PaiShoProtocol {
    async fn accept(&self, conn: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        if let Err(e) = self.peers.handle_connection(conn).await {
            warn!("error handling incoming connection: {}", e);
        }
        Ok(())
    }
}

/// The TUN backend's reserved resolver address (answers `.pai-sho` on :53 in-stack).
const TUN_RESOLVER_IP: std::net::Ipv4Addr = std::net::Ipv4Addr::new(10, 99, 0, 53);

#[allow(clippy::too_many_arguments)]
/// Chown the control socket to `user`, so a root daemon can hand it to the
/// logged-in user. Resolves the username to uid/gid via getpwnam.
fn set_socket_owner(path: &Path, user: &str) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cuser = CString::new(user).context("invalid --socket-owner")?;
    let pw = unsafe { libc::getpwnam(cuser.as_ptr()) };
    anyhow::ensure!(!pw.is_null(), "unknown --socket-owner user: {}", user);
    let (uid, gid) = unsafe { ((*pw).pw_uid, (*pw).pw_gid) };
    let cpath = CString::new(path.as_os_str().as_bytes()).context("bad socket path")?;
    let r = unsafe { libc::chown(cpath.as_ptr(), uid, gid) };
    anyhow::ensure!(
        r == 0,
        "chown {:?} to {}: {}",
        path,
        user,
        std::io::Error::last_os_error()
    );
    Ok(())
}

/// Chmod the control socket to an octal `mode` string (e.g. "660").
fn set_socket_mode(path: &Path, mode: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let m = u32::from_str_radix(mode, 8)
        .with_context(|| format!("invalid --socket-mode (octal): {}", mode))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(m))
        .with_context(|| format!("chmod {:?}", path))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    host: IpAddr,
    socket_path: &Path,
    peers: Vec<String>,
    ports: Vec<u16>,
    key_path: Option<PathBuf>,
    enroll: Option<String>,
    resolver: Option<std::net::SocketAddr>,
    tun: Option<String>,
    socket_owner: Option<String>,
    socket_mode: Option<String>,
    name: Option<String>,
) -> Result<()> {
    // Clean up old socket
    let _ = std::fs::remove_file(socket_path);

    // Bring up the TUN owned-network backend if requested. Its stream of
    // accepted connections is spliced onto peer tunnels below.
    let netstack = match &tun {
        Some(dev) => match crate::netstack::spawn(dev, TUN_RESOLVER_IP) {
            Ok((ns, accepts)) => {
                info!(
                    "tun backend up on {} (resolver {}:53)",
                    dev, TUN_RESOLVER_IP
                );
                Some((ns, accepts))
            }
            Err(e) => {
                error!("tun backend failed on {}: {}", dev, e);
                None
            }
        },
        None => None,
    };
    let ns_handle = netstack.as_ref().map(|(ns, _)| ns.clone());

    // Self-name: <name>.pai-sho resolves to where our own services are (the
    // --host forward address), so local traffic uses the same origin peers do
    // (CORS). On the TUN backend the in-stack resolver reads the name map
    // directly; in loopback mode PeerManager::resolve_name handles it.
    if let (Some(ns), Some(n), IpAddr::V4(h)) = (ns_handle.as_ref(), name.as_ref(), host) {
        ns.add_surface(h, Some(n.clone()));
    }

    let key_path = key_path.unwrap_or_else(default_key_path);
    let daemon = Daemon::new(host, &key_path, ns_handle, name).await?;

    println!("Ticket: {}", daemon.ticket());
    info!("daemon started, host={}, key={}", host, key_path.display());

    // Splice accepted TUN connections onto peer tunnels.
    if let Some((_, accepts)) = netstack {
        let peers = daemon.peers.clone();
        tokio::spawn(async move { peers.run_netstack_accepts(accepts).await });
    }

    // Serve the loopback owned resolver, if requested (independent of --tun,
    // which serves its own resolver in-stack).
    if let Some(listen) = resolver {
        let peers = daemon.peers.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::resolver::run(listen, peers).await {
                error!("resolver stopped: {}", e);
            }
        });
    }

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

    // Accept peer connections via iroh's Router: it runs the accept loop, routes
    // the ALPN to the peer manager, and drains in-flight connections on shutdown.
    let router = Router::builder(daemon.endpoint.clone())
        .accept(
            ALPN,
            PaiShoProtocol {
                peers: daemon.peers.clone(),
            },
        )
        .spawn();

    // Listen for CLI commands on Unix socket
    let listener = UnixListener::bind(socket_path).context("failed to bind Unix socket")?;

    // Set socket ownership/mode before the accept loop starts, so a root daemon
    // (launchd) can hand the socket to the logged-in user with no window where
    // the socket is accept-ready but still root-owned. No poll, no race.
    if let Some(owner) = &socket_owner {
        set_socket_owner(socket_path, owner)?;
        info!("socket {:?} owned by {}", socket_path, owner);
    }
    if let Some(mode) = &socket_mode {
        set_socket_mode(socket_path, mode)?;
        info!("socket {:?} mode {}", socket_path, mode);
    }

    info!("listening on {:?}", socket_path);

    // Serve CLI commands until asked to stop. Racing the accept loop against a
    // shutdown signal lets a supervisor (launchd, brew services) stop the daemon
    // cleanly; on exit the TUN fd closes and the kernel removes the utun and its
    // route. See cablehead/xs#150, cablehead/http-nu#53.
    let socket_loop = async {
        loop {
            let (stream, _) = listener.accept().await?;
            let daemon = daemon.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_client(stream, daemon).await {
                    error!("client error: {}", e);
                }
            });
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };

    tokio::select! {
        r = socket_loop => r?,
        _ = shutdown_signal() => info!("shutdown signal received, stopping"),
    }

    // Drain in-flight connections before exit.
    router.shutdown().await.ok();
    Ok(())
}

/// Resolve when the process is asked to terminate. Handles both SIGINT (Ctrl-C)
/// and SIGTERM: supervisors (launchd, brew services, systemd) send SIGTERM, so
/// a daemon that traps only SIGINT is hard-killed and skips orderly shutdown.
/// Modeled on cablehead/xs#150 and cablehead/http-nu#53.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
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
