use std::{future::Future, net::SocketAddr};

use protosocket::Connection;
use tokio::{net::TcpStream, sync::mpsc};

use crate::{
    client::reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor},
    Message,
};

use super::{reactor::completion_reactor::RpcCompletionConnectionBindings, RpcClient};

pub trait StreamConnector: std::fmt::Debug {
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send;
}

#[derive(Debug)]
pub struct TcpStreamConnector;
impl StreamConnector for TcpStreamConnector {
    type Stream = TcpStream;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send {
        std::future::ready(Ok(stream))
    }
}

/// Configuration for a `protosocket` rpc client.
#[derive(Debug, Clone)]
pub struct Configuration<TStreamConnector> {
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
    stream_connector: TStreamConnector,
}

impl<TStreamConnector> Configuration<TStreamConnector>
where
    TStreamConnector: StreamConnector,
{
    pub fn new(stream_connector: TStreamConnector) -> Self {
        log::trace!("new client configuration");
        Self {
            max_buffer_length: 4 * (1 << 20), // 4 MiB
            max_queued_outbound_messages: 256,
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
}

/// Connect a new protosocket rpc client to a server
pub async fn connect<Serializer, Deserializer, TStreamConnector>(
    address: SocketAddr,
    configuration: &Configuration<TStreamConnector>,
) -> Result<
    (
        RpcClient<Serializer::Message, Deserializer::Message>,
        protosocket::Connection<
            RpcCompletionConnectionBindings<Serializer, Deserializer, TStreamConnector::Stream>,
        >,
    ),
    crate::Error,
>
where
    Deserializer: protosocket::Deserializer + Default + 'static,
    Serializer: protosocket::Serializer + Default + 'static,
    Deserializer::Message: Message,
    Serializer::Message: Message,
    TStreamConnector: StreamConnector,
{
    log::trace!("new client {address}, {configuration:?}");

    let stream = tokio::net::TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;

    let message_reactor: RpcCompletionReactor<
        Deserializer::Message,
        DoNothingMessageHandler<Deserializer::Message>,
    > = RpcCompletionReactor::new(Default::default());
    let (outbound, outbound_messages) = mpsc::channel(configuration.max_queued_outbound_messages);
    let rpc_client = RpcClient::new(outbound, &message_reactor);
    let stream = configuration
        .stream_connector
        .connect_stream(stream)
        .await?;

    // Tie outbound_messages to message_reactor via a protosocket::Connection
    let connection = Connection::<
        RpcCompletionConnectionBindings<Serializer, Deserializer, TStreamConnector::Stream>,
    >::new(
        stream,
        address,
        Deserializer::default(),
        Serializer::default(),
        configuration.max_buffer_length,
        configuration.max_queued_outbound_messages,
        outbound_messages,
        message_reactor,
    );

    Ok((rpc_client, connection))
}
