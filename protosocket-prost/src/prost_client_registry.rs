use std::future::Future;

use protosocket::{pooled_encoder::PooledEncoder, Connection, MessageReactor};
use tokio::net::TcpStream;

use crate::{prost_serializer::ProstDecoder, ProstSerializer};

/// A factory for creating client connections to a `protosocket` server.
#[derive(Debug, Clone)]
pub struct ClientRegistry<TConnector = TcpConnector> {
    max_buffer_length: usize,
    buffer_allocation_increment: usize,
    max_queued_outbound_messages: usize,
    runtime: tokio::runtime::Handle,
    stream_connector: TConnector,
}

pub trait StreamConnector {
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send;
}

pub struct TcpConnector;
impl StreamConnector for TcpConnector {
    type Stream = TcpStream;
    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<TcpStream>> + Send {
        std::future::ready(Ok(stream))
    }
}

impl<TConnector> ClientRegistry<TConnector>
where
    TConnector: StreamConnector,
{
    /// Construct a new client registry. Connections will be spawned on the provided runtime.
    pub fn new(runtime: tokio::runtime::Handle, connector: TConnector) -> Self {
        log::trace!("new client registry");
        Self {
            max_buffer_length: 4 * (1 << 20),
            max_queued_outbound_messages: 256,
            buffer_allocation_increment: 1 << 20,
            runtime,
            stream_connector: connector,
        }
    }

    /// Sets the maximum read buffer length for connections created by this registry after
    /// the setting is applied.
    pub fn set_max_read_buffer_length(&mut self, max_buffer_length: usize) {
        self.max_buffer_length = max_buffer_length;
    }

    /// Sets the maximum queued outbound messages for connections created by this registry after
    /// the setting is applied.
    pub fn set_max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }

    /// Get a new connection to a `protosocket` server.
    pub async fn register_client<Request, Response, Reactor>(
        &self,
        address: impl Into<String>,
        message_reactor: Reactor,
    ) -> crate::Result<spillway::Sender<Request>>
    where
        Request: prost::Message + Default + Unpin + std::fmt::Debug + 'static,
        Response: prost::Message + Default + Unpin + std::fmt::Debug + 'static,
        Reactor: MessageReactor<Inbound = Response> + Send,
    {
        let address: std::net::SocketAddr = address.into().parse()?;
        let stream = TcpStream::connect(address)
            .await
            .map_err(std::sync::Arc::new)?;
        stream.set_nodelay(true).map_err(std::sync::Arc::new)?;
        let stream = self
            .stream_connector
            .connect_stream(stream)
            .await
            .map_err(std::sync::Arc::new)?;
        let (outbound, outbound_messages) = spillway::channel();
        let connection = Connection::<
            TConnector::Stream,
            ProstDecoder<Response>,
            PooledEncoder<ProstSerializer<Request>>,
            Reactor,
        >::new(
            stream,
            Default::default(),
            Default::default(),
            self.max_buffer_length,
            self.buffer_allocation_increment,
            self.max_queued_outbound_messages,
            outbound_messages,
            message_reactor,
        );
        self.runtime.spawn(connection);
        Ok(outbound)
    }
}
