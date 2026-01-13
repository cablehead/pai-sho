//! Tunnel - TCP <-> QUIC forwarding.

use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

const LOCALHOST: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// Bind a local port and forward connections to a peer's port
pub async fn bind_port<P>(port: u16, peer: &P) -> Result<()>
where
    P: PeerConnection + Send + Sync + 'static,
{
    let addr = SocketAddr::from((LOCALHOST, port));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {}", addr))?;

    info!("listening on {}", addr);

    loop {
        let (stream, client_addr) = listener.accept().await?;
        debug!("accepted connection from {} on {}", client_addr, addr);

        // Open connection to peer for this port
        match peer.open_tunnel(port).await {
            Ok((send, recv)) => {
                tokio::spawn(async move {
                    if let Err(e) = forward_bidirectional(stream, send, recv).await {
                        error!("tunnel error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("failed to open tunnel to peer for port {}: {}", port, e);
            }
        }
    }
}

/// Handle an incoming tunnel request - forward to local service
pub async fn handle_tunnel(
    host: IpAddr,
    port: u16,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
) -> Result<()> {
    let addr = SocketAddr::from((host, port));
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {}", addr))?;

    forward_bidirectional(stream, send, recv).await
}

/// Bidirectional forwarding between TCP and QUIC streams
async fn forward_bidirectional(
    tcp: TcpStream,
    mut quic_send: iroh::endpoint::SendStream,
    mut quic_recv: iroh::endpoint::RecvStream,
) -> Result<()> {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    let tcp_to_quic = async {
        let result = tokio::io::copy(&mut tcp_read, &mut quic_send).await;
        let _ = quic_send.finish();
        result
    };

    let quic_to_tcp = async { tokio::io::copy(&mut quic_recv, &mut tcp_write).await };

    tokio::select! {
        r = tcp_to_quic => { debug!("tcp->quic ended: {:?}", r); }
        r = quic_to_tcp => { debug!("quic->tcp ended: {:?}", r); }
    }

    Ok(())
}

/// Trait for opening tunnels to a peer
pub trait PeerConnection: Send + Sync {
    fn open_tunnel(
        &self,
        port: u16,
    ) -> impl std::future::Future<
        Output = Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)>,
    > + Send;
}
