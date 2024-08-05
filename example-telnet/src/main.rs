use std::{
    io::Read,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

use protosocket::{
    ConnectionBindings, ConnectionDriver, DeserializeError, Deserializer, MessageReactor,
    ReactorStatus, Serializer,
};
use protosocket_server::{Server, ServerConnector};

#[allow(clippy::expect_used)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut server = Server::new()?;
    let server_context = ServerContext::default();
    let address = "127.0.0.1:9000".parse()?;
    let port_nine_thousand =
        server.register_service_listener::<ServerContext>(address, server_context.clone())?;

    std::thread::spawn(move || server.serve().expect("server must serve"));

    tokio::spawn(port_nine_thousand)
        .await
        .expect("service must serve");
    Ok(())
}

#[derive(Default, Clone)]
struct ServerContext {
    _connections: Arc<AtomicUsize>,
}

impl ServerConnector for ServerContext {
    type Bindings = StringContext;
    type Reactor = StringReactor;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer {
        StringSerializer
    }

    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer {
        StringSerializer
    }

    fn take_new_connection(
        &self,
        address: std::net::SocketAddr,
        _outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
        connection_driver: ConnectionDriver<Self::Bindings, Self::Reactor>,
    ) {
        log::info!("new connection from {address:?}");
        // The StringReactor implements the server for this example server
        tokio::spawn(connection_driver);
    }

    fn new_reactor(
        &self,
        optional_outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
    ) -> Self::Reactor {
        StringReactor {
            outbound: optional_outbound,
        }
    }
}

struct StringReactor {
    outbound: tokio::sync::mpsc::Sender<String>,
}
impl MessageReactor for StringReactor {
    type Inbound = String;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        for mut message in messages.into_iter() {
            let outbound = self.outbound.clone();
            tokio::spawn(async move {
                let seconds: u64 = message
                    .split_ascii_whitespace()
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                tokio::time::sleep(Duration::from_secs(seconds)).await;
                message.push_str(" RAN");
                if let Err(e) = outbound.send(message).await {
                    log::error!("send error: {e:?}");
                }
            });
        }
        ReactorStatus::Continue
    }
}

struct StringContext;

impl ConnectionBindings for StringContext {
    type Deserializer = StringSerializer;
    type Serializer = StringSerializer;
}

struct StringSerializer;

impl Serializer for StringSerializer {
    type Message = String;

    fn encode(&mut self, mut response: Self::Message, buffer: &mut impl bytes::BufMut) {
        response.push_str(" ENCODED\n");
        buffer.put(response.as_bytes());
    }
}
impl Deserializer for StringSerializer {
    type Message = String;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError> {
        let mut read_buffer: [u8; 1] = [0; 1];
        let read = buffer
            .reader()
            .read(&mut read_buffer)
            .map_err(|_e| DeserializeError::InvalidBuffer)?;
        match String::from_utf8(read_buffer.to_vec()) {
            Ok(s) => {
                let mut s = s.trim().to_string();
                if s.is_empty() {
                    Err(DeserializeError::SkipMessage { distance: read })
                } else {
                    s.push_str(" DECODED");
                    Ok((read, s))
                }
            }
            Err(e) => {
                log::debug!("invalid message {e:?}");
                Err(DeserializeError::InvalidBuffer)
            }
        }
    }
}
