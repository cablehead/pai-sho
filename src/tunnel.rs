//! Tunnel - TCP <-> QUIC forwarding.

use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

/// Bind the local listener for a forwarded port at `addr`. Kept separate from
/// the accept loop so a failed bind (address already in use) surfaces to the
/// caller before any binding is recorded.
pub async fn bind_listener(addr: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {}", addr))
}

/// Accept loop: forward each connection on `listener` to the peer's port
pub async fn serve_listener<P>(listener: TcpListener, port: u16, peer: &P) -> Result<()>
where
    P: PeerConnection + Send + Sync + 'static,
{
    match listener.local_addr() {
        Ok(addr) => info!("listening on {}", addr),
        Err(_) => info!("listening on port {}", port),
    }

    loop {
        let (stream, client_addr) = listener.accept().await?;
        stream.set_nodelay(true).ok();
        debug!("accepted connection from {} on port {}", client_addr, port);

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
    stream.set_nodelay(true).ok();

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
        let result = copy_flush(&mut tcp_read, &mut quic_send).await;
        let _ = quic_send.finish();
        result
    };

    let quic_to_tcp = async { copy_flush(&mut quic_recv, &mut tcp_write).await };

    tokio::select! {
        r = tcp_to_quic => { debug!("tcp->quic ended: {:?}", r); }
        r = quic_to_tcp => { debug!("quic->tcp ended: {:?}", r); }
    }

    Ok(())
}

/// Copy from `reader` to `writer`, flushing after every chunk. Pattern from
/// n0-computer/pigeons (https://github.com/n0-computer/pigeons), the iroh team's
/// SSH-over-iroh tool. It forces each piece onto the wire instead of letting a
/// writer coalesce. Measured against plain `tokio::io::copy` on a Mac-to-Hetzner
/// forward, this changed nothing: the wide-area hop is QUIC (no Nagle) and the
/// TCP sockets are on localhost, so there was no coalescing delay to remove. Kept
/// because it is cheap and starts to matter once a forwarded hop is not localhost.
pub async fn copy_flush<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(total);
        }
        writer.write_all(&buf[..n]).await?;
        writer.flush().await?;
        total += n as u64;
    }
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
