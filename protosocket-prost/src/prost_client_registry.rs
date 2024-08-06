use protosocket::{Connection, MessageReactor};
use tokio::{net::TcpStream, sync::mpsc};

use crate::{ProstClientConnectionBindings, ProstSerializer};

#[derive(Debug, Clone)]
pub struct ClientRegistry {
    max_message_length: usize,
    max_queued_outbound_messages: usize,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientRegistry {
    /// Construct a new client registry. You will spawn the registry driver, probably on a dedicated thread.
    pub fn new() -> Self {
        log::trace!("new client registry");
        Self {
            max_message_length: 4 * (2 << 20),
            max_queued_outbound_messages: 256,
        }
    }

    pub fn set_max_message_length(&mut self, max_message_length: usize) {
        self.max_message_length = max_message_length;
    }

    pub fn set_max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }

    /// Get a new connection to a protosocket server.
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
        let (outbound, outbound_messages) = mpsc::channel(self.max_queued_outbound_messages);
        let connection =
            Connection::<ProstClientConnectionBindings<Request, Response, Reactor>>::new(
                stream,
                address,
                ProstSerializer::default(),
                ProstSerializer::default(),
                self.max_message_length,
                self.max_queued_outbound_messages,
                outbound_messages,
                message_reactor,
            );
        tokio::spawn(connection);
        Ok(outbound)
    }
}
