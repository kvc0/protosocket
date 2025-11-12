use std::{
    collections::VecDeque,
    io::Read,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

use protosocket::{
    Decoder, DeserializeError, Encoder, MessageReactor, ReactorStatus, SocketListener,
    StreamWithAddress, TcpSocketListener,
};
use protosocket_server::{ProtosocketServerConfig, ServerConnector};
use tokio::net::TcpStream;

#[allow(clippy::expect_used)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let server_context = ServerContext::default();
    let config = ProtosocketServerConfig::default();
    let server = config.bind_tcp("127.0.0.1:9000".parse()?, server_context)?;

    tokio::spawn(server).await??;
    Ok(())
}

#[derive(Default, Clone)]
struct ServerContext {
    _connections: Arc<AtomicUsize>,
}

impl ServerConnector for ServerContext {
    type SocketListener = TcpSocketListener;
    type RequestDecoder = StringSerializer;
    type ResponseEncoder = StringSerializer;
    type Reactor = StringReactor;

    fn encoder(&self) -> Self::ResponseEncoder {
        StringSerializer
    }

    fn decoder(&self) -> Self::RequestDecoder {
        StringSerializer
    }

    fn new_reactor(
        &self,
        optional_outbound: tokio::sync::mpsc::Sender<<Self::ResponseEncoder as Encoder>::Message>,
        _address: &StreamWithAddress<TcpStream>,
    ) -> Self::Reactor {
        StringReactor {
            outbound: optional_outbound,
        }
    }

    fn spawn_connection(
        &self,
        connection: protosocket::Connection<
            <Self::SocketListener as SocketListener>::Stream,
            Self::RequestDecoder,
            Self::ResponseEncoder,
            Self::Reactor,
        >,
    ) {
        tokio::spawn(connection);
    }
}

struct StringReactor {
    outbound: tokio::sync::mpsc::Sender<String>,
}
impl MessageReactor for StringReactor {
    type Inbound = String;

    fn on_inbound_message(&mut self, mut message: Self::Inbound) -> ReactorStatus {
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
        ReactorStatus::Continue
    }
}

struct StringSerializer;

impl Encoder for StringSerializer {
    type Message = String;
    type Serialized = VecDeque<u8>;

    fn encode(&mut self, mut response: Self::Message) -> Self::Serialized {
        response.push_str(" ENCODED\n");
        response.into_bytes().into()
    }
}
impl Decoder for StringSerializer {
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
