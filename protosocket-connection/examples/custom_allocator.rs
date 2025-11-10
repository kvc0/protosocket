use std::task::Poll;

use bytes::Bytes;
use protosocket::{Connection, Decoder, Encoder, MessageReactor};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};

fn main() {
    let (_network_in, _network_out, stream) = FakeIO::new();
    let (message_sender, outbound_messages) = mpsc::channel(8);
    Connection::new(
        stream,
        BulkDecoder::default(),
        BulkEncoder::default(),
        64,
        64,
        8,
        outbound_messages,
        EchoReactor {
            outbound_messages: message_sender,
        },
    );
}

struct EchoReactor {
    outbound_messages: mpsc::Sender<Bytes>,
}
impl MessageReactor for EchoReactor {
    type Inbound = Bytes;

    fn on_inbound_message(&mut self, message: Self::Inbound) -> protosocket::ReactorStatus {
        match self.outbound_messages.try_send(message) {
            Ok(_) => protosocket::ReactorStatus::Continue,
            Err(_) => {
                log::error!("outbound channel overwhelmed");
                protosocket::ReactorStatus::Disconnect
            }
        }
    }
}

/// This decoder allocates memory, but you could easily add a memory pool and reuse allocations.
/// If you did that, and implemented a passthrough `bytes::Buf` on your reusable reference type,
/// you could have no message allocation in steady state message handling.
#[derive(Default)]
struct BulkDecoder {}
impl Decoder for BulkDecoder {
    type Message = Bytes;

    fn decode(
        &mut self,
        mut buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), protosocket::DeserializeError> {
        if buffer.has_remaining() {
            let next = buffer.copy_to_bytes(buffer.remaining());
            Ok((next.len(), next))
        } else {
            Err(protosocket::DeserializeError::IncompleteBuffer {
                next_message_size: 1,
            })
        }
    }
}

#[derive(Default)]
struct BulkEncoder;
impl Encoder for BulkEncoder {
    type Message = Bytes;
    type Serialized = Bytes;

    fn encode(&mut self, message: Self::Message) -> Self::Serialized {
        message
    }
}

struct FakeIO {
    input: mpsc::UnboundedReceiver<u8>,
    output: mpsc::UnboundedSender<u8>,
}
impl FakeIO {
    fn new() -> (mpsc::UnboundedSender<u8>, mpsc::UnboundedReceiver<u8>, Self) {
        let (network_in, in_receiver) = mpsc::unbounded_channel();
        let (out_sender, network_out) = mpsc::unbounded_channel();
        let me = Self {
            input: in_receiver,
            output: out_sender,
        };

        (network_in, network_out, me)
    }
}
impl AsyncRead for FakeIO {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut buffer = Vec::new();
        loop {
            let limit = buf.remaining();
            break match self.input.poll_recv_many(context, &mut buffer, limit) {
                Poll::Ready(0) => Poll::Ready(Ok(())),
                Poll::Ready(_n) => {
                    buf.put_slice(&buffer);
                    buf.clear();
                    continue;
                }
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

impl AsyncWrite for FakeIO {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        for b in buf {
            self.output.send(*b).expect("send should work");
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}
