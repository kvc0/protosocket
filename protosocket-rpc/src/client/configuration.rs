use protosocket::Connection;
use socket2::TcpKeepalive;
use std::net::SocketAddr;
use tokio::net::TcpStream;

use crate::{
    client::{
        reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor},
        StreamConnector,
    },
    Message,
};

use super::RpcClient;

/// Configuration for a `protosocket` rpc client.
#[derive(Debug, Clone)]
pub struct Configuration<TStreamConnector> {
    max_buffer_length: usize,
    buffer_allocation_increment: usize,
    max_queued_outbound_messages: usize,
    tcp_keepalive_duration: Option<std::time::Duration>,
    stream_connector: TStreamConnector,
}

impl<TStreamConnector> Configuration<TStreamConnector>
where
    TStreamConnector: StreamConnector,
{
    /// Create a new protosocket rpc client that uses a stream connector
    pub fn new(stream_connector: TStreamConnector) -> Self {
        log::trace!("new client configuration");
        Self {
            max_buffer_length: 4 * (1 << 20), // 4 MiB
            buffer_allocation_increment: 1 << 20,
            max_queued_outbound_messages: 256,
            tcp_keepalive_duration: None,
            stream_connector,
        }
    }

    /// Max buffer length limits the max message size. Try to use a buffer length that is at least 4 times the largest message you want to support.
    ///
    /// Default: 4MiB
    pub fn max_buffer_length(&mut self, max_buffer_length: usize) {
        self.max_buffer_length = max_buffer_length;
    }

    /// Max messages that will be queued up waiting for send on the client channel.
    ///
    /// Default: 256
    pub fn max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }

    /// Amount of buffer to allocate at one time when buffer needs extension.
    ///
    /// Default: 1MiB
    pub fn buffer_allocation_increment(&mut self, buffer_allocation_increment: usize) {
        self.buffer_allocation_increment = buffer_allocation_increment;
    }

    /// The duration to set for tcp_keepalive on the underlying socket.
    ///
    /// Default: None
    pub fn tcp_keepalive_duration(&mut self, tcp_keepalive_duration: Option<std::time::Duration>) {
        self.tcp_keepalive_duration = tcp_keepalive_duration;
    }
}

/// Connect a new protosocket rpc client to a server
pub async fn connect<Codec, TStreamConnector>(
    address: SocketAddr,
    configuration: &Configuration<TStreamConnector>,
) -> Result<
    (
        RpcClient<
            <Codec as protosocket::Encoder>::Message,
            <Codec as protosocket::Decoder>::Message,
        >,
        protosocket::Connection<
            TStreamConnector::Stream,
            Codec,
            RpcCompletionReactor<
                <Codec as protosocket::Decoder>::Message,
                <Codec as protosocket::Encoder>::Message,
                DoNothingMessageHandler<<Codec as protosocket::Decoder>::Message>,
            >,
        >,
    ),
    crate::Error,
>
where
    Codec: protosocket::Codec + Default + 'static,
    <Codec as protosocket::Decoder>::Message: Message,
    <Codec as protosocket::Encoder>::Message: Message,
    TStreamConnector: StreamConnector,
{
    log::trace!("new client {address}, {configuration:?}");

    let stream = TcpStream::connect(&address).await?;

    // For setting socket configuration options available to socket2
    let socket = socket2::SockRef::from(&stream);

    let mut tcp_keepalive = TcpKeepalive::new();
    if let Some(duration) = configuration.tcp_keepalive_duration {
        tcp_keepalive = tcp_keepalive.with_time(duration);
    }
    socket.set_nonblocking(true)?;
    socket.set_tcp_keepalive(&tcp_keepalive)?;
    socket.set_tcp_nodelay(true)?;
    socket.set_reuse_address(true)?;

    let message_reactor: RpcCompletionReactor<
        <Codec as protosocket::Decoder>::Message,
        <Codec as protosocket::Encoder>::Message,
        DoNothingMessageHandler<<Codec as protosocket::Decoder>::Message>,
    > = RpcCompletionReactor::new(Default::default());
    let (outbound, outbound_messages) = spillway::channel();
    let rpc_client = RpcClient::new(outbound, &message_reactor);
    let stream = configuration
        .stream_connector
        .connect_stream(stream)
        .await?;

    // Tie outbound_messages to message_reactor via a protosocket::Connection
    let connection = Connection::<
        TStreamConnector::Stream,
        Codec,
        RpcCompletionReactor<
            <Codec as protosocket::Decoder>::Message,
            <Codec as protosocket::Encoder>::Message,
            DoNothingMessageHandler<<Codec as protosocket::Decoder>::Message>,
        >,
    >::new(
        stream,
        Codec::default(),
        configuration.max_buffer_length,
        configuration.buffer_allocation_increment,
        configuration.max_queued_outbound_messages,
        outbound_messages,
        message_reactor,
    );

    Ok((rpc_client, connection))
}
