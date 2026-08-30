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
    P: PeerConnection + Clone + Send + Sync + 'static,
{
    match listener.local_addr() {
        Ok(addr) => info!("listening on {}", addr),
        Err(_) => info!("listening on port {}", port),
    }

    loop {
        let (stream, client_addr) = listener.accept().await?;
        stream.set_nodelay(true).ok();
        debug!("accepted connection from {} on port {}", client_addr, port);

        // Open the QUIC stream off the accept loop so the next client is not
        // queued behind someone else's handshake, and this socket can start
        // buffering in the kernel immediately.
        let peer = peer.clone();
        tokio::spawn(async move {
            match peer.open_tunnel(port).await {
                Ok((send, recv)) => {
                    if let Err(e) = forward_bidirectional(stream, send, recv).await {
                        error!("tunnel error: {}", e);
                    }
                }
                Err(e) => {
                    error!("failed to open tunnel to peer for port {}: {}", port, e);
                }
            }
        });
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

/// Copy from `reader` to `writer` one read at a time.
///
/// Do not flush after each chunk. Kernel `TcpStream` and iroh `SendStream`
/// treat flush as a no-op, so it did nothing on the loopback path. On the TUN
/// path, tokio-smoltcp's flush waits until the TCP peer ACKs the tx buffer.
/// That turned this loop into stop-and-wait: an SSE event split across two
/// reads (`data: ...` then `\n\n`) sat mid-frame until the ACK of the first
/// half, and EventSource would not dispatch.
///
/// `write_all` already waits when the writer cannot take more. Callers shut
/// the stream down when this returns.
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

#[cfg(test)]
mod sse_flush_repro {
    use super::copy_flush;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    /// An SSE event that arrives as two `read`s: the data line, then the
    /// terminator. EventSource will not dispatch until it has both.
    const FIRST: &[u8] = b"data: hello";
    const REST: &[u8] = b"\n\n";

    struct SplitSse {
        stage: usize,
    }

    impl AsyncRead for SplitSse {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let chunk = match self.stage {
                0 => FIRST,
                1 => REST,
                _ => return Poll::Ready(Ok(())),
            };
            buf.put_slice(chunk);
            self.stage += 1;
            Poll::Ready(Ok(()))
        }
    }

    struct WriterState {
        written: Vec<u8>,
        /// If true, flush panics: that is smoltcp waiting for a TCP ACK.
        flush_blocks: bool,
    }

    #[derive(Clone)]
    struct ProbeWriter {
        state: Arc<Mutex<WriterState>>,
    }

    impl ProbeWriter {
        fn new(flush_blocks: bool) -> Self {
            Self {
                state: Arc::new(Mutex::new(WriterState {
                    written: Vec::new(),
                    flush_blocks,
                })),
            }
        }

        fn written(&self) -> Vec<u8> {
            self.state.lock().unwrap().written.clone()
        }
    }

    impl AsyncWrite for ProbeWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.state.lock().unwrap().written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            if self.state.lock().unwrap().flush_blocks {
                panic!("copy_flush must not flush; smoltcp flush waits for a TCP ACK");
            }
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Regression: smoltcp flush waits for ACK. This loop must still enqueue
    /// the SSE terminator without that ACK, or EventSource never fires.
    #[tokio::test]
    async fn copy_flush_writes_sse_terminator_without_waiting_for_ack() {
        let writer = ProbeWriter::new(true);
        let probe = writer.clone();
        let mut reader = SplitSse { stage: 0 };

        tokio::time::timeout(
            Duration::from_millis(200),
            copy_flush(&mut reader, &mut writer.clone()),
        )
        .await
        .expect("copy_flush blocked waiting for a TCP ACK")
        .unwrap();

        assert_eq!(probe.written(), [FIRST, REST].concat());
    }

    #[tokio::test]
    async fn copy_flush_forwards_whole_sse_event_when_flush_is_noop() {
        let writer = ProbeWriter::new(false);
        let probe = writer.clone();
        let mut reader = SplitSse { stage: 0 };

        copy_flush(&mut reader, &mut writer.clone()).await.unwrap();

        assert_eq!(probe.written(), [FIRST, REST].concat());
    }
}
