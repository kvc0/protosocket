use std::{collections::VecDeque, time::Duration};

use protosocket::{
    Codec, Decoder, DeserializeError, Encoder, MessageReactor, ReactorStatus, SocketListener,
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
struct ServerContext {}

impl ServerConnector for ServerContext {
    type SocketListener = TcpSocketListener;
    type Codec = ByteBufferRingCodec;
    type Reactor = PooledReactor;

    fn codec(&self) -> Self::Codec {
        ByteBufferRingCodec::default()
    }

    fn new_reactor(
        &self,
        optional_outbound: spillway::Sender<<Self::Codec as Encoder>::Message>,
        _address: &StreamWithAddress<TcpStream>,
    ) -> Self::Reactor {
        PooledReactor {
            outbound: optional_outbound,
        }
    }

    fn spawn_connection(
        &self,
        connection: protosocket::Connection<
            <Self::SocketListener as SocketListener>::Stream,
            Self::Codec,
            Self::Reactor,
        >,
    ) {
        tokio::spawn(connection);
    }
}

struct PooledReactor {
    outbound: spillway::Sender<String>,
}
impl MessageReactor for PooledReactor {
    type Inbound = String;
    type Outbound = String;
    type LogicalOutbound = String;

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
            if let Err(e) = outbound.send(message) {
                log::error!("send error: {e:?}");
            }
        });
        ReactorStatus::Continue
    }

    fn on_outbound_message(&mut self, message: Self::LogicalOutbound) -> Self::Outbound {
        message
    }
}

#[derive(Default)]
struct ByteBufferRingCodec {
    /// Reused buffers, so there's no steady state allocation for RPC messages
    buffer_pool: Vec<Vec<u8>>,
}

impl Codec for ByteBufferRingCodec {}
impl Encoder for ByteBufferRingCodec {
    type Message = String;
    type Serialized = VecDeque<u8>;

    fn encode(&mut self, response: Self::Message) -> Self::Serialized {
        // This response payload is from an earlier decode(), which comes from
        // self.buffer_pool.
        //
        // In a real application, you might need to make Self::Message a little more
        // interesting to both avoid allocation and carry a logical response.
        // For example, you might want a Message struct that carries the decoded buffer
        // along, so you can reuse it or return it to the pool here.
        //
        // convert String -> Vec<u8> is O(1), and Vec<u8> -> VecDeque<u8> is O(1).
        // buffer is carried through.
        response.into_bytes().into()
    }

    fn return_buffer(&mut self, mut buffer: Self::Serialized) {
        if 31 < self.buffer_pool.len() {
            // Limit the pooled buffer count (do what you want here)
            return;
        }
        // clear retains capacity
        buffer.clear();
        // Vec::Dequeue -> Vec conversion takes the inner buffer without copy when
        // the head is at 0 and length is at 0, which clear() provides.
        self.buffer_pool.push(buffer.into());
    }
}

impl Decoder for ByteBufferRingCodec {
    type Message = String;

    fn decode(
        &mut self,
        mut buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError> {
        // default() is how we get another buffer.
        let mut read_buffer = self.buffer_pool.pop().unwrap_or_default();
        let remaining = buffer.remaining();
        read_buffer.resize(remaining, 0);
        buffer.copy_to_slice(&mut read_buffer);

        let read_buffer = String::from_utf8(read_buffer).map_err(|e| {
            log::error!("Invalid UTF-8 buffer: {e:?}");
            DeserializeError::InvalidBuffer
        })?;

        Ok((remaining, read_buffer))
    }
}
