//! [`EfaSocketListener`]: a [`protosocket::SocketListener`] over a fabric.

use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};

use protosocket::{SocketListener, SocketResult};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use super::bootstrap;
use super::driver::Driver;
use super::error::EfaError;
use super::fabric::Fabric;
use super::provider::Provider;
use super::stream::EfaStream;

/// Accepts fabric connections, surfacing each as an [`EfaStream`].
///
/// One fabric (and one driver thread) backs the listener; every accepted
/// connection gets its own endpoint on the shared domain. Like
/// `TcpSocketListener`, this is meant to be driven by a protosocket server's
/// poll loop.
///
/// Connections are bootstrapped over a TCP listener bound at the same address:
/// a peer connects over TCP, the two sides exchange raw fabric addresses, and
/// then data flows over the fabric. The handshake for each peer runs
/// concurrently, so a slow handshake does not block other accepts.
pub struct EfaSocketListener {
    tcp: TcpListener,
    driver: Arc<Driver>,
    in_flight: JoinSet<Result<EfaStream, EfaError>>,
}

impl EfaSocketListener {
    /// Open a fabric for `provider` and bind the bootstrap TCP listener at
    /// `address`.
    pub async fn bind(provider: Provider, address: SocketAddr) -> Result<Self, EfaError> {
        let fabric = Fabric::open(provider)?;
        let driver = Driver::start(fabric);
        let tcp = TcpListener::bind(address).await?;
        Ok(EfaSocketListener {
            tcp,
            driver,
            in_flight: JoinSet::new(),
        })
    }

    /// The bound bootstrap (TCP) address. Useful when binding to port 0.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.tcp.local_addr()
    }
}

impl SocketListener for EfaSocketListener {
    type Stream = EfaStream;

    fn poll_accept(&mut self, context: &mut Context<'_>) -> Poll<SocketResult<EfaStream>> {
        // Drain pending TCP bootstrap connections, spawning a handshake for each.
        loop {
            match self.tcp.poll_accept(context) {
                Poll::Ready(Ok((tcp_stream, peer))) => {
                    let driver = Arc::clone(&self.driver);
                    self.in_flight.spawn(handshake(driver, tcp_stream, peer));
                    continue;
                }
                Poll::Ready(Err(e)) => {
                    log::error!("efa bootstrap listener failed to accept: {e:?}");
                    return Poll::Ready(SocketResult::Disconnect);
                }
                Poll::Pending => break,
            }
        }

        // Surface any completed handshakes.
        loop {
            return match self.in_flight.poll_join_next(context) {
                Poll::Ready(None) => Poll::Pending,
                Poll::Ready(Some(Ok(Ok(stream)))) => {
                    log::debug!("new efa connection from {}", stream.address());
                    Poll::Ready(SocketResult::Stream(stream))
                }
                Poll::Ready(Some(Ok(Err(e)))) => {
                    log::error!("efa connection handshake failed: {e}");
                    continue;
                }
                Poll::Ready(Some(Err(join_error))) => {
                    log::error!("efa handshake task panicked: {join_error}");
                    continue;
                }
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

/// Server-side bootstrap handshake for one peer.
async fn handshake(
    driver: Arc<Driver>,
    mut tcp: TcpStream,
    peer: SocketAddr,
) -> Result<EfaStream, EfaError> {
    tcp.set_nodelay(true).ok();
    let pending = driver.create_endpoint().await?;
    let peer_address = bootstrap::exchange(&mut tcp, &pending.local_address).await?;
    let conn = Driver::activate(&driver, pending, peer_address).await?;
    Ok(EfaStream::new(conn, peer))
}
