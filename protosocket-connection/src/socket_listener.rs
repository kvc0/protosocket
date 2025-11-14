use std::{
    ffi::c_int,
    net::SocketAddr,
    pin::pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use socket2::TcpKeepalive;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    task::JoinSet,
};

/// A pollable listener that produces socket connections
pub trait SocketListener {
    /// This type contract allows your `ServerConnector` or `SocketService`
    /// to use a connection type that is convenient for your listener.
    type Stream: AsyncRead + AsyncWrite + Unpin + 'static;

    /// Like `tokio::net::TcpListener::poll_accept`, this function returns a
    /// socket connection.
    fn poll_accept(&mut self, context: &mut Context<'_>) -> Poll<SocketResult<Self::Stream>>;
}

pub enum SocketResult<T> {
    Stream(T),
    Disconnect,
}

/// A stream wrapper for streams with addresses
pub struct StreamWithAddress<T: AsyncRead + AsyncWrite + Unpin + 'static> {
    stream: T,
    address: SocketAddr,
}
impl<T: AsyncRead + AsyncWrite + Unpin + 'static> StreamWithAddress<T> {
    /// inspect the remote address for this stream
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin + 'static> AsyncRead for StreamWithAddress<T> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        pin!(&mut self.stream).poll_read(context, buffer)
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin + 'static> AsyncWrite for StreamWithAddress<T> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        pin!(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        pin!(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        pin!(&mut self.stream).poll_shutdown(context)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        pin!(&mut self.stream).poll_write_vectored(context, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
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
        listen_backlog: u32,
        tcp_keepalive_duration: Option<Duration>,
    ) -> std::io::Result<Self> {
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
        socket.listen(listen_backlog.min(i32::MAX as u32) as i32 as c_int)?;

        let listener = tokio::net::TcpListener::from_std(socket.into())?;
        Ok(Self { listener })
    }
}
impl SocketListener for TcpSocketListener {
    type Stream = StreamWithAddress<TcpStream>;

    fn poll_accept(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<SocketResult<StreamWithAddress<TcpStream>>> {
        match self.listener.poll_accept(context) {
            Poll::Ready(Ok((stream, address))) => {
                if stream.set_nodelay(true).is_err() {
                    log::warn!("could not set nodelay on connection to {address}");
                }
                Poll::Ready(SocketResult::Stream(StreamWithAddress { stream, address }))
            }
            Poll::Ready(Err(e)) => {
                log::error!("failed to accept connection: {e:?}");
                Poll::Ready(SocketResult::Disconnect)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A socket listener that accepts TLS connections
pub struct TlsSocketListener {
    listener: TcpSocketListener,
    tls_acceptor: tokio_rustls::TlsAcceptor,
    in_flight_connections: JoinSet<(
        std::io::Result<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>,
        SocketAddr,
    )>,
}
impl TlsSocketListener {
    /// Decorate a TcpSocketListener with TLS
    pub fn wrap(
        listener: TcpSocketListener,
        server_config: tokio_rustls::rustls::ServerConfig,
    ) -> Self {
        Self {
            listener,
            tls_acceptor: Arc::new(server_config).into(),
            in_flight_connections: Default::default(),
        }
    }
}

impl SocketListener for TlsSocketListener {
    type Stream = StreamWithAddress<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>;

    fn poll_accept(&mut self, context: &mut Context<'_>) -> Poll<SocketResult<Self::Stream>> {
        loop {
            match self.listener.poll_accept(context) {
                Poll::Ready(SocketResult::Stream(StreamWithAddress { stream, address })) => {
                    let accept = self.tls_acceptor.accept(stream);
                    self.in_flight_connections
                        .spawn(async move { (accept.await, address) });
                    continue;
                }
                Poll::Ready(SocketResult::Disconnect) => {
                    return Poll::Ready(SocketResult::Disconnect)
                }
                Poll::Pending => break,
            }
        }
        // pending on accept
        loop {
            break match self.in_flight_connections.poll_join_next(context) {
                Poll::Ready(None) => {
                    // in-flight is empty
                    Poll::Pending
                }
                Poll::Ready(Some(Err(_e))) => {
                    log::error!("failed to join tls accept");
                    Poll::Ready(SocketResult::Disconnect)
                }
                Poll::Ready(Some(Ok((Ok(stream), address)))) => {
                    log::debug!("new connection completed tls accept: {address}");
                    Poll::Ready(SocketResult::Stream(StreamWithAddress { stream, address }))
                }
                Poll::Ready(Some(Ok((Err(e), address)))) => {
                    log::error!("failed to accept tls from {address}: {e:?}");
                    continue;
                }
                Poll::Pending => Poll::Pending,
            };
        }
    }
}
