//! The control socket on Windows. Windows 10 and later have AF_UNIX sockets,
//! which tokio does not expose. win_uds does, as futures-io types driven by
//! async-io; these wrappers adapt them to tokio's `AsyncRead` and `AsyncWrite`
//! so the client and daemon use them as they use tokio's `UnixStream` on unix.
//! Same shim as http-nu's `src/listener.rs`.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use win_uds::net::{AsyncListener, AsyncStream};

pub struct UnixStream(Compat<AsyncStream>);

impl UnixStream {
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let stream = AsyncStream::connect(path).await?;
        Ok(Self(stream.compat()))
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

pub struct UnixListener(AsyncListener);

impl UnixListener {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Ok(Self(AsyncListener::bind(path)?))
    }

    pub async fn accept(&self) -> io::Result<(UnixStream, ())> {
        let (stream, _addr) = self.0.accept().await?;
        Ok((UnixStream(stream.compat()), ()))
    }
}
