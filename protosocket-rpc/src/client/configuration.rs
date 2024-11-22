use std::net::SocketAddr;

use protosocket::Connection;
use tokio::sync::mpsc;

use crate::{
    client::reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor},
    Message,
};

use super::{reactor::completion_reactor::RpcCompletionConnectionBindings, RpcClient};

/// Configuration for a `protosocket` rpc client.
#[derive(Debug, Clone)]
pub struct Configuration {
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            max_buffer_length: 4 * (2 << 20),
            max_queued_outbound_messages: 256,
        }
    }
}

impl Configuration {
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
pub async fn connect<Serializer, Deserializer>(
    address: SocketAddr,
    configuration: &Configuration,
) -> Result<
    (
        RpcClient<Serializer::Message, Deserializer::Message>,
        protosocket::Connection<RpcCompletionConnectionBindings<Serializer, Deserializer>>,
    ),
    crate::Error,
>
where
    Deserializer: protosocket::Deserializer + Default + 'static,
    Serializer: protosocket::Serializer + Default + 'static,
    Deserializer::Message: Message,
    Serializer::Message: Message,
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

    // Tie outbound_messages to message_reactor via a protosocket::Connection
    let connection = Connection::<RpcCompletionConnectionBindings<Serializer, Deserializer>>::new(
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
