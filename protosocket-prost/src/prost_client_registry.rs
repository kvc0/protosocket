use protosocket::{Connection, MessageReactor};
use tokio::{net::TcpStream, sync::mpsc};

use crate::{ProstClientConnectionBindings, ProstSerializer};

/// A factory for creating client connections to a `protosocket` server.
#[derive(Debug, Clone)]
pub struct ClientRegistry {
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
    runtime: tokio::runtime::Handle,
}

impl ClientRegistry {
    /// Construct a new client registry. Connections will be spawned on the provided runtime.
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        log::trace!("new client registry");
        Self {
            max_buffer_length: 4 * (2 << 20),
            max_queued_outbound_messages: 256,
            runtime,
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
    ) -> crate::Result<mpsc::Sender<Request>>
    where
        Request: prost::Message + Default + Unpin + 'static,
        Response: prost::Message + Default + Unpin + 'static,
        Reactor: MessageReactor<Inbound = Response>,
    {
        let address = address.into().parse()?;
        let stream = TcpStream::connect(address)
            .await
            .map_err(std::sync::Arc::new)?;
        stream.set_nodelay(true).map_err(std::sync::Arc::new)?;
        let (outbound, outbound_messages) = mpsc::channel(self.max_queued_outbound_messages);
        let connection =
            Connection::<ProstClientConnectionBindings<Request, Response, Reactor>>::new(
                stream,
                address,
                ProstSerializer::default(),
                ProstSerializer::default(),
                self.max_buffer_length,
                self.max_queued_outbound_messages,
                outbound_messages,
                message_reactor,
            );
        self.runtime.spawn(connection);
        Ok(outbound)
    }
}
