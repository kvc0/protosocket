use std::{
    ffi::c_int,
    net::SocketAddr,
    task::{Context, Poll},
    time::Duration,
};

use socket2::TcpKeepalive;
use tokio::net::TcpStream;

pub trait SocketListener {
    type RawSocketConnection;

    fn poll_accept(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<SocketResult<Self::RawSocketConnection>>;
}

pub enum SocketResult<T> {
    Socket(T),
    Disconnect,
}

/// A socket listener for TCP
pub struct TcpSocketListener {
    listener: tokio::net::TcpListener,
}
impl TcpSocketListener {
    /// Create a new TCP listener.
    /// Note that this synchronously binds and listens.
    pub fn listen(
        address: SocketAddr,
        listen_backlog: i32,
        tcp_keepalive_duration: Option<Duration>,
    ) -> crate::Result<Self> {
        let socket = socket2::Socket::new(
            match address {
                std::net::SocketAddr::V4(_) => socket2::Domain::IPV4,
                std::net::SocketAddr::V6(_) => socket2::Domain::IPV6,
            },
            socket2::Type::STREAM,
            None,
        )?;

        let mut tcp_keepalive = TcpKeepalive::new();
        if let Some(duration) = tcp_keepalive_duration {
            tcp_keepalive = tcp_keepalive.with_time(duration);
        }

        socket.set_nonblocking(true)?;
        socket.set_tcp_nodelay(true)?;
        socket.set_tcp_keepalive(&tcp_keepalive)?;
        socket.set_reuse_port(true)?;
        socket.set_reuse_address(true)?;

        socket.bind(&address.into())?;
        socket.listen(listen_backlog as c_int)?;

        let listener = tokio::net::TcpListener::from_std(socket.into())?;
        Ok(Self { listener })
    }
}
impl SocketListener for TcpSocketListener {
    type RawSocketConnection = (TcpStream, SocketAddr);

    fn poll_accept(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<SocketResult<(TcpStream, SocketAddr)>> {
        match self.listener.poll_accept(context) {
            Poll::Ready(Ok((next, address))) => {
                if next.set_nodelay(true).is_err() {
                    log::warn!("could not set nodelay on connection to {address}");
                }
                Poll::Ready(SocketResult::Socket((next, address)))
            }
            Poll::Ready(Err(e)) => {
                log::error!("failed to accept connection: {e:?}");
                Poll::Ready(SocketResult::Disconnect)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
