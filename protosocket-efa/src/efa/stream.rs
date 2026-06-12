//! [`EfaStream`]: a connection-oriented byte stream over the fabric.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use super::bootstrap;
use super::driver::{Driver, StreamConn};
use super::error::EfaError;
use super::fabric::Fabric;
use super::provider::Provider;

/// A reliable, ordered byte stream carried over a libfabric fabric.
///
/// `EfaStream` implements [`AsyncRead`] + [`AsyncWrite`], so it plugs directly
/// into protosocket's `Connection`. Server-side streams are produced by
/// [`EfaSocketListener`](crate::EfaSocketListener); client-side streams are
/// created with [`EfaStream::connect`].
///
/// Unlike protosocket's `TcpSocketListener`, the peer address surfaced by
/// [`EfaStream::address`] is the address of the *bootstrap* TCP channel, not the
/// raw fabric address (which is opaque and provider-specific).
pub struct EfaStream {
    conn: StreamConn,
    address: SocketAddr,
}

impl EfaStream {
    pub(crate) fn new(conn: StreamConn, address: SocketAddr) -> Self {
        EfaStream { conn, address }
    }

    /// The bootstrap (TCP side-channel) address of the peer.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Connect to an [`EfaSocketListener`](crate::EfaSocketListener) listening
    /// at `bootstrap_address`, using `provider` for the fabric.
    ///
    /// This opens a private fabric and driver for the client; they live as long
    /// as the returned stream.
    pub async fn connect(
        provider: Provider,
        bootstrap_address: SocketAddr,
    ) -> Result<Self, EfaError> {
        let fabric = Fabric::open(provider)?;
        let driver = Driver::start(fabric);

        let pending = driver.create_endpoint().await?;

        let mut tcp = TcpStream::connect(bootstrap_address).await?;
        let peer_address = bootstrap::exchange(&mut tcp, &pending.local_address).await?;

        // `conn` (StreamConn) holds its own `Arc<Driver>`, keeping the client's
        // driver thread alive for the lifetime of the stream.
        let conn = Driver::activate(&driver, pending, peer_address).await?;
        Ok(EfaStream {
            conn,
            address: bootstrap_address,
        })
    }
}

impl AsyncRead for EfaStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let unfilled = buf.initialize_unfilled();
        match this.conn.poll_read(cx, unfilled) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for EfaStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().conn.poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().conn.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().conn.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().conn.poll_shutdown(cx)
    }
}
