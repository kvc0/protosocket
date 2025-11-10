use crate::pooled_encoder::Serialize;

/// An encoder takes messages and produces outbound bytes.
pub trait Encoder {
    /// The message type consumed by this serializer.
    type Message;

    /// The type this serializer produces.
    ///
    /// If you want to write to raw vectors, consider wrapping your serializer
    /// with [PooledEncoder] and using that instead.
    type Serialized: bytes::Buf;

    /// Encode a message into a buffer.
    fn encode(&mut self, message: Self::Message) -> Self::Serialized;
}

/// A decoder takes inbound bytes and produces messages.
pub trait Decoder {
    /// The message type produced by this deserializer.
    type Message;

    /// Decode a message from the buffer, or tell why you can't.
    ///
    /// You must not consume more bytes than the message you produce.
    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError>;
}

/// Errors that can occur when deserializing a message.
#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    /// Buffer will be retained and you will be called again later with more bytes
    #[error("Need more bytes to decode the next message")]
    IncompleteBuffer {
        /// This is a hint to the connection for how many more bytes should be read.
        /// You may be called again before you get another buffer with at least this
        /// many bytes.
        next_message_size: usize,
    },
    /// Buffer will be discarded
    #[error("Bad buffer")]
    InvalidBuffer,
    /// distance will be skipped
    #[error("Skip message")]
    SkipMessage {
        /// If a message is not to be serviced, you can skip it. This is how many
        /// bytes will be skipped.
        /// You may be called again before this message is skipped and you may need
        /// to repeat the skip.
        distance: usize,
    },
}

/// A message reactor is a stateful object that processes inbound messages.
/// You receive &mut self, and you receive your messages by value.
///
/// A message reactor may be a server which spawns a task per message, or a client which
/// matches response ids to a HashMap of concurrent requests with oneshot completions.
///
/// Your message reactor and your tcp connection share their fate - when one drops or
/// disconnects, the other does too.
pub trait MessageReactor: Unpin + Send + 'static {
    type Inbound;

    /// Called from the connection's driver task when messages are received.
    ///
    /// You must take the message quickly: Blocking here will block the connection.
    /// If you can't accept new messages and you can't queue, you should consider returning
    /// Disconnect.
    fn on_inbound_message(&mut self, message: Self::Inbound) -> ReactorStatus;
}

/// What the connection should do after processing a batch of inbound messages.
#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    /// Continue processing messages.
    Continue,
    /// Disconnect the tcp connection.
    Disconnect,
}

impl<T> Encoder for T
where
    T: Serialize,
{
    type Message = T::Message;
    type Serialized = OwnedBuffer;

    fn encode(&mut self, message: Self::Message) -> Self::Serialized {
        let mut buffer = Vec::new();
        self.serialize_into_buffer(message, &mut buffer);
        OwnedBuffer::new(buffer)
    }
}

/// A basic Buf wrapper for a byte array. This works for simple apis, but if you
/// have latency, memory, or cpu constraints, you should be using PooledEncoder
/// or another more sophisticated memory reuse mechanism.
pub struct OwnedBuffer {
    buffer: Vec<u8>,
    cursor: usize,
}
impl OwnedBuffer {
    fn new(buffer: Vec<u8>) -> Self {
        Self { buffer, cursor: 0 }
    }
}
impl bytes::Buf for OwnedBuffer {
    #[inline(always)]
    fn remaining(&self) -> usize {
        self.buffer.len() - self.cursor
    }

    #[inline(always)]
    fn chunk(&self) -> &[u8] {
        &self.buffer[self.cursor..]
    }

    #[inline(always)]
    fn advance(&mut self, cnt: usize) {
        assert!(self.buffer.len() <= self.cursor + cnt);
        self.cursor += cnt;
    }
}
