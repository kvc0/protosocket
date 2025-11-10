use protosocket::Connection;
use protosocket::Decoder;
use protosocket::Encoder;
use protosocket::MessageReactor;
use socket2::TcpKeepalive;
use std::ffi::c_int;
use std::future::Future;
use std::io::Error;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

/// The ServerConnector listens to a socket and spawns a Reactor for each new connection.
pub trait ServerConnector: Unpin {
    /// Outbound message type
    type Encoder: Encoder;
    /// Inbound message type
    type Decoder: Decoder<Message = <Self::Reactor as MessageReactor>::Inbound>;
    /// Per-connection message handler
    type Reactor: MessageReactor;
    /// Type of connected stream (possibly tls)
    type Stream: AsyncRead + AsyncWrite + Unpin + 'static;

    /// Create a new encoder
    fn encoder(&self) -> Self::Encoder;
    /// Create a new decoder
    fn decoder(&self) -> Self::Decoder;

    /// Create a per-connection message handler
    fn new_reactor(
        &self,
        optional_outbound: mpsc::Sender<<Self::Encoder as Encoder>::Message>,
        address: SocketAddr,
    ) -> Self::Reactor;

    /// Wrap a tcp stream, or just return it
    fn connect(&self, stream: tokio::net::TcpStream) -> Self::Stream;

    /// Spawn a connection - probably you just want tokio::spawn, but you might have other needs.
    fn spawn_connection(
        &self,
        connection: Connection<Self::Stream, Self::Decoder, Self::Encoder, Self::Reactor>,
    );
}

/// A `protosocket::Connection` is an IO driver. It directly uses tokio's io wrapper of mio to poll
/// the OS's io primitives, manages read and write buffers, and vends messages to & from connections.
/// Connections send messages to the ConnectionServer through an mpsc channel, and they receive
/// inbound messages via a reactor callback.
///
/// Protosockets are monomorphic messages: You can only have 1 kind of message per service.
/// The expected way to work with this is to use prost and protocol buffers to encode messages.
/// Of course you can do whatever you want, as the telnet example shows.
///
/// Protosocket messages are not opinionated about request & reply. If you are, you will need
/// to implement such a thing. This allows you freely choose whether you want to send
/// fire-&-forget messages sometimes; however it requires you to write your protocol's rules.
/// You get an inbound iterable of <MessageIn> batches and an outbound stream of <MessageOut> per
/// connection - you decide what those mean for you!
///
/// A ProtosocketServer is a future: You spawn it and it runs forever.
///
/// Construct a new ProtosocketServer by creating a ProtosocketServerConfig and calling the {{bind_tcp}} method.
pub struct ProtosocketServer<Connector: ServerConnector> {
    connector: Connector,
    listener: tokio::net::TcpListener,
    max_buffer_length: usize,
    buffer_allocation_increment: usize,
    max_queued_outbound_messages: usize,
}

/// Socket configuration options for a ProtosocketServer.
pub struct ProtosocketSocketConfig {
    nodelay: bool,
    reuse: bool,
    keepalive_duration: Option<std::time::Duration>,
    listen_backlog: u32,
}

impl ProtosocketSocketConfig {
    /// Whether nodelay should be set on the socket.
    pub fn nodelay(mut self, nodelay: bool) -> Self {
        self.nodelay = nodelay;
        self
    }
    /// Whether reuseaddr and reuseport should be set on the socket.
    pub fn reuse(mut self, reuse: bool) -> Self {
        self.reuse = reuse;
        self
    }
    /// The keepalive window to be set on the socket.
    pub fn keepalive_duration(mut self, keepalive_duration: std::time::Duration) -> Self {
        self.keepalive_duration = Some(keepalive_duration);
        self
    }
    /// The backlog to be set on the socket when invoking `listen`.
    pub fn listen_backlog(mut self, backlog: u32) -> Self {
        self.listen_backlog = backlog;
        self
    }
}

impl Default for ProtosocketSocketConfig {
    fn default() -> Self {
        Self {
            nodelay: true,
            reuse: true,
            keepalive_duration: None,
            listen_backlog: 65536,
        }
    }
}

pub struct ProtosocketServerConfig {
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
    buffer_allocation_increment: usize,
    socket_config: ProtosocketSocketConfig,
}

impl ProtosocketServerConfig {
    /// The maximum buffer length per connection on this server.
    pub fn max_buffer_length(mut self, max_buffer_length: usize) -> Self {
        self.max_buffer_length = max_buffer_length;
        self
    }
    /// The maximum number of queued outbound messages per connection on this server.
    pub fn max_queued_outbound_messages(mut self, max_queued_outbound_messages: usize) -> Self {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
        self
    }
    /// The step size for allocating additional memory for connection buffers on this server.
    pub fn buffer_allocation_increment(mut self, buffer_allocation_increment: usize) -> Self {
        self.buffer_allocation_increment = buffer_allocation_increment;
        self
    }
    /// The tcp socket configuration options for this server.
    pub fn socket_config(mut self, config: ProtosocketSocketConfig) -> Self {
        self.socket_config = config;
        self
    }

    /// Binds a tcp listener to the given address and returns a ProtosocketServer with this configuration.
    /// After binding, you must await the returned server future to process requests.
    pub async fn bind_tcp<Connector: ServerConnector>(
        self,
        address: SocketAddr,
        connector: Connector,
    ) -> crate::Result<ProtosocketServer<Connector>> {
        ProtosocketServer::new(address, connector, self).await
    }
}

impl Default for ProtosocketServerConfig {
    fn default() -> Self {
        Self {
            max_buffer_length: 16 * (2 << 20),
            max_queued_outbound_messages: 128,
            buffer_allocation_increment: 1 << 20,
            socket_config: Default::default(),
        }
    }
}

impl<Connector: ServerConnector> ProtosocketServer<Connector> {
    /// Construct a new `ProtosocketServer` listening on the provided address.
    /// The address will be bound and listened upon with `SO_REUSEADDR` set.
    /// The server will use the provided runtime to spawn new tcp connections as `protosocket::Connection`s.
    async fn new(
        address: SocketAddr,
        connector: Connector,
        config: ProtosocketServerConfig,
    ) -> crate::Result<Self> {
        let socket = socket2::Socket::new(
            match address {
                SocketAddr::V4(_) => socket2::Domain::IPV4,
                SocketAddr::V6(_) => socket2::Domain::IPV6,
            },
            socket2::Type::STREAM,
            None,
        )?;

        let mut tcp_keepalive = TcpKeepalive::new();
        if let Some(duration) = config.socket_config.keepalive_duration {
            tcp_keepalive = tcp_keepalive.with_time(duration);
        }

        socket.set_nonblocking(true)?;
        socket.set_tcp_nodelay(config.socket_config.nodelay)?;
        socket.set_tcp_keepalive(&tcp_keepalive)?;
        socket.set_reuse_port(config.socket_config.reuse)?;
        socket.set_reuse_address(config.socket_config.reuse)?;

        socket.bind(&address.into())?;
        socket.listen(config.socket_config.listen_backlog as c_int)?;

        let listener = tokio::net::TcpListener::from_std(socket.into())?;
        Ok(Self {
            connector,
            listener,
            max_buffer_length: config.max_buffer_length,
            max_queued_outbound_messages: config.max_queued_outbound_messages,
            buffer_allocation_increment: config.buffer_allocation_increment,
        })
    }
}

impl<Connector: ServerConnector> Future for ProtosocketServer<Connector> {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            break match self.listener.poll_accept(context) {
                Poll::Ready(result) => match result {
                    Ok((stream, address)) => {
                        stream.set_nodelay(true)?;
                        let (outbound_submission_queue, outbound_messages) =
                            mpsc::channel(self.max_queued_outbound_messages);
                        let reactor = self
                            .connector
                            .new_reactor(outbound_submission_queue.clone(), address);
                        let stream = self.connector.connect(stream);
                        let connection = Connection::new(
                            stream,
                            self.connector.decoder(),
                            self.connector.encoder(),
                            self.max_buffer_length,
                            self.buffer_allocation_increment,
                            self.max_queued_outbound_messages,
                            outbound_messages,
                            reactor,
                        );
                        self.connector.spawn_connection(connection);
                        continue;
                    }
                    Err(e) => {
                        log::error!("failed to accept connection: {e:?}");
                        continue;
                    }
                },
                Poll::Pending => Poll::Pending,
            };
        }
    }
}
